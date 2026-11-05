//! Experimental v1 framed JSON service over the embedded transaction engine.
use crate::transactions::{Database, Error, ResourceLimits, ScanLimits, Transaction};
use serde_json::{Value, json};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const MAX_REQUEST: usize = 1024 * 1024;
pub const MAX_RESPONSE: usize = 2 * 1024 * 1024 + 8192;
const MAX_VALUE: usize = 256 * 1024;
const MAX_PAGE_BYTES: usize = 512 * 1024;

pub struct Config {
    pub bind: SocketAddr,
    pub database: PathBuf,
    pub token: Vec<u8>,
    pub initialize: bool,
    pub allow_remote: bool,
    pub max_connections: usize,
    pub request_timeout: Duration,
    pub transaction_timeout: Duration,
}

impl Config {
    pub fn validate(&self) -> io::Result<()> {
        if !self.bind.ip().is_loopback() && !self.allow_remote {
            return Err(io::Error::other(
                "non-loopback bind requires --allow-remote; use an encrypted tunnel outside the host",
            ));
        }
        if !(32..=256).contains(&self.token.len())
            || !(1..=256).contains(&self.max_connections)
            || self.request_timeout.is_zero()
            || self.transaction_timeout.is_zero()
        {
            return Err(io::Error::other(
                "invalid token length, connection limit or timeout",
            ));
        }
        Ok(())
    }
}

pub struct Server {
    config: Arc<Config>,
    db: Database,
    listener: TcpListener,
}

impl Server {
    pub fn bind(config: Config) -> Result<Self, Box<dyn std::error::Error>> {
        config.validate()?;
        let listener = TcpListener::bind(config.bind)?;
        listener.set_nonblocking(true)?;
        let limits = ResourceLimits {
            max_transactions: config.max_connections,
            ..ResourceLimits::default()
        };
        let db = if config.database.try_exists()? {
            Database::open_with_limits(&config.database, limits)?
        } else if config.initialize {
            let id = SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_nanos()
                .to_le_bytes();
            let db = Database::create_with_limits(&config.database, id, limits)?;
            #[cfg(unix)]
            std::fs::File::open(
                config
                    .database
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or_else(|| std::path::Path::new(".")),
            )?
            .sync_all()?;
            db
        } else {
            return Err(
                "database absent; use --init to create (existing files are never overwritten)"
                    .into(),
            );
        };
        if let Ok(value) = std::env::var("CATDB_EXPERIMENTAL_FIXED_LOG_BYTES") {
            #[cfg(feature = "fixed-log")]
            {
                let capacity: u64 = value.parse()?;
                match db.fixed_log_capacity()? {
                    None => {
                        db.checkpoint_fixed_log(capacity)?;
                    }
                    Some(existing) if existing == capacity => {}
                    Some(_) => {
                        return Err(
                            "fixed log capacity differs; expand through explicit checkpoint API"
                                .into(),
                        );
                    }
                }
            }
            #[cfg(not(feature = "fixed-log"))]
            {
                let _ = value;
                return Err("experimental fixed-log build feature required".into());
            }
        }
        if let Ok(value) = std::env::var("CATDB_BACKGROUND_RECLAMATION") {
            match value.as_str() {
                "1" => db.set_background_reclamation(true)?,
                "0" => {}
                _ => return Err("CATDB_BACKGROUND_RECLAMATION must be 0 or 1".into()),
            }
        }
        db.set_group_commit(true);
        db.set_group_commit_wake_on_four(true);
        Ok(Self {
            config: Arc::new(config),
            db,
            listener,
        })
    }

