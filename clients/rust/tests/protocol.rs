use serde_json::{Value, json};
use silverhamu_client::{Client, Error, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::thread::{self, JoinHandle};
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(3);

fn receive(socket: &mut TcpStream) -> Value {
    let mut header = [0; 4];
    socket.read_exact(&mut header).unwrap();
    let mut body = vec![0; u32::from_be_bytes(header) as usize];
    socket.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
    bytes.extend(body);
    bytes
}

fn send(socket: &mut TcpStream, value: Value) {
    socket
        .write_all(&frame(&serde_json::to_vec(&value).unwrap()))
        .unwrap();
}

fn peer(action: impl FnOnce(TcpStream) + Send + 'static) -> (u16, JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let task = thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        socket.set_read_timeout(Some(TIMEOUT)).unwrap();
        socket.set_write_timeout(Some(TIMEOUT)).unwrap();
        assert_eq!(
            receive(&mut socket),
            json!({"op":"auth", "version":1, "token":"test-token"})
        );
        send(&mut socket, json!({"ok":true, "protocol":1}));
        action(socket);
    });
    (port, task)
}

fn connected(port: u16) -> Client {
    Client::connect_token("127.0.0.1", port, "test-token", TIMEOUT).unwrap()
}

#[test]
fn eq_binary_missing_empty_and_u64_boundaries() {
    let (port, task) = peer(|mut socket| {
        for value in [Value::Null, json!(""), json!("00Ff80")] {
            assert_eq!(
                receive(&mut socket),
                json!({"op":"get_latest", "key":"00ff"})
            );
            send(&mut socket, json!({"ok":true, "value":value}));
        }
        for version in ["0", "18446744073709551615"] {
            assert_eq!(
                receive(&mut socket),
                json!({"op":"put_commit", "key":"00ff", "value":"00ff80"})
            );
            send(&mut socket, json!({"ok":true, "version":version}));
        }
    });
    let mut client = connected(port);
    assert_eq!(client.get_latest(&[0, 255]).unwrap(), None);
    assert_eq!(client.get_latest(&[0, 255]).unwrap(), Some(vec![]));
    assert_eq!(
        client.get_latest(&[0, 255]).unwrap(),
        Some(vec![0, 255, 128])
    );
    assert_eq!(client.put_commit(&[0, 255], &[0, 255, 128]).unwrap(), 0);
    assert_eq!(
        client.put_commit(&[0, 255], &[0, 255, 128]).unwrap(),
        u64::MAX
    );
    task.join().unwrap();
}

#[test]
fn er_malformed_typed_replies_discard_connection_without_replay() {
    let cases = [
        (false, json!({"ok":true})),
        (false, json!({"ok":true,"value":42})),
        (false, json!({"ok":true,"value":"zz"})),
        (false, json!({"ok":true,"value":"f"})),
        (false, json!({"ok":true,"value":"é"})),
        (true, json!({"ok":true})),
        (true, json!({"ok":true,"version":3})),
        (true, json!({"ok":true,"version":"18446744073709551616"})),
        (true, json!({"ok":true,"version":"+1"})),
        (true, json!({"ok":true,"version":""})),
        (true, json!({"ok":true,"version":"１"})),
    ];
    for (write, reply) in cases {
        let (port, task) = peer(move |mut socket| {
            receive(&mut socket);
            send(&mut socket, reply);
            let mut byte = [0];
            assert_eq!(socket.read(&mut byte).unwrap(), 0);
        });
        let mut client = connected(port);
        let result = if write {
            client.put_commit(b"k", b"v").map(|_| ())
        } else {
            client.get_latest(b"k").map(|_| ())
        };
        assert!(matches!(result, Err(Error::Connection(_))));
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("may have committed")
        );
        assert!(matches!(client.ping(), Err(Error::Closed)));
        task.join().unwrap();
    }
}

#[test]
fn er_invalid_frames_envelopes_and_truncation_close_connection() {
    for bytes in [
        0_u32.to_be_bytes().to_vec(),
        ((MAX_RESPONSE_BYTES + 1) as u32).to_be_bytes().to_vec(),
        frame(b"[]"),
        frame(b"null"),
        frame(b"{\"ok\":1}"),
        frame(b"{\"ok\":false}"),
        frame(b"{\"ok\":false,\"error\":\"\"}"),
        frame(b"{\"ok\":false,\"error\":7}"),
        frame(b"{\"ok\":true,\"value\":\"\xff\"}"),
        frame(b"{"),
        vec![0, 0],
        [30_u32.to_be_bytes().as_slice(), b"{"].concat(),
        vec![],
    ] {
        let (port, task) = peer(move |mut socket| {
            receive(&mut socket);
            socket.write_all(&bytes).unwrap();
            socket.shutdown(Shutdown::Write).unwrap();
        });
        let mut client = connected(port);
        assert!(matches!(
            client.request(&json!({"op":"ping"})),
            Err(Error::Connection(_))
        ));
        assert!(matches!(client.ping(), Err(Error::Closed)));
        task.join().unwrap();
    }
}

