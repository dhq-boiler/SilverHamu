"""Check published summaries against raw samples and cross-engine content hashes."""
import gzip, hashlib, json, math, pathlib, sys
root=pathlib.Path(sys.argv[1]);records=json.loads((root/'results.json').read_text());env=json.loads((root/'environment.json').read_text())
if 'cases' in env:
    cases = {tuple(case) for case in env['cases']}
    engines = set(env['engines'])
    assert cases and engines and env['reps'] >= 1
else:
    cases = {(work, clients) for work in ['get','range100','update'] for clients in [1,4]}
    engines = {'catdb-embedded','sqlite','catdb-service','postgres','mysql','oracle'}
    assert env['reps'] == 3 and env['ops_per_case'] == 8000 and env['rows'] == 100000
expected = {(e, rep, w, c) for e in engines for rep in range(1,env['reps']+1) for w,c in cases}
stages = {'initial','reopen'} | {f'{w}-{c}' for w,c in cases}
assert len(records) == len(expected) and len(env['runs']) == len(engines)*env['reps']
seen=set()
for r in records:
    identity=(r['engine'],r['rep'],r['workload'],r['clients']);assert identity not in seen;seen.add(identity)
    with gzip.open(root/r['latency_file'],'rt') as f:samples=sorted(json.load(f))
    assert len(samples)==r['completed']==r['attempted']==env['ops_per_case']
    assert not r['errors']
    retries = r.get('retry_events', [])
    assert len(retries) == r['retries']
    assert all(1 <= retry['attempt'] <= env.get('serialization_retries', 0) for retry in retries)
    assert all(retry['code'] in {'ORA-08177','40001','conflict'} for retry in retries)
    assert all(isinstance(v,int) and v>0 for v in samples)
    for p in [50,95,99]:assert r[f'p{p}_ms']==samples[math.ceil(len(samples)*p/100)-1]/1e6
    assert r['max_ms']==samples[-1]/1e6
    assert abs(r['ops_s']-len(samples)/r['elapsed_s'])<1e-8
assert seen == expected
for rep in range(1,env['reps']+1):
    runs=[r for r in env['runs'] if r['rep']==rep];assert {r['engine'] for r in runs} == engines
    assert all(r['status']=='passed' and len(r['verification']) == len(stages) and {v['stage'] for v in r['verification']} == stages for r in runs)
    for stage in [v['stage'] for v in runs[0]['verification']]:
        checks=[next(v for v in r['verification'] if v['stage']==stage) for r in runs]
        assert len({v['sha256'] for v in checks})==1,(rep,stage)
        assert all(v['sha256']==v['expected_sha256'] and v['rows']==env['rows'] and v['empty_and_last'] for v in checks)
for engine in {r['engine'] for r in env['runs']}:
    versions=[r['metadata'] for r in env['runs'] if r['engine']==engine]
    assert len(versions)==env['reps'] and all(v==versions[0] for v in versions),engine
source=pathlib.Path(__file__).parent
for name,expected in env['source_sha256'].items():
    assert name in {'runner.py','run.py','Dockerfile'}
    archived = root/'sources'/name
    data=(archived if archived.is_file() else source/name).read_bytes();lf=data.replace(b'\r\n',b'\n')
    variants=[data,lf,lf.rstrip(b'\n')+b'\n',lf.rstrip(b'\n')+b'\n\n']
    variants += [v.replace(b'\r\n',b'\n').replace(b'\n',b'\r\n') for v in variants]
    variants += [v.rstrip(b'\r\n')+b'\r\n' for v in variants]
    assert expected in {hashlib.sha256(v).hexdigest() for v in variants},name
print(f"Validated {len(records)} cases, {len(records)*env['ops_per_case']} latencies, "
      f"{len(env['runs'])*len(stages)} content checks, cross-engine equality and source hashes.")
