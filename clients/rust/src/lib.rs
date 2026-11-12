//! Synchronous client for the SilverHamu TCP service.
//!
//! Each client owns one connection and transaction context. Use separate clients
//! for independent concurrent transactions. Requests are never retried.
//! See the package README for authentication and transaction examples.

#![doc = include_str!("../README.md")]

use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use serde_json::{Value, json};
use std::fmt;
use std::io::{self, Read, Write};
use std::net::{Shutdown, TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024 + 8192;

/// A server rejection is distinct from an exchange with an unknown outcome.
#[derive(Debug)]
pub enum Error {
    InvalidInput(String),
    Server {
        code: String,
    },
    /// The connection is discarded. A submitted write may have committed.
    Connection(io::Error),
    Closed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(message) => write!(f, "invalid silverhamu input: {message}"),
            Self::Server { code } => write!(f, "silverhamu server: {code}"),
            Self::Connection(error) => write!(
                f,
                "silverhamu connection discarded; a submitted write may have committed: {error}"
            ),
            Self::Closed => f.write_str("silverhamu client is closed"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Connection(error) => Some(error),
            _ => None,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// Account authentication always uses verified TLS 1.3.
pub struct AccountOptions<'a> {
    pub username: &'a str,
    pub password: &'a str,
    pub database: &'a str,
    /// A PEM CA bundle replaces the system trust store when supplied.
    pub ca_file: Option<&'a Path>,
}

impl<'a> AccountOptions<'a> {
    pub fn new(username: &'a str, password: &'a str) -> Self {
        Self {
            username,
            password,
            database: "main",
            ca_file: None,
        }
    }
}

struct TimedSocket {
    socket: TcpStream,
    deadline: Instant,
}

impl TimedSocket {
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "exchange deadline expired"))
    }
}

impl Read for TimedSocket {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.socket.set_read_timeout(Some(self.remaining()?))?;
        self.socket.read(bytes)
    }
}

impl Write for TimedSocket {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.socket.set_write_timeout(Some(self.remaining()?))?;
        self.socket.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}

enum Transport {
    Plain(TimedSocket),
    Tls(Box<StreamOwned<ClientConnection, TimedSocket>>),
}

impl Transport {
    fn socket(&mut self) -> &mut TimedSocket {
        match self {
            Self::Plain(socket) => socket,
            Self::Tls(stream) => &mut stream.sock,
        }
    }
}

impl Read for Transport {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Plain(socket) => socket.read(bytes),
            Self::Tls(stream) => stream.read(bytes),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Plain(socket) => socket.write(bytes),
            Self::Tls(stream) => stream.write(bytes),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Plain(socket) => socket.flush(),
            Self::Tls(stream) => stream.flush(),
        }
    }
}

/// One synchronous connection. Dropping it releases any staged transaction.
pub struct Client {
    transport: Option<Transport>,
    timeout: Duration,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn input(error: impl fmt::Display) -> Error {
    Error::InvalidInput(error.to_string())
}

fn deadline(timeout: Duration) -> Result<Instant> {
    if timeout.is_zero() {
        return Err(input("timeout must be positive"));
    }
    Instant::now()
        .checked_add(timeout)
        .ok_or_else(|| input("timeout is too large"))
}

fn connect(host: &str, port: u16, timeout: Duration) -> Result<TimedSocket> {
    let expires = deadline(timeout)?;
    let addresses = (host, port).to_socket_addrs().map_err(Error::Connection)?;
    let mut last = io::Error::new(
        io::ErrorKind::AddrNotAvailable,
        "host resolved to no addresses",
    );
    for address in addresses {
        let remaining = expires
            .checked_duration_since(Instant::now())
            .filter(|time| !time.is_zero())
            .ok_or_else(|| {
                Error::Connection(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "connect deadline expired",
                ))
            })?;
        match TcpStream::connect_timeout(&address, remaining) {
            Ok(socket) => {
                return Ok(TimedSocket {
                    socket,
                    deadline: expires,
                });
            }
            Err(error) => last = error,
        }
    }
    Err(Error::Connection(last))
}

