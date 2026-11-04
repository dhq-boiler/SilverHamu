"""Check published summaries against raw samples and cross-engine content hashes."""
import gzip, hashlib, json, math, pathlib, sys
root=pathlib.Path(sys.argv[1]);records=json.loads((root/'results.json').read_text());env=json.loads((root/'environment.json').read_text())
assert len(records)==108 and len(env['runs'])==18
seen=set()
for r in records:
    identity=(r['engine'],r['rep'],r['workload'],r['clients']);assert identity not in seen;seen.add(identity)
    with gzip.open(root/r['latency_file'],'rt') as f:samples=sorted(json.load(f))
    assert len(samples)==r['completed']==r['attempted']==8000
    assert not r['errors'] and r['retries']==0
    assert all(isinstance(v,int) and v>0 for v in samples)
    for p in [50,95,99]:assert r[f'p{p}_ms']==samples[math.ceil(len(samples)*p/100)-1]/1e6
    assert r['max_ms']==samples[-1]/1e6
    assert abs(r['ops_s']-len(samples)/r['elapsed_s'])<1e-8
for rep in [1,2,3]:
    runs=[r for r in env['runs'] if r['rep']==rep];assert len(runs)==6
    assert all(r['status']=='passed' and len(r['verification'])==8 for r in runs)
    for stage in [v['stage'] for v in runs[0]['verification']]:
        checks=[next(v for v in r['verification'] if v['stage']==stage) for r in runs]
        assert len({v['sha256'] for v in checks})==1,(rep,stage)
        assert all(v['sha256']==v['expected_sha256'] and v['rows']==100000 and v['empty_and_last'] for v in checks)
for engine in {r['engine'] for r in env['runs']}:
    versions=[r['metadata'] for r in env['runs'] if r['engine']==engine]
    assert len(versions)==3 and all(v==versions[0] for v in versions),engine
source=pathlib.Path(__file__).parent
for name,expected in env['source_sha256'].items():
    data=(source/name).read_bytes();lf=data.replace(b'\r\n',b'\n')
    variants=[data,lf,lf.rstrip(b'\n')+b'\n',lf.rstrip(b'\n')+b'\n\n']
    variants += [v.replace(b'\r\n',b'\n').replace(b'\n',b'\r\n') for v in variants]
    variants += [v.rstrip(b'\r\n')+b'\r\n' for v in variants]
    assert expected in {hashlib.sha256(v).hexdigest() for v in variants},name
print('Validated: 108 cases, 864000 latencies, 144 content checks, cross-engine equality, source hashes.')
