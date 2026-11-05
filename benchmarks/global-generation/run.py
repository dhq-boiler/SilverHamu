import os,json,pathlib,subprocess,sys
binaries=json.loads(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8'))
root=pathlib.Path(sys.argv[2]).resolve();root.mkdir(exist_ok=False)
for repeat in range(1,4):
    for size in [100000,1000000]:
        for shape in ['local','spread']:
            for mode in (['before','after'] if repeat%2 else ['after','before']):
                output=root/f'{size}-{shape}-{repeat}-{mode}'
                env=os.environ.copy();env.update(CATDB_GUARD_OUTPUT=str(output),CATDB_GUARD_ROWS=str(size),CATDB_GUARD_SHAPE=shape)
                run=subprocess.run([binaries[mode],'--ignored','--exact','transactions::generation_guard_bench::production_global_guard_benchmark','--nocapture','--test-threads=1'],env=env,text=True,capture_output=True)
                root.joinpath(output.name+'.log').write_text(run.stdout+'\n'+run.stderr,encoding='utf-8')
                if run.returncode:print(run.stdout,run.stderr);raise SystemExit(run.returncode)
                print(output.name+': verified',flush=True)
