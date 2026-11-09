//! Experimental v1 framed JSON service over the embedded transaction engine.
use crate::transactions::{
    CheckpointLog, Database, Error, ResourceLimits, ScanLimits, Transaction,
};
use serde_json::{Value, json};
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
#[path = "service_accounts.rs"]
mod accounts;
#[path = "service_tls.rs"]
mod tls;
pub use accounts::AccountOptions;
mod sql;
use accounts::{Accounts, Role};
pub use sql::{ReadRole, SqlOptions};

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
        self.validate_mode(false)
    }
    fn validate_mode(&self, accounts: bool) -> io::Result<()> {
        if !self.bind.ip().is_loopback() && !self.allow_remote {
            return Err(io::Error::other(
                "non-loopback bind requires --allow-remote; use an encrypted tunnel outside the host",
            ));
        }
        if (accounts && !self.token.is_empty())
            || (!accounts && !(32..=256).contains(&self.token.len()))
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
    sql: Option<Arc<SqlOptions>>,
    accounts: Option<Arc<Accounts>>,
    config: Arc<Config>,
    db: Database,
    listener: TcpListener,
}

impl Server {
    pub fn bind(config: Config) -> Result<Self, Box<dyn std::error::Error>> {
        let limits = ResourceLimits {
            max_transactions: config.max_connections,
            ..ResourceLimits::default()
        };
        Self::bind_with_limits(config, limits)
    }

    /// Binds using explicit engine budgets. These budgets are not a process RSS cap.
    pub fn bind_with_limits(
        config: Config,
        limits: ResourceLimits,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::open(config, limits, None, None)
    }

    pub fn bind_accounts(
        config: Config,
        limits: ResourceLimits,
        accounts: AccountOptions,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        Self::open(config, limits, Some(accounts), None)
    }

    /// Enables the administrator-configured SQL host on the same database transaction engine.
    pub fn bind_with_sql(
        config: Config,
        limits: ResourceLimits,
        accounts: Option<AccountOptions>,
        sql: SqlOptions,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        sql.validate(&config.token, accounts.is_some())?;
        Self::open(config, limits, accounts, Some(Arc::new(sql)))
    }

