import csv
from pathlib import Path
import sys
from xml.etree.ElementTree import parse

def render(out):
    with (out/"summary.csv").open() as f:rows=[r for r in csv.DictReader(f) if r["phase"]=="full"]
    rows.sort(key=lambda r:(int(r["rep"]),("standard","fifo","aged").index(r["mode"])))
    parts=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 980 470" role="img"><title>Old checkpoint I/O at WAL sync entry</title><rect width="980" height="470" fill="#fff"/><g font-family="system-ui,sans-serif" fill="#1a3548">']
    def text(x,y,value,size=14):
        parts.append(f'<text x="{x}" y="{y}" font-size="{size}">{value}</text>')
    text(18,34,"Was old checkpoint I/O still pending when WAL sync began?",23)
    text(18,60,"aged = Aged writer priority (16 ms). Each run has 16,384 updates and 63 checkpoint publications.",14)
    text(18,98,"Trial · lock");text(150,98,"Pending / quiet syncs");text(345,98,"Quiet mean sync (ms)");text(550,98,"Updates/s");text(675,98,"Update p99 (ms)");text(835,98,"Worst (ms)")
    top=max((float(r["quiet_ms"]) for r in rows if r.get("quiet_ms")),default=1)
    for idx,r in enumerate(rows):
        y=132+idx*33
        text(18,y,f"{r['rep']} · {r['mode']}")
        if r.get("paired"):
            bound="≤" if r.get("upper_bound")=="True" else ""
            text(150,y,f"{bound}{int(r['quiet_old']):,} / {int(r['quiet']):,}")
            v=float(r["quiet_ms"])
            parts.append(f'<rect x="348" y="{y-12}" width="{105*v/top:.1f}" height="14" fill="#327383"/>')
            text(465,y,f"{v:.2f}")
        else:text(150,y,"Trace incomplete")
        for x,k in ((550,"updates_s"),(675,"p99_ms"),(835,"max_ms")):
            text(x,y,f"{float(r[k]):,.2f}")
    text(18,427,"Quiet: no target submission in the previous 10 ms or during sync. Pending: submitted ≥10 ms earlier and still outstanding.",12)
    text(18,451,"≤ marks an upper bound when the completion probe missed events.",13)
    parts.append("</g></svg>")
    path=out/"pending.svg";path.write_text("".join(parts),encoding="utf8");parse(path)

if __name__=="__main__":render(Path(sys.argv[1]))