    pub fn address(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    pub fn run(self, stop: Arc<AtomicBool>) -> io::Result<()> {
        let active = Arc::new(AtomicUsize::new(0));
        #[cfg(feature = "single-client-commit")]
        self.db.set_service_clients(Arc::clone(&active));
        let mut workers: Vec<thread::JoinHandle<()>> = Vec::new();
        let mut failure = None;
        while !stop.load(Ordering::Acquire) {
            let mut i = 0;
            while i < workers.len() {
                if workers[i].is_finished() {
                    let _ = workers.swap_remove(i).join();
                } else {
                    i += 1;
                }
            }
            match self.listener.accept() {
                Ok((stream, _)) => {
                    if active.load(Ordering::Acquire) >= self.config.max_connections {
                        // An overloaded accept loop must not wait for a peer to receive an error.
                        drop(stream);
                        continue;
                    }
                    active.fetch_add(1, Ordering::AcqRel);
                    let count = Arc::clone(&active);
                    let db = self.db.clone();
                    let config = Arc::clone(&self.config);
                    let stopping = Arc::clone(&stop);
                    match thread::Builder::new()
                        .name("catdb-client".into())
                        .spawn(move || {
                            struct Permit(Arc<AtomicUsize>);
                            impl Drop for Permit {
                                fn drop(&mut self) {
                                    self.0.fetch_sub(1, Ordering::AcqRel);
                                }
                            }
                            let _permit = Permit(count);
                            let _ = connection(stream, db, &config, &stopping);
                        }) {
                        Ok(worker) => workers.push(worker),
                        Err(error) => {
                            active.fetch_sub(1, Ordering::AcqRel);
                            failure = Some(error);
                            break;
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        stop.store(true, Ordering::Release);
        for worker in workers {
            let _ = worker.join();
        }
        failure.map_or(Ok(()), Err)
    }
}

fn transfer_read(
    stream: &mut TcpStream,
    bytes: &mut [u8],
    deadline: Instant,
    stop: &AtomicBool,
) -> io::Result<()> {
    let mut done = 0;
    while done < bytes.len() {
        if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "request expired"));
        }
        match stream.read(&mut bytes[done..]) {
            Ok(0) => return Err(io::ErrorKind::UnexpectedEof.into()),
            Ok(n) => done += n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

fn receive(stream: &mut TcpStream, deadline: Instant, stop: &AtomicBool) -> io::Result<Value> {
    let mut prefix = [0; 4];
    transfer_read(stream, &mut prefix, deadline, stop)?;
    let length = u32::from_be_bytes(prefix) as usize;
    if !(1..=MAX_REQUEST).contains(&length) {
        return Err(io::Error::other("frame length"));
    }
    let mut body = vec![0; length];
    transfer_read(stream, &mut body, deadline, stop)?;
    serde_json::from_slice(&body).map_err(io::Error::other)
}

fn send(
    stream: &mut TcpStream,
    value: &Value,
    deadline: Instant,
    stop: &AtomicBool,
) -> io::Result<()> {
    let body = serde_json::to_vec(value)?;
    if body.len() > MAX_RESPONSE {
        return Err(io::Error::other("response limit"));
    }
    let prefix = (body.len() as u32).to_be_bytes();
    for bytes in [&prefix[..], &body[..]] {
        let mut done = 0;
        while done < bytes.len() {
            if stop.load(Ordering::Acquire) || Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            match stream.write(&bytes[done..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => done += n,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e),
            }
        }
    }
    Ok(())
}

fn error(code: &str) -> Value {
    json!({"ok": false, "error": code})
}
fn same_token(a: &[u8], b: &[u8]) -> bool {
    let mut difference = a.len() ^ b.len();
    for i in 0..256 {
        difference |= usize::from(a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0));
    }
    difference == 0
}

#[cfg(feature = "lock-timing")]
struct CommitDiagnostic {
    request_index: u64,
    dispatch_ns: u64,
    finished_ns: u64,
    trace: [u64; 8],
    detail: [u64; 8],
    costs: [u64; 2],
    sync_attempts: u64,
    commit_ok: bool,
}

#[cfg(feature = "lock-timing")]
impl CommitDiagnostic {
    fn json(&self) -> Value {
        json!({
            "request_index": self.request_index,
            "dispatch_ns": self.dispatch_ns,
            "finished_ns": self.finished_ns,
            "trace": self.trace,
            "detail": self.detail,
            "costs": self.costs,
            "sync_attempts": self.sync_attempts,
            "commit_ok": self.commit_ok,
        })
    }
}

fn connection(
    mut stream: TcpStream,
    db: Database,
    config: &Config,
    stop: &AtomicBool,
) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(Duration::from_millis(100)))?;
    let deadline = Instant::now() + config.request_timeout.min(Duration::from_secs(5));
    let auth = receive(&mut stream, deadline, stop)?;
    let token = auth.get("token").and_then(Value::as_str).unwrap_or("");
    if auth.get("op").and_then(Value::as_str) != Some("auth")
        || auth.get("version").and_then(Value::as_u64) != Some(1)
        || !same_token(token.as_bytes(), &config.token)
    {
        send(&mut stream, &error("unauthorized"), deadline, stop)?;
        return Ok(());
    }
    send(
        &mut stream,
        &json!({"ok":true,"protocol":1}),
        deadline,
        stop,
    )?;
    #[cfg(feature = "lock-timing")]
    let diagnostic = std::env::var_os("CATDB_DIAGNOSTICS").is_some();
    #[cfg(feature = "lock-timing")]
    let pull_diagnostic = std::env::var("CATDB_DIAGNOSTICS").as_deref() == Ok("pull");
    #[cfg(feature = "lock-timing")]
    let mut last_diagnostic: Option<CommitDiagnostic> = None;
    #[cfg(feature = "lock-timing")]
    let mut request_index = 0_u64;
    let mut transaction: Option<Transaction> = None;
    let mut expires = None;
    while !stop.load(Ordering::Acquire) {
        let mut deadline = Instant::now() + config.request_timeout;
        if let Some(expiry) = expires {
            deadline = deadline.min(expiry);
        }
        let request = receive(&mut stream, deadline, stop)?;
        if Instant::now() >= deadline || stop.load(Ordering::Acquire) {
            break;
        }
        #[cfg(feature = "lock-timing")]
        {
            if pull_diagnostic
                && request.get("op").and_then(Value::as_str) == Some("diagnostic_last")
            {
                let response = json!({"ok": true, "diagnostic": last_diagnostic.as_ref().map(CommitDiagnostic::json)});
                send(&mut stream, &response, deadline, stop)?;
                continue;
            }
            request_index = request_index.wrapping_add(1);
            last_diagnostic = None;
        }
        #[cfg(feature = "lock-timing")]
        let dispatch_started = {
            crate::transactions::take_commit_trace();
            crate::transactions::take_commit_detail();
            crate::transactions::take_commit_costs();
            crate::transactions::diagnostic_now_ns()
        };
        let result = dispatch(&db, &mut transaction, &request);
        #[cfg(feature = "lock-timing")]
        let dispatch_finished = crate::transactions::diagnostic_now_ns();
        if transaction.is_some() && expires.is_none() {
            expires = Some(Instant::now() + config.transaction_timeout);
        }
        if transaction.is_none() {
            expires = None;
        }
        let fatal = result.as_ref().err().is_some_and(|e| {
            matches!(
                e,
                Error::Poisoned | Error::CommitUncertain(_) | Error::CheckpointUncertain(_)
            )
        });
        let response = match result {
            Ok(value) => value,
            Err(e) => error(match e {
                Error::Conflict(_) => "conflict",
                Error::Invalid(_) => "invalid_request",
                Error::Limit(_) => "limit",
                Error::ResourceExhausted(_) => "resource_exhausted",
                Error::CommitUncertain(_) => "commit_uncertain",
                Error::CheckpointUncertain(_) => "checkpoint_uncertain",
                Error::Poisoned => "reopen_required",
                Error::Io(_) | Error::Locked(_) => "storage_error",
            }),
        };
        #[cfg(feature = "lock-timing")]
        let response = if diagnostic
            && matches!(
                request.get("op").and_then(Value::as_str),
                Some("commit" | "put_commit" | "delete_commit")
            ) {
            let record = CommitDiagnostic {
                request_index,
                dispatch_ns: dispatch_finished.saturating_sub(dispatch_started),
                finished_ns: dispatch_finished,
                trace: crate::transactions::take_commit_trace(),
                detail: crate::transactions::take_commit_detail(),
                costs: crate::transactions::take_commit_costs(),
                sync_attempts: db.commit_sync_attempts(),
                commit_ok: response.get("ok").and_then(Value::as_bool) == Some(true),
            };
            let mut response = response;
            if pull_diagnostic {
                last_diagnostic = Some(record);
            } else {
                response["diagnostic"] = record.json();
            }
            response
        } else {
            response
        };
        send(
            &mut stream,
            &response,
            Instant::now() + config.request_timeout,
            stop,
        )?;
        if fatal {
            break;
        }
    }
    Ok(())
}

fn text<'a>(v: &'a Value, name: &str) -> Result<&'a str, Error> {
    v.get(name)
        .and_then(Value::as_str)
        .ok_or(Error::Invalid("string field required"))
}
fn hex_decode(value: &str, max: usize) -> Result<Vec<u8>, Error> {
    if !value.len().is_multiple_of(2) || value.len() / 2 > max {
        return Err(Error::Limit("hex size"));
    }
    fn digit(c: u8) -> Result<u8, Error> {
        match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            b'A'..=b'F' => Ok(c - b'A' + 10),
            _ => Err(Error::Invalid("hex")),
        }
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| Ok(digit(c[0])? * 16 + digit(c[1])?))
        .collect()
}
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 15) as usize] as char);
    }
    s
}
fn bytes(v: &Value, name: &str, max: usize) -> Result<Vec<u8>, Error> {
    hex_decode(text(v, name)?, max)
}
fn optional_bytes(v: &Value, name: &str) -> Result<Option<Vec<u8>>, Error> {
    match v.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(_) => bytes(v, name, 1024).map(Some),
    }
}
fn limit(v: &Value, name: &str, default: usize, max: usize) -> Result<usize, Error> {
    match v.get(name) {
        None => Ok(default),
        Some(value) => value
            .as_u64()
            .filter(|n| (1..=max as u64).contains(n))
            .map(|n| n as usize)
            .ok_or(Error::Limit("page limit")),
    }
}

