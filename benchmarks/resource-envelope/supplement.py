"""Alternating observer runs and a separately labelled memory-failure rescue."""
import argparse, pathlib, subprocess, sys

def main():
    p=argparse.ArgumentParser();p.add_argument('--out-root',type=pathlib.Path,required=True);a=p.parse_args()
    a.out_root.mkdir(parents=True,exist_ok=True)
    runner=pathlib.Path(__file__).with_name('run.py')
    def execute(name,*args):
        subprocess.run([sys.executable,str(runner),'--out',str(a.out_root/name),'--rows','100000','--reps','1',*args],check=True)
    for rep in range(1,4):
        for sample in ([0,1] if rep%2 else [1,0]):
            execute(f'resource-load-r{rep}-sample{sample}','--cpus','1','--memory','512','--rate','5000','--sample',str(sample))
        execute(f'resource-load-r{rep}-cpu2','--cpus','2','--memory','512','--rate','5000')
    execute('resource-oom','--cpus','1','--memory','32','--recover-memory','512')

if __name__=='__main__':main()
