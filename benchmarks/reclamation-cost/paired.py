"""Native diagnostic comparison; isolated containers, alternating order, all samples retained."""
import argparse, gzip, json, pathlib, subprocess
p=argparse.ArgumentParser(); p.add_argument('--out',type=pathlib.Path,required=True); p.add_argument('--image-prefix',default='catdb-reclaim-cost-before'); p.add_argument('--scenarios',nargs='+',default=['mixed','large','gc']); p.add_argument('--background-only',action='store_true'); a=p.parse_args()
a.out.mkdir(parents=True,exist_ok=False)
images={'before':'catdb-reclaim-cost-before-diag:local','after':a.image_prefix+'-diag:local','before-light':'catdb-reclaim-cost-before-light:local','after-light':a.image_prefix+'-light:local'}
meta={'scenarios':a.scenarios,'cpus':2,'memory_gib':4,'reps':3,'images':{k:subprocess.check_output(['docker','image','inspect',v,'--format','{{.Id}}'],text=True).strip() for k,v in images.items()}}
(a.out/'environment.json').write_text(json.dumps(meta,indent=2),encoding='utf-8')
results=[]
for rep in range(1,4):
 for scenario in a.scenarios:
  for variant in (list(images) if rep%2 else list(reversed(images))):
   raw=subprocess.check_output(['docker','run','--rm','--cpus','2','--memory','4g','-e','CATDB_BACKGROUND_RECLAMATION=1',images[variant],'reclamation-bench','/tmp/probe',scenario],timeout=300)
   r=json.loads(raw); assert r['verified']; r.update(variant=variant,rep=rep)
   name=f'{variant}-{scenario}-r{rep}.json.gz'
   with gzip.open(a.out/name,'wt',encoding='utf-8') as f:json.dump(r,f)
   for group in ('readers','writers'):
    for thread in r[group]:thread['latency'].pop('samples_ns')
   r['raw_file']=name;results.append(r)
   (a.out/'results.json').write_text(json.dumps(results,indent=2),encoding='utf-8')
   print(variant,scenario,rep,'verified',flush=True)
