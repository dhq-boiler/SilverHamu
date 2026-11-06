"""Pilot-gated aged-service comparison; never repeat a failed/OOM condition."""
import argparse, datetime, hashlib, json, pathlib, subprocess, sys
ROOT=pathlib.Path(__file__).resolve().parents[2]

def main():
    p=argparse.ArgumentParser();p.add_argument('--out',type=pathlib.Path,required=True);p.add_argument('--rows',type=int,default=1000000);p.add_argument('--smoke',action='store_true');a=p.parse_args()
    out=a.out.resolve();out.mkdir(parents=True,exist_ok=False);schedule=[];eligible=[];blocked=set()
    sources=['Cargo.toml','src/transactions.rs','benchmarks/resource-envelope/run.py','benchmarks/resource-envelope/client.py','benchmarks/resource-envelope/supervisor.py','benchmarks/aged-service/age_seed.py']
    (out/'source-sha256.json').write_text(json.dumps({x:hashlib.sha256((ROOT/x).read_bytes().replace(b'\r\n',b'\n')).hexdigest() for x in sources},indent=2))
    def execute(stage,rep,variant,memory):
        label=f'{stage}-{rep}-{variant}-{memory}';dest=out/label
        args=[sys.executable,'-X','utf8',str(ROOT/'benchmarks/resource-envelope/run.py'),'--out',str(dest),
              '--rows',str(a.rows),'--cpus','1','--memory',str(memory),'--reps','1','--ops',str(200 if a.smoke else 2000 if stage=='pilot' else 8000),
              '--rate','1000','--retained-bytes','671088640','--staged-bytes','4096','--pin-reader','0','--age-seed','--safety-probe','--image',f'catdb-aged:{variant}']
        entry={'label':label,'stage':stage,'rep':rep,'variant':variant,'memory_mib':memory,'start':datetime.datetime.now(datetime.timezone.utc).isoformat(),'command':args}
        print('START',label,flush=True);subprocess.run(args,cwd=ROOT,check=True)
        results=json.loads((dest/'results.json').read_text());assert len(results)==1
        result=results[0];entry['status']=result['status'];entry['end']=datetime.datetime.now(datetime.timezone.utc).isoformat()
        resources=dest/result['name']/'boot-resources.jsonl'
        entry['oom_kills']=max([json.loads(x)['events']['oom_kill'] for x in resources.read_text().splitlines()] or [0]) if resources.exists() else 0
        schedule.append(entry);(out/'schedule.json').write_text(json.dumps(schedule,indent=2))
        print('END',label,entry['status'],'oom',entry['oom_kills'],flush=True)
        if entry['status']!='passed':
            blocked.add((variant,memory))
            assert entry['oom_kills']>0,('Unexpected failure; stop rather than silently exclude',result)
            return False
        assert len(result['verification'])==5 and all(v['passed'] for v in result['verification'])
        assert result['safety_probe']['commit_after_rollback']
        return True
    conditions=[('deque',512),('inline',512),('deque',1024),('inline',1024)]
    if a.smoke:conditions=[('deque',512),('inline',512)]
    for variant,memory in conditions:
        if execute('pilot',1,variant,memory):eligible.append((variant,memory))
    if not a.smoke:
        for rep in range(1,4):
            order=eligible[rep-1:]+eligible[:rep-1]
            if rep%2==0:order=list(reversed(order))
            for variant,memory in order:
                if (variant,memory) not in blocked:execute('main',rep,variant,memory)
    (out/'decision.json').write_text(json.dumps({'pilot_eligible':eligible,'blocked_conditions':sorted(blocked),'no_automatic_extension':True},indent=2))

if __name__=='__main__':main()
