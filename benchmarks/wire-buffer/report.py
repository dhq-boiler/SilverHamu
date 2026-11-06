"""All qualification trials, with a pinned old reader and no diagnostic feature."""
import argparse,gzip,importlib.util,json,pathlib,statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
ROOT=pathlib.Path(__file__).resolve().parents[2]
spec=importlib.util.spec_from_file_location('aged_analysis',ROOT/'benchmarks/aged-service/analyze.py')
aged=importlib.util.module_from_spec(spec);spec.loader.exec_module(aged)

def main():
    p=argparse.ArgumentParser();p.add_argument('root',type=pathlib.Path);a=p.parse_args();root=a.root
    schedule=json.loads((root/'raw/schedule.json').read_text())
    assert len(schedule)==6
    for row in schedule:
        row['memory_mib']=512
        folder=root/'raw'/row['label'];result=json.loads((folder/'results.json').read_text())[0]
        row['oom_kills']=max(json.loads(line)['events']['oom_kill'] for line in aged.data(folder/result['name']/'boot-resources.jsonl').splitlines())
    (root/'raw/schedule.json').write_text(json.dumps(schedule,indent=2))
    s=aged.summarize(root/'raw',pin_reader=True)
    assert s['requests']==96000 and s['full_checks']==30 and s['seed_full_checks']==6
    groups={v:[x for x in s['trials'] if x['variant']==v] for v in ['baseline','buffered']}
    assert all(len(x)==3 for x in groups.values())
    def metric(x,n):
        w=x['workloads']['mixed']
        if n in ('read','write'):return w['by_kind'][n]['response_ms']['p99']
        if n=='max':return max(v['response_ms']['max'] for v in w['by_kind'].values())
        return w['resources'][n]
    table=['| Trial | Mixed read p99 ms | Write p99 ms | Mixed max ms | Requests/s | Queue refusals | RSS MiB | Cgroup MiB | CPU % | Reopen ms |','|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|']
    for x in s['trials']:
        w=x['workloads']['mixed']
        table.append('| '+x['label']+' | '+' | '.join(f'{v:.3f}' for v in [metric(x,'read'),metric(x,'write'),metric(x,'max'),w['ops_s'],w['queue_rejected'],metric(x,'rss_peak_mib'),metric(x,'cgroup_peak_mib'),metric(x,'cpu_core_percent'),x['reopen_ready_ms']])+' |')
    (root/'summary.json').write_text(json.dumps(s,indent=2))
    (root/'table.md').write_text('\n'.join(table)+'\n')
    fig,axes=plt.subplots(1,5,figsize=(16,4),layout='constrained')
    for ax,n,title,unit in zip(axes,['read','write','max','rss_peak_mib','cpu_core_percent'],['Mixed read p99','Mixed write p99','Mixed maximum','Server RSS','CPU utilization'],['ms','ms','ms','MiB','% of one CPU']):
        values=[[metric(x,n) for x in groups[v]] for v in groups]
        bars=ax.bar(['Original wire','Buffered wire'],[statistics.median(x) for x in values],color=['#546e7a','#197d83'])
        for i,arr in enumerate(values):ax.scatter([i-.08,i,i+.08],arr,color='black',s=18,zorder=3)
        ax.bar_label(bars,fmt='%.2f',padding=3);ax.set_ylim(0,max(map(max,values))*1.25)
        ax.set_title(title);ax.set_ylabel(unit);ax.tick_params(axis='x',labelsize=8)
    fig.suptitle('Wire buffering comparison: aged 1M rows / 1 CPU / 512 MiB / old reader retained\nBars: median; dots: all three trials. Same 1,000 requests/s offered load.',fontsize=11)
    fig.savefig(root/'comparison.svg');fig.savefig(root/'comparison.png',dpi=150);plt.close(fig)
    path=root/'comparison.svg';path.write_text('\n'.join(line.rstrip() for line in path.read_text().splitlines())+'\n')
    print('\n'.join(table));print('Requests',s['requests'],'full checks',s['full_checks'])

if __name__=='__main__':main()
