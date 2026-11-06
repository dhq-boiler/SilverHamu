"""Keep setup/counter calls outside the measured workload; use the original operations."""
import argparse,importlib.util,json,pathlib,runpy,sys,time
source=pathlib.Path(__file__).parents[1]/'resource-envelope/client.py'
if len(sys.argv)<2 or sys.argv[1]!='measure':
    runpy.run_path(str(source),run_name='__main__');raise SystemExit
spec=importlib.util.spec_from_file_location('resource_client',source);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
p=argparse.ArgumentParser();p.add_argument('action');p.add_argument('--work');p.add_argument('--ops',type=int);p.add_argument('--rate',type=float);p.add_argument('--pin-reader',type=int);a=p.parse_args()
control=pathlib.Path('/host-results')/(a.work+'-kernel');control.mkdir(exist_ok=True);state={}
def counter():
    with m.connect() as c:return int(m.require(c.request(op='benchmark_counters'))['sync_attempts'])
def arm():
    state['sync_before']=counter();(control/'armed').write_text('ready')
    deadline=time.monotonic()+45
    while not (control/'go').exists():
        if time.monotonic()>deadline:raise TimeoutError('OS collector gate expired')
        time.sleep(.01)
r=m.measure(a.work,a.ops,a.rate,64,bool(a.pin_reader),before_start=arm)
r['sync_before']=state['sync_before'];r['sync_after']=counter();r['sync_attempts']=r['sync_after']-r['sync_before']
(m.DATA/(a.work+'-raw.json')).write_text(json.dumps(r));r.pop('records');print(json.dumps(r))