impl Client {
    /// Plaintext token connections are intended for loopback or encrypted tunnels.
    pub fn connect_token(host: &str, port: u16, token: &str, timeout: Duration) -> Result<Self> {
        let mut client = Self {
            transport: Some(Transport::Plain(connect(host, port, timeout)?)),
            timeout,
        };
        client.authenticate(json!({"op":"auth", "version":1, "token":token}), 1)?;
        Ok(client)
    }

    /// Connect with hostname verification and system roots or an explicit CA file.
    pub fn connect_account(
        host: &str,
        port: u16,
        account: AccountOptions<'_>,
        timeout: Duration,
    ) -> Result<Self> {
        deadline(timeout)?;
        let mut roots = RootCertStore::empty();
        if let Some(path) = account.ca_file {
            let pem = std::fs::read(path).map_err(input)?;
            for cert in rustls_pemfile::certs(&mut pem.as_slice()) {
                roots.add(cert.map_err(input)?).map_err(input)?;
            }
        } else {
            let native = rustls_native_certs::load_native_certs();
            for cert in native.certs {
                roots.add(cert).map_err(input)?;
            }
        }
        if roots.is_empty() {
            return Err(input("no trusted CA certificates found"));
        }
        let config =
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_protocol_versions(&[&rustls::version::TLS13])
                .map_err(input)?
                .with_root_certificates(roots)
                .with_no_client_auth();
        let server_name =
            rustls::pki_types::ServerName::try_from(host.to_owned()).map_err(input)?;
        let connection = ClientConnection::new(Arc::new(config), server_name).map_err(input)?;
        let stream = StreamOwned::new(connection, connect(host, port, timeout)?);
        let mut client = Self {
            transport: Some(Transport::Tls(Box::new(stream))),
            timeout,
        };
        client.authenticate(
            json!({"op":"auth", "version":2, "username":account.username,
            "password":account.password, "database":account.database}),
            2,
        )?;
        Ok(client)
    }

    fn authenticate(&mut self, request: Value, protocol: u64) -> Result<()> {
        self.execute(&request, |reply| {
            if reply.get("protocol").and_then(Value::as_u64) == Some(protocol) {
                Ok(())
            } else {
                Err(invalid("unexpected authentication protocol"))
            }
        })
    }

    /// Close the socket; calling this more than once is harmless.
    pub fn close(&mut self) {
        if let Some(mut transport) = self.transport.take() {
            let _ = transport.socket().socket.shutdown(Shutdown::Both);
        }
    }

    /// Send any wire operation, including paginated scans and SQL requests.
    /// Versions in the raw response remain decimal strings.
    pub fn request(&mut self, request: &Value) -> Result<Value> {
        self.execute(request, Ok)
    }

    fn execute<T>(
        &mut self,
        request: &Value,
        decode: impl FnOnce(Value) -> io::Result<T>,
    ) -> Result<T> {
        if self.transport.is_none() {
            return Err(Error::Closed);
        }
        if !request.is_object() {
            return Err(input("request must be a JSON object"));
        }
        let payload = serde_json::to_vec(request).map_err(input)?;
        if payload.len() > MAX_REQUEST_BYTES {
            return Err(input("request exceeds 1 MiB"));
        }
        let expires = deadline(self.timeout)?;
        let transport = self.transport.as_mut().ok_or(Error::Closed)?;
        transport.socket().deadline = expires;
        let result = exchange(transport, &payload)
            .and_then(|reply| decode(reply).map_err(Error::Connection));
        match &result {
            Err(Error::Connection(_)) => self.close(),
            Err(Error::Server { code }) if code == "unauthorized" => self.close(),
            _ => (),
        }
        result
    }

    fn version(&mut self, request: Value) -> Result<u64> {
        self.execute(&request, decode_version)
    }
    fn read(&mut self, op: &str, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.execute(&json!({"op":op, "key":encode_hex(key)}), decode_value)
    }

