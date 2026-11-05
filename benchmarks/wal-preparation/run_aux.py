import os,json,subprocess,sys
from pathlib import Path
binaries=json.loads(Path(sys.argv[1]).read_text());out=Path(sys.argv[2]).resolve();out.mkdir(parents=True,exist_ok=False)
for repeat in range(1,4):
 for shape in ['small','large','hot']:
  for variant in (['baseline','direct','prepare'] if repeat%2 else ['prepare','direct','baseline']):
   name=f'{variant}-{shape}-{repeat}';env=os.environ.copy();env.update(CATDB_WAL_BENCH_DIR=str(out/name),CATDB_WAL_BENCH_SHAPE=shape)
   p=subprocess.run([binaries[variant],'--ignored','--exact','transactions::wal_preparation_bench::wal_preparation_benchmark','--nocapture','--test-threads=1'],env=env,text=True,capture_output=True,timeout=240)
   (out/(name+'.log')).write_text(p.stdout+'\n'+p.stderr,encoding='utf-8')
   if p.returncode:print(p.stdout,p.stderr);raise SystemExit(p.returncode)
   print(name+': verified',flush=True)
