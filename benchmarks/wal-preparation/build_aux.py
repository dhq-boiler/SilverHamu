"""Freeze three instrumented engine executables; not the main service timing builds."""
import json,os,shutil,subprocess,sys
from pathlib import Path
repo=Path(__file__).resolve().parents[2]
out=Path(sys.argv[1]).resolve();out.mkdir(parents=True,exist_ok=False);result={}
for name,feature in [('baseline',''),('direct',',wal-direct'),('prepare',',wal-prepare')]:
 env=os.environ.copy();env['CATDB_ELISION_ALLOC']='1'
 p=subprocess.run([os.environ.get('CARGO','cargo'),'test','--locked','--release','--no-default-features','--features','service,lock-timing'+feature,'--lib','--no-run','--message-format=json'],cwd=repo,env=env,text=True,capture_output=True)
 print(p.stderr,flush=True)
 if p.returncode:print(p.stdout);raise SystemExit(p.returncode)
 exe=None
 for line in p.stdout.splitlines():
  r=json.loads(line)
  if r.get('reason')=='compiler-artifact' and r.get('executable') and r['profile']['test']:exe=r['executable']
 assert exe
 dest=out/(name+('.exe' if os.name=='nt' else ''));shutil.copy2(exe,dest);result[name]=str(dest)
(out/'binaries.json').write_text(json.dumps(result,indent=2)+'\n',encoding='utf-8')
