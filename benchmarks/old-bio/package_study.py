"""Build the public artifact from captured workloads without exporting pointer values."""
from pathlib import Path
import csv
import json
import shutil
import sys
from statistics import median
from analyze_bio import audit
from replay import load,replay
from summarize import summarize
from figure import render

def build(root,out):
    out.mkdir(exist_ok=True)
    shifts=[]
    target=sorted((root/"full").glob("*-trace"))
    assert len(target)==9
    for folder in target:
        meta=json.loads((folder/"meta.json").read_text())
        misses={s.strip().split("_")[0]:int(s.split()[-1]) for s in meta["probe_profile"]}
        if misses["bio"]:
            continue
        prefix="upper-" if misses["completed"] else ""
        audit(folder,16384,256,bound=bool(prefix))
        calls=load(folder/(prefix+"calls.csv.gz"))
        bios=load(folder/(prefix+"lifetimes.csv.gz"))
        for shift in (-2000,0,2000):
            quiet,pending=replay(calls,bios,shift)
            shifts.append((folder.name,shift,bool(prefix),quiet,pending))
        for file in ("calls.csv.gz","lifetimes.csv.gz"):
            shutil.copyfile(folder/(prefix+file),out/(folder.name+"-"+prefix+file))
    with (out/"clock.csv").open("w",newline="") as f:
        w=csv.writer(f);w.writerow(("trial","clock_shift_ns","upper_bound","quiet","quiet_old"));w.writerows(shifts)
    rows=summarize(root,out);render(out)
    full=[r for r in rows if r["phase"]=="full"]
    full.sort(key=lambda r:(int(r["rep"]),("standard","fifo","aged").index(r["mode"])))
    table=[];rates=[]
    for r in full:
        base=f"| {r['rep']} | {r['mode']} | {r['updates_s']:.2f} | {r['p99_ms']:.2f} | {r['max_ms']:.2f}"
        if "quiet" in r:
            bound="≤" if r["upper_bound"] else ""
            table.append(base+f" | {r['quiet_ms']:.3f} | {bound}{r['quiet_old']:,} / {r['quiet']:,} | {bound}{r['quiet_old_sync_s']*1000/r['quiet']:.3f} |")
        else:table.append(base+" | — | unavailable | — |")
    for r in rows:
        if r["phase"]=="lean" and r["kind"]=="trace":
            off=next(s for s in rows if s["phase"]=="lean" and s["kind"]=="plain" and s["rep"]==r["rep"])
            rates.append(f"| {r['rep']} | {off['updates_s']:.2f} | {r['updates_s']:.2f} | {off['p99_ms']:.2f} | {r['p99_ms']:.2f} |")
    perf=[]
    for mode in ("standard","fifo","aged"):
        arm=[r for r in full if r["mode"]==mode]
        speeds=[r["updates_s"] for r in arm]
        perf.append(f"| {mode} | {min(speeds):.2f}–{max(speeds):.2f} | {median(speeds):.2f} | {median(r['p99_ms'] for r in arm):.2f} | {max(r['max_ms'] for r in arm):.2f} |")
    aged=[r for r in full if r["mode"]=="aged" and "quiet" in r]
    assert aged
    ceiling=max(r["quiet_old_sync_s"]*1000/r["quiet"] for r in aged)
    overlap=max(r["quiet_old_overlap_s"]*1000/r["quiet"] for r in aged)
    sample=next(r for r in aged if str(r["rep"])=="2")
    exact=next(r for r in aged if str(r["rep"])=="3")
    def lines(v):return "\n".join(v)
    text=f"""# Outstanding checkpoint data at WAL sync entry

The previous third pair measured mean WAL sync at 5.39 ms with the current lock and 7.36 ms with aged writer priority on calls with no new checkpoint submission in the preceding 10 ms or during sync. Those recordings contained only submission events. These independent repetitions measure guest ext4 callback lifetimes and cannot retrospectively assign a completion time to an earlier request.

![Outstanding checkpoint writes and update performance](pending.svg)

The second aged writer priority trial reached a quiet mean of **{sample['quiet_ms']:.3f} ms**. Across the {len(aged)} usable aged writer priority trials, charging the full duration of every possibly exposed quiet sync contributed at most **{ceiling:.3f} ms** to a run's mean. Counting only the part of each sync when an old BIO could still be outstanding lowered that ceiling to **{overlap:.3f} ms**. These figures bound this specific guest-level mechanism in the measured trials. The original 1.97 ms difference and the downstream cause of longer syncs remain unresolved.

The third aged writer priority capture had zero probe misses and zero ring loss. It recorded {exact["long_bios"]:,} checkpoint BIOs with lifetimes of at least 10 ms and {exact["all_old"]:,} WAL sync starts with an old BIO still pending. Among the {exact["quiet"]:,} quiet syncs, the count was **{exact["quiet_old"]}**, with mean sync duration **{exact["quiet_ms"]:.3f} ms**.

| Trial | Lock | Updates/s | Update p99 ms | Maximum ms | Quiet mean sync ms | Quiet syncs with old I/O / all quiet | Time on such calls per quiet sync, ms |
|---:|---|---:|---:|---:|---:|---:|---:|
{lines(table)}

The last column charges the *entire* sync interval to any quiet call with an old BIO pending. It is a ceiling on the direct time contribution for these calls. It does not quantify downstream congestion after a callback. All nine trials ran on fresh million-key ext4 volumes at 2 CPUs and 4 GiB, with four writers, 8 MiB writeback, 16,384 durable updates and 63 independent checkpoint publications. The order rotated. Every trial checked the retained old snapshot, the latest contents and the reopened generation and values. The production binary and lock policies were unchanged.

| Lock | Updates/s range | Median updates/s | Median update p99 ms | Worst over all runs ms |
|---|---:|---:|---:|---:|
{lines(perf)}

## Matching and capture quality

A successful checkpoint snapshot fsync identifies the inode for its publication. On the volume device the submission hook records ext4 data BIOs for that inode across its preparation and final sync windows. The callback hook observes entry to ext4_end_bio. A live BIO pointer identifies one lifetime until that callback. Reuse starts a new lifetime. Only relative timestamps, sizes and application request IDs are exported.

Old means submitted at least 10 ms before sync entry and still pending at entry. Quiet means no target submission in [entry − 10 ms, entry) or [entry, return). The exported [clock sensitivity](clock.csv) recomputes both after moving the application clock by ±2 µs. This measures guest callback timing. The host device, metadata writes and I/O for other files are outside the instrumented data path.

Exact counts require zero ring loss, zero probe misses and complete pointer pairing. The ≤ symbol denotes a **separate conservative analysis** when only completion callbacks were missed. A missing callback can extend its inferred lifetime until a later callback at the same address, a new submission reusing that address, or the last measured response. These bounds can overstate outstanding work. Submission loss invalidates this analysis too. Full observation counts and trace quality appear in [summary.csv](summary.csv).

## Observation overhead

Two short 10,000-key, 512-update paired pilots alternated trace order. The initial probe reading inode fields at callback missed one event in each pilot. A function-entry alternative failed registration on this kernel. The final probe reads only a pointer at callback. Both lightweight pilots paired all 60 target BIOs with zero misses.

| Pilot | Untraced updates/s | Traced updates/s | Untraced p99 ms | Traced p99 ms |
|---|---:|---:|---:|---:|
{lines(rates)}

These short pilots indicate the scale and variability of measurement overhead. All full runs used the same observer. See the [original lock comparison](../aged-load-2026-11-08/README.md) and [prior submission analysis](../writer-overlap-2026-11-08/README.md).

## Reproduction

Build the existing aged-writer benchmark image as catdb-aged-load and use the catdb-fifo-products observer image. Run `python benchmarks/old-bio/launch.py RAW/lean` followed by `python benchmarks/old-bio/launch.py RAW/full --full`. Run `test_old_bio.py` for interval boundary checks. The published per-trial call and lifetime tables can be replayed by passing them to the load and replay functions in `benchmarks/old-bio/replay.py`. Render the SVG with `python benchmarks/old-bio/figure.py REPORT`.
"""
    (out/"README.md").write_text(text,encoding="utf8")
if __name__=="__main__":build(Path(sys.argv[1]),Path(sys.argv[2]))
