"""Build and freeze matched time/allocation executables in a new directory."""
import io,json,os,shutil,subprocess,sys,zipfile
from pathlib import Path
repo=Path(__file__).resolve().parents[2]
out=Path(sys.argv[1]).resolve();out.mkdir(parents=True,exist_ok=False)
baseline=out/'before';baseline.mkdir()
archive=subprocess.check_output(['git','archive','--format=zip','d50fe50e02d3377b3078ffa61c37c4951b7f91f1'],cwd=repo)
with zipfile.ZipFile(io.BytesIO(archive)) as z:
 for name in z.namelist():
  if not (baseline/name).resolve().is_relative_to(baseline):raise ValueError('Archive path escapes output')
 z.extractall(baseline)
folder=baseline/'benchmarks/scan-merge';folder.mkdir()
shutil.copy2(repo/'benchmarks/scan-merge/engine_bench.rs',folder/'engine_bench.rs')
p=baseline/'src/transactions.rs'
p.write_text(p.read_text(encoding='utf-8')+'\n#[cfg(test)]\n#[path = "../benchmarks/scan-merge/engine_bench.rs"]\nmod scan_merge_bench;\n',encoding='utf-8')
result={}
for label,root in [('before',baseline),('after',repo)]:
 for mode in ['time','alloc']:
  env=os.environ.copy();env.pop('CATDB_ELISION_ALLOC',None)
  if mode=='alloc':env['CATDB_ELISION_ALLOC']='1'
  run=subprocess.run([os.environ.get('CARGO','cargo'),'test','--locked','--release','--features','service','--lib','--no-run','--message-format=json'],cwd=root,env=env,text=True,capture_output=True)
  print(run.stderr)
  if run.returncode:print(run.stdout);raise SystemExit(run.returncode)
  executable=None
  for line in run.stdout.splitlines():
   record=json.loads(line)
   if record.get('reason')=='compiler-artifact' and record.get('executable') and record['profile']['test']:executable=record['executable']
  assert executable
  target=out/(label+'-'+mode+('.exe' if os.name=='nt' else ''))
  shutil.copy2(executable,target);result[label+'-'+mode]=str(target)
(out/'binaries.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