#[test]
fn bv_request_size_rejects_only_above_limit_without_sending() {
    let (port, task) = peer(|mut socket| {
        for size in [MAX_REQUEST_BYTES - 1, MAX_REQUEST_BYTES] {
            let request = receive(&mut socket);
            assert_eq!(serde_json::to_vec(&request).unwrap().len(), size);
            send(&mut socket, json!({"ok":true}));
        }
        assert_eq!(receive(&mut socket), json!({"op":"ping"}));
        send(&mut socket, json!({"ok":true,"version":"1"}));
    });
    let mut client = connected(port);
    assert!(matches!(
        client.request(&json!([])),
        Err(Error::InvalidInput(_))
    ));
    for size in [
        MAX_REQUEST_BYTES - 1,
        MAX_REQUEST_BYTES,
        MAX_REQUEST_BYTES + 1,
    ] {
        let request = json!({"x":"a".repeat(size - 8)});
        assert_eq!(serde_json::to_vec(&request).unwrap().len(), size);
        let result = client.request(&request);
        if size <= MAX_REQUEST_BYTES {
            assert!(result.is_ok());
        } else {
            assert!(matches!(result, Err(Error::InvalidInput(_))));
        }
    }
    assert_eq!(client.ping().unwrap(), 1);
    task.join().unwrap();
}

#[test]
fn bv_response_size_accepts_limit_and_limit_minus_one() {
    for size in [MAX_RESPONSE_BYTES - 1, MAX_RESPONSE_BYTES] {
        let (port, task) = peer(move |mut socket| {
            receive(&mut socket);
            let mut body = br#"{"ok":true}"#.to_vec();
            body.resize(size, b' ');
            socket.write_all(&frame(&body)).unwrap();
        });
        let mut client = connected(port);
        assert_eq!(
            client.request(&json!({"op":"custom"})).unwrap(),
            json!({"ok":true})
        );
        task.join().unwrap();
    }
}

#[test]
fn st_server_rejection_is_recoverable_but_unauthorized_closes() {
    let (port, task) = peer(|mut socket| {
        receive(&mut socket);
        send(&mut socket, json!({"ok":false,"error":"invalid_request"}));
        receive(&mut socket);
        send(&mut socket, json!({"ok":true,"version":"4"}));
        receive(&mut socket);
        send(&mut socket, json!({"ok":false,"error":"unauthorized"}));
    });
    let mut client = connected(port);
    assert!(matches!(client.commit(), Err(Error::Server { code }) if code == "invalid_request"));
    assert_eq!(client.ping().unwrap(), 4);
    assert!(matches!(client.ping(), Err(Error::Server { code }) if code == "unauthorized"));
    assert!(matches!(client.ping(), Err(Error::Closed)));
    task.join().unwrap();
}

#[test]
fn auth_wrong_protocol_or_rejection_drops_socket() {
    for reply in [
        json!({"ok":true}),
        json!({"ok":true,"protocol":2}),
        json!({"ok":true,"protocol":"1"}),
        json!({"ok":false,"error":"unauthorized"}),
    ] {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let task = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(TIMEOUT)).unwrap();
            receive(&mut socket);
            send(&mut socket, reply);
            assert_eq!(socket.read(&mut [0]).unwrap(), 0);
        });
        assert!(Client::connect_token("127.0.0.1", port, "test-token", TIMEOUT).is_err());
        task.join().unwrap();
    }
}

#[test]
fn er_timeout_and_drop_close_without_resubmitting() {
    let (port, task) = peer(|mut socket| {
        assert_eq!(receive(&mut socket)["op"], "put_commit");
        assert_eq!(socket.read(&mut [0]).unwrap(), 0);
    });
    let mut client =
        Client::connect_token("127.0.0.1", port, "test-token", Duration::from_millis(150)).unwrap();
    assert!(matches!(
        client.put_commit(b"k", b"v"),
        Err(Error::Connection(_))
    ));
    assert!(matches!(client.ping(), Err(Error::Closed)));
    task.join().unwrap();
    let (port, task) = peer(|mut socket| {
        assert_eq!(socket.read(&mut [0]).unwrap(), 0);
    });
    drop(connected(port));
    task.join().unwrap();
}

#[test]
fn bv_timeout_must_be_positive_and_representable() {
    for timeout in [Duration::ZERO, Duration::MAX] {
        assert!(matches!(
            Client::connect_token("127.0.0.1", 1, "token", timeout),
            Err(Error::InvalidInput(_))
        ));
    }
}

#[test]
fn er_partial_reads_do_not_extend_exchange_deadline() {
    let (port, task) = peer(|mut socket| {
        receive(&mut socket);
        let bytes = frame(br#"{"ok":true,"version":"1"}"#);
        for byte in bytes {
            if socket.write_all(&[byte]).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(50));
        }
    });
    let mut client =
        Client::connect_token("127.0.0.1", port, "test-token", Duration::from_millis(250)).unwrap();
    let started = std::time::Instant::now();
    assert!(matches!(client.ping(), Err(Error::Connection(_))));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(matches!(client.ping(), Err(Error::Closed)));
    task.join().unwrap();
}

#[test]
fn eq_successful_fragmented_reply_is_reassembled() {
    let (port, task) = peer(|mut socket| {
        receive(&mut socket);
        for byte in frame(br#"{"ok":true,"version":"42"}"#) {
            socket.write_all(&[byte]).unwrap();
        }
    });
    let mut client = connected(port);
    assert_eq!(client.ping().unwrap(), 42);
    task.join().unwrap();
}
