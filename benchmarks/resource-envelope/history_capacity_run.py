"""Bounded history-aging diagnostics; only uniquely owned Docker resources."""
import argparse,datetime,hashlib,json,pathlib,subprocess,uuid
ROOT=pathlib.Path(__file__).resolve().parents[2]
p=argparse.ArgumentParser();p.add_argument('--out',type=pathlib.Path,required=True);p.add_argument('--rows',type=int,default=100000);p.add_argument('--reps',type=int,default=1);p.add_argument('--observer',choices=['both','counted'],default='both');p.add_argument('--check512',action='store_true');p.add_argument('--image',default='catdb-history-capacity:local');a=p.parse_args()
assert 100<=a.rows<=1000000 and a.rows%100==0 and 1<=a.reps<=3
a.out.mkdir(parents=True,exist_ok=False);image=a.image
def run(args,check=True,timeout=240):
    r=subprocess.run(list(map(str,args)),capture_output=True,text=True,encoding='utf8',errors='replace',timeout=timeout)
    if check and r.returncode:raise RuntimeError(r.stderr[-2500:]+r.stdout[-1000:])
    return r
def command(*args):return run(args).stdout.strip()
sources=['Cargo.toml','src/transactions.rs','src/bin/history-capacity-bench.rs','benchmarks/resource-envelope/history_capacity_run.py','benchmarks/resource-envelope/history_capacity_observer.py','benchmarks/resource-envelope/history-capacity.Dockerfile']
meta={'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'image':command('docker','image','inspect',image,'--format','{{.Id}}'),'base_commit':command('git','-C',ROOT,'rev-parse','HEAD'),'args':{k:str(v) if isinstance(v,pathlib.Path) else v for k,v in vars(a).items()},'source_sha256':{n:hashlib.sha256((ROOT/n).read_bytes().replace(b'\r\n',b'\n')).hexdigest() for n in sources}}
(a.out/'environment.json').write_text(json.dumps(meta,indent=2));results=[]
def save():(a.out/'results.json').write_text(json.dumps(results,indent=2))
for rep in range(1,a.reps+1):
    for pattern in (['spread','hot'] if rep%2 else ['hot','spread']):
        for counted in (([False,True] if rep%2 else [True,False]) if a.observer=='both' else [True]):
            tag=f'r{rep}-{pattern}-'+('counted' if counted else 'plain');folder=a.out/tag;folder.mkdir();vol='catdb-history-'+uuid.uuid4().hex[:12];entry={'name':tag,'rep':rep,'pattern':pattern,'counted':counted,'status':'running','phases':{}};results.append(entry);save()
            print(tag,'start',flush=True);command('docker','volume','create',vol)
            def phase(label,mode,memory=1024,observed=None,expected=True):
                observe=counted if observed is None else observed;binary='probe-counted' if observe else 'probe-plain';name=vol+'-'+label
                cmd=['docker','run','--name',name,'--network','none','--cpus','1' if mode!='init' else '2','--memory',f'{memory}m','--memory-swap',f'{memory}m','--pids-limit','128','-v',vol+':/data','-v',str(ROOT/'benchmarks/resource-envelope')+':/bench:ro',image,'python','/bench/history_capacity_observer.py',label,binary,mode,'/data/db.wal',a.rows,pattern,'1' if observe else '0']
                try:
                    r=run(cmd,check=False);state=json.loads(command('docker','inspect',name,'--format','{{json .State}}'));config=json.loads(command('docker','inspect',name,'--format','{{json .HostConfig}}'))
                    assert config['Memory']==memory*2**20 and config['MemorySwap']==memory*2**20 and config['NanoCpus']==(2 if mode=='init' else 1)*10**9
                    for suffix in ['.jsonl','.stderr','-resources.jsonl','-exit.json']:
                        command('docker','cp',name+':/data/'+label+suffix,folder/(label+suffix))
                    events=[json.loads(l) for l in (folder/(label+'.jsonl')).read_text().splitlines()];samples=[json.loads(l) for l in (folder/(label+'-resources.jsonl')).read_text().splitlines()];exit_info=json.loads((folder/(label+'-exit.json')).read_text())
                    assert samples and all(s['swap']==0 for s in samples)
                    success=r.returncode==0 and exit_info['returncode']==0
                    checks=[e for e in events if e['event']=='verified'];complete=any(e['event']=='complete' for e in events)
                    if success:assert checks and (mode!='run' or complete)
                    result={'success':success,'state':state,'exit':exit_info,'memory_mib':memory,'checks':checks,'complete':complete,'oom_kills':samples[-1]['events']['oom_kill'],'command':cmd}
                    entry['phases'][label]=result;save()
                    if expected:assert success,(tag,label,(folder/(label+'.stderr')).read_text())
                    elif not success:assert result['oom_kills']>0 and not exit_info['timed_out'],result
                    return success
                finally:run(['docker','rm','-f',name],check=False)
            try:
                phase('seed','init',4096,False)
                phase('aging','run');phase('reopen','verify')
                if a.check512:
                    phase('aged512','verify',512,expected=False)
                    phase('checkpoint1024','checkpoint')
                    phase('compact512','verify',512)
                entry['status']='passed'
            except BaseException as e:
                entry['status']='failed';entry['error']=str(e);raise
            finally:
                save();run(['docker','volume','rm',vol],check=False)
            print(tag,entry['status'],flush=True)
