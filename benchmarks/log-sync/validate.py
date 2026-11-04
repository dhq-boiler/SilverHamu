"""Check every repeat, raw sample, value hash and persisted restart result."""
import gzip, itertools, json, math, pathlib, sys
root=pathlib.Path(sys.argv[1])
env=json.loads((root/'environment.json').read_text(encoding='utf-8'))
rows=json.loads((root/'results.json').read_text(encoding='utf-8'))
engines=set(env['images'])
conditions=set(itertools.product(env['workloads'],env['clients']))
expected=set(itertools.product(engines,range(1,env['reps']+1),conditions))
assert env['reps']==3 and env['rows']==100000 and env['value_bytes']==256 and env['ops_per_case']==8000
seen=set()
for row in rows:
    identity=(row['engine'],row['rep'],(row['workload'],row['clients']))
    assert identity not in seen
    seen.add(identity)
    with gzip.open(root/row['latency_file'],'rt',encoding='utf-8') as f:values=sorted(json.load(f))
    assert len(values)==row['completed']==row['attempted']==8000
    assert not row['errors'] and row['retries']==0 and all(isinstance(v,int) and v>0 for v in values)
    for p in (50,95,99):assert row[f'p{p}_ms']==values[math.ceil(len(values)*p/100)-1]/1e6
    assert row['max_ms']==values[-1]/1e6
    assert abs(row['ops_s']-len(values)/row['elapsed_s'])<1e-8
assert seen==expected
assert len(env['runs'])==len(engines)*3
stages={'initial','reopen'}|{f'{w}-{c}' for w,c in conditions}
for rep in (1,2,3):
    runs=[r for r in env['runs'] if r['rep']==rep]
    assert len(runs)==len(engines) and {r['engine'] for r in runs}==engines
    for run in runs:
        assert run['status']=='passed' and len(run['verification'])==len(stages)
        assert {c['stage'] for c in run['verification']}==stages
        for c in run['verification']:assert c['sha256']==c['expected_sha256'] and c['rows']==100000 and c['empty_and_last']
    for stage in stages:assert len({c['sha256'] for r in runs for c in r['verification'] if c['stage']==stage})==1
for engine in engines:
    versions=[r['metadata'] for r in env['runs'] if r['engine']==engine]
    assert all(v==versions[0] for v in versions)
print(f"Validated {len(rows)} cases, {sum(r['completed'] for r in rows)} operations and {len(env['runs'])*len(stages)} content checks, including restarts and cross-engine equality.")
