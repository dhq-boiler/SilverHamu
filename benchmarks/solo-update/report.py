"""Validate all observations before drawing summaries; never add separate p99s."""
import argparse,gzip,hashlib,json,math,pathlib,statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
NAMES=['cabi-on','cabi-off','rust-on','rust-off','sqlite','rust-on-diag','rust-off-diag','sqlite-pinned']
def read_record(path):
    if path.suffix=='.gz':
        with gzip.open(path,'rt') as f:return json.load(f)
    return json.loads(path.read_text())
def phases(d,group,sample):
    t=d['trace'];a=d['entered'];c=d['commit'];end=d['ended'];sync=d['sync_start']
    assert a<=c<=t[4]<=t[5]<=sync<=t[6]<=t[7]<=end
    collect=t[1]-d['detail'][1] if group else 0
    assert collect>=0 and collect<=t[4]-c
    out={'begin_put':c-a,'collection':collect,'commit_front':t[4]-c-collect,'log_write':t[5]-t[4],
         'sync_entry':sync-t[5],'sync':t[6]-sync,'publish':t[7]-t[6],'finish':end-t[7],
         'clock_edges':sample-(end-a)}
    assert all(v>=0 for v in out.values()) and sum(out.values())==sample
    return out
def main():
    p=argparse.ArgumentParser();p.add_argument('root',type=pathlib.Path);p.add_argument('--smoke',action='store_true');a=p.parse_args()
    root=a.root;env=json.loads((root/'environment.json').read_text());rs=[]
    expected=1 if a.smoke else 3
    assert env['reps']==expected and len(env['runs'])==8*expected
    assert {(r['rep'],r['variant']) for r in env['runs']}=={(rep,n) for rep in range(1,expected+1) for n in NAMES}
    assert env['old_snapshot_retained'] is False
    source=pathlib.Path(__file__).resolve().parents[2]
    for name,h in env['source_sha256'].items():assert hashlib.sha256((source/name).read_bytes().replace(b'\r\n',b'\n')).hexdigest()==h,name
    for run in env['runs']:
        assert run['status']=='passed';r=read_record(root/run['file']);ns=r['samples_ns'];assert len(ns)==env['ops']
        assert all(isinstance(v,int) and v>0 for v in ns) and r['checks']==['initial','old','latest','reopen']
        assert r['diagnostic']==run['variant'].endswith('-diag')
        if not run['variant'].startswith('sqlite'):assert r['group']==('-off' not in run['variant'])
        if r['api']=='rust':assert r['sync_attempts']==len(ns)
        v=sorted(ns);row={'variant':run['variant'],'rep':run['rep'],'ops_s':len(ns)/r['elapsed_s'],'p99_ms':v[math.ceil(len(v)*.99)-1]/1e6,'max_ms':v[-1]/1e6}
        if r['diagnostic']:
            assert len(r['diagnostics'])==len(ns)
            breakdown=[phases(d,r['group'],sample) for d,sample in zip(r['diagnostics'],ns)]
            tail=sorted(range(len(ns)),key=lambda i:ns[i])[-math.ceil(len(ns)*.01):]
            row['mean_ms']={k:statistics.mean(x[k] for x in breakdown)/1e6 for k in breakdown[0]}
            row['slowest_one_percent_mean_ms']={k:statistics.mean(breakdown[i][k] for i in tail)/1e6 for k in breakdown[0]}
        else:assert not r['diagnostics']
        rs.append(row)
    for rep in range(1,expected+1):
        checks=[read_record(root/x['file'])['hashes'] for x in env['runs'] if x['rep']==rep and not x['variant'].startswith('rust')]
        assert len(checks)==4 and all(v==checks[0] for v in checks)
    (root/'summary.json').write_text(json.dumps(rs,indent=2))
    table=['| Variant | Rep | Updates/s | p99 ms | Max ms |','|---|---:|---:|---:|---:|']
    for r in rs:table.append(f'| {r["variant"]} | {r["rep"]} | {r["ops_s"]:.3f} | {r["p99_ms"]:.3f} | {r["max_ms"]:.3f} |')
    (root/'table.md').write_text('\n'.join(table)+'\n');print('\n'.join(table))
    if a.smoke:return
    fig,axes=plt.subplots(1,3,figsize=(15,5),layout='constrained')
    for ax,m,title in zip(axes,['ops_s','p99_ms','max_ms'],['Updates/second','p99 milliseconds','Maximum milliseconds']):
        groups=[[r[m] for r in rs if r['variant']==n] for n in NAMES]
        bars=ax.bar(range(8),[statistics.median(v) for v in groups],color=['#607d8b','#607d8b','#197d83','#197d83','#a8761e','#8ab8ba','#8ab8ba','#ba9d6d'])
        for i,v in enumerate(groups):ax.scatter([i-.08,i,i+.08],v,color='black',s=12)
        ax.bar_label(bars,fmt='%.2f',padding=3);ax.set_ylim(0,max(map(max,groups))*1.2)
        ax.set_xticks(range(8),NAMES,rotation=40,ha='right');ax.set_title(title)
    fig.suptitle('Single-writer isolation: 100k rows /256B /2 CPU /4GiB /old snapshot tested outside timing; SQLite pinned is auxiliary\nBars: median; dots: all three runs. diag includes measurement overhead.')
    fig.savefig(root/'comparison.svg');fig.savefig(root/'comparison.png',dpi=150);plt.close(fig)
    fig,axes=plt.subplots(1,2,figsize=(12,5),layout='constrained')
    for ax,which,title in zip(axes,['mean_ms','slowest_one_percent_mean_ms'],['All requests: mean elapsed phases','Slowest 1% per run: mean phases']):
        groups=[[r for r in rs if r['variant']==n] for n in ['rust-on-diag','rust-off-diag']]
        bottom=[0,0]
        for k in groups[0][0][which]:
            v=[statistics.mean(r[which][k] for r in g) for g in groups]
            ax.bar(['Rust group on','Rust group off'],v,bottom=bottom,label=k);bottom=[x+y for x,y in zip(bottom,v)]
        ax.set_ylabel('milliseconds');ax.set_title(title)
    axes[1].legend(loc='upper left',bbox_to_anchor=(1,1));fig.savefig(root/'phases.svg');fig.savefig(root/'phases.png',dpi=150);plt.close(fig)
    for f in root.glob('*.svg'):f.write_text('\n'.join(s.rstrip() for s in f.read_text().splitlines())+'\n')
if __name__=='__main__':main()
