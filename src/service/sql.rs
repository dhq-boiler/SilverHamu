//! A configured SQL interpreter plans statements. The server owns every transaction.
use super::{MAX_RESPONSE, dispatch, error_code, same_token};
use crate::transactions::{Database, Error, Transaction};
use serde_json::{Value, json};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub struct ReadRole {
    pub name: String,
    pub token: Vec<u8>,
}

pub struct SqlOptions {
    pub host: PathBuf,
    pub read_role: Option<ReadRole>,
}

impl SqlOptions {
    pub(super) fn validate(&self, admin_token: &[u8], accounts: bool) -> io::Result<()> {
        if !self.host.is_absolute() || !self.host.is_file() {
            return Err(io::Error::other(
                "--sql-host requires an absolute executable file path",
            ));
        }
        #[cfg(windows)]
        if !self
            .host
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("exe"))
        {
            return Err(io::Error::other("--sql-host requires an executable (.exe)"));
        }
        if let Some(role) = &self.read_role
            && (accounts
                || role.name.trim().is_empty()
                || role.name.encode_utf16().count() > 128
                || role.name.chars().any(char::is_control)
                || !(32..=256).contains(&role.token.len())
                || same_token(admin_token, &role.token))
        {
            return Err(io::Error::other(
                "a view token requires token mode, a valid role name and a distinct token",
            ));
        }
        Ok(())
    }
}

const HOST_LINE_LIMIT: usize = 8 * 1024 * 1024;
const MAX_SQL_HOSTS: usize = 4;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::AcqRel);
    }
}

struct Host(Child);
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn failure(message: &str, tx: &Option<Transaction>) -> Value {
    json!({"ok":false,"error":message,"in_transaction":tx.is_some()})
}

fn send_line(output: &mut impl Write, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *output, value)?;
    output.write_all(b"\n")?;
    output.flush()
}

fn receive_line(input: &mut impl BufRead) -> io::Result<Value> {
    let mut bytes = Vec::new();
    input
        .take(HOST_LINE_LIMIT as u64 + 1)
        .read_until(b'\n', &mut bytes)?;
    if bytes.last() != Some(&b'\n') {
        return Err(io::Error::other("SQL host frame missing or oversized"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn execute(
    db: &Database,
    tx: &mut Option<Transaction>,
    request: &Value,
    options: &SqlOptions,
    view_role: Option<&str>,
    transaction_timeout: Duration,
    deadline: Instant,
    expires: &mut Option<Instant>,
    stop: &AtomicBool,
    mut authorized: impl FnMut() -> bool,
) -> Result<Value, Error> {
    let Some(sql) = request["sql"].as_str() else {
        return Ok(failure("invalid_request", tx));
    };
    let previous = ACTIVE.fetch_add(1, Ordering::AcqRel);
    let _permit = Permit;
    if previous >= MAX_SQL_HOSTS {
        return Ok(failure("sql_host_busy", tx));
    }
    let mut command = Command::new(&options.host);
    command
        .arg("bridge")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let Ok(child) = command.spawn() else {
        return Ok(failure("sql_host_unavailable", tx));
    };
    let mut host = Host(child);
    let mut input = BufReader::new(host.0.stdout.take().expect("piped stdout"));
    let mut output = BufWriter::new(host.0.stdin.take().expect("piped stdin"));
    // The watchdog owns the process. Killing it releases both blocked pipe reads and writes.
    let deadline = deadline.min(Instant::now() + transaction_timeout);
    let exchange = thread::scope(|scope| -> io::Result<Result<Value, Error>> {
        let (done, finished) = mpsc::channel::<()>();
        let _watchdog = thread::Builder::new()
            .name("catdb-sql-deadline".into())
            .spawn_scoped(scope, move || {
                let _host = host;
                while !stop.load(Ordering::Acquire) {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero()
                        || !matches!(
                            finished.recv_timeout(remaining.min(Duration::from_millis(50))),
                            Err(mpsc::RecvTimeoutError::Timeout)
                        )
                    {
                        break;
                    }
                }
            })?;
        let _finish_on_return = done;
        let mut command = json!({"sql":sql,"in_transaction":tx.is_some()});
        if let Some(role) = view_role {
            command["role"] = role.into();
        }
        send_line(&mut output, &command)?;
        loop {
            let mut reply = receive_line(&mut input)?;
            if !authorized() {
                tx.take();
                *expires = None;
                return Ok(Ok(failure("unauthorized", tx)));
            }
            // An older SQL host ignores the role field. Never give it access to role-bound data.
            if let Some(role) = view_role
                && reply["read_role"].as_str() != Some(role)
            {
                return Err(io::Error::other(
                    "SQL host does not support role-bound queries",
                ));
            }
            if Instant::now() >= deadline || stop.load(Ordering::Acquire) {
                return Err(io::ErrorKind::TimedOut.into());
            }
            if let Some(op) = reply.get("request") {
                if view_role.is_some()
                    && !matches!(
                        op["op"].as_str(),
                        Some("begin" | "rollback" | "get" | "scan")
                    )
                {
                    return Err(io::Error::other("invalid read-only SQL callback"));
                }
                if !matches!(
                    op["op"].as_str(),
                    Some("begin" | "commit" | "rollback" | "get" | "put" | "delete" | "scan")
                ) {
                    return Err(io::Error::other("invalid SQL callback"));
                }
                let response = match dispatch(db, tx, op) {
                    Ok(value) => value,
                    Err(
                        e @ (Error::CommitUncertain(_)
                        | Error::CheckpointUncertain(_)
                        | Error::Poisoned),
                    ) => {
                        tx.take();
                        return Ok(Err(e));
                    }
                    Err(e) => super::error(error_code(&e)),
                };
                if tx.is_none() {
                    *expires = None;
                } else if expires.is_none() {
                    *expires = Some(Instant::now() + transaction_timeout);
                }
                send_line(
                    &mut output,
                    &json!({"response":response,"in_transaction":tx.is_some()}),
                )?;
                continue;
            }
            if reply["in_transaction"].as_bool() != Some(tx.is_some()) {
                return Err(io::Error::other("inconsistent SQL host state"));
            }
            if reply["results"].is_array() {
                reply["ok"] = true.into();
            } else if reply["error"].is_string() {
                reply["ok"] = false.into();
            } else {
                return Err(io::Error::other("missing SQL host result"));
            }
            if serde_json::to_vec(&reply)?.len() > MAX_RESPONSE {
                return Ok(Ok(failure(
                    "sql_result_too_large_check_execution_outcome",
                    tx,
                )));
            }
            return Ok(Ok(reply));
        }
    });
    match exchange {
        Ok(result) => result,
        Err(_) => {
            // Completed commits survive. Pending changes cannot escape an interrupted script.
            tx.take();
            *expires = None;
            Ok(failure("sql_host_failed_outcome_unknown", tx))
        }
    }
}
