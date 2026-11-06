"""Qualify the default against the previously explicit feature with an active old reader."""
import argparse,datetime,hashlib,json,pathlib,subprocess,sys
ROOT=pathlib.Path(__file__).resolve().parents[2]

def main():
    p=argparse.ArgumentParser();p.add_argument('--out',type=pathlib.Path,required=True);a=p.parse_args()
    out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);schedule=[]
    files=['Cargo.toml','src/transactions.rs','src/group_commit.rs','src/service.rs','benchmarks/resource-envelope/run.py','benchmarks/resource-envelope/client.py','benchmarks/aged-service/age_seed.py','benchmarks/inline-default/run.py']
    (out/'sources.json').write_text(json.dumps({f:hashlib.sha256((ROOT/f).read_bytes().replace(b'\r\n',b'\n')).hexdigest() for f in files},indent=2))
    conditions=[('main',rep,v,8000) for rep in range(1,4) for v in (['explicit','default'] if rep%2 else ['default','explicit'])]
    for stage,rep,variant,ops in conditions:
        label=f'{stage}-{rep}-{variant}';folder=out/label
        command=[sys.executable,'-X','utf8',str(ROOT/'benchmarks/resource-envelope/run.py'),'--out',str(folder),
            '--rows','1000000','--cpus','1','--memory','512','--reps','1','--ops',str(ops),'--rate','1000',
            '--retained-bytes','671088640','--staged-bytes','4096','--pin-reader','1','--age-seed','--safety-probe',
            '--image',f'catdb-inline-default:{variant}']
        row={'label':label,'stage':stage,'rep':rep,'variant':variant,'ops':ops,'memory_mib':512,'command':command,'start':datetime.datetime.now(datetime.timezone.utc).isoformat()}
        print('START',label,flush=True);subprocess.run(command,cwd=ROOT,check=True)
        results=json.loads((folder/'results.json').read_text());assert len(results)==1
        result=results[0];row['status']=result['status'];row['oom_kills']=max(json.loads(line)['events']['oom_kill'] for line in (folder/result['name']/'boot-resources.jsonl').read_text().splitlines());row['end']=datetime.datetime.now(datetime.timezone.utc).isoformat()
        schedule.append(row);(out/'schedule.json').write_text(json.dumps(schedule,indent=2))
        assert result['status']=='passed',result
        assert len(result['verification'])==5 and all(v['passed'] for v in result['verification'])
        assert all(m['pin_reader'] for m in result['measurements'])
        mixed=next(m for m in result['measurements'] if m['work']=='mixed')
        assert mixed['old_snapshot_checked_keys']==mixed['by_kind']['write']['completed']
        print('END',label,flush=True)

if __name__=='__main__':main()