    fn open(
        config: Config,
        limits: ResourceLimits,
        options: Option<AccountOptions>,
        sql: Option<Arc<SqlOptions>>,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        config.validate_mode(options.is_some())?;
        if config
            .database
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("accounts"))
        {
            return Err("reserved account catalog path".into());
        }
        if options.is_none()
            && (accounts::catalog_path(&config.database).try_exists()?
                || accounts::owner_path(&config.database).try_exists()?)
        {
            return Err("this database is configured for TLS accounts".into());
        }
        let limits = limits.validate()?;
        let range_writeback = match std::env::var_os("CATDB_EXT4_CHECKPOINT_WRITEBACK") {
            None => false,
            Some(value) if value == "0" => false,
            Some(value) if value == "1" => true,
            _ => return Err("CATDB_EXT4_CHECKPOINT_WRITEBACK must be 0 or 1".into()),
        };
        let requested_log = match (
            std::env::var_os("CATDB_EXPERIMENTAL_FIXED_LOG_BYTES"),
            std::env::var_os("CATDB_EXPERIMENTAL_REDO_LOG_BYTES"),
            std::env::var_os("CATDB_MIGRATE_WAL"),
        ) {
            (None, None, None) => None,
            (Some(bytes), None, None) => Some(CheckpointLog::Fixed(
                bytes.to_str().ok_or("invalid fixed log size")?.parse()?,
            )),
            (None, Some(bytes), None) => Some(CheckpointLog::Redo(
                bytes.to_str().ok_or("invalid redo log size")?.parse()?,
            )),
            (None, None, Some(mode)) if mode == "managed" => Some(CheckpointLog::DEFAULT_MANAGED),
            (None, None, Some(_)) => return Err("CATDB_MIGRATE_WAL must be managed".into()),
            _ => return Err("select one WAL format".into()),
        };
        let listener = TcpListener::bind(config.bind)?;
        listener.set_nonblocking(true)?;
        let db = if config.database.try_exists()? {
            Database::open_with_limits(&config.database, limits)?
        } else if config.initialize {
            let parent = config
                .database
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or_else(|| std::path::Path::new("."));
            if range_writeback {
                Database::check_range_writeback_path(parent)?;
            }
            let id = SystemTime::now()
                .duration_since(UNIX_EPOCH)?
                .as_nanos()
                .to_le_bytes();
            let db = if let Some(layout) = requested_log {
                Database::create_with_log(&config.database, id, limits, Some(layout))?
            } else {
                Database::create_with_limits(&config.database, id, limits)?
            };
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
            db
        } else {
            return Err(
                "database absent; use --init to create (existing files are never overwritten)"
                    .into(),
            );
        };
        db.set_checkpoint_range_writeback(range_writeback)?;
        match requested_log {
            Some(CheckpointLog::ManagedRedo(_)) if !db.uses_managed_log()? => {
                db.checkpoint_managed_log()?;
            }
            Some(CheckpointLog::Redo(capacity)) if db.uses_redo_log()? => {
                if db.fixed_log_capacity()? != Some(capacity) {
                    return Err("redo log capacity differs; use explicit checkpoint API".into());
                }
                if db.uses_managed_log()? {
                    db.checkpoint_redo_log(capacity)?;
                }
            }
            Some(CheckpointLog::Redo(capacity)) => {
                db.checkpoint_redo_log(capacity)?;
            }
            Some(CheckpointLog::Fixed(capacity)) => {
                if db.uses_redo_log()? {
                    return Err("existing WAL uses redo-witness format".into());
                }
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
            _ => {}
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
        let accounts = options
            .map(|opts| Accounts::open(&config, &db, limits, opts).map(Arc::new))
            .transpose()?;
        Ok(Self {
            sql,
            accounts,
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
                    let accounts = self.accounts.clone();
                    let sql = self.sql.clone();
                    match thread::Builder::new()
                        .name(client_thread_name())
                        .spawn(move || {
                            struct Permit(Arc<AtomicUsize>);
                            impl Drop for Permit {
                                fn drop(&mut self) {
                                    self.0.fetch_sub(1, Ordering::AcqRel);
                                }
                            }
                            let _permit = Permit(count);
                            let _ = connection(
                                stream,
                                db,
                                &config,
                                &stopping,
                                accounts.as_deref(),
                                sql.as_deref(),
                            );
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
    stream: &mut impl Read,
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

fn receive(stream: &mut impl Read, deadline: Instant, stop: &AtomicBool) -> io::Result<Value> {
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
    stream: &mut impl Write,
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
fn error_code(e: &Error) -> &'static str {
    match e {
        Error::Conflict(_) => "conflict",
        Error::Invalid(_) => "invalid_request",
        Error::Limit(_) => "limit",
        Error::ResourceExhausted(_) => "resource_exhausted",
        Error::CommitUncertain(_) => "commit_uncertain",
        Error::CheckpointUncertain(_) => "checkpoint_uncertain",
        Error::Poisoned => "reopen_required",
        Error::Io(_) | Error::Locked(_) => "storage_error",
    }
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
    #[cfg(feature = "retirement-timing")]
    retirement: Option<[u64; 6]>,
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
        let value = json!({
            "request_index": self.request_index,
            "dispatch_ns": self.dispatch_ns,
            "finished_ns": self.finished_ns,
            "trace": self.trace,
            "detail": self.detail,
            "costs": self.costs,
            "sync_attempts": self.sync_attempts,
            "commit_ok": self.commit_ok,
        });
        #[cfg(feature = "retirement-timing")]
        let value = {
            let mut v = value;
            v["retirement"] = json!(self.retirement);
            v
        };
        value
    }
}

#[cfg(feature = "lock-timing")]
thread_local! {
    static TAIL_ITEMS: std::cell::RefCell<Vec<CommitDiagnostic>> = const { std::cell::RefCell::new(Vec::new()) };
    static TAIL_SYNC: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) };
    static TAIL_ENABLED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
#[cfg(feature = "lock-timing")]
struct TailRecord {
    index: u64,
    start: u64,
    end: u64,
    sent: u64,
    items: Vec<CommitDiagnostic>,
    sync_starts: Vec<u64>,
}
#[cfg(feature = "lock-timing")]
impl TailRecord {
    fn json(&self) -> Value {
        json!({"request_index":self.index,"start_ns":self.start,"end_ns":self.end,
            "sent_ns":self.sent,"items":self.items.iter().map(CommitDiagnostic::json).collect::<Vec<_>>(),
            "sync_starts":self.sync_starts})
    }
}
#[cfg(feature = "lock-timing")]
#[derive(Default)]
struct TailBuffer {
    records: std::collections::VecDeque<TailRecord>,
    dropped: u64,
}
#[cfg(feature = "lock-timing")]
impl TailBuffer {
    fn retain(&mut self, record: TailRecord) {
        if record.sent.saturating_sub(record.start) < 10_000_000 {
            return;
        }
        if self.records.len() == 8192 {
            self.dropped += 1;
        } else {
            self.records.push_back(record);
        }
    }
    fn drain(&mut self) -> Value {
        let rows: Vec<Value> = self
            .records
            .drain(..self.records.len().min(128))
            .map(|r| r.json())
            .collect();
        json!({"ok":true,"records":rows,"remaining":self.records.len(),"dropped":self.dropped,"threshold_ns":10_000_000,"capacity":8192})
    }
}

#[cfg(feature = "wire-buffer")]
#[path = "service_wire.rs"]
mod buffered_wire;
#[cfg(feature = "wire-buffer")]
use buffered_wire::Wire;
#[cfg(not(feature = "wire-buffer"))]
#[derive(Default)]
struct Wire {}
#[cfg(not(feature = "wire-buffer"))]
impl Wire {
    fn receive(
        &mut self,
        stream: &mut impl Read,
        deadline: Instant,
        stop: &AtomicBool,
    ) -> io::Result<Value> {
        receive(stream, deadline, stop)
    }
    fn send(
        &mut self,
        stream: &mut impl Write,
        value: &Value,
        deadline: Instant,
        stop: &AtomicBool,
    ) -> io::Result<()> {
        send(stream, value, deadline, stop)
    }
}

fn connection(
    stream: TcpStream,
    db: Database,
    config: &Config,
    stop: &AtomicBool,
    accounts: Option<&Accounts>,
    sql: Option<&SqlOptions>,
) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    stream.set_write_timeout(Some(Duration::from_millis(100)))?;
    let deadline = Instant::now() + config.request_timeout.min(Duration::from_secs(5));
    let clock = std::cell::Cell::new(deadline);
    if let Some(accounts) = accounts {
        let peer = stream.peer_addr()?.ip();
        let conn = rustls::ServerConnection::new(accounts.tls.clone()).map_err(io::Error::other)?;
        let socket = tls::TimedSocket {
            socket: stream,
            deadline: &clock,
            stop,
        };
        return session(
            rustls::StreamOwned::new(conn, socket),
            db,
            config,
            stop,
            Some((accounts, peer)),
            &clock,
            sql,
        );
    }
    session(stream, db, config, stop, None, &clock, sql)
}

fn session(
    mut stream: impl Read + Write,
    mut db: Database,
    config: &Config,
    stop: &AtomicBool,
    accounts: Option<(&Accounts, std::net::IpAddr)>,
    clock: &std::cell::Cell<Instant>,
    sql: Option<&SqlOptions>,
) -> io::Result<()> {
    let deadline = clock.get();
    let mut wire = Wire::default();
    let auth = wire.receive(&mut stream, deadline, stop)?;
    let mut view_role = None;
    let mut principal = if let Some((accounts, peer)) = accounts {
        let Some((selected, principal)) = accounts.login(&auth, peer) else {
            wire.send(&mut stream, &error("unauthorized"), deadline, stop)?;
            stream.flush()?;
            return Ok(());
        };
        db = selected;
        Some(principal)
    } else {
        let token = auth.get("token").and_then(Value::as_str).unwrap_or("");
        let admin = same_token(token.as_bytes(), &config.token);
        view_role = sql
            .and_then(|s| s.read_role.as_ref())
            .filter(|role| same_token(token.as_bytes(), &role.token))
            .map(|role| role.name.as_str());
        if auth.get("op").and_then(Value::as_str) != Some("auth")
            || auth.get("version").and_then(Value::as_u64) != Some(1)
            || (!admin && view_role.is_none())
        {
            wire.send(&mut stream, &error("unauthorized"), deadline, stop)?;
            return Ok(());
        }
        None
    };
    drop(auth);
    let mut authenticated = match principal.as_ref() {
        Some(user) => json!({"ok":true,"protocol":2,"account_id":hex(&user.id)}),
        None => json!({"ok":true,"protocol":1}),
    };
    if let Some(user) = &principal {
        authenticated["role"] = match user.role {
            Role::Reader => "reader",
            Role::Writer => "writer",
            Role::Admin => "admin",
        }
        .into();
    }
    if sql.is_some() {
        authenticated["sql"] = principal
            .as_ref()
            .is_none_or(|u| u.role.permits("sql"))
            .into();
    }
    if let Some(role) = view_role {
        authenticated["read_role"] = role.into();
    }
    wire.send(&mut stream, &authenticated, deadline, stop)?;
    stream.flush()?;
    #[cfg(feature = "lock-timing")]
    let buffered = std::env::var("CATDB_DIAGNOSTICS").as_deref() == Ok("buffer");
    #[cfg(feature = "lock-timing")]
    TAIL_ENABLED.with(|value| value.set(buffered));
    #[cfg(feature = "lock-timing")]
    let mut tail_buffer = TailBuffer::default();
    #[cfg(feature = "lock-timing")]
    let diagnostic = std::env::var_os("CATDB_DIAGNOSTICS").is_some() && !buffered;
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
        clock.set(deadline);
        let mut request = wire.receive(&mut stream, deadline, stop)?;
        if Instant::now() >= deadline || stop.load(Ordering::Acquire) {
            break;
        }
        if view_role.is_some() && !matches!(request["op"].as_str(), Some("sql" | "ping")) {
            wire.send(&mut stream, &error("forbidden"), deadline, stop)?;
            stream.flush()?;
            continue;
        }
        if let (Some((accounts, _)), Some(user)) = (accounts, principal.as_mut()) {
            if !accounts.current(&db, user) {
                wire.send(&mut stream, &error("unauthorized"), deadline, stop)?;
                stream.flush()?;
                break;
            }
            let op = request["op"].as_str().unwrap_or("");
            let response = if !user.role.permits(op) || request.get("database").is_some() {
                let mut denied = error("forbidden");
                if op == "sql" {
                    denied["in_transaction"] = transaction.is_some().into();
                }
                Some(denied)
            } else if matches!(op, "create_user" | "set_role") {
                Some(if transaction.is_some() {
                    error("invalid_request")
                } else {
                    accounts.manage(&db, user, &request)
                })
            } else {
                None
            };
            if let Some(response) = response {
                wire.send(&mut stream, &response, deadline, stop)?;
                stream.flush()?;
                continue;
            }
            if op == "begin" && user.role == Role::Reader {
                request["op"] = json!("begin_read");
            }
        }
        #[cfg(feature = "lock-timing")]
        {
            if buffered && request.get("op").and_then(Value::as_str) == Some("diagnostic_drain") {
                wire.send(&mut stream, &tail_buffer.drain(), deadline, stop)?;
                continue;
            }
            if pull_diagnostic
                && request.get("op").and_then(Value::as_str) == Some("diagnostic_last")
            {
                let response = json!({"ok": true, "diagnostic": last_diagnostic.as_ref().map(CommitDiagnostic::json)});
                wire.send(&mut stream, &response, deadline, stop)?;
                continue;
            }
            request_index = request_index.wrapping_add(1);
            last_diagnostic = None;
        }
        #[cfg(feature = "lock-timing")]
        let dispatch_started = {
            #[cfg(feature = "retirement-timing")]
            crate::transactions::take_retirement_trace();
            crate::transactions::take_commit_trace();
            crate::transactions::take_commit_detail();
            crate::transactions::take_commit_costs();
            crate::transactions::take_commit_sync_start();
            TAIL_ITEMS.with(|items| items.borrow_mut().clear());
            TAIL_SYNC.with(|items| items.borrow_mut().clear());
            crate::transactions::diagnostic_now_ns()
        };
        let sql_request = request["op"].as_str() == Some("sql");
        let result = if sql_request {
            if let Some(options) = sql {
                sql::execute(
                    &db,
                    &mut transaction,
                    &request,
                    options,
                    view_role,
                    config.transaction_timeout,
                    deadline,
                    &mut expires,
                    stop,
                    || match (accounts, principal.as_mut()) {
                        (Some((accounts, _)), Some(user)) => {
                            accounts.current(&db, user) && user.role.permits("sql")
                        }
                        _ => true,
                    },
                )
            } else {
                Ok(
                    json!({"ok":false,"error":"sql_unavailable","in_transaction":transaction.is_some()}),
                )
            }
        } else {
            dispatch(&db, &mut transaction, &request)
        };
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
        #[cfg(feature = "backlog-batch")]
        let fatal = fatal
            || result
                .as_ref()
                .is_ok_and(|v| v.get("reopen_required").and_then(Value::as_bool) == Some(true));
        let response = match result {
            Ok(value) => value,
            Err(e) => {
                let mut response = error(error_code(&e));
                if sql_request {
                    response["in_transaction"] = transaction.is_some().into();
                }
                response
            }
        };
        #[cfg(feature = "lock-timing")]
        let response = if diagnostic
            && matches!(
                request.get("op").and_then(Value::as_str),
                Some("commit" | "put_commit" | "delete_commit")
            ) {
            let record = CommitDiagnostic {
                #[cfg(feature = "retirement-timing")]
                retirement: Some(crate::transactions::take_retirement_trace()),
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
        let response_deadline = Instant::now() + config.request_timeout;
        clock.set(response_deadline);
        wire.send(&mut stream, &response, response_deadline, stop)?;
        stream.flush()?;
        #[cfg(feature = "lock-timing")]
        if buffered
            && matches!(
                request.get("op").and_then(Value::as_str),
                Some("put_batch_commit" | "put_commit" | "delete_commit" | "commit")
            )
        {
            let sent = crate::transactions::diagnostic_now_ns();
            if sent.saturating_sub(dispatch_started) >= 10_000_000 {
                let mut items = TAIL_ITEMS.with(|items| std::mem::take(&mut *items.borrow_mut()));
                let mut sync_starts =
                    TAIL_SYNC.with(|items| std::mem::take(&mut *items.borrow_mut()));
                if items.is_empty() {
                    items.push(CommitDiagnostic {
                        #[cfg(feature = "retirement-timing")]
                        retirement: Some(crate::transactions::take_retirement_trace()),
                        request_index,
                        dispatch_ns: dispatch_finished.saturating_sub(dispatch_started),
                        finished_ns: dispatch_finished,
                        trace: crate::transactions::take_commit_trace(),
                        detail: crate::transactions::take_commit_detail(),
                        costs: crate::transactions::take_commit_costs(),
                        sync_attempts: db.commit_sync_attempts(),
                        commit_ok: response.get("ok").and_then(Value::as_bool) == Some(true),
                    });
                    sync_starts.push(crate::transactions::take_commit_sync_start());
                }
                tail_buffer.retain(TailRecord {
                    index: request_index,
                    start: dispatch_started,
                    end: dispatch_finished,
                    sent,
                    items,
                    sync_starts,
                });
            }
        }
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
    let pairs = value.as_bytes().as_chunks::<2>().0;
    let mut decoded = Vec::with_capacity(pairs.len());
    for pair in pairs {
        decoded.push(digit(pair[0])? * 16 + digit(pair[1])?);
    }
    Ok(decoded)
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

fn stage_request(tx: Option<&mut Transaction>, v: &Value, put: bool) -> Result<(), Error> {
    let key = bytes(v, "key", 1024)?;
    let tx = tx.ok_or(Error::Invalid("no transaction"))?;
    let value = if put {
        Some(bytes(v, "value", MAX_VALUE)?.into())
    } else {
        None
    };
    tx.stage(key.into(), value)
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
        #[cfg(feature = "benchmark-metrics")]
        "benchmark_counters" => {
            #[allow(unused_mut)]
            let mut response = json!({"ok":true,"version":db.current_version()?.to_string(),"sync_attempts":db.commit_sync_attempts()});
            #[cfg(feature = "sync-stall")]
            {
                response["injected_stalls"] = json!(db.diagnostic_stall_count());
                response["injected_stall_ns"] = json!(db.diagnostic_stall_timing());
            }
            Ok(response)
        }
        #[cfg(all(feature = "benchmark-metrics", feature = "sync-stall"))]
        "benchmark_stall" => {
            if tx.is_some() || std::env::var("CATDB_BENCH_STALL").as_deref() != Ok("1") {
                return Err(Error::Invalid(
                    "diagnostic stall disabled or transaction active",
                ));
            }
            let field = |name: &str, max: u64| -> Result<u64, Error> {
                v.get(name)
                    .and_then(Value::as_u64)
                    .filter(|n| *n <= max)
                    .ok_or(Error::Invalid("stall limit"))
            };
            let start = field("start_ms", 30000)?;
            let duration = field("duration_ms", 10000)?;
            let delay = field("delay_ms", 1000)?;
            let count = field("count", 10000)?;
            db.set_diagnostic_sync_stall_limit(
                Duration::from_millis(start),
                Duration::from_millis(duration),
                Duration::from_millis(delay),
                Some(count),
            );
            Ok(json!({"ok":true}))
        }
        #[cfg(feature = "backlog-batch")]
        "put_batch_commit" => {
            if tx.is_some() {
                return Err(Error::Invalid("batch requires no transaction"));
            }
            let entries = v
                .get("entries")
                .and_then(Value::as_array)
                .filter(|x| !x.is_empty() && x.len() <= 8)
                .ok_or(Error::Limit("batch count"))?;
            let mut decoded = Vec::with_capacity(entries.len());
            let mut size = 0_usize;
            for entry in entries {
                let key = bytes(entry, "key", 1024)?;
                let value = bytes(entry, "value", MAX_VALUE)?;
                size += key.len() + value.len();
                if size > MAX_VALUE {
                    return Err(Error::Limit("batch bytes"));
                }
                if decoded.iter().any(|(k, _)| k == &key) {
                    return Err(Error::Invalid("duplicate batch key"));
                }
                decoded.push((key, value));
            }
            let mut transactions = Vec::with_capacity(decoded.len());
            for (key, value) in decoded {
                let mut transaction = db.begin()?;
                transaction.stage(key.into(), Some(value.into()))?;
                transactions.push(transaction);
            }
            let mut reopen = false;
            let results: Vec<Value> = db
                .commit_batch(transactions)?
                .into_iter()
                .map(|outcome| {
                    #[cfg(feature = "lock-timing")]
                    if TAIL_ENABLED.with(|v| v.get()) {
                        TAIL_SYNC.with(|items| items.borrow_mut().push(outcome.sync_start));
                        TAIL_ITEMS.with(|items| {
                            items.borrow_mut().push(CommitDiagnostic {
                                #[cfg(feature = "retirement-timing")]
                                retirement: None,
                                request_index: 0,
                                dispatch_ns: 0,
                                finished_ns: outcome.detail[6],
                                trace: outcome.trace,
                                detail: outcome.detail,
                                costs: outcome.costs,
                                sync_attempts: 0,
                                commit_ok: outcome.result.is_ok(),
                            })
                        });
                    }
                    match outcome.result {
                        Ok(version) => json!({"ok":true,"version":version.to_string()}),
                        Err(e) => {
                            let code = match e {
                                Error::Conflict(_) => "conflict",
                                Error::Invalid(_) => "invalid_request",
                                Error::Limit(_) => "limit",
                                Error::ResourceExhausted(_) => "resource_exhausted",
                                Error::CommitUncertain(_) => {
                                    reopen = true;
                                    "commit_uncertain"
                                }
                                Error::CheckpointUncertain(_) => {
                                    reopen = true;
                                    "checkpoint_uncertain"
                                }
                                Error::Poisoned => {
                                    reopen = true;
                                    "reopen_required"
                                }
                                Error::Io(_) | Error::Locked(_) => "storage_error",
                            };
                            error(code)
                        }
                    }
                })
                .collect();
            Ok(json!({"ok":true,"results":results,"reopen_required":reopen}))
        }
        "ping" => Ok(json!({"ok":true,"version":db.current_version()?.to_string()})),
        "get_latest" | "scan_latest" | "put_commit" | "delete_commit" => {
            if tx.is_some() {
                return Err(Error::Invalid("single request requires no transaction"));
            }
            if matches!(op, "put_commit" | "delete_commit") {
                let mut local = db.begin()?;
                stage_request(Some(&mut local), v, op == "put_commit")?;
                let version = local.commit()?;
                Ok(json!({"ok":true,"version":version.to_string()}))
            } else {
                let operation = if op == "get_latest" { "get" } else { "scan" };
                dispatch_operation(db, &mut Some(db.begin_read()?), v, operation)
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
            stage_request(tx.as_mut(), v, op == "put")?;
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
            Ok(scan_response(&page))
        }
        _ => Err(Error::Invalid("unknown operation")),
    }
}

fn scan_response(page: &crate::transactions::ScanPage) -> Value {
    let rows: Value = page
        .rows
        .iter()
        .map(|(key, value)| Value::from([hex(key), hex(value)]))
        .collect();
    let mut response = serde_json::Map::new();
    response.insert("ok".into(), true.into());
    response.insert("rows".into(), rows);
    response.insert(
        "next_after".into(),
        page.next_after.as_deref().map(hex).into(),
    );
    response.insert("visited".into(), page.visited_entries.into());
    Value::Object(response)
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
    #[ignore = "compile with CATDB_ELISION_ALLOC=1 for allocation measurements"]
    fn scan_response_moves_encoded_rows_without_extra_payload_copies() {
        use crate::work_elision_metrics as meter;
        assert!(meter::counted(), "set CATDB_ELISION_ALLOC=1 at build time");
        let db = Fixture::new().db();
        let mut tx = db.begin().unwrap();
        for id in 0..101u64 {
            tx.put(&id.to_be_bytes(), &[id as u8; 256]).unwrap();
        }
        tx.commit().unwrap();
        let page = db
            .begin_read()
            .unwrap()
            .scan_page(
                b"",
                None,
                None,
                ScanLimits {
                    max_rows: 100,
                    ..ScanLimits::default()
                },
            )
            .unwrap();
        assert_eq!(page.rows.len(), 100);
        assert!(page.next_after.is_some());

        meter::start();
        let rows: Vec<_> = page
            .rows
            .iter()
            .map(|(key, value)| json!([hex(key), hex(value)]))
            .collect();
        let previous = json!({
            "ok": true, "rows": rows,
            "next_after": page.next_after.as_deref().map(hex), "visited": page.visited_entries
        });
        let (old_calls, old_bytes) = meter::stop();

        meter::start();
        let current = scan_response(&page);
        let (new_calls, new_bytes) = meter::stop();
        assert_eq!(
            serde_json::to_vec(&current).unwrap(),
            serde_json::to_vec(&previous).unwrap()
        );
        println!(
            "allocations {old_calls} -> {new_calls}, requested bytes {old_bytes} -> {new_bytes}"
        );
        assert!(new_calls < old_calls);
        assert!(new_bytes < old_bytes);
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
    fn eq_explicit_service_budgets_preserve_legacy_config_and_reject_zero_before_creation() {
        let fixture = Fixture::new();
        let config = || Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            database: fixture.0.join("limits.ndb"),
            token: vec![b'x'; 32],
            initialize: true,
            allow_remote: false,
            max_connections: 12,
            request_timeout: Duration::from_secs(1),
            transaction_timeout: Duration::from_secs(1),
        };
        let bad = ResourceLimits {
            max_retained_bytes: 0,
            ..ResourceLimits::default()
        };
        assert!(Server::bind_with_limits(config(), bad).is_err());
        assert!(!config().database.exists());
        let server = Server::bind(config()).unwrap();
        let expected = ResourceLimits {
            max_transactions: 12,
            ..ResourceLimits::default()
        };
        assert_eq!(
            server.db.resource_usage().unwrap().limits.values(),
            expected.values()
        );
        drop(server);
        let limits = ResourceLimits {
            max_transactions: 2,
            max_staged_bytes: 1024,
            max_dependency_bytes: 512,
            max_retained_bytes: 2048,
            max_buffer_bytes: 4096,
        };
        let server = Server::bind_with_limits(config(), limits).unwrap();
        assert_eq!(
            server.db.resource_usage().unwrap().limits.values(),
            limits.values()
        );
        let _first = server.db.begin().unwrap();
        let _second = server.db.begin().unwrap();
        assert!(matches!(
            server.db.begin(),
            Err(Error::ResourceExhausted(_))
        ));
    }

    #[test]
    fn sql_capability_follows_native_authority() {
        assert!(Role::Admin.permits("sql"));
        assert!(!Role::Reader.permits("sql"));
        assert!(!Role::Writer.permits("sql"));
        let options = SqlOptions {
            host: std::env::current_exe().unwrap(),
            read_role: Some(ReadRole {
                name: "published".into(),
                token: vec![b'r'; 32],
            }),
        };
        assert!(options.validate(&[b'x'; 32], false).is_ok());
        assert!(options.validate(&[b'r'; 32], false).is_err());
        assert!(options.validate(&[], true).is_err());
    }

    #[test]
    #[ignore = "set CATDB_SQL_HOST to the built Catdb.Sql.Host executable"]
    fn sql_host_frames_use_session_transaction_and_authenticated_view_role() {
        struct Script {
            input: std::io::Cursor<Vec<u8>>,
            output: Vec<u8>,
        }
        impl Read for Script {
            fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
                self.input.read(b)
            }
        }
        impl Write for Script {
            fn write(&mut self, b: &[u8]) -> io::Result<usize> {
                self.output.write(b)
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let fixture = Fixture::new();
        let db = fixture.db();
        let config = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            database: fixture.0.join("state.ndb"),
            token: vec![b'x'; 32],
            initialize: false,
            allow_remote: false,
            max_connections: 4,
            request_timeout: Duration::from_secs(10),
            transaction_timeout: Duration::from_secs(10),
        };
        let options = SqlOptions {
            host: PathBuf::from(std::env::var_os("CATDB_SQL_HOST").expect("CATDB_SQL_HOST")),
            read_role: Some(ReadRole {
                name: "published".into(),
                token: vec![b'r'; 32],
            }),
        };
        options.validate(&config.token, false).unwrap();
        let run = |token: &str, requests: &[Value]| {
            let mut bytes = Vec::new();
            let stop = AtomicBool::new(false);
            let deadline = Instant::now() + Duration::from_secs(30);
            send(
                &mut bytes,
                &json!({"op":"auth","version":1,"token":token}),
                deadline,
                &stop,
            )
            .unwrap();
            for request in requests {
                send(&mut bytes, request, deadline, &stop).unwrap();
            }
            let mut stream = Script {
                input: std::io::Cursor::new(bytes),
                output: Vec::new(),
            };
            let error = session(
                &mut stream,
                db.clone(),
                &config,
                &stop,
                None,
                &std::cell::Cell::new(deadline),
                Some(&options),
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
            let mut output = std::io::Cursor::new(stream.output);
            (0..=requests.len())
                .map(|_| receive(&mut output, deadline, &stop).unwrap())
                .collect::<Vec<_>>()
        };
        let admin = run(
            &"x".repeat(32),
            &[
                json!({"op":"sql","sql":"CREATE TABLE t(id INT); INSERT INTO t VALUES (1); CREATE VIEW v AS SELECT id FROM t; GRANT SELECT ON v TO published"}),
                json!({"op":"begin"}),
                json!({"op":"sql","sql":"INSERT INTO t VALUES (2)"}),
                json!({"op":"rollback"}),
            ],
        );
        assert!(admin.iter().all(|r| r["ok"] == true), "{admin:?}");
        assert_eq!(admin[0]["sql"], true);
        assert_eq!(admin[3]["in_transaction"], true);
        let reader = run(
            &"r".repeat(32),
            &[
                json!({"op":"get_latest","key":""}),
                json!({"op":"put_commit","key":"","value":"01"}),
                json!({"op":"diagnostic_last"}),
                json!({"op":"sql","role":"admin","sql":"SELECT id FROM t"}),
                json!({"op":"sql","sql":"SELECT id FROM v"}),
                json!({"op":"sql","sql":"SHOW VIEWS"}),
            ],
        );
        assert_eq!(reader[0]["read_role"], "published");
        for denied in &reader[1..=3] {
            assert_eq!(denied["error"], "forbidden");
        }
        assert_eq!(reader[4]["ok"], false);
        assert_eq!(
            reader[5]["results"][0]["Rows"],
            json!([[{"Kind":"NUMBER","Text":"1"}]])
        );
        assert_eq!(reader[6]["results"][0]["Rows"][0][0]["Text"], "v");
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

#[cfg(all(test, feature = "lock-timing"))]
mod tail_buffer_tests {
    use super::*;
    fn record(duration: u64) -> TailRecord {
        TailRecord {
            index: 1,
            start: 1,
            end: duration + 1,
            sent: duration + 1,
            items: Vec::new(),
            sync_starts: Vec::new(),
        }
    }
    #[test]
    fn bv_tail_threshold_retains_at_and_above_ten_milliseconds() {
        let mut buffer = TailBuffer::default();
        buffer.retain(record(9_999_999));
        assert!(buffer.records.is_empty());
        buffer.retain(record(10_000_000));
        buffer.retain(record(10_000_001));
        assert_eq!(buffer.records.len(), 2);
    }
    #[test]
    fn bv_tail_capacity_reports_loss_and_drain_reuses_slots() {
        let mut buffer = TailBuffer::default();
        for _ in 0..8192 {
            buffer.retain(record(10_000_000));
        }
        assert_eq!(buffer.dropped, 0);
        buffer.retain(record(10_000_000));
        assert_eq!(buffer.dropped, 1);
        assert_eq!(buffer.records.len(), 8192);
        assert_eq!(buffer.drain()["records"].as_array().unwrap().len(), 128);
        buffer.retain(record(10_000_000));
        assert_eq!(buffer.records.len(), 8065);
        while !buffer.records.is_empty() {
            buffer.drain();
        }
        assert_eq!(buffer.drain()["remaining"], 0);
        assert_eq!(buffer.dropped, 1);
    }
}

fn client_thread_name() -> String {
    #[cfg(feature = "benchmark-metrics")]
    {
        benchmark_thread_name(std::env::var("CATDB_BENCH_THREAD_NAME").ok().as_deref())
    }
    #[cfg(not(feature = "benchmark-metrics"))]
    {
        "catdb-client".into()
    }
}

#[cfg(feature = "benchmark-metrics")]
fn benchmark_thread_name(requested: Option<&str>) -> String {
    requested
        .filter(|name| {
            (1..=15).contains(&name.len())
                && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
        .unwrap_or("catdb-client")
        .into()
}

#[cfg(all(test, feature = "benchmark-metrics"))]
mod observer_name_tests {
    #[test]
    fn bv_observer_name_respects_kernel_length_and_rejects_unsafe_filters() {
        for value in [
            None,
            Some(""),
            Some("abcdefghijklmnop"),
            Some("x\"||"),
            Some("日本語"),
        ] {
            assert_eq!(super::benchmark_thread_name(value), "catdb-client");
        }
        for value in ["a", "as-123456789abc", "abcdefghijklmno"] {
            assert_eq!(super::benchmark_thread_name(Some(value)), value);
        }
    }
}

#[cfg(feature = "wire-metrics")]
#[doc(hidden)]
pub fn wire_probe(
    iterations: usize,
    begin: impl FnOnce(),
    end: impl FnOnce(),
) -> io::Result<(usize, usize)> {
    struct Reader {
        input: std::io::Cursor<Vec<u8>>,
        calls: usize,
    }
    impl Read for Reader {
        fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            self.calls += 1;
            self.input.read(bytes)
        }
    }
    struct Writer {
        calls: usize,
    }
    impl Write for Writer {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.calls += 1;
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let body = serde_json::to_vec(&json!({"op":"get_latest","key":"0000000000000001"}))?;
    let mut frame = (body.len() as u32).to_be_bytes().to_vec();
    frame.extend(body);
    let mut input = Reader {
        input: std::io::Cursor::new(frame),
        calls: 0,
    };
    let mut output = Writer { calls: 0 };
    let mut wire = Wire::default();
    let response = json!({"ok":true,"value":"ab".repeat(256)});
    let stop = AtomicBool::new(false);
    let deadline = Instant::now() + Duration::from_secs(60);
    wire.receive(&mut input, deadline, &stop)?;
    wire.send(&mut output, &response, deadline, &stop)?;
    input.calls = 0;
    output.calls = 0;
    begin();
    for _ in 0..iterations {
        input.input.set_position(0);
        let request = wire.receive(&mut input, deadline, &stop)?;
        wire.send(&mut output, &response, deadline, &stop)?;
        drop(request);
    }
    end();
    Ok((input.calls, output.calls))
}
