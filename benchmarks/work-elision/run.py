import os,subprocess,json,sys
from pathlib import Path
binary=json.loads(Path(sys.argv[1]).read_text(encoding='utf-8'));out=Path(sys.argv[2]).resolve();out.mkdir(parents=True,exist_ok=False)
for mode in ['time','alloc']:
 for repeat in range(1,4):
  for case in ['100000','1000000','service']:
   for variant in (['before','after'] if repeat%2 else ['after','before']):
    name=f'{case}-{repeat}-{variant}-{mode}';folder=out/name;env=os.environ.copy();env['CATDB_ELISION_OUTPUT']=str(folder);env['CATDB_ELISION_ROWS']=case
    test='service::work_elision_bench::work_elision_single_requests' if case=='service' else 'transactions::work_elision_bench::work_elision_points_and_export'
    p=subprocess.run([binary[f'{variant}-{mode}'],'--ignored','--exact',test,'--nocapture','--test-threads=1'],env=env,text=True,capture_output=True)
    (out/(name+'.log')).write_text(p.stdout+'\n'+p.stderr,encoding='utf-8')
    if p.returncode:print(p.stdout,p.stderr);raise SystemExit(p.returncode)
    print(name+': verified',flush=True)
