"""Validate raw percentiles, resource drain and per-read data-lock acquisition counts."""
import json,pathlib,subprocess,sys
root=pathlib.Path(sys.argv[1])
for folder in ('native','atomic'):
 subprocess.run([sys.executable,'benchmarks/reclamation-cost/report.py',str(root/folder)],check=True)
 rows=json.loads((root/folder/'results.json').read_text(encoding='utf-8'))
 for row in rows:
  expected=3 if row['variant'].startswith('before') else 2
  for thread in row['readers']:
   assert thread['hold'][0]==(expected*thread['latency']['count'] if row['lock_timing'] else 0)
 print(folder+': every profiled reader uses the expected number of data-state locks')
subprocess.run([sys.executable,'benchmarks/service-update/validate.py',str(root/'service')],check=True)
