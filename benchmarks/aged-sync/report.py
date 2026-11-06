"""Summarize same-image trials without attributing blocked time to physical I/O."""
import gzip,importlib.util,json,pathlib,statistics,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
ROOT=pathlib.Path(__file__).resolve().parents[2]
def module(name,path):
    s=importlib.util.spec_from_file_location(name,ROOT/path);m=importlib.util.module_from_spec(s);s.loader.exec_module(m);return m
aged=module('aged_metrics_report','benchmarks/aged-service/analyze.py')
timing=module('aged_timing_report','benchmarks/aged-tail/analyze.py')
from analyze import check

def summarize(root):
    schedule=json.loads((root/'raw/schedule.json').read_text());out=[];hashes=set()
    image=json.loads((root/'raw/image.json').read_text())['id']
    for info in schedule:
        f=root/'raw'/info['label'];r=json.loads((f/'results.json').read_text())[0];case=f/r['name']
        assert r['status']=='passed' and len(r['verification'])==5 and all(v['passed'] for v in r['verification'])
        assert r['safety_probe']=={'old_generation_updates_checked':8,'staged_refusal':'resource_exhausted','prior_staged_value_preserved':True,'commit_after_rollback':True}
        assert json.loads((f/'environment.json').read_text())['image']==image
        seed=json.loads((f/'aged-seed-1000000.json').read_text());hashes.add(seed['wal_sha256'])
        assert seed['verification']['sha256']==seed['verification']['expected_sha256']
        row={k:info[k] for k in ['label','stage','variant','rep']};row['workloads']={}
        samples=[json.loads(x) for x in aged.data(case/'boot-resources.jsonl').splitlines()]
        for work in ['read','mixed']:
            m=json.loads(aged.data(case/(work+'-raw.json')))
            assert m['pin_reader'] and not m['errors'] and len(m['records'])==m['ops']
            w={'requests':timing.requests(m['records']),'ops':m['ops'],'ops_s':m['ops_s'],'rejected':m['queue_rejected'],'resources':aged.measured(samples,m['start_ns'],m['finish_ns']),'sync_attempts':m['sync_attempts']}
            if work=='mixed':
                writes=[x for x in m['records'] if x['kind']=='write' and x['status']=='ok'];assert len(writes)==m['old_snapshot_checked_keys']
                parts=[timing.phases(x) for x in writes];w['phases_ms']={k:aged.stats([p[k] for p in parts]) for k in parts[0]}
                worst=max(writes,key=lambda x:x['ended']-x['planned']);w['worst_write']={'seq':worst['seq'],'response_ms':(worst['ended']-worst['planned'])/1e6,'before_call_ms':(worst['began']-worst['planned'])/1e6,'phases_ms':timing.phases(worst)}
            row['workloads'][work]=w
        row['reopen_ms']=r['reopen']['process_ready']['process_ready_ms']
        if info['variant']=='trace':
            kernel=check(f)['mixed'];rows=kernel['syncs'];row['kernel']={'count':len(rows),'capture_bytes':kernel['capture_bytes'],'kernel_buffer_bytes':kernel['kernel_buffer_bytes'],'ms':{k:aged.stats([x[k]/1e6 for x in rows]) for k in ['duration_ns','scheduled_ns','runnable_ns','blocked_ns']},'slowest_sync':max(rows,key=lambda x:x['duration_ns']),'total_fraction':{k:sum(x[k] for x in rows)/sum(x['duration_ns'] for x in rows) for k in ['scheduled_ns','runnable_ns','blocked_ns']}}
        out.append(row)
    assert len(hashes)==1
    main=[x for x in out if x['stage']=='main'];assert len(main)==6 and all(sum(x['variant']==v for x in main)==3 for v in ['off','trace'])
    return {'image':image,'seed_wal_sha256':hashes.pop(),'trials':out,'main_requests':sum(w['ops'] for x in main for w in x['workloads'].values()),'main_full_checks':len(main)*5}

