"""Render only complete, verified repetitions; retain every measured product."""
import argparse, csv, datetime, json, pathlib, statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
plt.rcParams['svg.hashsalt']='catdb-api-comparison-v1'
NAMES={'catdb-embedded':'catdb (C ABI)','sqlite':'SQLite','catdb-service':'catdb (TCP)','postgres':'PostgreSQL','mysql':'MySQL','oracle':'Oracle Free'}
COLORS={'catdb-embedded':'#0072B2','sqlite':'#757575','catdb-service':'#0072B2','postgres':'#009E73','mysql':'#E69F00','oracle':'#CC79A7'}
WORK={'get':'Key lookup','range100':'Read 100 consecutive records','update':'Replace one value + durable commit'}
def main():
    p=argparse.ArgumentParser();p.add_argument('directory',type=pathlib.Path);root=p.parse_args().directory
    records=json.loads((root/'results.json').read_text());env=json.loads((root/'environment.json').read_text())
    assert env['reps']==3 and len(env['runs'])==18
    assert all(r['status']=='passed' and len(r['verification'])==8 for r in env['runs'])
    assert len(records)==108 and all(not r['errors'] and r['completed']==r['attempted'] for r in records)
    measured_date=datetime.datetime.fromisoformat(env['utc']).astimezone(datetime.timezone(datetime.timedelta(hours=9))).date().isoformat()
    aggregate=[]
    for mode,engines in [('embedded',['catdb-embedded','sqlite']),('service',['catdb-service','postgres','mysql','oracle'])]:
        fig,axes=plt.subplots(3,2,figsize=(13,10 if mode=='embedded' else 14),layout='constrained')
        fig.suptitle(f'{mode.capitalize()} API comparison - 100,000 records, 256-byte values',fontsize=17,fontweight='bold')
        for row,work in enumerate(WORK):
            labels=[];ys=[];medians={m:[] for m in ['ops_s','p99_ms']};ranges={m:[[],[]] for m in medians};colors=[]
            for e in engines:
                for clients in [1,4]:
                    subset=[r for r in records if r['engine']==e and r['workload']==work and r['clients']==clients]
                    assert len(subset)==3
                    labels.append(f'{NAMES[e]} - {clients} client'+('s' if clients==4 else ''));ys.append(len(ys));colors.append(COLORS[e])
                    a={'engine':e,'workload':work,'clients':clients}
                    for metric in ['ops_s','p50_ms','p95_ms','p99_ms','max_ms']:
                        values=[r[metric] for r in subset];median=statistics.median(values)
                        a[metric]=median;a[metric+'_min']=min(values);a[metric+'_max']=max(values)
                        if metric in medians:medians[metric].append(median);ranges[metric][0].append(median-min(values));ranges[metric][1].append(max(values)-median)
                    aggregate.append(a)
            for col,metric in enumerate(['ops_s','p99_ms']):
                ax=axes[row,col];bars=ax.barh(ys,medians[metric],xerr=ranges[metric],color=colors,capsize=3,error_kw={'elinewidth':1},height=.68)
                for i,b in enumerate(bars):
                    if i%2:b.set_hatch('//');b.set_edgecolor('white')
                ax.set_yticks(ys,labels,fontsize=9);ax.invert_yaxis();ax.set_axisbelow(True);ax.grid(axis='x',alpha=.2)
                ax.spines[['top','right']].set_visible(False)
                ax.set_title(WORK[work],loc='left',fontsize=11,fontweight='bold')
                ax.set_xlabel('Operations / second - higher is better' if metric=='ops_s' else 'p99 response time (ms) - lower is better',fontsize=10)
                maximum=max(v+h for v,h in zip(medians[metric],ranges[metric][1]));ax.set_xlim(0,maximum*1.25)
                for y,v,h in zip(ys,medians[metric],ranges[metric][1]):
                    label=f'{v:,.0f}' if metric=='ops_s' else f'{v:.3f}'
                    ax.text(v+h+maximum*.015,y,label,va='center',fontsize=9)
        versions=[]
        for e in engines:
            m=next(r['metadata'] for r in env['runs'] if r['engine']==e)
            if e.startswith('catdb'):v=m['version']
            elif e=='sqlite':v=m['select sqlite_version()'][0][0]+' (bundled)'
            elif e=='postgres':v=m['select version()'][0][0].split()[1]
            elif e=='mysql':v=m['select version()'][0][0]
            else:v=m['select banner_full from v$version'][0][0].split('Version ')[-1]
            versions.append(NAMES[e]+' '+v)
        fig.supxlabel('Median of 3 runs; whiskers = min/max. Python client overhead included.\nExperimental catdb; warm-cache microbenchmarks, not a general DBMS ranking. '+measured_date+'\n'+' | '.join(versions),fontsize=9)
        fig.savefig(root/f'{mode}.svg',metadata={'Date':env['utc']});fig.savefig(root/f'{mode}.png',dpi=140);plt.close(fig)
        svg=root/f'{mode}.svg'
        svg.write_text('\n'.join(line.rstrip() for line in svg.read_text(encoding='utf-8').splitlines())+'\n',encoding='utf-8')
    with (root/'summary.csv').open('w',newline='',encoding='utf-8') as f:
        writer=csv.DictWriter(f,fieldnames=list(aggregate[0]));writer.writeheader();writer.writerows(aggregate)
    table=['| API | Clients | Workload | ops/s | p50 ms | p95 ms | p99 ms |','|---|---:|---|---:|---:|---:|---:|']
    for a in aggregate:table.append(f"| {NAMES[a['engine']]} | {a['clients']} | {WORK[a['workload']]} | {a['ops_s']:,.1f} | {a['p50_ms']:.3f} | {a['p95_ms']:.3f} | {a['p99_ms']:.3f} |")
    (root/'table.md').write_text('\n'.join(table)+'\n',encoding='utf-8')
if __name__=='__main__':main()
