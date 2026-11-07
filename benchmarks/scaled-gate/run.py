"""Stop at the first failed gate; no retries or production paths."""
import argparse,datetime,hashlib,json,pathlib,subprocess,time,uuid
ROOT=pathlib.Path(__file__).resolve().parents[2]
def call(*args,timeout=600):
    p=subprocess.run(list(map(str,args)),capture_output=True,text=True,encoding='utf-8',errors='replace',timeout=timeout)
    if p.returncode:raise RuntimeError(p.stdout[-2000:]+p.stderr[-2000:])
    return p.stdout.strip()
def main(include_dirty=False):
    p=argparse.ArgumentParser();p.add_argument('--out',type=pathlib.Path,required=True);p.add_argument('--stage',choices=['pilot','100k','1m'],required=True);a=p.parse_args()
    n,ops,cap,reps={'pilot':(10000,64,16,1),'100k':(100000,1024,256,3),'1m':(1000000,1024,256,3)}[a.stage]
    image='catdb-dirty-gate' if include_dirty else 'catdb-scaled-gate'
    orders=([('wal','dual','dirty'),('dual','dirty','wal'),('dirty','wal','dual')] if include_dirty else [('wal','dual'),('dual','wal'),('wal','dual')])
    root=a.out.resolve();root.mkdir(parents=True,exist_ok=False);results=[]
    files=['Cargo.toml','Cargo.lock','src/transactions.rs','src/lib.rs',*[str(p.relative_to(ROOT)).replace('\\','/') for p in (ROOT/'benchmarks/scaled-gate').glob('*.rs')]]
    (root/'environment.json').write_text(json.dumps({'commit':call('git','rev-parse','HEAD'),'image':call('docker','image','inspect',image,'--format','{{.Id}}'),'stage':a.stage,'rows':n,'ops':ops,'log_capacity':cap,'cpu':2,'memory_bytes':4*1024**3,'swap_bytes':0,'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'hashes':{f:hashlib.sha256((ROOT/f).read_bytes().replace(b'\r\n',b'\n')).hexdigest() for f in files}},indent=2))
    for rep in range(1,reps+1):
        for mode in orders[rep-1]:
            uid='catdb-scaled-'+uuid.uuid4().hex[:12];volume=uid+'-data';label=f'{rep}-{mode}';row={'rep':rep,'mode':mode,'rows':n,'ops':ops,'capacity':cap,'label':label,'status':'running'}
            results.append(row);(root/'runs.json').write_text(json.dumps(results,indent=2));print('START',a.stage,label,flush=True)
            call('docker','volume','create',volume)
            command=['docker','run','--name',uid,'--network','none','--cpus','2','--memory','4g','--memory-swap','4g','-v',volume+':/data','-e','SCALED_ROOT=/data','-e',f'SCALED_ROWS={n}','-e',f'SCALED_OPS={ops}','-e',f'SCALED_CAP={cap}','-e',f'SCALED_VARIANT={mode}',image,'cargo','test','--locked','--release','--lib','transactions::scaled_gate::checks::timing::compare','--','--ignored','--exact','--nocapture','--test-threads=1']
            row['command']=command;start=time.monotonic()
            try:
                p=subprocess.run(command,capture_output=True,text=True,encoding='utf-8',errors='replace',timeout=600)
                log=p.stdout+'\n'+p.stderr;(root/(label+'.log')).write_text(log,encoding='utf-8')
                row['state']=json.loads(call('docker','inspect',uid,'--format','{{json .State}}'))
                summary=[l[l.index('SCALED_SUMMARY,'):] for l in log.splitlines() if 'SCALED_SUMMARY,' in l]
                requests=[l for l in log.splitlines() if l.startswith('SCALED_REQUEST,')]
                assert p.returncode==0 and not row['state']['OOMKilled'],(p.returncode,log[-1500:])
                assert len(summary)==1 and len(requests)==ops,('missing measurements',len(summary),len(requests))
                row['summary']=summary[0];row['status']='passed'
            except Exception as e:
                row['status']='failed';row['error']=str(e);raise
            finally:
                row['elapsed_s']=time.monotonic()-start;(root/'runs.json').write_text(json.dumps(results,indent=2))
                call('docker','rm','-f',uid);call('docker','volume','rm',volume)
            print('END',a.stage,label,row['summary'],flush=True)
if __name__=='__main__':main()