fn dispatch(db: &Database, tx: &mut Option<Transaction>, v: &Value) -> Result<Value, Error> {
    dispatch_operation(db, tx, v, text(v, "op")?)
}

fn dispatch_operation(
    db: &Database,
    tx: &mut Option<Transaction>,
    v: &Value,
    op: &str,
) -> Result<Value, Error> {
    match op {
        #[cfg(all(feature = "fixed-log", feature = "lock-timing"))]
        "fixed_log_metrics" => Ok(json!({"ok":true,"metrics":db.fixed_log_metrics()?})),
        "ping" => Ok(json!({"ok":true,"version":db.current_version()?.to_string()})),
        "get_latest" | "scan_latest" | "put_commit" | "delete_commit" => {
            if tx.is_some() {
                return Err(Error::Invalid("single request requires no transaction"));
            }
            let (operation, writes) = match op {
                "get_latest" => ("get", false),
                "scan_latest" => ("scan", false),
                "put_commit" => ("put", true),
                _ => ("delete", true),
            };
            let mut local = Some(if writes {
                db.begin()?
            } else {
                db.begin_read()?
            });
            let response = dispatch_operation(db, &mut local, v, operation)?;
            if writes {
                let version = local.take().unwrap().commit()?;
                Ok(json!({"ok":true,"version":version.to_string()}))
            } else {
                Ok(response)
            }
        }
        "begin" | "begin_read" => {
            if tx.is_some() {
                return Err(Error::Invalid("transaction already active"));
            }
            let next = if op == "begin_read" {
                db.begin_read()?
            } else {
                db.begin()?
            };
            let version = next.version();
            *tx = Some(next);
            Ok(json!({"ok":true,"version":version.to_string()}))
        }
        "commit" => {
            let version = tx
                .take()
                .ok_or(Error::Invalid("no transaction"))?
                .commit()?;
            Ok(json!({"ok":true,"version":version.to_string()}))
        }
        "rollback" => {
            tx.take()
                .ok_or(Error::Invalid("no transaction"))?
                .rollback();
            Ok(json!({"ok":true}))
        }
        "checkpoint" => {
            if tx.is_some() {
                return Err(Error::Invalid("checkpoint requires no transaction"));
            }
            let info = db.checkpoint()?;
            Ok(json!({"ok":true,"version":info.version.to_string(),"records":info.records}))
        }
        "get" => {
            let key = bytes(v, "key", 1024)?;
            let value = tx
                .as_mut()
                .ok_or(Error::Invalid("no transaction"))?
                .get(&key)?;
            if value.as_ref().is_some_and(|b| b.len() > MAX_PAGE_BYTES) {
                return Err(Error::Limit("value exceeds service response limit"));
            }
            Ok(json!({"ok":true,"value":value.as_deref().map(hex)}))
        }
        "put" | "delete" => {
            let key = bytes(v, "key", 1024)?;
            let t = tx.as_mut().ok_or(Error::Invalid("no transaction"))?;
            if op == "put" {
                t.put(&key, &bytes(v, "value", MAX_VALUE)?)?;
            } else {
                t.delete(&key)?;
            }
            Ok(json!({"ok":true}))
        }
        "scan" => {
            let start = bytes(v, "start", 1024)?;
            let end = optional_bytes(v, "end")?;
            let after = optional_bytes(v, "after")?;
            let limits = ScanLimits {
                max_rows: limit(v, "rows", 100, 4096)?,
                max_bytes: limit(v, "bytes", 65536, MAX_PAGE_BYTES)?,
                max_visits: limit(v, "visits", 4096, 65536)?,
            };
            let page = tx
                .as_mut()
                .ok_or(Error::Invalid("no transaction"))?
                .scan_page(&start, end.as_deref(), after.as_deref(), limits)?;
            let rows: Vec<_> = page
                .rows
                .iter()
                .map(|(k, v)| json!([hex(k), hex(v)]))
                .collect();
            Ok(
                json!({"ok":true,"rows":rows,"next_after":page.next_after.as_deref().map(hex),"visited":page.visited_entries}),
            )
        }
        _ => Err(Error::Invalid("unknown operation")),
    }
}

