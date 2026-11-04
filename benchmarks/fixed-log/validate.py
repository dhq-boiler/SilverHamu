"""Validate the paired samples plus fixed-log accounting and reopen probes."""
import json,pathlib,subprocess,sys
root=pathlib.Path(sys.argv[1]);repo=pathlib.Path(__file__).resolve().parents[2]
subprocess.run([sys.executable,str(repo/'benchmarks/service-update/validate.py'),str(root)],check=True)
env=json.loads((root/'environment.json').read_text(encoding='utf-8'));rows=json.loads((root/'results.json').read_text(encoding='utf-8'))
for row in rows:
    delta=row['io_delta'];fixed=row['variant'] in env['fixed_variants']
    assert delta['logical_bytes']==row['completed']*317
    assert row['application_written_bytes']==delta['logical_bytes']+84*delta['end_generation']
    if fixed:
        assert row['io_before']['physical_bytes']==row['io_after']['physical_bytes']==env['capacity']+8192
        assert (row['completed']+31)//32<=delta['end_generation']<=row['completed']
    else:assert delta['end_generation']==0 and delta['physical_bytes']==delta['logical_bytes']
    if 'block_write_bytes' in delta:
        assert delta['block_write_bytes']>0 and delta['block_write_requests']>0
    if 'fixed_sync_metrics' in row:
        groups,data,end=row['fixed_sync_metrics'];assert groups==delta['end_generation'] and data>0 and end>0
for run in env['runs']:
    last=[r for r in rows if r['variant']==run['variant'] and r['rep']==run['rep']][-1]
    probe=run['reopen_probe'];assert probe['rows']==100000 and probe['open_ns']>0 and probe['wal_bytes']==last['io_after']['logical_bytes']
print('Fixed physical length, application byte accounting, two-phase sync counts and timed reopen verified.')
