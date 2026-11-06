"""Pilot only: a failed gate must not silently launch a longer comparison."""
import argparse,datetime,hashlib,json,pathlib,subprocess,sys
ROOT=pathlib.Path(__file__).resolve().parents[2]
def main():
    p=argparse.ArgumentParser();p.add_argument('--out',type=pathlib.Path,required=True);p.add_argument('--budget-aware',action='store_true');p.add_argument('--safety-only',action='store_true');a=p.parse_args();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);raw=out/'raw';raw.mkdir();schedule=[]
    image='catdb-constrained-backlog:profile';imageid=subprocess.check_output(['docker','image','inspect',image,'--format','{{.Id}}'],text=True).strip();(raw/'image.json').write_text(json.dumps({'image':image,'id':imageid}))
    files=['benchmarks/resource-envelope/run.py','benchmarks/resource-envelope/client.py','benchmarks/resource-envelope/supervisor.py']+[str(f.relative_to(ROOT)).replace('\\','/') for f in (ROOT/'benchmarks/constrained-backlog').glob('*.py')]
    (raw/'sources.json').write_text(json.dumps({f:hashlib.sha256((ROOT/f).read_bytes().replace(b'\r\n',b'\n')).hexdigest() for f in files},indent=2))
    conditions=[('single',1),('single',8)] if a.safety_only else [('normal',1),('normal',8),('single',8),('single',1)]
    for scenario,limit in conditions:
        label=f'pilot-{scenario}-{limit}';folder=raw/label
        cmd=[sys.executable,'-X','utf8',str(ROOT/'benchmarks/resource-envelope/run.py'),'--out',str(folder),'--rows','1000000','--cpus','1','--memory','512','--reps','1','--ops','2000','--rate','1000','--retained-bytes','671088640','--staged-bytes','4096','--pin-reader','1','--age-seed','--safety-probe','--diagnostics','--image',image,'--storage-mode','volume','--batch-limit',str(limit),'--stall-scenario',scenario]
        if a.budget_aware:cmd+=['--preparation-budget','4096']
        entry={'label':label,'scenario':scenario,'limit':limit,'command':cmd,'started':datetime.datetime.now(datetime.timezone.utc).isoformat()}
        print('START',label,flush=True);subprocess.run(cmd,cwd=ROOT,check=True);res=json.loads((folder/'results.json').read_text())[0];entry['status']=res['status'];entry['finished']=datetime.datetime.now(datetime.timezone.utc).isoformat();schedule.append(entry);(raw/'schedule.json').write_text(json.dumps(schedule,indent=2))
        assert res['status']=='passed' and len(res['verification'])==5 and all(v['passed'] for v in res['verification']),res
        assert json.loads((folder/'environment.json').read_text())['image']==imageid
        print('END',label,flush=True)
if __name__=='__main__':main()
