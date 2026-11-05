import os,json,subprocess,sys
from pathlib import Path
binary=json.loads(Path(sys.argv[1]).read_text(encoding='utf-8'));out=Path(sys.argv[2]).resolve();out.mkdir(parents=True,exist_ok=False)
for mode in ['time','alloc']:
 for repeat in range(1,4):
  for size in [100000,1000000]:
   for shape in ['plain','mixed']:
    for variant in (['before','after'] if repeat%2 else ['after','before']):
     name=f'{size}-{shape}-{repeat}-{variant}-{mode}';env=os.environ.copy();env.update(CATDB_COPY_OUTPUT=str(out/name),CATDB_COPY_ROWS=str(size),CATDB_COPY_SHAPE=shape)
     p=subprocess.run([binary[f'{variant}-{mode}'],'--ignored','--exact','transactions::scan_copy_bench::full_scan_copy_benchmark','--nocapture','--test-threads=1'],env=env,text=True,capture_output=True)
     (out/(name+'.log')).write_text(p.stdout+'\n'+p.stderr,encoding='utf-8')
     if p.returncode:print(p.stdout,p.stderr);raise SystemExit(p.returncode)
     print(name+': verified',flush=True)
