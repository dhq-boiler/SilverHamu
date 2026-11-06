"""Storage-path results include all trials; a sync envelope is not physical I/O."""
import importlib.util,json,pathlib,statistics,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
ROOT=pathlib.Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('storage_timing',ROOT/'benchmarks/aged-tail/analyze.py');timing=importlib.util.module_from_spec(spec);spec.loader.exec_module(timing)
def summarize(root):
    schedule=json.loads((root/'raw/schedule.json').read_text());image=json.loads((root/'raw/image.json').read_text())['id'];rows=[];hashes=set();pilots=[]
    for e in schedule:
        f=root/'raw'/e['label'];r=json.loads((f/'results.json').read_text())[0]
        assert e['status']==r['status']=='passed' and r['storage_probe']['exclusive_lock_refused']
        assert len(r['verification'])==5 and all(v['passed'] for v in r['verification'])
        assert r['safety_probe']=={'old_generation_updates_checked':8,'staged_refusal':'resource_exhausted','prior_staged_value_preserved':True,'commit_after_rollback':True}
        assert json.loads((f/'environment.json').read_text())['image']==image
        for phase in ['boot','reopen']:
            assert r[phase]['limits']['memory.max']=='536870912' and r[phase]['limits']['memory.swap.max']=='0'
            q,period=map(int,r[phase]['limits']['cpu.max'].split());assert q==period
        seed=json.loads((f/f"aged-seed-{e['rows']}.json").read_text());assert seed['verification']['sha256']==seed['verification']['expected_sha256']
        if e['stage']=='pilot':pilots.append({'label':e['label'],'status':r['status'],'storage_probe':r['storage_probe']});continue
        hashes.add(seed['wal_sha256']);assert seed['updated_rows']==1000000
        row=timing.trial(f,{'label':e['label'],'stage':e['stage'],'rep':e['rep'],'variant':'diagnostic'});row['variant']=e['variant']
        row['storage_probe']=r['storage_probe'];row['mounts']=r['mounts'];row['reopen_ms']=r['reopen']['process_ready']['process_ready_ms']
        for m in r['measurements']:
            assert m['pin_reader'] and m['ops']==8000
            if m['work']=='mixed':assert m['old_snapshot_checked_keys']==m['by_kind']['write']['completed'];row['sync_attempts']=m['sync_attempts']
        rows.append(row)
    assert len(rows)==6 and len(pilots)==2 and len(hashes)==1
    assert all(sum(x['variant']==v for x in rows)==3 for v in ['volume','bind'])
    return {'image':image,'seed_wal_sha256':hashes.pop(),'pilots':pilots,'trials':rows,'requests':sum(x['requests'] for x in rows),'full_checks':sum(x['full_checks'] for x in rows)}
def main():
    root=pathlib.Path(sys.argv[1]);s=summarize(root);(root/'summary.json').write_text(json.dumps(s,indent=2));rows=s['trials']
    groups={v:[x for x in rows if x['variant']==v] for v in ['volume','bind']}
    def value(x,key):
        if key=='sync_p99':return x['phases_ms']['sync_envelope']['p99']
        if key=='write_p99':return x['mixed']['write']['response_ms']['p99']
        if key=='write_max':return x['mixed']['write']['response_ms']['max']
        if key=='read_p99':return x['mixed']['read']['response_ms']['p99']
        return x['resources'][key]
    med=lambda v,k:statistics.median(value(x,k) for x in groups[v])
    fig,axs=plt.subplots(1,3,figsize=(12,4),layout='constrained')
    for ax,key,title in zip(axs,['write_p99','write_max','sync_p99'],['Mixed write p99','Maximum write response','Per-request sync envelope p99']):
        bars=ax.bar(['Docker volume','C: shared folder'],[med(v,key) for v in groups],color=['#526777','#16817b']);ax.bar_label(bars,fmt='%.3f',label_type='center',color='white',fontweight='bold')
        for i,v in enumerate(groups):ax.scatter([i+.28,i+.32,i+.36],[value(x,key) for x in groups[v]],color='black',s=18,zorder=3)
        ax.set_title(title);ax.set_ylabel('ms');ax.set_ylim(0,max(value(x,key) for x in rows)*1.2)
    fig.suptitle('Same binary / aged 1M rows / 1 CPU / 512 MiB / 3 trials per storage path\nBars: trial-statistic median; dots: every trial. No durability/default change.',fontsize=11)
    fig.savefig(root/'latency.svg');fig.savefig(root/'latency.png',dpi=150);plt.close(fig)
    for p in root.glob('*.svg'):p.write_text('\n'.join(x.rstrip() for x in p.read_text().splitlines())+'\n')
    table=['| Trial | Write p99 ms | Write max ms | Sync envelope p99 ms | Mixed read p99 ms | Requests/s | Rejected | RSS MiB | CPU % | Throttled ms | Reopen ms | Syncs |','|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|']
    for x in rows:
        r=x['resources'];table.append('| '+x['label']+' | '+' | '.join(f'{value(x,k):.3f}' for k in ['write_p99','write_max','sync_p99','read_p99'])+f" | {x['ops_s']:.2f} | {sum(v['queue_refused'] for v in x['mixed'].values())} | {r['rss_peak_mib']:.2f} | {r['cpu_core_percent']:.2f} | {r['throttled_usec']/1000:.3f} | {x['reopen_ms']:.1f} | {x['sync_attempts']} |")
    table += ['', '| Trial | First-second write p99 ms | Remaining-seven-second write p99 ms | Worst request pre-call ms | Same worst request sync envelope ms |', '|---|---:|---:|---:|---:|']
    for x in rows:
        z=x['worst_write'];table.append(f"| {x['label']} | {x['first_second']['write']['response_ms']['p99']:.3f} | {x['remaining']['write']['response_ms']['p99']:.3f} | {z['before_call_ms']:.3f} | {z['phases_ms']['sync_envelope']:.3f} |")
    (root/'trials.md').write_text('\n'.join(table)+'\n')
    summary={v:{k:med(v,k) for k in ['write_p99','write_max','sync_p99','read_p99','rss_peak_mib']} for v in groups};print(json.dumps(summary,indent=2))
if __name__=='__main__':main()
