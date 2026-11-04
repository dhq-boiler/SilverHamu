use newdbms::service::{Config, Server, healthcheck};
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
    let mut init = false;
    let mut remote = false;
    let mut max_connections = 32;
    let mut request_timeout = 30;
    let mut transaction_timeout = 60;
    let mut check = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
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
            "--init" => init = true,
            "--allow-remote" => remote = true,
            "--healthcheck" => check = true,
            "--help" => {
                println!(
                    "catdb-server --db PATH --token-file PATH [--init] [--bind 127.0.0.1:7400] [--allow-remote] [--max-connections 32] [--request-timeout-secs 30] [--transaction-timeout-secs 60]\ncatdb-server --healthcheck --token-file PATH [--bind 127.0.0.1:7400]\nExperimental framed JSON v1, not SQL or HTTP. Remote transport requires an encrypted tunnel."
                );
                return Ok(());
            }
            _ => return Err(format!("unknown argument: {arg}").into()),
        }
    }
    if !(1..=3600).contains(&request_timeout) || !(1..=3600).contains(&transaction_timeout) {
        return Err("timeout must be 1..3600 seconds".into());
    }
    let mut token = String::new();
    let token_input = std::fs::File::open(token_file.ok_or("--token-file is required")?)?;
    if token_input.metadata()?.len() > 258 {
        return Err("token file too large".into());
    }
    token_input.take(258).read_to_string(&mut token)?;
    let token = token.trim_end_matches(['\r', '\n']).as_bytes().to_vec();
    if !(32..=256).contains(&token.len()) {
        return Err("token must be 32..256 UTF-8 bytes".into());
    }
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
    let server = Server::bind(Config {
        bind: address,
        database: database.ok_or("--db is required")?.into(),
        token,
        initialize: init,
        allow_remote: remote,
        max_connections,
        request_timeout: Duration::from_secs(request_timeout),
        transaction_timeout: Duration::from_secs(transaction_timeout),
    })?;
    println!("catdb-server ready {} protocol=1", server.address()?);
    server.run(stop)?;
    Ok(())
}
