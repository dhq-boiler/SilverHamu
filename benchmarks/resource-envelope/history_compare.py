"""Counterbalanced rounds using the existing resource-envelope workload."""
import argparse, hashlib, json, pathlib, subprocess, sys, time

p=argparse.ArgumentParser()
p.add_argument('--out',type=pathlib.Path,required=True)
p.add_argument('--before',default='catdb-resource-config:local')
p.add_argument('--after',default='catdb-history-one:local')
a=p.parse_args()
repo=pathlib.Path(__file__).resolve().parents[2]
a.out.mkdir(parents=True,exist_ok=False)
manifest={'images':{},'source_sha256':{},'runs':[]}
for name in ['before','after']:
    image=getattr(a,name)
    manifest['images'][name]={'tag':image,'id':subprocess.check_output(['docker','image','inspect',image,'--format','{{.Id}}'],text=True).strip()}
for name in ['src/transactions.rs','benchmarks/resource-envelope/run.py','benchmarks/resource-envelope/client.py','benchmarks/resource-envelope/supervisor.py','benchmarks/resource-envelope/history_compare.py']:
    manifest['source_sha256'][name]=hashlib.sha256((repo/name).read_bytes().replace(b'\r\n',b'\n')).hexdigest()
def save(): (a.out/'manifest.json').write_text(json.dumps(manifest,indent=2),encoding='utf8')
save()
for rep in range(1,4):
    for cohort in (['before','after'] if rep%2 else ['after','before']):
        for scale in ['million','control']:
            dest=a.out/f'{scale}-{cohort}-r{rep}'
            args=[sys.executable,'-X','utf8',str(repo/'benchmarks/resource-envelope/run.py'),'--out',str(dest),'--image',getattr(a,cohort),'--reps','1']
            if scale=='million': args+=['--rows','1000000','--cpus','1','2','--memory','512','1024','--retained-bytes','671088640','--recover-memory','1024']
            else: args+=['--rows','100000','--cpus','1','--memory','512']
            entry={'cohort':cohort,'scale':scale,'rep':rep,'directory':dest.name,'start_unix':time.time(),'command':args,'status':'running'}
            manifest['runs'].append(entry);save()
            print(f'ROUND {rep} {cohort} {scale}',flush=True)
            try:
                subprocess.run(args,cwd=repo,check=True)
                entries=json.loads((dest/'results.json').read_text())
                assert len(entries)==(4 if scale=='million' else 1)
                assert all(e['status'] in ['passed','failed'] for e in entries)
                entry['status']='completed'
            except BaseException:
                entry['status']='interrupted';raise
            finally: entry['finish_unix']=time.time();save()
