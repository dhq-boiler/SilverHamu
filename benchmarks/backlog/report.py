"""Compare bounded backlog draining with exact per-request and injection records."""
import gzip
import heapq
import json
import math
from pathlib import Path
import shutil
import statistics as st
import sys

def percentile(xs,p):return sorted(xs)[math.ceil(len(xs)*p)-1]

def validate(r,events,batches):
    assert len(events)==r['attempted'] and not r['errors']
    origin=r['schedule']['origin_ns'];limit=r['schedule']['batch_limit']
    groups={(b['worker'],b['id']):b for b in batches}
    assert len(groups)==len(batches)
    actual={}
    for i,e in enumerate(events):
        assert e['id']==i and e['worker']==i%r['clients'] and e['sequence']==i//r['clients']
        assert e['planned_ns']==origin+i*1_000_000_000//r['rate']
        assert e['planned_ns']<=e['admitted_ns']<=e['finished_ns']
        if e['status']=='rejected':
            assert 'sent_ns' not in e
            continue
        assert e['status']=='completed' and e['admitted_ns']<=e['sent_ns']<=e['finished_ns']
        group=(e['worker'],e['batch_id']);b=groups[group]
        assert (e['sent_ns'],e['finished_ns'])==(b['sent_ns'],b['finished_ns'])
        actual.setdefault(group,[]).append(e)
    assert len(actual)==len(batches)
    for group,items in actual.items():
        assert 1<=len(items)==groups[group]['size']<=limit
        assert len({e['key_index'] for e in items})==len(items)
    for w in range(r['clients']):
        ordered=sorted((b for b in batches if b['worker']==w),key=lambda b:b['id'])
        assert [b['id'] for b in ordered]==list(range(len(ordered)))
        assert all(a['finished_ns']<=b['sent_ns'] for a,b in zip(ordered,ordered[1:]))
        ids=[e['id'] for b in ordered for e in actual[(w,b['id'])]]
        assert ids==sorted(ids)
        slots=[0]*(r['schedule']['outstanding_capacity']//r['clients'])
        previous_grant=0
        for e in events:
            if e['worker']!=w or e['status']=='rejected':continue
            grant=max(heapq.heappop(slots),e['admitted_ns'],previous_grant)
            assert grant<=e['sent_ns']
            previous_grant=grant
            heapq.heappush(slots,e['finished_ns'])
    assert sum(e['status']=='completed' for e in events)==r['completed']
    assert sum(e['status']=='rejected' for e in events)==r['rejected']
    counters=r['schedule']['counters']
    assert int(counters['after']['version'])-int(counters['before']['version'])==r['completed']
    assert counters['after']['sync_attempts']-counters['before']['sync_attempts']==r['sync_attempts']
    assert 0<r['sync_attempts']<=r['completed']
    count=r['injected_stalls']
    timing=counters['after'].get('injected_stall_ns',[0,0,0])
    if r['scenario']=='normal':assert count==0 and timing==[0,0,0]
    else:
        assert count>0 and 0<timing[0]<=timing[1] and 0<timing[2]<=timing[1]-timing[0]
        assert timing[2]>=r['stall']['delay_ms']*count*1_000_000
        if r['scenario']=='single':assert count==1 and timing[1]-timing[0]==timing[2]
    return actual

def recovery(r,events,stable_ns=100_000_000,queue_ns=5_000_000):
    """First 100 ms offered window with no rejects and <=5 ms pre-send delay."""
    if r['scenario']=='normal':return None
    schedule=r['schedule'];last=schedule['counters']['after']['injected_stall_ns'][1]
    low=schedule['armed_client_ns']+last;high=schedule['armed_done_ns']+last
    window=[e for e in events if e['planned_ns']>=high]
    begin=0;found=None
    for i,e in enumerate(window):
        if e['status']!='completed' or e['sent_ns']-e['planned_ns']>queue_ns:
            begin=i+1;continue
        if e['planned_ns']-window[begin]['planned_ns']>=stable_ns:
            found=window[begin]['planned_ns'];break
    result=dict(last_injection_end_client_bounds_ns=[low,high],alignment_width_ms=(high-low)/1e6,
                stable_window_ms=stable_ns/1e6,pre_send_threshold_ms=queue_ns/1e6,
                recovered=found is not None)
    if found is not None:
        result.update(recovery_ms=(found-(low+high)/2)/1e6,recovery_bounds_ms=[(found-high)/1e6,(found-low)/1e6])
    else:result['observed_after_end_ms']=(events[-1]['planned_ns']-high)/1e6
    return result

def summarize(rows,trials):
    result=[]
    for scenario in ['normal','single','burst']:
        for c,rate in [(1,200),(1,250),(4,400),(4,800)]:
            for variant in dict.fromkeys(r['variant'] for r in rows):
                group=[t for t in trials if (t['scenario'],t['clients'],t['rate'],t['variant'])==(scenario,c,rate,variant)]
                if not group:continue
                assert len(group)==3
                s=dict(scenario=scenario,clients=c,rate=rate,variant=variant,attempted=sum(t['attempted'] for t in group),rejected=sum(t['rejected'] for t in group))
                for metric in ['ops_s','syncs_s','updates_per_sync','mean_batch_size','mean_send_rate']:
                    vals=[t[metric] for t in group];s[metric]=dict(median=st.median(vals),min=min(vals),max=max(vals))
                for kind in ['response','operation','queue']:
                    s[kind]={}
                    for metric in ['p99_ms','max_ms']:
                        vals=[t[kind][metric] for t in group];s[kind][metric]=dict(median=st.median(vals),min=min(vals),max=max(vals))
                rec=[t['recovery']['recovery_ms'] for t in group if t['recovery'] and t['recovery']['recovered']]
                s['recovery_ms']=dict(observed_median=st.median(rec) if rec else None,
                    observed_min=min(rec) if rec else None,observed_max=max(rec) if rec else None,
                    recovered_runs=len(rec),censored_runs=3-len(rec)) if scenario!='normal' else None
                result.append(s)
    return result

def main():
    source,out=map(Path,sys.argv[1:3]);out.mkdir(parents=True,exist_ok=True)
    rows=json.loads((source/'results.json').read_text());env=json.loads((source/'environment.json').read_text())
    assert len(rows) in (72,12) and len(env['runs'])==6
    assert all(x['status']=='passed' for x in env['runs'])
    assert sum(len(x['checks']) for x in env['runs'])==len(rows)+12
    for f in source.iterdir():
        if f.is_file():shutil.copy2(f,out/f.name)
    trials=[]
    for r in rows:
        events=json.load(gzip.open(source/r['events_file']));batches=json.load(gzip.open(source/r['batches_file']))
        validate(r,events,batches)
        t={k:r[k] for k in ['variant','scenario','clients','rate','rep','attempted','completed','rejected','ops_s','syncs_s','sync_attempts','updates_per_sync','injected_stalls','response','operation','queue']}
        t['recovery']=recovery(r,events)
        t['mean_batch_size']=r['completed']/len(batches)
        t['batch_size_counts']={str(n):sum(b['size']==n for b in batches) for n in range(1,9)}
        completed=[e for e in events if e['status']=='completed']
        sends=[e['sent_ns'] for e in completed]
        t['mean_send_rate']=(len(sends)-1)*1e9/(max(sends)-min(sends))
        t['producer_p99_ms']=percentile([e['admitted_ns']-e['planned_ns'] for e in events],.99)/1e6
        maximum=max(completed,key=lambda e:e['finished_ns']-e['planned_ns'])
        t['maximum_event']={**maximum,'producer_ms':(maximum['admitted_ns']-maximum['planned_ns'])/1e6,
                            'client_queue_ms':(maximum['sent_ns']-maximum['admitted_ns'])/1e6,
                            'operation_ms':(maximum['finished_ns']-maximum['sent_ns'])/1e6}
        if r['scenario']!='normal':
            low,high=t['recovery']['last_injection_end_client_bounds_ns']
            first=r['schedule']['armed_client_ns']+r['schedule']['counters']['after']['injected_stall_ns'][0]
            local=[e for e in completed if e['finished_ns']>=first and e['planned_ns']<=high+2_000_000_000]
            t['injection_neighborhood']=dict(window_start_ns=first,window_end_ns=high+2_000_000_000,
                completed=len(local),response_p99_ms=percentile([e['finished_ns']-e['planned_ns'] for e in local],.99)/1e6 if local else None,
                rejected=sum(e['status']=='rejected' and e['finished_ns']>=first and e['planned_ns']<=high+2_000_000_000 for e in events))
        trials.append(t)
    summary=summarize(rows,trials)
    (out/'summary.json').write_text(json.dumps(dict(summary=summary,trials=trials),indent=2)+'\n')
    table=['| Scenario | Clients | Offered/s | Mode | Updates/s | Response p99 ms | Worst response ms | Rejected / 24k | Updates/sync | Recovery ms |','|---|---:|---:|---|---:|---:|---:|---:|---:|---:|']
    for s in summary:
        rec=s['recovery_ms']
        rec=('—' if rec is None else f"{rec['observed_median']:.3f} (3/3)" if rec['recovered_runs']==3 else f"not reached in {rec['censored_runs']}/3")
        table.append(f"| {s['scenario']} | {s['clients']} | {s['rate']} | {s['variant']} | {s['ops_s']['median']:.1f} | {s['response']['p99_ms']['median']:.3f} | {s['response']['max_ms']['max']:.3f} | {s['rejected']} | {s['updates_per_sync']['median']:.2f} | {rec} |")
    (out/'table.md').write_text('\n'.join(table)+'\n',encoding='utf-8')
    if len(rows)==72:render(summary,trials,out)
    print('\n'.join(table))

def render(summary,trials,out):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    cases=[(1,200),(1,250),(4,400),(4,800)];colors=['#667085','#007f85']
    fig,axes=plt.subplots(3,3,figsize=(15,11),layout='constrained')
    for row,scenario in enumerate(['normal','single','burst']):
        for col,title in enumerate(['Response p99 (ms)','Per-run maximum (ms)','Rejected requests / 24,000']):
            ax=axes[row,col]
            for j,variant in enumerate(['before','candidate']):
                group=[next(s for s in summary if (s['scenario'],s['clients'],s['rate'],s['variant'])==(scenario,c,r,variant)) for c,r in cases]
                xs=[i+(j-.5)*.32 for i in range(4)]
                if col==2:ax.bar(xs,[s['rejected'] for s in group],.3,label=variant,color=colors[j])
                else:
                    key='p99_ms' if col==0 else 'max_ms';data=[s['response'][key] for s in group]
                    ax.bar(xs,[d['median'] for d in data],.3,label=variant,color=colors[j],yerr=[[d['median']-d['min'] for d in data],[d['max']-d['median'] for d in data]],capsize=3)
            ax.set_xticks(range(4),[f'{c} conn.\n{r}/s' for c,r in cases]);ax.set_title(scenario+' — '+title);ax.spines[['top','right']].set_visible(False)
    axes[0,0].legend()
    fig.suptitle('Drain ready independent updates together; do not wait for new arrivals')
    fig.supxlabel('100k rows / 256B / 8,000 offered updates. Latency: median and range of 3 runs; rejects: all 3 runs.\nSame 64 outstanding credits and real durability sync. Single: 100 ms once; burst: 20 ms before each sync for 1 second.',fontsize=10)
    for ext in ['png','svg']:fig.savefig(out/f'backlog-comparison.{ext}',dpi=140)
    plt.close(fig)
    fig,axes=plt.subplots(1,2,figsize=(12,5),layout='constrained')
    for ax,scenario in zip(axes,['single','burst']):
        for j,variant in enumerate(['before','candidate']):
            ax.scatter([],[],color=colors[j],label=variant)
            for i,(c,rate) in enumerate(cases):
                selected=[t for t in trials if (t['scenario'],t['clients'],t['rate'],t['variant'])==(scenario,c,rate,variant)]
                for t in selected:
                    d=t['recovery'];x=i+(j-.5)*.32+(t['rep']-2)*.045
                    if d['recovered']:ax.scatter(x,max(.001,d['recovery_ms']),color=colors[j],s=28)
                    else:ax.scatter(x,max(.001,d['observed_after_end_ms']),marker='^',facecolors='none',edgecolors=colors[j],s=60)
        ax.scatter([],[],marker='^',facecolors='none',edgecolors='#444444',label='Not recovered: lower bound')
        ax.set_xlim(-.5,3.5);ax.set_yscale('log');ax.set_xticks(range(4),[f'{c} conn.\n{r}/s' for c,r in cases]);ax.set_title(scenario);ax.set_ylabel('Milliseconds after final injected sleep (log scale)');ax.spines[['top','right']].set_visible(False)
    axes[0].legend(loc='upper left', bbox_to_anchor=(0, -.18), ncol=3, fontsize=8)
    fig.suptitle('Recovery to <=5 ms pre-send wait, sustained for 100 ms without rejection')
    fig.supxlabel('All 3 runs: dots show observed recovery; triangles show right-censoring bounds, never zero.\nRecovery is a defined threshold, not zero backlog or device health; alignment uses the bounded arm-call interval.',fontsize=9)
    for ext in ['png','svg']:fig.savefig(out/f'recovery.{ext}',dpi=140)
    plt.close(fig)
    for svg in out.glob('*.svg'):svg.write_text('\n'.join(line.rstrip() for line in svg.read_text().splitlines())+'\n')

if __name__=='__main__':main()
