use ring::hmac;
use serde_json::Value;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::{Duration, Instant};

const MAX_FRAME: usize = 5 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_millis(400);

fn nonce() -> io::Result<[u8; 32]> {
    let mut bytes = [0; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes)
}

fn read(stream: &mut TcpStream, mut buf: &mut [u8], deadline: Instant) -> io::Result<()> {
    while !buf.is_empty() {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or(io::ErrorKind::TimedOut)?;
        stream.set_read_timeout(Some(remaining))?;
        let count = stream.read(buf)?;
        if count == 0 {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        buf = &mut buf[count..];
    }
    Ok(())
}

fn write(stream: &mut TcpStream, mut bytes: &[u8], deadline: Instant) -> io::Result<()> {
    // A peer can keep a per-write timeout alive by repeatedly opening a small receive window.
    while !bytes.is_empty() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        stream.set_write_timeout(Some(remaining))?;
        let written = stream.write(bytes)?;
        if written == 0 {
            return Err(io::ErrorKind::WriteZero.into());
        }
        bytes = &bytes[written..];
    }
    Ok(())
}

fn signed_data(direction: u8, challenge: &[u8; 64], body: &[u8]) -> Vec<u8> {
    let mut data = Vec::with_capacity(65 + body.len());
    data.push(direction);
    data.extend_from_slice(challenge);
    data.extend_from_slice(body);
    data
}

fn send(
    stream: &mut TcpStream,
    key: &hmac::Key,
    challenge: &[u8; 64],
    direction: u8,
    value: &Value,
    deadline: Instant,
) -> io::Result<()> {
    let body = serde_json::to_vec(value)?;
    if body.len() > MAX_FRAME {
        return Err(io::Error::other("peer frame too large"));
    }
    let signature = hmac::sign(key, &signed_data(direction, challenge, &body));
    write(stream, &(body.len() as u32).to_be_bytes(), deadline)?;
    write(stream, signature.as_ref(), deadline)?;
    write(stream, &body, deadline)
}

fn receive(
    stream: &mut TcpStream,
    key: &hmac::Key,
    challenge: &[u8; 64],
    direction: u8,
    deadline: Instant,
) -> io::Result<Value> {
    let mut size = [0; 4];
    read(stream, &mut size, deadline)?;
    let size = u32::from_be_bytes(size) as usize;
    if size == 0 || size > MAX_FRAME {
        return Err(io::Error::other("invalid peer frame size"));
    }
    let mut tag = [0; 32];
    read(stream, &mut tag, deadline)?;
    let mut body = vec![0; size];
    read(stream, &mut body, deadline)?;
    hmac::verify(key, &signed_data(direction, challenge, &body), &tag)
        .map_err(|_| io::Error::other("peer authentication failed"))?;
    Ok(serde_json::from_slice(&body)?)
}

pub(super) fn call(address: SocketAddr, key: &hmac::Key, request: &Value) -> io::Result<Value> {
    let deadline = Instant::now() + TIMEOUT;
    let mut stream = TcpStream::connect_timeout(&address, TIMEOUT)?;
    stream.set_nodelay(true)?;
    let mut challenge = [0; 64];
    read(&mut stream, &mut challenge[..32], deadline)?;
    challenge[32..].copy_from_slice(&nonce()?);
    write(&mut stream, &challenge[32..], deadline)?;
    send(&mut stream, key, &challenge, 0, request, deadline)?;
    receive(&mut stream, key, &challenge, 1, deadline)
}

pub(super) fn serve(
    mut stream: TcpStream,
    key: &hmac::Key,
    handle: impl FnOnce(Value) -> io::Result<Value>,
) -> io::Result<()> {
    let deadline = Instant::now() + TIMEOUT;
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    let mut challenge = [0; 64];
    challenge[..32].copy_from_slice(&nonce()?);
    write(&mut stream, &challenge[..32], deadline)?;
    read(&mut stream, &mut challenge[32..], deadline)?;
    let request = receive(&mut stream, key, &challenge, 0, deadline)?;
    let reply = handle(request)?;
    send(&mut stream, key, &challenge, 1, &reply, deadline)
}
