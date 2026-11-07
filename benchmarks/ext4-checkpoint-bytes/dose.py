"""Plot every trial's median, omitting empty exposure groups."""
import csv
import math

def dose_plot(out):
    from volume import amounts
    amounts(out)
    with (out/"volume.csv").open() as f: groups=list(csv.DictReader(f))
    labels=("zero","0-8 MiB","8-32 MiB","32+ MiB")
    keep=[r for r in groups if r["exposure"] in labels]
    ymax=max(10,10**math.ceil(math.log10(max(float(r["median_ms"]) for r in keep))))
    ymin=min(1,10**math.floor(math.log10(min(float(r["median_ms"]) for r in keep))))
    y=lambda v:230-142*(math.log10(v)-math.log10(ymin))/math.log10(ymax/ymin)
    s=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 850 325" role="img"><title>Journal commit duration by checkpoint data volume</title><desc>Per-trial medians on a logarithmic scale. No marker means no qualifying journal commit.</desc><rect width="850" height="325" fill="#fff"/><g font-family="system-ui,sans-serif" fill="#203247">',
       '<text x="18" y="28" font-size="21">Journal commit duration by checkpoint data volume</text>',
       '<text x="18" y="51" font-size="13">Per-trial medians in ms, logarithmic scale. Each mark is one run.</text>']
    for panel,period in enumerate(["prior","during"]):
        left=54+panel*415
        s.append(f'<text x="{left}" y="76" font-size="15">{"10 ms before commit start" if panel==0 else "During commit (variable exposure time)"}</text>')
        for power in range(round(math.log10(ymin)),round(math.log10(ymax))+1):
            v=10**power; cy=y(v)
            s.append(f'<path d="M{left} {cy}h365" stroke="#d6dce1"/><text x="{left-6}" y="{cy+4}" text-anchor="end" font-size="11">{v:g}</text>')
        for idx,label in enumerate(labels):
            cx=left+40+idx*90
            s.append(f'<text x="{cx}" y="252" text-anchor="middle" font-size="12">{["0 B","(0, 8) MiB","[8, 32) MiB","≥32 MiB"][idx]}</text>')
            bucket=[r for r in keep if r["window"]==period and r["exposure"]==label]
            for r in bucket:
                x=cx+(int(r["rep"])-2)*8+(13 if r["mode"]=="fifo" else -13)
                cy=y(float(r["median_ms"]))
                s.append(f'<circle cx="{x}" cy="{cy}" r="4" fill="#20658f"/>' if r["mode"]=="standard" else
                         f'<path d="M{x} {cy-5}l5 5 -5 5 -5 -5z" fill="#a64a26"/>')
            counts=[sum(int(r["n"]) for r in bucket if r["mode"]==m) for m in ("standard","fifo")]
            s.append(f'<text x="{cx}" y="273" font-size="10" text-anchor="middle">{counts[0]:,} / {counts[1]:,}</text>')
    s.extend(['<circle cx="24" cy="301" r="5" fill="#20658f"/><text x="37" y="306" font-size="13">Standard</text><path d="M125 295l6 6 -6 6 -6 -6z" fill="#a64a26"/><text x="137" y="306" font-size="13">FIFO</text>',
              '<text x="220" y="306" font-size="12">Counts under each bucket: standard / FIFO. Positive buckets contain only different-target data.</text></g></svg>'])
    (out/"dose.svg").write_text("".join(s),encoding="utf-8")
