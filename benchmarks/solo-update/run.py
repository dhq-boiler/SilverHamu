"""Bounded isolated fixtures; no server or existing data is used."""
import argparse,datetime,hashlib,json,pathlib,random,subprocess,uuid
ROOT=pathlib.Path(__file__).resolve().parents[2]
def run(*args):
    p=subprocess.run(list(map(str,args)),capture_output=True,text=True,encoding='utf-8',errors='replace',timeout=600)
    if p.returncode:raise RuntimeError(p.stderr[-3000:]+p.stdout[-1000:])
    return p.stdout.strip()
def main():
    p=argparse.ArgumentParser();p.add_argument('--out',type=pathlib.Path,required=True);p.add_argument('--rows',type=int,default=100000);p.add_argument('--ops',type=int,default=8000);p.add_argument('--reps',type=int,default=3);a=p.parse_args()
    assert a.rows>0 and 0<a.ops<=8000
    out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
    variants=['cabi-on','rust-on','sqlite','cabi-off','rust-off','rust-on-diag','rust-off-diag','sqlite-pinned']
    meta={'utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'rows':a.rows,'ops':a.ops,'reps':a.reps,'cpu':2,'memory_gib':4,'old_snapshot_retained':False,'image':run('docker','image','inspect','catdb-solo:local','--format','{{.Id}}'),'source_sha256':{f.relative_to(ROOT).as_posix():hashlib.sha256(f.read_bytes().replace(b'\r\n',b'\n')).hexdigest() for f in [ROOT/'src/bin/solo-update-bench.rs',ROOT/'benchmarks/solo-update/client.py',ROOT/'benchmarks/solo-update/run.py',ROOT/'src/group_commit.rs',ROOT/'src/transactions.rs']},'runs':[]}
    def save():(out/'environment.json').write_text(json.dumps(meta,indent=2))
    save()
    for rep in range(1,a.reps+1):
        rng=random.Random(424242+rep*100)
        plan={'rows':a.rows,'operations':[(rng.randrange(a.rows),rep*1000000000+10000000+i+1) for i in range(a.ops)]}
        planpath=out/f'plan-{rep}.json';planpath.write_text(json.dumps(plan))
        order=variants if rep%2 else list(reversed(variants))
        if rep==3:order=order[2:]+order[:2]
        for variant in order:
            label=f'r{rep}-{variant}';name='catdb-solo-'+uuid.uuid4().hex[:10];volume=name+'-data'
            entry={'rep':rep,'variant':variant,'status':'running','file':label+'.json'};meta['runs'].append(entry);save()
            print(label+' start',flush=True);run('docker','volume','create',volume)
            try:
                group='off' if '-off' in variant else 'on'
                command=['solo-update-diag' if variant.endswith('-diag') else 'solo-update-bench','/input/plan.json',group] if variant.startswith('rust') else ['python','/bench/solo-update/client.py','/input/plan.json','sqlite' if variant.startswith('sqlite') else 'catdb-embedded',group,'1' if variant=='sqlite-pinned' else '0']
                data=json.loads(run('docker','run','--name',name,'--network','none','--cpus','2','--memory','4g','--memory-swap','4g','-v',volume+':/data','-v',str(planpath)+':/input/plan.json:ro','catdb-solo:local',*command))
                assert data['checks']==['initial','old','latest','reopen'] and len(data['samples_ns'])==a.ops
                (out/entry['file']).write_text(json.dumps(data))
                entry['status']='passed';save();ns=sorted(data['samples_ns']);print(f"{label} {a.ops/data['elapsed_s']:.1f}/s p99={ns[(len(ns)*99+99)//100-1]/1e6:.3f}ms",flush=True)
            except Exception as e:entry['status']='failed';entry['error']=str(e);save();raise
            finally:
                run('docker','rm','-f',name);run('docker','volume','rm',volume)
if __name__=='__main__':main()
