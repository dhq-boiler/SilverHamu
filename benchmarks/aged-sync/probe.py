"""Bounded private-instance probe for trace PID namespace and event availability."""
import json,pathlib,subprocess,uuid
root=pathlib.Path('/trace');subprocess.run(['mount','-t','tracefs','tracefs',str(root)],check=True)
p=root/'instances'/('catdb-probe-'+uuid.uuid4().hex);p.mkdir()
try:
    (p/'tracing_on').write_text('0');(p/'buffer_size_kb').write_text('64');(p/'trace_clock').write_text('mono')
    (p/'events/syscalls/sys_enter_fsync/enable').write_text('1');(p/'events/syscalls/sys_exit_fsync/enable').write_text('1')
    (p/'tracing_on').write_text('1')
    child=subprocess.check_output(['python','-c',"import ctypes,os,tempfile,json; ctypes.CDLL(None).prctl(15,b'catdb-os-probe',0,0,0); f=tempfile.TemporaryFile(); f.write(b'test'); f.flush(); os.fsync(f.fileno()); print(json.dumps({'namespace_pid':os.getpid()}))"],text=True)
    (p/'tracing_on').write_text('0')
    lines=[l for l in (p/'trace').read_text().splitlines() if 'catdb-os-probe' in l]
    print(json.dumps({'child':json.loads(child),'events':lines,'trace_header':(p/'trace').read_text().splitlines()[:4]},indent=2))
finally:
    (p/'tracing_on').write_text('0');(p/'events/syscalls/sys_enter_fsync/enable').write_text('0');(p/'events/syscalls/sys_exit_fsync/enable').write_text('0');p.rmdir()
