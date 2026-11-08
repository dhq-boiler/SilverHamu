use newdbms::service::{AccountOptions, Config, Server, healthcheck};
use newdbms::transactions::ResourceLimits;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

fn main() {
    if let Err(error) = run() {
        eprintln!("catdb-server: {error}");
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut bind = "127.0.0.1:7400".to_string();
    let mut database = None;
    let mut token_file = None;
    let mut cert = None;
    let mut key = None;
    let mut admin = None;
    let mut password_file = None;
    let mut attachments = Vec::new();
    let mut init = false;
    let mut remote = false;
    let mut max_connections = 32;
    let mut request_timeout = 30;
    let mut transaction_timeout = 60;
    let mut check = false;
    let mut limits = ResourceLimits::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tls-cert" => cert = Some(args.next().ok_or("missing certificate path")?.into()),
            "--tls-key" => key = Some(args.next().ok_or("missing private key path")?.into()),
            "--bootstrap-admin" => admin = Some(args.next().ok_or("missing admin name")?),
            "--bootstrap-password-file" => {
                password_file = Some(args.next().ok_or("missing password file")?)
            }
            "--attach" => {
                let spec = args.next().ok_or("missing NAME=PATH")?;
                let (name, path) = spec.split_once('=').ok_or("--attach requires NAME=PATH")?;
                attachments.push((name.to_owned(), path.into()));
            }
            "--bind" => bind = args.next().ok_or("--bind requires an address")?,
            "--db" => database = Some(args.next().ok_or("--db requires a file")?),
            "--token-file" => token_file = Some(args.next().ok_or("--token-file requires a file")?),
            "--max-connections" => max_connections = args.next().ok_or("missing limit")?.parse()?,
            "--request-timeout-secs" => {
                request_timeout = args.next().ok_or("missing timeout")?.parse()?
            }
            "--transaction-timeout-secs" => {
                transaction_timeout = args.next().ok_or("missing timeout")?.parse()?
            }
            "--max-retained-bytes" => {
                limits.max_retained_bytes =
                    budget_bytes(&args.next().ok_or("missing resource budget")?)?;
            }
            "--max-staged-bytes" => {
                limits.max_staged_bytes =
                    budget_bytes(&args.next().ok_or("missing resource budget")?)?;
            }
            "--max-dependency-bytes" => {
                limits.max_dependency_bytes =
                    budget_bytes(&args.next().ok_or("missing resource budget")?)?;
            }
            "--max-buffer-bytes" => {
                limits.max_buffer_bytes =
                    budget_bytes(&args.next().ok_or("missing resource budget")?)?;
            }
            "--init" => init = true,
            "--allow-remote" => remote = true,
            "--healthcheck" => check = true,
            "--help" => {
                println!(
                    "Account service: --db PATH --tls-cert CERT.pem --tls-key KEY.pem [--attach NAME=EXISTING_DB] [--bootstrap-admin NAME --bootstrap-password-file FILE]"
                );
                println!(
                    "catdb-server --db PATH --token-file PATH [--init] [--bind 127.0.0.1:7400] [--allow-remote] [--max-connections 32] [--request-timeout-secs 30] [--transaction-timeout-secs 60] [--max-retained-bytes 134217728] [--max-staged-bytes 33554432] [--max-dependency-bytes 16777216] [--max-buffer-bytes 16777216]\ncatdb-server --healthcheck --token-file PATH [--bind 127.0.0.1:7400]\nExperimental framed JSON v1, not SQL or HTTP. Remote transport requires an encrypted tunnel."
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    if !(1..=3600).contains(&request_timeout) || !(1..=3600).contains(&transaction_timeout) {
        return Err("timeout must be 1..3600 seconds".into());
    }
    let secure = cert.is_some()
        || key.is_some()
        || admin.is_some()
        || password_file.is_some()
        || !attachments.is_empty();
    let accounts = if secure {
        if token_file.is_some() || check {
            return Err("account mode cannot use legacy tokens or token healthchecks".into());
        }
        let bootstrap = match (admin, password_file) {
            (None, None) => None,
            (Some(name), Some(file)) => {
                let mut password = String::new();
                std::fs::File::open(file)?
                    .take(1027)
                    .read_to_string(&mut password)?;
                if password.len() > 1026 {
                    return Err("password file too large".into());
                }
                if password.ends_with('\n') {
                    password.pop();
                    if password.ends_with('\r') {
                        password.pop();
                    }
                }
                Some((name, password))
            }
            _ => return Err("supply both bootstrap flags".into()),
        };
        Some(AccountOptions {
            certificate: cert.ok_or("missing --tls-cert")?,
            private_key: key.ok_or("missing --tls-key")?,
            bootstrap,
            attachments,
        })
    } else {
        None
    };
    let token = if let Some(file) = token_file {
        let mut token = String::new();
        let token_input = std::fs::File::open(file)?;
        if token_input.metadata()?.len() > 258 {
            return Err("token file too large".into());
        }
        token_input.take(258).read_to_string(&mut token)?;
        let token = token.trim_end_matches(['\r', '\n']).as_bytes().to_vec();
        if !(32..=256).contains(&token.len()) {
            return Err("token must be 32..256 UTF-8 bytes".into());
        }
        token
    } else if secure {
        Vec::new()
    } else {
        return Err("--token-file or TLS account configuration required".into());
    };
    let address: SocketAddr = bind.parse()?;
    if check {
        let address = if address.ip().is_unspecified() {
            SocketAddr::new(
                if address.is_ipv6() {
                    IpAddr::V6(Ipv6Addr::LOCALHOST)
                } else {
                    IpAddr::V4(Ipv4Addr::LOCALHOST)
                },
                address.port(),
            )
        } else {
            address
        };
        healthcheck(address, &token)?;
        return Ok(());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let signal = Arc::clone(&stop);
    ctrlc::set_handler(move || signal.store(true, Ordering::Release))?;
    limits.max_transactions = max_connections;
    let config = Config {
        bind: address,
        database: database.ok_or("--db is required")?.into(),
        token,
        initialize: init,
        allow_remote: remote,
        max_connections,
        request_timeout: Duration::from_secs(request_timeout),
        transaction_timeout: Duration::from_secs(transaction_timeout),
    };
    let server = match accounts {
        Some(options) => Server::bind_accounts(config, limits, options)?,
        None => Server::bind_with_limits(config, limits)?,
    };
    println!(
        "catdb-server ready {} protocol={}",
        server.address()?,
        if secure { 2 } else { 1 }
    );
    server.run(stop)?;
    Ok(())
}

fn budget_bytes(value: &str) -> Result<usize, Box<dyn std::error::Error>> {
    let bytes: usize = value.parse()?;
    if bytes == 0 {
        return Err("resource budget must be positive bytes".into());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bv_resource_budgets_reject_zero_invalid_and_overflow() {
        for value in ["0", "-1", "", "1MiB", "18446744073709551616"] {
            assert!(budget_bytes(value).is_err(), "{value}");
        }
        for value in [1, 134217727, 134217728, 134217729, usize::MAX] {
            assert_eq!(budget_bytes(&value.to_string()).unwrap(), value);
        }
    }
}
