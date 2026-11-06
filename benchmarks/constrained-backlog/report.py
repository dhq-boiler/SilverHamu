"""Publish failed screening without promoting refusal-driven queue clearance."""
import json,pathlib,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from analyze import summarize,gate

def main():
    root=pathlib.Path(sys.argv[1]);s=summarize(root);s['gate']=gate(s);(root/'summary.json').write_text(json.dumps(s,indent=2))
    rows=s['trials'];order=[('normal',1),('normal',8),('single',1),('single',8)];rows=[next(x for x in rows if (x['scenario'],x['limit'])==k) for k in order]
    budget=rows[1].get('preparation_budget');effective=rows[1].get('effective_limit',8)
    fig,axs=plt.subplots(1,4,figsize=(16,4),layout='constrained');labels=['Normal\n1','Normal\nup to 8','100ms stall\n1','100ms stall\nup to 8']
    if budget:labels=['Normal\n1','Normal\nbudget cap '+str(effective),'100ms stall\n1','100ms stall\nbudget cap '+str(effective)]
    metrics=[([x['mixed']['write']['response_ms']['p99'] for x in rows],'Successful write p99','ms'),([x['mixed']['write']['response_ms']['max'] for x in rows],'Maximum write response','ms'),([x['syncs'] for x in rows],'Durable synchronization calls','count'),([x['mixed']['write']['service_refused'] for x in rows],'Resource-refused updates','count')]
    for ax,(values,title,unit) in zip(axs,metrics):
        bars=ax.bar(labels,values,color=['#526777','#16817b','#526777','#16817b']);ax.bar_label(bars,fmt='%.2f' if unit=='ms' else '%.0f',padding=3);ax.set_title(title);ax.set_ylabel(unit);ax.set_ylim(0,max(values+[1])*1.23);ax.tick_params(axis='x',labelsize=8)
    fig.suptitle('Pilot only / 1M rows / 1 CPU / 512 MiB / 4 KiB staged-write budget\nOne verified run per condition; each offers 400 updates. '+('Screening gate passed.' if s['gate']['proceed'] else 'Gate failed; no longer comparison.'),fontsize=11)
    fig.savefig(root/'pilot.svg');fig.savefig(root/'pilot.png',dpi=150);plt.close(fig)
    for p in root.glob('*.svg'):p.write_text('\n'.join(x.rstrip() for x in p.read_text().splitlines())+'\n')
    table=['| Condition | Requested / effective limit | Accepted updates | Service refused | Write p99 ms | Write max ms | Read p99 ms | Syncs | Accepted/sync | Backlog recovery ms | RSS MiB |','|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|']
    for x in rows:
        w=x['mixed']['write'];recovery=x['recovery']['recovery_ms'] if x['recovery'] else None
        table.append(f"| {x['scenario']} | {x['limit']} / {x.get('effective_limit',x['limit'])} | {w['ok']} | {w['service_refused']} | {w['response_ms']['p99']:.3f} | {w['response_ms']['max']:.3f} | {x['mixed']['read']['response_ms']['p99']:.3f} | {x['syncs']} | {x['updates_per_sync']:.3f} | "+(f'{recovery:.3f}' if recovery is not None else 'not applicable')+f" | {x['resources']['rss_peak_mib']:.2f} |")
    (root/'trials.md').write_text('\n'.join(table)+'\n');print(json.dumps(s['gate'],indent=2))
if __name__=='__main__':main()
