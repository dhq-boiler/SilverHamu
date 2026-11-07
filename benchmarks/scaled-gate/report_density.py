"""Report checkpoint cost for six update densities. Unprofiled trials drive comparison."""
from pathlib import Path
from statistics import median
import argparse,csv
LABELS=("1","4","16","25%","50%","100%")
MODES=("arrivals","patch")
def parse(path,mode,rep):
    data={};counts={};phases={};patch={}
    for line in path.read_text().splitlines():
        for label,target in [("PAGE_COUNTS",counts),("CHECKPOINT_PHASE",phases),("PATCH_PHASE",patch)]:
            if label+"," in line:
                ver,*rest=map(int,line.split(label+",")[1].split(","))
                target[ver]=rest
        if line.startswith("DENSITY,"):
            density,n,update,total,rss,ver=map(int,line.split(",")[1:])
            assert density not in data
            hit,fallback,read=counts[ver]
            assert hit+fallback==96 and hit==(96 if mode=="patch" else 0)
            assert read==(96*65536 if mode=="patch" else 0)
            row=dict(mode=mode,rep=rep,density=density,label=LABELS[density],changes=n,dirty_pages=96,checkpoint_ms=total/1e6,update_ms=update/1e6,rss_MiB=rss/2**20,read_bytes=read,patched=hit,rescanned=fallback)
            if rep=="diagnostic":
                other,scan,assemble,write,sync=phases[ver]
                rd,validate,modify,*_=patch[ver]
                residual=total-sum([scan,assemble,write,sync,rd,validate,modify])
                assert residual>=0
                row.update(read_ms=rd/1e6,validate_ms=validate/1e6,modify_ms=modify/1e6,scan_ms=scan/1e6,assemble_ms=assemble/1e6,write_ms=write/1e6,sync_ms=sync/1e6,remainder_ms=residual/1e6)
            data[density]=row
    assert sorted(data)==list(range(6))
    return list(data.values())
def save(path,rows):
    with path.open("w",newline="") as f:
        w=csv.DictWriter(f,rows[0],lineterminator="\n");w.writeheader();w.writerows(rows)
def main():
    parser=argparse.ArgumentParser();parser.add_argument("raw",type=Path);parser.add_argument("out",type=Path);args=parser.parse_args();args.out.mkdir(exist_ok=True,parents=True)
    rows=[r for i in (1,2,3) for m in MODES for r in parse(args.raw/f"{i}-{m}.log",m,i)]
    diag=[r for m in MODES for r in parse(args.raw/f"diagnostic-{m}.log",m,"diagnostic")]
    save(args.out/"trials.csv",rows);save(args.out/"phases.csv",diag)
    summary=[]
    for idx,label in enumerate(LABELS):
        def values(m,k):return [r[k] for r in rows if r["density"]==idx and r["mode"]==m]
        a,b=(values(m,"checkpoint_ms") for m in MODES)
        assert values(MODES[0],"changes")==values(MODES[1],"changes")
        summary.append(dict(label=label,changes_per_page=values("patch","changes")[0]/96,scan_ms=median(a),patch_ms=median(b),patch_delta_pct=100*(median(b)/median(a)-1),patch_faster_pairs=sum(y<x for x,y in zip(a,b)),scan_rss=median(values("arrivals","rss_MiB")),patch_rss=median(values("patch","rss_MiB"))))
    save(args.out/"summary.csv",summary)
    chart=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 820 405" role="img"><title>Checkpoint time by changed rows per page</title><rect width="820" height="405" fill="white"/><g fill="#182d3b" font-family="sans-serif">']
    def text(x,y,s,size):chart.append(f'<text x="{x}" y="{y}" font-size="{size}">{s}</text>')
    text(16,30,"Checkpoint elapsed time by changed rows per 64 KiB page",22)
    text(16,55,"96 updated pages, 1M rows, warm files. Median ms of three runs; smaller is faster.",15)
    text(16,84,"Changed rows",14);text(155,84,"MVCC scan",14);text(480,84,"Patch new copy",14)
    peak=max(max(r["scan_ms"],r["patch_ms"]) for r in summary)
    for i,r in enumerate(summary):
        y=121+46*i;text(16,y,f"{r['changes_per_page']:.0f}"+(f" ({r['label']})" if i>=3 else ""),15)
        for x,k,color in [(155,"scan_ms","#396494"),(480,"patch_ms","#ae572d")]:
            v=r[k];width=225*v/peak;chart.append(f'<rect x="{x}" y="{y-13}" width="{width:.2f}" height="16" fill="{color}"/>');text(x+width+7,y,f"{v:.2f}",14)
    (args.out/"density.svg").write_text("\n".join(chart+["</g></svg>"])+"\n")
    for row in summary:print(row)
if __name__=="__main__":main()
