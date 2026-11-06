"""Run in a fresh disposable container with LD_PRELOAD enabled."""
import ctypes as c,os,tempfile
l=c.CDLL('/usr/local/lib/libiotrace.so')
fd,path=tempfile.mkstemp();assert l.trace_track(fd)==0
l.trace_request(0)
assert os.write(fd,b'a')==1
assert os.pwrite(fd,b'b',0)==1
os.ftruncate(fd,1);os.fsync(fd);os.fdatasync(fd)
l.trace_request(-1);assert l.trace_count()==5
os.write(fd,b'c');assert l.trace_count()==5
l.trace_request(1)
for _ in range(200000):os.pwrite(fd,b'd',0)
assert l.trace_count()==200005 and l.trace_overflow()==5
os.close(fd);assert l.trace_closed()==1
l.trace_request(-1);os.unlink(path)
print('observer: write/pwrite/truncate/fsync/fdatasync, disabled capture, capacity overflow and close detection passed')