pub fn healthcheck(address: SocketAddr, token: &[u8]) -> io::Result<()> {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(Duration::from_millis(100)))?;
    let stop = AtomicBool::new(false);
    let deadline = Instant::now() + Duration::from_secs(3);
    let token = std::str::from_utf8(token).map_err(io::Error::other)?;
    send(
        &mut stream,
        &json!({"op":"auth","version":1,"token":token}),
        deadline,
        &stop,
    )?;
    if receive(&mut stream, deadline, &stop)?["ok"] != true {
        return Err(io::Error::other("authentication failed"));
    }
    send(&mut stream, &json!({"op":"ping"}), deadline, &stop)?;
    if receive(&mut stream, deadline, &stop)?["ok"] != true {
        return Err(io::Error::other("service unavailable"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn elision_single_dispatch_preserves_request_and_validation() {
        let f = Fixture::new();
        let db = f.db();
        for request in [
            json!({"op":"get_latest","key":""}),
            json!({"op":"get_latest","key":"zz"}),
            json!({"op":"put_commit","key":"00","value":""}),
            json!({"op":"put_commit","key":"01","value":"ab".repeat(MAX_VALUE)}),
            json!({"op":"put_commit","key":"02","value":"ab".repeat(MAX_VALUE+1)}),
            json!({"op":"put_commit","key":"03"}),
            json!({"op":"scan_latest","start":"","rows":1}),
            json!({"op":"scan_latest","start":"","rows":0}),
            json!({"op":"delete_commit","key":"01"}),
        ] {
            let original = request.clone();
            let result = dispatch(&db, &mut None, &request);
            let invalid = request["key"] == "zz"
                || request["key"] == "02"
                || request["key"] == "03"
                || request["rows"] == 0;
            assert_eq!(result.is_err(), invalid, "{result:?}");
            assert_eq!(request, original);
        }
        let mut active = Some(db.begin().unwrap());
        let request = json!({"op":"put_commit","key":"00","value":"ff"});
        assert!(dispatch(&db, &mut active, &request).is_err());
        assert!(active.is_some());
    }

    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("artifacts")
                .join(format!(
                    "service-unit-{}-{}-{}",
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn db(&self) -> Database {
            Database::create_with_limits(
                self.0.join("state.ndb"),
                [42; 16],
                ResourceLimits::default(),
            )
            .unwrap()
        }
    }
    #[test]
    fn st_single_update_waits_for_sync_and_hides_uncommitted_values() {
        let fixture = Fixture::new();
        let db = fixture.db();
        db.set_group_commit(true);
        #[cfg(feature = "single-client-commit")]
        db.set_service_clients(Arc::new(AtomicUsize::new(1)));
        let mut old = db.begin().unwrap();
        let gate = Arc::new([std::sync::Barrier::new(2), std::sync::Barrier::new(2)]);
        db.hold_test_sync(gate.clone());
        let (send, recv) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                send.send(dispatch(
                    &db,
                    &mut None,
                    &json!({"op":"put_commit","key":"01","value":"aa"}),
                ))
                .unwrap();
            });
            gate[0].wait();
            let response_pending = recv.try_recv().is_err();
            let hidden = dispatch(&db, &mut None, &json!({"op":"get_latest","key":"01"})).unwrap();
            let version = dispatch(&db, &mut None, &json!({"op":"ping"})).unwrap();
            gate[1].wait();
            assert!(response_pending);
            assert!(hidden["value"].is_null());
            assert_eq!(version["version"], "0");
            assert!(recv.recv_timeout(Duration::from_secs(10)).unwrap().is_ok());
        });
        assert!(old.get(&[1]).unwrap().is_none());
        assert_eq!(db.current_version().unwrap(), db.stats().unwrap().version);
    }

    #[test]
    #[ignore]
    fn single_request_crash_worker() {
        let db = Database::open(std::env::var_os("CATDB_SINGLE_CRASH_PATH").unwrap()).unwrap();
        db.set_group_commit(true);
        #[cfg(feature = "single-client-commit")]
        db.set_service_clients(Arc::new(AtomicUsize::new(1)));
        dispatch(
            &db,
            &mut None,
            &json!({"op":"put_commit","key":"02","value":"bb"}),
        )
        .unwrap();
        panic!("crash hook not reached");
    }

    #[test]
    fn er_single_request_crash_keeps_acknowledged_values() {
        for phase in ["before-sync", "after-sync"] {
            let fixture = Fixture::new();
            let db = fixture.db();
            dispatch(
                &db,
                &mut None,
                &json!({"op":"put_commit","key":"01","value":"aa"}),
            )
            .unwrap();
            drop(db);
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "service::tests::single_request_crash_worker",
                ])
                .env("CATDB_SINGLE_CRASH_PATH", fixture.0.join("state.ndb"))
                .env("NEWDBMS_GROUP_CRASH_PHASE", phase)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(77));
            let db = Database::open(fixture.0.join("state.ndb")).unwrap();
            assert_eq!(db.begin().unwrap().get(&[1]).unwrap(), Some(vec![170]));
            let next = db.begin().unwrap().get(&[2]).unwrap();
            assert!(next.is_none() || next == Some(vec![187]));
            if phase == "after-sync" {
                assert_eq!(next, Some(vec![187]));
            }
        }
    }

    #[test]
    fn eq_service_mutations_and_embedded_reopen_agree() {
        let fixture = Fixture::new();
        let db = fixture.db();
        let mut tx = None;
        dispatch(&db, &mut tx, &json!({"op":"begin"})).unwrap();
        for (key, value) in [("00", "ff"), ("01", "")] {
            dispatch(&db, &mut tx, &json!({"op":"put","key":key,"value":value})).unwrap();
        }
        dispatch(&db, &mut tx, &json!({"op":"commit"})).unwrap();
        assert_eq!(
            db.begin().unwrap().scan(&[], None).unwrap(),
            vec![(vec![0], vec![255]), (vec![1], vec![])]
        );
        dispatch(&db, &mut tx, &json!({"op":"begin"})).unwrap();
        dispatch(&db, &mut tx, &json!({"op":"put","key":"00","value":"aa"})).unwrap();
        dispatch(&db, &mut tx, &json!({"op":"delete","key":"01"})).unwrap();
        dispatch(&db, &mut tx, &json!({"op":"commit"})).unwrap();
        drop(db);
        assert_eq!(
            Database::open(fixture.0.join("state.ndb"))
                .unwrap()
                .begin()
                .unwrap()
                .scan(&[], None)
                .unwrap(),
            vec![(vec![0], vec![170])]
        );
    }
    #[test]
    fn bv_hex_limits_and_page_limits_reject_both_sides() {
        for size in [1023, 1024] {
            assert_eq!(hex_decode(&"ab".repeat(size), 1024).unwrap().len(), size);
        }
        assert!(hex_decode(&"ab".repeat(1025), 1024).is_err());
        assert!(hex_decode("a", 1024).is_err());
        assert!(hex_decode("zz", 1024).is_err());
        for value in [0, 4097] {
            assert!(limit(&json!({"rows":value}), "rows", 100, 4096).is_err());
        }
        for value in [1, 4095, 4096] {
            assert_eq!(
                limit(&json!({"rows":value}), "rows", 100, 4096).unwrap(),
                value as usize
            );
        }
    }
    #[test]
    fn st_invalid_states_leave_existing_transaction_intact() {
        let fixture = Fixture::new();
        let db = fixture.db();
        let mut tx = None;
        assert!(dispatch(&db, &mut tx, &json!({"op":"commit"})).is_err());
        dispatch(&db, &mut tx, &json!({"op":"begin"})).unwrap();
        dispatch(&db, &mut tx, &json!({"op":"put","key":"01","value":"02"})).unwrap();
        assert!(dispatch(&db, &mut tx, &json!({"op":"begin"})).is_err());
        assert!(dispatch(&db, &mut tx, &json!({"op":"checkpoint"})).is_err());
        assert_eq!(
            dispatch(&db, &mut tx, &json!({"op":"get","key":"01"})).unwrap()["value"],
            "02"
        );
        dispatch(&db, &mut tx, &json!({"op":"rollback"})).unwrap();
        assert_eq!(db.begin().unwrap().get(&[1]).unwrap(), None);
    }
    #[test]
    fn bv_auth_and_connection_configuration_bounds() {
        for size in [31, 32, 256, 257] {
            let cfg = Config {
                bind: "127.0.0.1:0".parse().unwrap(),
                database: "unused".into(),
                token: vec![b'x'; size],
                initialize: false,
                allow_remote: false,
                max_connections: 32,
                request_timeout: Duration::from_secs(1),
                transaction_timeout: Duration::from_secs(1),
            };
            assert_eq!(cfg.validate().is_ok(), (32..=256).contains(&size));
        }
        assert!(same_token(b"abc", b"abc"));
        assert!(!same_token(b"abc", b"abd"));
        assert!(!same_token(b"abc", b"abc\0"));
    }
}

#[cfg(test)]
#[path = "../benchmarks/work-elision/service_bench.rs"]
mod work_elision_bench;
