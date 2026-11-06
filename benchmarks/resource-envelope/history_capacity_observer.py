"""Sample a single owned child and preserve OOM evidence after child exit."""
import json,pathlib,subprocess,sys,time
root=pathlib.Path('/sys/fs/cgroup');out=pathlib.Path('/data');phase=sys.argv[1]
def pairs(name):return {k:int(v) for k,v in (l.split() for l in (root/name).read_text().splitlines())}
def sample(pid):
    try:status=pathlib.Path(f'/proc/{pid}/status').read_text()
    except FileNotFoundError:status=''
    return {'time_ns':time.monotonic_ns(),'memory':int((root/'memory.current').read_text()),'peak':int((root/'memory.peak').read_text()),'swap':int((root/'memory.swap.current').read_text()),'stat':pairs('memory.stat'),'events':pairs('memory.events'),'cpu':pairs('cpu.stat'),'rss':next((int(l.split()[1])*1024 for l in status.splitlines() if l.startswith('VmRSS:')),0)}
with (out/f'{phase}.jsonl').open('w') as log,(out/f'{phase}.stderr').open('w') as err,(out/f'{phase}-resources.jsonl').open('w') as resources:
    p=subprocess.Popen(sys.argv[2:],stdout=log,stderr=err)
    begin=time.monotonic();timed_out=False
    while p.poll() is None:
        resources.write(json.dumps(sample(p.pid))+'\n');resources.flush()
        if time.monotonic()-begin>180:timed_out=True;p.kill();break
        time.sleep(.1)
    p.wait();resources.write(json.dumps(sample(p.pid))+'\n')
    (out/f'{phase}-exit.json').write_text(json.dumps({'returncode':p.returncode,'timed_out':timed_out,'wall_s':time.monotonic()-begin}))
sys.exit(0 if p.returncode==0 else 1)
