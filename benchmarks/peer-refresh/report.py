"""Keep every measured outcome and separate embedded/service API paths."""
import argparse,json,pathlib,statistics,subprocess,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

def main():
    p=argparse.ArgumentParser();p.add_argument('root',type=pathlib.Path);a=p.parse_args()
    here=pathlib.Path(__file__).parent
    subprocess.run([sys.executable,str(here/'validate.py'),str(a.root)],check=True)
    subprocess.run([sys.executable,str(here.parent/'plot.py'),str(a.root/'main')],check=True)
    rows=[]
    for name in ['main','legacy']:
        for r in json.loads((a.root/name/'results.json').read_text()):
            if r['engine']=='catdb-service':rows.append(dict(r,api=name))
    fig,axes=plt.subplots(3,2,figsize=(12,10),layout='constrained')
    table=['| API | Clients | Work | ops/s | p50 ms | p95 ms | p99 ms |','|---|---:|---|---:|---:|---:|---:|']
    for j,work in enumerate(['get','range100','update']):
        for i,metric in enumerate(['ops_s','p99_ms']):
            groups=[[r[metric] for r in rows if r['api']==api and r['clients']==clients and r['workload']==work] for api in ['legacy','main'] for clients in [1,4]]
            assert all(len(g)==3 for g in groups)
            ax=axes[j,i];bars=ax.bar(['Legacy 1','Legacy 4','Single 1','Single 4'],[statistics.median(g) for g in groups],color=['#607d8b','#607d8b','#197d83','#197d83'])
            for k,g in enumerate(groups):ax.scatter([k-.08,k,k+.08],g,color='black',s=12)
            ax.bar_label(bars,fmt='%.2f',padding=3);ax.set_ylim(0,max(map(max,groups))*1.2)
            ax.set_title(work);ax.set_ylabel('operations/s' if metric=='ops_s' else 'p99 ms')
        for api in ['legacy','main']:
            for clients in [1,4]:
                rs=[r for r in rows if r['api']==api and r['clients']==clients and r['workload']==work]
                table.append('| '+('single' if api=='main' else api)+f' | {clients} | {work} | '+' | '.join(f'{statistics.median(r[m] for r in rs):.3f}' for m in ['ops_s','p50_ms','p95_ms','p99_ms'])+' |')
    fig.suptitle('Same current catdb binary: legacy versus one-request service API\n100k rows /256B /2 CPU /4GiB each; median and all three repetitions')
    fig.savefig(a.root/'api-paths.svg');fig.savefig(a.root/'api-paths.png',dpi=140);plt.close(fig)
    (a.root/'api-paths.md').write_text('\n'.join(table)+'\n')
    f=a.root/'api-paths.svg';f.write_text('\n'.join(s.rstrip() for s in f.read_text().splitlines())+'\n')
if __name__=='__main__':main()
