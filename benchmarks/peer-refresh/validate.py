"""Verify refreshed product data and the supplementary legacy API run."""
import gzip,json,math,pathlib,subprocess,sys
root=pathlib.Path(sys.argv[1]);main=root/'main';legacy=root/'legacy'
subprocess.run([sys.executable,str(pathlib.Path(__file__).parents[1]/'validate.py'),str(main)],check=True)
env=json.loads((legacy/'environment.json').read_text());rows=json.loads((legacy/'results.json').read_text())
assert env['reps']==3 and env['rows']==100000 and not env['single_request']
assert len(rows)==18 and len(env['runs'])==3
current=json.loads((main/'environment.json').read_text());assert current['single_request']
assert env['images']['catdb-service']==current['images']['catdb-service']
assert env['source_sha256']==current['source_sha256']
seen=set()
for row in rows:
    identity=(row['rep'],row['workload'],row['clients']);assert identity not in seen;seen.add(identity)
    assert row['engine']=='catdb-service' and row['completed']==row['attempted']==8000
    assert not row['errors'] and row['retries']==0
    with gzip.open(legacy/row['latency_file'],'rt') as f:values=sorted(json.load(f))
    assert len(values)==8000 and all(isinstance(v,int) and v>0 for v in values)
    for p in [50,95,99]:assert row[f'p{p}_ms']==values[math.ceil(len(values)*p/100)-1]/1e6
    assert row['max_ms']==values[-1]/1e6
    assert abs(row['ops_s']-8000/row['elapsed_s'])<1e-8
for run in env['runs']:
    assert run['status']=='passed' and len(run['verification'])==8
    peer=next(r for r in current['runs'] if r['engine']=='catdb-service' and r['rep']==run['rep'])
    for check in run['verification']:
        match=next(v for v in peer['verification'] if v['stage']==check['stage'])
        assert check['sha256']==check['expected_sha256']==match['sha256']
        assert check['rows']==100000 and check['empty_and_last']
print('Supplement: 18 cases, 144000 timings, 24 full checks; same binary and cross-API contents.')
