"""Bounded pilot: two short trials per mode, reverse the second pair's order."""
import argparse, hashlib, json, pathlib, subprocess, sys
ROOT=pathlib.Path(__file__).resolve().parents[2]

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--out',type=pathlib.Path,required=True);a=parser.parse_args()
    out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
    sources=['Cargo.toml','src/transactions.rs','src/group_commit.rs','src/service.rs',
             'benchmarks/resource-envelope/client.py','benchmarks/resource-envelope/run.py']
    (out/'sources.json').write_text(json.dumps({s:hashlib.sha256((ROOT/s).read_bytes()).hexdigest() for s in sources},indent=2))
    for i,mode in enumerate(['off','on','on','off'],1):
        subprocess.run([sys.executable,str(ROOT/'benchmarks/resource-envelope/run.py'),'--out',str(out/f'pilot-{i}-{mode}'),
            '--rows','1000000','--cpus','1','--memory','1024','--reps','1','--ops','2000','--rate','1000',
            '--retained-bytes','671088640','--pin-reader','0','--image',f'catdb-retirement:{mode}',
            *(['--diagnostics'] if mode=='on' else [])],cwd=ROOT,check=True)
    subprocess.run([sys.executable,str(pathlib.Path(__file__).with_name('analyze.py')),str(out)],check=True)

if __name__=='__main__':main()
