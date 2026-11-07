"""Generate the three-way comparison from run_dirty.py output."""
import argparse
import csv
from pathlib import Path
from statistics import median
from report import trials

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("raw", type=Path)
    parser.add_argument("out", type=Path)
    args = parser.parse_args()
    variants = ("wal", "dual", "dirty")
    rows = [r for stage in ("100k", "1m") for r in trials(args.raw, stage, variants)]
    args.out.mkdir(parents=True, exist_ok=True)
    with (args.out/"trials.csv").open("w",newline="") as f:
        w=csv.DictWriter(f,rows[0],lineterminator="\n")
        w.writeheader()
        w.writerows(rows)
    metrics = [("written_MiB","Application writes (MiB)"),("checkpoint_ms","Checkpoint (ms)"),
               ("ops_s","Transactions per second"),("p99_ms","Update p99 (ms)"),
               ("max_ms","Worst update (ms)"),("reopen_ms","Warm reopen (ms)")]
    svg=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 960 554" role="img"><title>Single writer, two CPUs and four GiB</title><rect width="960" height="554" fill="white"/><g font-family="sans-serif" fill="#17263a">']
    def label(x,y,s,size):
        svg.append(f'<text x="{x}" y="{y}" font-size="{size}">{s}</text>')
    label(16,30,"Checkpoint comparison including publication and reuse",24)
    label(16,54,"Median of three runs per condition. Worst update includes every request. Each panel has its own scale.",14)
    order=[(n,v) for n in (100000,1000000) for v in variants]
    table=[]
    for n,v in order:
        group=[r for r in rows if (r["rows"],r["variant"])==(n,v)]
        table.append({k:(max if k=="max_ms" else median)(r[k] for r in group) for k in rows[0] if k not in ("variant","rep")})
    for i,(key,title) in enumerate(metrics):
        x,y=16+(i%3)*318,88+(i//3)*229
        label(x,y,title,17)
        peak=max(g[key] for g in table)
        for j,((n,v),g) in enumerate(zip(order,table)):
            yy=y+25+j*30
            label(x,yy,("100k" if n==100000 else "1M")+" "+v,12)
            width=145*g[key]/peak
            color=["#396494","#a54c31","#28795d"][j%3]
            svg.append(f'<rect x="{x+76}" y="{yy-11}" width="{width:.2f}" height="13" fill="{color}"/>')
            label(x+81+width,yy,f"{g[key]:,.2f}",12)
    (args.out/"comparison.svg").write_text("\n".join(svg+["</g></svg>"])+"\n")
    header="| Rows | Path | Tx/s | p99 ms | Worst ms | Checkpoint ms | Writes MiB | Syncs | End RSS MiB | Reopen ms |"
    lines=[header,"|---:|:---|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for (n,v),g in zip(order,table):
        vals=[g[k] for k in ("ops_s","p99_ms","max_ms","checkpoint_ms","written_MiB","syncs","rss_end_MiB","reopen_ms")]
        lines.append(f"| {n:,} | {v} | "+" | ".join(f"{x:,.2f}" for x in vals)+" |")
    print("\n".join(lines))

if __name__=="__main__":
    main()