def main():
    root=pathlib.Path(sys.argv[1]);s=summarize(root);(root/'summary.json').write_text(json.dumps(s,indent=2))
    rows=[x for x in s['trials'] if x['stage']=='main'];groups={v:[x for x in rows if x['variant']==v] for v in ['off','trace']}
    def val(x,key):
        w=x['workloads']['mixed']
        if key=='write_p99':return w['requests']['write']['response_ms']['p99']
        if key=='write_max':return w['requests']['write']['response_ms']['max']
        if key=='read_p99':return w['requests']['read']['response_ms']['p99']
        return w['resources'][key]
    med=lambda v,k:statistics.median(val(x,k) for x in groups[v])
    fig,axs=plt.subplots(1,3,figsize=(12,4),layout='constrained')
    for ax,key,title in zip(axs,['write_p99','write_max','read_p99'],['Mixed write p99','Maximum write response','Mixed read p99']):
        values=[med(v,key) for v in groups];bars=ax.bar(['Observer off','Kernel trace'],values,color=['#526777','#16817b']);ax.bar_label(bars,fmt='%.3f',label_type='center',color='white',fontweight='bold')
        for i,v in enumerate(groups):ax.scatter([i+.28,i+.32,i+.36],[val(x,key) for x in groups[v]],c='black',s=20,zorder=3)
        ax.set_title(title);ax.set_ylabel('ms from planned arrival');ax.set_ylim(0,max(val(x,key) for x in rows)*1.23)
    fig.suptitle('Same diagnostic binary / aged 1M rows / 1 CPU / 512 MiB\nBars: median of 3 trials; dots: every trial. No engine change.',fontsize=11)
    fig.savefig(root/'latency.svg');fig.savefig(root/'latency.png',dpi=150);plt.close(fig)
    fig,ax=plt.subplots(figsize=(8,4),layout='constrained');bottom=[0.,0.,0.]
    for field,label,color in [('blocked_ns','Blocked (not physical disk I/O)','#526777'),('runnable_ns','Runnable, awaiting CPU','#d57d25'),('scheduled_ns','Scheduled on guest CPU','#16817b')]:
        values=[x['kernel']['slowest_sync'][field]/1e6 for x in groups['trace']];ax.bar(['Trace 1','Trace 2','Trace 3'],values,bottom=bottom,label=label,color=color);bottom=[a+b for a,b in zip(bottom,values)]
    for i,v in enumerate(bottom):ax.text(i,v+.03,f'{v:.3f}',ha='center')
    ax.set_ylabel('ms');ax.set_title('Components of the same slowest log-sync call in each trial');ax.set_ylim(0,max(bottom)*1.45);ax.legend(loc='upper left',fontsize=8)
    fig.savefig(root/'sync.svg');fig.savefig(root/'sync.png',dpi=150);plt.close(fig)
    table=['| Trial | Write p99 ms | Write max ms | Mixed read p99 ms | Requests/s | Rejected | RSS MiB | CPU % | Throttled ms | Reopen ms |','|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|']
    for x in s['trials']:
        w=x['workloads']['mixed'];r=w['resources'];table.append(f"| {x['label']} | {val(x,'write_p99'):.3f} | {val(x,'write_max'):.3f} | {val(x,'read_p99'):.3f} | {w['ops_s']:.2f} | {w['rejected']} | {r['rss_peak_mib']:.2f} | {r['cpu_core_percent']:.2f} | {r['throttled_usec']/1000:.3f} | {x['reopen_ms']:.1f} |")
    kt=['| Trial | Sync count | Sync p99 ms | Slowest ms | Scheduled ms | Runnable ms | Blocked ms | Total blocked fraction |','|---|---:|---:|---:|---:|---:|---:|---:|']
    for x in groups['trace']:
        k=x['kernel'];z=k['slowest_sync'];kt.append(f"| {x['label']} | {k['count']} | {k['ms']['duration_ns']['p99']:.3f} | "+' | '.join(f'{z[f]/1e6:.3f}' for f in ['duration_ns','scheduled_ns','runnable_ns','blocked_ns'])+f" | {k['total_fraction']['blocked_ns']*100:.2f}% |")
    for path in root.glob('*.svg'):
        path.write_text('\n'.join(line.rstrip() for line in path.read_text().splitlines())+'\n')
    (root/'trials.md').write_text('\n'.join(table)+'\n\n'+'\n'.join(kt)+'\n')
    print(json.dumps({'medians':{v:{k:med(v,k) for k in ['write_p99','write_max','read_p99','rss_peak_mib']} for v in groups},'kernel':[x['kernel'] for x in groups['trace']]},indent=2))
if __name__=='__main__':main()
