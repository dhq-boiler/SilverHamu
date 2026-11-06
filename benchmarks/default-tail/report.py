"""Every matched-build trial and identical-binary control, including adverse results."""
import argparse,json,pathlib,statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from analyze import summarize

def main():
    p=argparse.ArgumentParser();p.add_argument('root',type=pathlib.Path);a=p.parse_args();r=a.root;s=summarize(r)
    variants=['explicit','default','same-a','same-b'];groups={v:[x for x in s['trials'] if x['variant']==v] for v in variants}
    assert [len(groups[v]) for v in variants]==[3,3,2,2]
    def val(x,n):
        m=x['workloads']['mixed']
        if n in ('read','write'):return m['by_kind'][n]['response_ms']['p99']
        if n=='max':return max(k['response_ms']['max'] for k in m['by_kind'].values())
        return m['resources'][n]
    s['medians']={v:{n:statistics.median(val(x,n) for x in groups[v]) for n in ['read','write','max','rss_peak_mib']} for v in variants}
    s['paired_write_p99_delta_percent']=[(val(groups['default'][i],'write')/val(groups['explicit'][i],'write')-1)*100 for i in range(3)]
    s['same_binary_write_p99_range']=[min(val(x,'write') for v in ['same-a','same-b'] for x in groups[v]),max(val(x,'write') for v in ['same-a','same-b'] for x in groups[v])]
    table=['| Trial | Mixed read p99 ms | Write p99 ms | Mixed max ms | Writes >=10/20/50 ms | Requests/s | Queue refusals | RSS MiB | Cgroup MiB | CPU throttled ms | Reopen ms |','|---|---:|---:|---:|---|---:|---:|---:|---:|---:|---:|']
    for x in s['trials']:
        m=x['workloads']['mixed'];t=x['tails']['mixed']['write'];counts='/'.join(str(t['at_least_ms'][str(k)]) for k in [10,20,50])
        table.append(f"| {x['label']} | {val(x,'read'):.3f} | {val(x,'write'):.3f} | {val(x,'max'):.3f} | {counts} | {m['ops_s']:.3f} | {m['queue_rejected']} | {val(x,'rss_peak_mib'):.3f} | {val(x,'cgroup_peak_mib'):.3f} | {val(x,'throttled_usec')/1000:.3f} | {x['reopen_ready_ms']:.3f} |")
    (r/'summary.json').write_text(json.dumps(s,indent=2));(r/'table.md').write_text('\n'.join(table)+'\n')
    windows=['| Trial | First-second write p99 ms (200 requests) | Later write p99 ms (1,400 requests) | Before-call p99 ms | Before-call max ms | Call p99 ms | Call max ms |','|---|---:|---:|---:|---:|---:|---:|']
    for x in s['trials']:
        t=x['tails']['mixed']['write']
        windows.append('| '+x['label']+' | '+' | '.join(f'{v:.3f}' for v in [t['first_second_ms']['p99'],t['later_ms']['p99'],t['before_call_ms']['p99'],t['before_call_ms']['max'],t['call_ms']['p99'],t['call_ms']['max']])+' |')
    (r/'timing-windows.md').write_text('\n'.join(windows)+'\n')
    fig,axes=plt.subplots(2,2,figsize=(12,8),layout='constrained')
    labels=['Explicit\n3 trials','Default\n3 trials','Same binary A\n2 trials','Same binary B\n2 trials']
    for ax,n,title,unit in zip(axes.flat,['write','read','max','rss_peak_mib'],['Mixed write p99','Mixed read p99','Mixed maximum response','Server RSS'],['ms','ms','ms','MiB']):
        values=[[val(x,n) for x in groups[v]] for v in variants]
        bars=ax.bar(labels,[statistics.median(x) for x in values],color=['#546e7a','#197d83','#a86a26','#a86a26'])
        for i,arr in enumerate(values):ax.scatter([i+(j-(len(arr)-1)/2)*.15 for j in range(len(arr))],arr,color='black',s=20,zorder=3)
        top=max(map(max,values))
        for i,arr in enumerate(values):ax.text(i,max(arr)+top*.045,f'{statistics.median(arr):.2f}',ha='center',va='bottom')
        ax.set_ylim(0,top*1.25);ax.set_title(title);ax.set_ylabel(unit);ax.tick_params(axis='x',labelsize=9)
    fig.suptitle('Aged 1M rows / 1 CPU / 512 MiB / retained old reader\nBars: median; dots: every trial. Same A/B use the identical default image.',fontsize=13)
    fig.savefig(r/'comparison.svg');fig.savefig(r/'comparison.png',dpi=150);plt.close(fig)
    path=r/'comparison.svg';path.write_text('\n'.join(x.rstrip() for x in path.read_text().splitlines())+'\n')
    print('\n'.join(table));print(json.dumps({'medians':s['medians'],'paired_delta_percent':s['paired_write_p99_delta_percent'],'same_binary_range':s['same_binary_write_p99_range']},indent=2))
if __name__=='__main__':main()
