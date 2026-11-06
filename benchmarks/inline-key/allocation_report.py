from pathlib import Path
import json,statistics as st,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=Path(sys.argv[1]);a=json.loads(Path(sys.argv[2]).read_text());s=json.loads((root/'summary.json').read_text())
fig,axes=plt.subplots(1,3,figsize=(12,4),layout='constrained')
items=[('Open: live requested bytes','MiB',[[x['live_bytes']/2**20 for x in a if x['key_bytes']==8 and x['variant']==v] for v in ['baseline','inline']]),('Open: allocation calls','million',[[x['allocation_calls']/1e6 for x in a if x['key_bytes']==8 and x['variant']==v] for v in ['baseline','inline']]),('Service reopen readiness','ms',[[x['reopen_ready_ms'] for x in s['trials'] if x['variant']==v] for v in ['baseline','inline']])]
for ax,(title,unit,values) in zip(axes,items):
 bars=ax.bar(['Vec keys','Inline keys'],[st.median(v) for v in values],color=['#546e7a','#197d83']);ax.bar_label(bars,fmt='%.2f',padding=3)
 for i,v in enumerate(values):ax.scatter([i-.08,i,i+.08],v,color='black',s=18,zorder=3)
 ax.set_title(title);ax.set_ylabel(unit);ax.set_ylim(0,max(map(max,values))*1.22)
fig.suptitle('1M rows / 1 CPU / 512 MiB; median and all three trials\nAllocation probes run separately; service readiness has 100ms polling resolution.',fontsize=11)
fig.savefig(root/'allocation.svg');fig.savefig(root/'allocation.png',dpi=150)
for p in root.glob('*.svg'):p.write_text('\n'.join(x.rstrip() for x in p.read_text().splitlines())+'\n')