    pub fn ping(&mut self) -> Result<u64> {
        self.version(json!({"op":"ping"}))
    }
    /// Missing keys return `None`; stored empty values return `Some(vec![])`.
    pub fn get_latest(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.read("get_latest", key)
    }
    pub fn put_commit(&mut self, key: &[u8], value: &[u8]) -> Result<u64> {
        self.version(json!({"op":"put_commit", "key":encode_hex(key), "value":encode_hex(value)}))
    }
    pub fn delete_commit(&mut self, key: &[u8]) -> Result<u64> {
        self.version(json!({"op":"delete_commit", "key":encode_hex(key)}))
    }
    pub fn begin(&mut self) -> Result<u64> {
        self.version(json!({"op":"begin"}))
    }
    pub fn begin_read(&mut self) -> Result<u64> {
        self.version(json!({"op":"begin_read"}))
    }
    pub fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        self.read("get", key)
    }
    pub fn put(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        self.request(&json!({"op":"put", "key":encode_hex(key), "value":encode_hex(value)}))
            .map(|_| ())
    }
    pub fn delete(&mut self, key: &[u8]) -> Result<()> {
        self.request(&json!({"op":"delete", "key":encode_hex(key)}))
            .map(|_| ())
    }
    pub fn commit(&mut self) -> Result<u64> {
        self.version(json!({"op":"commit"}))
    }
    pub fn rollback(&mut self) -> Result<()> {
        self.request(&json!({"op":"rollback"})).map(|_| ())
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

fn exchange(transport: &mut Transport, payload: &[u8]) -> Result<Value> {
    let mut exchange = || -> io::Result<Value> {
        transport.write_all(&(payload.len() as u32).to_be_bytes())?;
        transport.write_all(payload)?;
        transport.flush()?;
        let mut header = [0; 4];
        transport.read_exact(&mut header)?;
        let length = u32::from_be_bytes(header) as usize;
        if !(1..=MAX_RESPONSE_BYTES).contains(&length) {
            return Err(invalid("invalid response size"));
        }
        let mut body = vec![0; length];
        transport.read_exact(&mut body)?;
        serde_json::from_slice(&body).map_err(|_| invalid("invalid response JSON"))
    };
    let reply = exchange().map_err(Error::Connection)?;
    match reply.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(reply),
        Some(false) => match reply
            .get("error")
            .and_then(Value::as_str)
            .filter(|code| !code.is_empty())
        {
            Some(code) => Err(Error::Server {
                code: code.to_owned(),
            }),
            None => Err(Error::Connection(invalid("invalid server error"))),
        },
        None => Err(Error::Connection(invalid("invalid response envelope"))),
    }
}

fn decode_version(reply: Value) -> io::Result<u64> {
    let version = reply
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("missing version string"))?;
    if version.is_empty() || !version.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid("invalid version"));
    }
    version.parse().map_err(|_| invalid("version exceeds u64"))
}

fn decode_value(reply: Value) -> io::Result<Option<Vec<u8>>> {
    match reply.get("value") {
        Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => decode_hex(value).map(Some),
        _ => Err(invalid("missing or invalid value")),
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                DIGITS[(byte >> 4) as usize] as char,
                DIGITS[(byte & 15) as usize] as char,
            ]
        })
        .collect()
}

fn decode_hex(value: &str) -> io::Result<Vec<u8>> {
    fn digit(byte: u8) -> io::Result<u8> {
        match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            b'a'..=b'f' => Ok(byte - b'a' + 10),
            b'A'..=b'F' => Ok(byte - b'A' + 10),
            _ => Err(invalid("invalid hexadecimal value")),
        }
    }
    if value.len() % 2 != 0 {
        return Err(invalid("odd hexadecimal value length"));
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Ok(digit(pair[0])? * 16 + digit(pair[1])?))
        .collect()
}
