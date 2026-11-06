import argparse,datetime,gzip,hashlib,json,pathlib,random,subprocess,uuid
def run(*args):
    p=subprocess.run(list(map(str,args)),capture_output=True,text=True,encoding='utf-8',errors='replace',timeout=600)
    if p.returncode:raise RuntimeError(p.stderr[-3000:]+p.stdout[-1000:])
    return p.stdout.strip()
def main():
    p=argparse.ArgumentParser();p.add_argument('--out',type=pathlib.Path,required=True);p.add_argument('--ops',type=int,default=8000);p.add_argument('--reps',type=int,default=3);a=p.parse_args()
    assert 0<a.ops<=8000 and a.reps in (1,3)
    out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
    variants=['catdb-plain','catdb-trace','sqlite-plain','sqlite-trace','sqlite-pinned-plain','sqlite-pinned-trace']
    source=pathlib.Path(__file__).parent
    env={'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'rows':100000,'ops':a.ops,'reps':a.reps,'cpus':2,'memory':'4g','swap':'disabled','image':run('docker','image','inspect','catdb-io-phases:local','--format','{{.Id}}'),'source_sha256':{p.name:hashlib.sha256(p.read_bytes().replace(b'\r\n',b'\n')).hexdigest() for p in source.iterdir() if p.suffix in ('.py','.c') or p.name=='Dockerfile'},'runs':[]}
    def save():(out/'environment.json').write_text(json.dumps(env,indent=2))
    save()
    for rep in range(1,a.reps+1):
        rng=random.Random(424242+rep*100);plan={'rows':100000,'operations':[(rng.randrange(100000),rep*1000000000+10000000+i+1) for i in range(a.ops)]}
        pp=out/f'plan-{rep}.json';pp.write_text(json.dumps(plan))
        order=variants if rep%2 else list(reversed(variants))
        if rep==3:order=order[2:]+order[:2]
        for variant in order:
            label=f'r{rep}-{variant}';name='catdb-io-'+uuid.uuid4().hex[:12];vol=name+'-data'
            row={'rep':rep,'variant':variant,'file':label+'.json.gz','status':'running'};env['runs'].append(row);save();print(label+' start',flush=True)
            run('docker','volume','create',vol)
            try:
                trace=variant.endswith('trace');opts=['-e','LD_PRELOAD=/usr/local/lib/libiotrace.so'] if trace else []
                data=json.loads(run('docker','run','--name',name,'--network','none','--cpus','2','--memory','4g','--memory-swap','4g','-v',vol+':/data','-v',str(pp)+':/input/plan.json:ro',*opts,'catdb-io-phases:local','python','/io/client.py','/input/plan.json','catdb-embedded' if variant.startswith('catdb') else 'sqlite',str(int(trace)),str(int('pinned' in variant))))
                assert data['checks']==['initial','old','latest','reopen'] and len(data['samples_ns'])==a.ops
                with gzip.open(out/row['file'],'wt') as f:json.dump(data,f)
                row['status']='passed';save();ns=sorted(data['samples_ns']);print(f"{label} {a.ops/data['elapsed_s']:.1f}/s p99={ns[(len(ns)*99+99)//100-1]/1e6:.3f}ms events={len(data['events'])}",flush=True)
            except Exception as e:row['status']='failed';row['error']=str(e);save();raise
            finally:run('docker','rm','-f',name);run('docker','volume','rm',vol)
if __name__=='__main__':main()
