"""Bounded reproduction and observer comparison; existing durability checks remain enabled."""
import argparse,datetime,hashlib,json,pathlib,subprocess,sys
ROOT=pathlib.Path(__file__).resolve().parents[2]

def main():
    p=argparse.ArgumentParser();p.add_argument('--out',type=pathlib.Path,required=True);a=p.parse_args()
    out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);schedule=[]
    files=['Cargo.toml','src/transactions.rs','src/group_commit.rs','src/service.rs','benchmarks/resource-envelope/run.py','benchmarks/resource-envelope/client.py','benchmarks/aged-service/age_seed.py','benchmarks/aged-tail/run.py']
    (out/'sources.json').write_text(json.dumps({f:hashlib.sha256((ROOT/f).read_bytes().replace(b'\r\n',b'\n')).hexdigest() for f in files},indent=2))
    conditions=[('main',rep,v,8000) for rep in range(1,4) for v in (['deque','inline'] if rep%2 else ['inline','deque'])]
    conditions += [('probe',i,v,2000) for i,v in enumerate(['inline','diagnostic','diagnostic','inline'],1)]
    for stage,rep,variant,ops in conditions:
        label=f'{stage}-{rep}-{variant}';folder=out/label
        command=[sys.executable,'-X','utf8',str(ROOT/'benchmarks/resource-envelope/run.py'),'--out',str(folder),
            '--rows','1000000','--cpus','1','--memory','1024','--reps','1','--ops',str(ops),'--rate','1000',
            '--retained-bytes','671088640','--staged-bytes','4096','--pin-reader','0','--age-seed','--safety-probe',
            '--image',f'catdb-aged-tail:{variant}',*(['--diagnostics'] if variant=='diagnostic' else [])]
        row={'label':label,'stage':stage,'rep':rep,'variant':variant,'ops':ops,'command':command,'start':datetime.datetime.now(datetime.timezone.utc).isoformat()}
        print('START',label,flush=True);subprocess.run(command,cwd=ROOT,check=True)
        results=json.loads((folder/'results.json').read_text());assert len(results)==1
        result=results[0];row['status']=result['status'];row['end']=datetime.datetime.now(datetime.timezone.utc).isoformat()
        schedule.append(row);(out/'schedule.json').write_text(json.dumps(schedule,indent=2))
        assert result['status']=='passed',result
        assert len(result['verification'])==5 and all(v['passed'] for v in result['verification'])
        print('END',label,flush=True)

if __name__=='__main__':main()
