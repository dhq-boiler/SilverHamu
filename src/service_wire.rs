use super::{MAX_REQUEST, MAX_RESPONSE, transfer_read};
use serde_json::Value;
use std::{
    io::{self, Read, Write},
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};
const RETAIN: usize = 4096;
#[derive(Default)]
pub(super) struct Wire {
    rx: Vec<u8>,
    tx: Vec<u8>,
}
fn recycle(slot: &mut Vec<u8>, mut bytes: Vec<u8>) {
    // Large replies must not permanently multiply their capacity by connection count.
    if bytes.capacity() <= RETAIN {
        bytes.clear();
        *slot = bytes;
    }
}
struct Frame<'a>(&'a mut Vec<u8>);
impl Write for Frame<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > (MAX_RESPONSE + 4).saturating_sub(self.0.len()) {
            return Err(io::Error::other("response limit"));
        }
        self.0.try_reserve(bytes.len()).map_err(io::Error::other)?;
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
impl Wire {
    pub(super) fn receive(
        &mut self,
        stream: &mut impl Read,
        deadline: Instant,
        stop: &AtomicBool,
    ) -> io::Result<Value> {
        let mut prefix = [0; 4];
        transfer_read(stream, &mut prefix, deadline, stop)?;
        let length = u32::from_be_bytes(prefix) as usize;
        if !(1..=MAX_REQUEST).contains(&length) {
            return Err(io::Error::other("frame length"));
        }
        let mut body = std::mem::take(&mut self.rx);
        let result = (|| {
            body.try_reserve(length).map_err(io::Error::other)?;
            body.resize(length, 0);
            transfer_read(stream, &mut body, deadline, stop)?;
            serde_json::from_slice(&body).map_err(io::Error::other)
        })();
        recycle(&mut self.rx, body);
        result
    }
    pub(super) fn send(
        &mut self,
        stream: &mut impl Write,
        value: &Value,
        deadline: Instant,
        stop: &AtomicBool,
    ) -> io::Result<()> {
        let mut bytes = std::mem::take(&mut self.tx);
        let result = (|| {
            bytes.extend_from_slice(&[0; 4]);
            serde_json::to_writer(Frame(&mut bytes), value).map_err(io::Error::other)?;
            let length = (bytes.len() - 4) as u32;
            bytes[..4].copy_from_slice(&length.to_be_bytes());
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
            Ok(())
        })();
        recycle(&mut self.tx, bytes);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, time::Duration};
    fn deadline() -> Instant {
        Instant::now() + Duration::from_secs(2)
    }
    fn frame(v: &Value) -> Vec<u8> {
        let b = serde_json::to_vec(v).unwrap();
        let mut f = (b.len() as u32).to_be_bytes().to_vec();
        f.extend(b);
        f
    }
    struct Split {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
        chunk: usize,
        interrupted: bool,
        zero: bool,
    }
    impl Read for Split {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            let n = b.len().min(self.chunk);
            self.input.read(&mut b[..n])
        }
    }
    impl Write for Split {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            if self.interrupted {
                self.interrupted = false;
                return Err(io::ErrorKind::Interrupted.into());
            }
            if self.zero {
                return Ok(0);
            }
            let n = b.len().min(self.chunk);
            self.output.extend_from_slice(&b[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn eq_st_fragmented_frames_keep_order_and_exact_bytes() {
        let stop = AtomicBool::new(false);
        let values = [
            serde_json::json!({"ok":true}),
            serde_json::json!({"value":"ab".repeat(256)}),
        ];
        let input = values.iter().flat_map(frame).collect();
        let mut io = Split {
            input: Cursor::new(input),
            output: vec![],
            chunk: 3,
            interrupted: true,
            zero: false,
        };
        let mut w = Wire::default();
        for v in &values {
            assert_eq!(w.receive(&mut io, deadline(), &stop).unwrap(), *v);
            w.send(&mut io, v, deadline(), &stop).unwrap();
        }
        assert_eq!(io.output, values.iter().flat_map(frame).collect::<Vec<_>>());
        assert!(w.rx.capacity() <= RETAIN && w.tx.capacity() <= RETAIN);
    }
    #[test]
    fn bv_retain_and_frame_limits_release_large_buffers() {
        let stop = AtomicBool::new(false);
        let mut w = Wire::default();
        for n in [
            RETAIN - 1,
            RETAIN,
            RETAIN + 1,
            MAX_REQUEST - 1,
            MAX_REQUEST,
            MAX_REQUEST + 1,
        ] {
            let text = serde_json::json!("a".repeat(n - 2));
            let mut bytes = Cursor::new(frame(&text));
            assert_eq!(
                w.receive(&mut bytes, deadline(), &stop).is_ok(),
                n <= MAX_REQUEST
            );
            assert!(w.rx.capacity() <= RETAIN);
            if n > RETAIN {
                assert_eq!(w.rx.capacity(), 0);
            }
        }
        for n in [
            RETAIN - 1,
            RETAIN,
            RETAIN + 1,
            MAX_RESPONSE - 1,
            MAX_RESPONSE,
            MAX_RESPONSE + 1,
        ] {
            let text = serde_json::json!("a".repeat(n - 2));
            let mut bytes = Vec::new();
            assert_eq!(
                w.send(&mut bytes, &text, deadline(), &stop).is_ok(),
                n <= MAX_RESPONSE
            );
            if n > MAX_RESPONSE {
                assert!(bytes.is_empty());
            } else {
                assert_eq!(bytes.len(), n + 4);
            }
            assert!(w.tx.capacity() <= RETAIN);
        }
    }
    #[test]
    fn er_malformed_truncated_stop_timeout_and_zero_write() {
        let stop = AtomicBool::new(false);
        let mut w = Wire::default();
        for bytes in [
            vec![0, 0, 0, 0],
            vec![0, 0, 0, 1, b'x'],
            vec![0, 0, 0, 2, b'"'],
        ] {
            assert!(
                w.receive(&mut Cursor::new(bytes), deadline(), &stop)
                    .is_err()
            );
        }
        let v = serde_json::json!({"ok":true});
        assert!(w.send(&mut Vec::new(), &v, Instant::now(), &stop).is_err());
        let stopped = AtomicBool::new(true);
        assert!(
            w.receive(&mut Cursor::new(frame(&v)), deadline(), &stopped)
                .is_err()
        );
        assert!(w.send(&mut Vec::new(), &v, deadline(), &stopped).is_err());
        let mut zero = Split {
            input: Cursor::new(vec![]),
            output: vec![],
            chunk: 3,
            interrupted: false,
            zero: true,
        };
        assert_eq!(
            w.send(&mut zero, &v, deadline(), &stop).unwrap_err().kind(),
            io::ErrorKind::WriteZero
        );
        let mut recovered = Vec::new();
        w.send(&mut recovered, &v, deadline(), &stop).unwrap();
        assert_eq!(recovered, frame(&v));
    }
}
