"""Only enter the bounded six-trial comparison after trace identity and counts pass."""
import argparse,hashlib,json,pathlib,subprocess,sys,datetime
from analyze import check
ROOT=pathlib.Path(__file__).resolve().parents[2]
def main():
    p=argparse.ArgumentParser();p.add_argument('--out',type=pathlib.Path,required=True);a=p.parse_args();out=a.out.resolve()
    check(out/'pilot-trace-named')
    raw=out/'raw';raw.mkdir(exist_ok=False);schedule=[]
    files=['src/service.rs','src/transactions.rs','src/group_commit.rs','Cargo.toml','Cargo.lock','benchmarks/resource-envelope/run.py','benchmarks/resource-envelope/client.py']+[str(f.relative_to(ROOT)).replace('\\','/') for f in (ROOT/'benchmarks/aged-sync').glob('*.py')]
    (raw/'sources.json').write_text(json.dumps({f:hashlib.sha256((ROOT/f).read_bytes().replace(b'\r\n',b'\n')).hexdigest() for f in files},indent=2))
    image='catdb-aged-sync:profile';image_id=subprocess.check_output(['docker','image','inspect',image,'--format','{{.Id}}'],text=True).strip();(raw/'image.json').write_text(json.dumps({'image':image,'id':image_id}))
    conditions=[('pilot',1,'off',1000)]+[('main',rep,mode,8000) for rep in range(1,4) for mode in (['off','trace'] if rep%2 else ['trace','off'])]
    for stage,rep,mode,ops in conditions:
        label=f'{stage}-{rep}-{mode}';folder=raw/label
        command=[sys.executable,'-X','utf8',str(ROOT/'benchmarks/resource-envelope/run.py'),'--out',str(folder),'--rows','1000000','--cpus','1','--memory','512','--reps','1','--ops',str(ops),'--rate','1000','--retained-bytes','671088640','--staged-bytes','4096','--pin-reader','1','--age-seed','--safety-probe','--diagnostics','--kernel-mode',mode,'--image',image]
        row={'label':label,'stage':stage,'rep':rep,'variant':mode,'memory_mib':512,'ops':ops,'command':command,'start':datetime.datetime.now(datetime.timezone.utc).isoformat()}
        print('START',label,flush=True);subprocess.run(command,cwd=ROOT,check=True)
        result=json.loads((folder/'results.json').read_text())[0];row['status']=result['status'];row['end']=datetime.datetime.now(datetime.timezone.utc).isoformat()
        schedule.append(row);(raw/'schedule.json').write_text(json.dumps(schedule,indent=2))
        assert result['status']=='passed',result
        assert len(result['verification'])==5 and all(v['passed'] for v in result['verification'])
        assert json.loads((folder/'environment.json').read_text())['image']==image_id
        if mode=='trace':check(folder)
        print('END',label,flush=True)
if __name__=='__main__':main()
