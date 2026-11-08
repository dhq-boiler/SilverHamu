use super::*;
use std::cell::Cell;

pub(super) struct TimedSocket<'a> {
    pub socket: TcpStream,
    pub deadline: &'a Cell<Instant>,
    pub stop: &'a AtomicBool,
}

impl TimedSocket<'_> {
    fn check(&self) -> io::Result<()> {
        if self.stop.load(Ordering::Acquire) || Instant::now() >= self.deadline.get() {
            return Err(io::Error::other("connection expired"));
        }
        Ok(())
    }
}
impl Read for TimedSocket<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        self.socket.read(buf)
    }
}
impl Write for TimedSocket<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.check()?;
        self.socket.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.socket.flush()
    }
}
