import pathlib,subprocess,time,uuid,re,json,sys
root=pathlib.Path(__file__).resolve().parent;out=pathlib.Path(sys.argv[1]).resolve();out.mkdir();(out/'trace.txt.gz').mkdir()
name='catdb-cleanup-'+uuid.uuid4().hex[:10]
def run(*a):return subprocess.check_output(a,text=True).strip()
try:
 run('docker','run','-d','--name',name,'--network','none','--read-only','--cap-add','SYS_ADMIN','--tmpfs','/trace:rw,nosuid,nodev,mode=700','-v',str(out)+':/control','-v',str(root)+':/collector:ro','catdb-comparison:local','python','/collector/collect.py')
 deadline=time.monotonic()+30
 while not (out/'ready').exists() or not re.fullmatch('catdb-[0-9a-f]{32}',(out/'ready').read_text()):
  if time.monotonic()>deadline:raise TimeoutError(run('docker','logs',name))
  time.sleep(.05)
 instance=(out/'ready').read_text();assert re.fullmatch('catdb-[0-9a-f]{32}',instance)
 (out/'stop').write_text('stop');assert run('docker','wait',name)=='1'
 command='mount -t tracefs -o ro tracefs /trace && test ! -e /trace/instances/'+instance
 run('docker','run','--rm','--network','none','--read-only','--cap-add','SYS_ADMIN','--tmpfs','/trace:rw,nosuid,nodev,mode=700','catdb-comparison:local','sh','-c',command)
 registry=run('docker','run','--rm','--network','none','--read-only','--cap-add','SYS_ADMIN','--tmpfs','/trace:rw,nosuid,nodev,mode=700','catdb-comparison:local','sh','-c','mount -t tracefs -o ro tracefs /trace && cat /trace/kprobe_events')
 assert not re.search(r'/(?:fp_(?:insert|seq|end|return)|st_(?:queue|queue_return|done|channel|channel_return)) ',registry)
 result={'probes_removed':True,'export_failed_as_injected':True,'instance_removed':True,'instance':instance}
 (out/'result.json').write_text(json.dumps(result,indent=2));print(json.dumps(result))
finally:run('docker','rm','-f',name)
