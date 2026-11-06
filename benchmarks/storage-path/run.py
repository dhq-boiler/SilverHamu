"""A failed storage pilot stops that path before large-data comparisons."""
import argparse,datetime,hashlib,json,pathlib,subprocess,sys
ROOT=pathlib.Path(__file__).resolve().parents[2]
def main():
    p=argparse.ArgumentParser();p.add_argument('--out',type=pathlib.Path,required=True);p.add_argument('--bind-root',type=pathlib.Path,required=True);a=p.parse_args()
    out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);a.bind_root=a.bind_root.resolve();a.bind_root.mkdir(parents=True,exist_ok=False)
    raw=out/'raw';raw.mkdir();schedule=[];image='catdb-aged-sync:profile'
    imageid=subprocess.check_output(['docker','image','inspect',image,'--format','{{.Id}}'],text=True).strip()
    (raw/'image.json').write_text(json.dumps({'image':image,'id':imageid}))
    files=['benchmarks/resource-envelope/run.py','benchmarks/resource-envelope/client.py','benchmarks/resource-envelope/supervisor.py']+[str(f.relative_to(ROOT)).replace('\\','/') for f in (ROOT/'benchmarks/storage-path').glob('*.py')]
    (raw/'sources.json').write_text(json.dumps({f:hashlib.sha256((ROOT/f).read_bytes().replace(b'\r\n',b'\n')).hexdigest() for f in files},indent=2))
    conditions=[('pilot',1,m,1000,200) for m in ['volume','bind']]+[('main',rep,m,1000000,8000) for rep in range(1,4) for m in (['volume','bind'] if rep%2 else ['bind','volume'])]
    for stage,rep,mode,rows,ops in conditions:
        label=f'{stage}-{rep}-{mode}';folder=raw/label
        cmd=[sys.executable,'-X','utf8',str(ROOT/'benchmarks/resource-envelope/run.py'),'--out',str(folder),'--rows',str(rows),'--cpus','1','--memory','512','--reps','1','--ops',str(ops),'--rate','1000','--retained-bytes','671088640','--staged-bytes','4096','--pin-reader','1','--age-seed','--safety-probe','--diagnostics','--kernel-mode','off','--image',image,'--storage-mode',mode]
        if mode=='bind':cmd+=['--bind-root',str(a.bind_root/label)]
        entry={'label':label,'stage':stage,'rep':rep,'variant':mode,'rows':rows,'ops':ops,'command':cmd,'started':datetime.datetime.now(datetime.timezone.utc).isoformat()}
        print('START',label,flush=True);process=subprocess.run(cmd,cwd=ROOT)
        result=json.loads((folder/'results.json').read_text())[0] if (folder/'results.json').exists() else {'status':'failed','error':'runner failed before case'}
        entry.update(status=result['status'],returncode=process.returncode,finished=datetime.datetime.now(datetime.timezone.utc).isoformat());schedule.append(entry);(raw/'schedule.json').write_text(json.dumps(schedule,indent=2))
        assert process.returncode==0 and result['status']=='passed',result
        assert result['storage_probe']['exclusive_lock_refused'] and len(result['verification'])==5 and all(x['passed'] for x in result['verification'])
        assert json.loads((folder/'environment.json').read_text())['image']==imageid
        print('END',label,flush=True)
if __name__=='__main__':main()
