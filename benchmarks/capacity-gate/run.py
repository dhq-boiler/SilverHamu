"""Bounded capacity comparison; the output and data volume are new per run."""
import argparse,json,subprocess,uuid
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--image',default='catdb-capacity-gate');p.add_argument('--output',type=Path,required=True);a=p.parse_args();a.output.mkdir(parents=True,exist_ok=False)
volume='catdb-capacity-'+uuid.uuid4().hex;subprocess.run(['docker','volume','create',volume],check=True)
cases=[(4,1),(16,1),(64,1),(16,2),(64,2),(4,2),(64,3),(4,3),(16,3)]
image=subprocess.check_output(['docker','image','inspect',a.image,'--format','{{.Id}}'],text=True).strip()
(a.output/'environment.json').write_text(json.dumps({'image':image,'volume':volume,'cpus':2,'memory':'4g','order':cases},indent=2)+'\n')
for cap,trial in cases:
    command=['docker','run','--rm','--cpus=2','--memory=4g','--mount',f'type=volume,source={volume},target=/data','-e','CAP_ROOT=/data','-e',f'CAP_TRIAL={trial}',a.image,'cargo','test','--locked','--release','--lib',f'transactions::capacity_gate::cap{cap}::tests::capacity_speed','--','--exact','--ignored','--nocapture','--test-threads=1']
    path=a.output/f'cap-{cap}-{trial}.log'
    with path.open('w',encoding='utf-8') as log:subprocess.run(command,stdout=log,stderr=subprocess.STDOUT,check=True)
    if 'CAP_SUMMARY' not in path.read_text():raise RuntimeError('Benchmark did not execute')
    print(f'Completed capacity {cap}, trial {trial}',flush=True)
print(f'Volume retained: {volume}')
