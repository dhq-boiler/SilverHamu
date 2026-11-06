import argparse,subprocess,sys,pathlib,json,datetime
r=pathlib.Path(__file__).resolve().parents[2]
p=argparse.ArgumentParser();p.add_argument('--out',type=pathlib.Path,required=True);a=p.parse_args();out=a.out.resolve();out.mkdir(parents=True,exist_ok=False)
def run(args):
 print('RUN',*args,flush=True);subprocess.run(args,cwd=r,check=True)
for kind,dockerfile in [('','benchmarks/resource-envelope/history-capacity.Dockerfile'),('-service','benchmarks/Dockerfile')]:
 for variant in ['before','after']:
  args=['docker','build','-f',dockerfile,'-t',f'catdb-inline{kind}-{variant}:local']
  if variant=='after':args+=['--build-arg','EXTRA_FEATURES=,inline-latest']
  run(args+['.'])
run(['docker','run','--rm','-e','CATDB_SERVER=/usr/local/bin/catdb-server','-v',str(r)+':/source:ro','--tmpfs','/source/artifacts:rw,mode=1777','catdb-inline-service-after:local','python','/source/tests/service_protocol.py'])
run(['docker','run','--rm','-e','NEWDBMS_TEST_DLL=/usr/local/lib/libnewdbms.so','-v',str(r)+':/source:ro','--tmpfs','/source/artifacts:rw,mode=1777','-w','/source','catdb-inline-service-after:local','python','-m','unittest','discover','-s','tests','-p','*ffi_checks.py'])
schedule=[]
for kind in ['memory','service']:
 for rep in range(1,4):
  for variant in (['before','after'] if rep%2 else ['after','before']):
   dest=out/f'{kind}-{variant}-{rep}';start=datetime.datetime.now(datetime.timezone.utc).isoformat()
   if kind=='memory':
    args=[sys.executable,'-X','utf8','benchmarks/resource-envelope/history_capacity_run.py','--out',str(dest),'--rows','1000000','--reps','1','--observer','counted','--check512','--image',f'catdb-inline-{variant}:local']
   else:
    args=[sys.executable,'-X','utf8','benchmarks/resource-envelope/run.py','--out',str(dest),'--rows','1000000','--cpus','1','--memory','1024','--reps','1','--ops','8000','--retained-bytes','671088640','--image',f'catdb-inline-service-{variant}:local']
   run(args);schedule.append({'kind':kind,'variant':variant,'round':rep,'start':start,'end':datetime.datetime.now(datetime.timezone.utc).isoformat()});(out/'schedule.json').write_text(json.dumps(schedule,indent=2))
