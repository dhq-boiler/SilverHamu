"""Allocation diagnostics are separate from the uninstrumented service runs."""
import argparse,json,pathlib,subprocess,uuid

def run(*args):
    p=subprocess.run(args,text=True,stdout=subprocess.PIPE,stderr=subprocess.PIPE,timeout=600)
    if p.returncode:raise RuntimeError(p.stdout+p.stderr)
    return p.stdout

def main():
    p=argparse.ArgumentParser();p.add_argument('--out',type=pathlib.Path,required=True);a=p.parse_args();a.out.mkdir(exist_ok=False,parents=True);results=[]
    for rows,length in [(1000000,8),(100000,32)]:
        volume='catdb-key-diagnostic-'+uuid.uuid4().hex[:12];run('docker','volume','create',volume)
        try:
            seed=run('docker','run','--rm','--cpus','2','--memory','2g','-v',volume+':/data','catdb-inline-key:baseline','key-allocation-bench','seed','/data/key.wal',str(rows),str(length))
            for rep in range(1,4):
                for variant in (['baseline','inline'] if rep%2 else ['inline','baseline']):
                    command=['docker','run','--rm','--cpus','1','--memory','512m','--memory-swap','512m','-v',volume+':/data','catdb-inline-key:'+variant,'key-allocation-bench','open','/data/key.wal',str(rows),str(length)]
                    output=run(*command);assert output.splitlines()[-1]=='verified'
                    row=json.loads(output.splitlines()[0]);row.update(rep=rep,variant=variant,verified=True);results.append(row)
                    (a.out/'results.json').write_text(json.dumps(results,indent=2));print(rows,length,rep,variant,row['live_bytes'],flush=True)
        finally:run('docker','volume','rm',volume)
if __name__=='__main__':main()
