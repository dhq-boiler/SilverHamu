"""Run bounded observer pilots or the approved main comparison; retain data volume."""
import argparse,json,subprocess,uuid
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--stage',choices=['pilot','main'],required=True);p.add_argument('--image',default='catdb-reuse-phases');p.add_argument('--output',type=Path,required=True);a=p.parse_args()
a.output.mkdir(parents=True,exist_ok=False)
volume='catdb-reuse-phases-'+uuid.uuid4().hex
subprocess.run(['docker','volume','create',volume],check=True)
if a.stage=='pilot':
    count=400;cases=[('clear',0,1),('clear',1,1),('reuse',0,1),('reuse',1,1),('reuse',1,2),('reuse',0,2),('clear',1,2),('clear',0,2)]
else:
    count=2000;cases=[('clear',1,3),('reuse',1,3),('reuse',1,4),('clear',1,4),('clear',1,5),('reuse',1,5)]
image=subprocess.check_output(['docker','image','inspect',a.image,'--format','{{.Id}}'],text=True).strip()
(a.output/'environment.json').write_text(json.dumps({'volume':volume,'image':image,'stage':a.stage,'cpu_quota':2,'memory':'4g','count':count,'cases':cases},indent=2)+'\n')
for mode,enabled,trial in cases:
    command=['docker','run','--rm','--cpus=2','--memory=4g','--mount',f'type=volume,source={volume},target=/data','-e',f'REUSE_SPEED_MODE={mode}','-e',f'REUSE_SPEED_TRIAL={trial}','-e','REUSE_SPEED_ROOT=/data','-e',f'PHASE_ENABLED={enabled}','-e',f'PHASE_COUNT={count}',a.image,'cargo','test','--locked','--release','--lib','transactions::variable_gate::tests::reuse_phases','--','--exact','--ignored','--nocapture','--test-threads=1']
    with (a.output/f'{a.stage}-{mode}-{enabled}-{trial}.log').open('w',encoding='utf-8') as log:subprocess.run(command,stdout=log,stderr=subprocess.STDOUT,check=True)
    print(f'Completed {mode}, observer={enabled}, trial={trial}',flush=True)
print(f'Volume retained: {volume}')
