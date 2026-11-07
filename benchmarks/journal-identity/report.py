"""Build tables and figures from every workload and its bounded trace."""
import argparse
import csv
import gzip
import json
from collections import Counter
from pathlib import Path
from statistics import mean, median
from xml.sax.saxutils import escape

from identity import analyze, identity, records

def save(path, rows):
    with path.open("w", newline="",encoding="utf-8") as stream:
        writer=csv.DictWriter(stream, fieldnames=rows[0],lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)

def chart(path, title, metrics, samples):
    parts=['<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 800 365" role="img">',
           '<title>'+escape(title)+'</title><rect width="800" height="365" fill="#fafafa"/>',
           '<g font-family="system-ui,sans-serif" fill="#182f45">',
           '<text x="24" y="36" font-size="23">'+escape(title)+'</text>']
    for index,(label,key,unit,aggregate) in enumerate(metrics):
        top=65+index*98
        parts.append(f'<text x="24" y="{top}" font-size="16">{escape(label)}</text>')
        values=[aggregate([r[key] for r in samples if r["mode"]==mode]) for mode in ("standard","fifo")]
        for i,(mode,value,color) in enumerate(zip(("Standard","FIFO"),values,("#235883","#af562b"))):
            y=top+10+i*24
            w=490*value/(max(values) or 1)
            parts.append(f'<text x="30" y="{y+14}" font-size="14">{mode}</text><rect x="110" y="{y}" width="{w:.2f}" height="16" fill="{color}"/><text x="{119+w:.2f}" y="{y+14}" font-size="14">{value:,.3f} {unit}</text>')
    path.write_text("".join(parts)+'</g></svg>',encoding="utf-8")

def summarize(root,out,pilot):
    environment=json.loads((root/"environment.json").read_text())
    ops,cap=environment["ops"],environment["cap"]
    trials=[]
    counters=[]
    for folder in sorted(p for p in root.iterdir() if p.is_dir()):
        rep,mode,trace=folder.name.split("-")
        log=(folder/"app.log").read_text()
        trial,_,_=analyze(log,mode,int(rep),ops,cap)
        assert trial["checkpoints"]==(ops-1)//cap and not trial["coalesced"]
        trial.update(trace=trace)
        if trace=="trace":
            stats,data=identity(folder)
            meta=json.loads((folder/"meta.json").read_text())
            counters.append(dict(run=folder.name,kernel=meta["kernel"],device=meta["device"],
                buffer_kib_per_cpu=meta["buffer_KiB_per_cpu"],cpu_buffers=len(meta["stats"]),
                target_probe_missed=int(meta["probe_profile"][0].split()[-1]),
                wait_probe_missed=int(meta["probe_profile"][1].split()[-1]),
                ring_overruns=0,dropped_events=0,cleaned=meta["cleaned"]))
            trial.update(stats)
            with gzip.open(out/(folder.name+("-pilot" if pilot else "")+".csv.gz"),"wt",newline="") as f:
                w=csv.DictWriter(f,data[0].keys(),lineterminator="\n")
                w.writeheader()
                w.writerows(data)
        trials.append(trial)
    save(out/("pilot.csv" if pilot else "trials.csv"),[{k:r.get(k,"") for k in max(trials,key=len)} for r in trials])
    save(out/("pilot-quality.csv" if pilot else "quality.csv"),counters)
    return trials

def main():
    p=argparse.ArgumentParser()
    for name in ("pilot","full","out"):
        p.add_argument(name,type=Path)
    args=p.parse_args()
    args.out.mkdir(exist_ok=True,parents=True)
    pilot=summarize(args.pilot,args.out,True)
    rows=summarize(args.full,args.out,False)
    assert len(rows)==6 and len(pilot)==8
    chart(args.out/"performance.svg","Update latency and throughput under tracing",[
        ("Through last response, median run","updates_s","updates/s",median),("Update p99, median run","p99_ms","ms",median),("Worst update across three runs","max_ms","ms",max)],rows)
    chart(args.out/"shared.svg","Journal sleep per run, arithmetic mean",[
        ("All WAL sleep in the journal commit wait","journal_s","s",mean),("WAL sleep in calls proven to share a checkpoint commit","shared_call_sleep_s","s",mean),
        ("Time both threads slept awaiting that completion","simultaneous_s","s",mean)],rows)
    def pair(key,fn=mean):
        return [fn(r[key] for r in rows if r["mode"]==mode) for mode in ("standard","fifo")]
    def line(label,key,fn=mean):
        a,b=pair(key,fn)
        return f"| {label} | {a:,.3f} | {b:,.3f} | {b-a:+,.3f} |"
    metrics="\n".join(line(*v) for v in [
        ("WAL fsync, s","sync_s"),("WAL journal sleep, s","journal_s"),("Attributed sleep before JBD2 start, s","before_start_s"),("Attributed sleep during JBD2 commit, s","during_commit_s"),("Attributed sleep after JBD2 end, s","after_end_s"),("WAL calls targeting any observed checkpoint epoch","same_epoch"),("Shared WAL calls","shared"),("Shared with snapshot sync","shared_snapshot"),("Shared with tail sync","shared_tail"),("Shared with replacement","shared_replace"),("Shared with other checkpoint calls","shared_other"),
        ("WAL journal sleep in shared calls, s","shared_call_sleep_s"),("Simultaneous sleep to shared completion, s","simultaneous_s"),
        ("Updates/s to last response, median","updates_s",median),("Updates/s through drain, median","drained_s",median),
        ("Update p99, ms, median","p99_ms",median),("Maximum update, ms, worst of three","max_ms",max)])
    overhead=[]
    for mode in ("standard","fifo"):
        for key in ("updates_s","p99_ms","max_ms"):
            means=[mean(r[key] for r in pilot if r["mode"]==mode and r["trace"]==tr) for tr in ("plain","trace")]
            overhead.append(f"| {mode} | {key} | {means[0]:.3f} | {means[1]:.3f} | {(means[1]/means[0]-1)*100:+.1f}% |")
    total=pair("journal_s"); shared=pair("shared_call_sleep_s")
    delta=total[1]-total[0]; sd=shared[1]-shared[0]
    counts=[sum(r["shared"] for r in rows if r["mode"]==m) for m in ("standard","fifo")]
    result=f"Standard and FIFO had **{counts[0]} and {counts[1]}** WAL calls respectively that slept for the same journal transaction as a checkpoint across three runs each. FIFO minus standard was **{delta:+.3f} s/run** for all WAL journal sleep and **{sd:+.3f} s/run** for WAL journal sleep in the proven shared calls."
    report=f"""# Do WAL and checkpoint fsyncs wait for the same ext4 commit?

{result} The remaining difference was **{delta-sd:+.3f} s/run**. Other I/O on the guest device, journal batching and underlying storage latency can affect different transactions too. This experiment establishes identity and overlap, not how either call would perform without the other.

![Application performance](performance.svg)

| Measure | Standard | FIFO | FIFO minus standard |
|---|---:|---:|---:|
{metrics}

![Shared journal wait](shared.svg)

Each run used one million 256-byte values, four writers, 16,384 acknowledged updates, 63 independently published checkpoints, prefix prewriting enabled, two CPUs and 4 GiB in Docker on WSL2/ext4. Conditions alternated by pair. The workload checks the retained reader, successful version sequence, and reopened data. Both arms used synchronous persistence. The only Rust instrumentation change labels the checkpoint thread. Rates and p99 summarize the three trial-level observations. Sleep figures are per-run means.

## How a shared wait is identified

The five workload threads carry unique labels. Device-scoped journal events include other activity on that ext4 filesystem, so a nearby commit event alone carries no attribution. Each successful WAL syscall is joined to its application sync interval and thread. Each checkpoint snapshot sync is matched to its publication. The ext4 inode event identifies the workload device. Two entry probes read the requested transaction ID in ext4_fc_commit and jbd2_log_wait_commit. A qualifying sleep requires a scheduler stack in jbd2_log_wait_commit, a matching wait entry, and one unique start/end pair for that device and transaction ID. The end event must lie in both sleep intervals. Previously completed IDs, missing epochs and duplicate or ambiguous IDs do not qualify. Trace timestamps have microsecond precision on this guest. The time in shared calls includes the calls' entire journal sleep. The simultaneous column counts only the overlap through commit end. Guest sleep does not measure physical disk time. See [ext4 fsync](https://github.com/torvalds/linux/blob/v6.18/fs/ext4/fsync.c) and [JBD2 commit publication](https://github.com/torvalds/linux/blob/v6.18/fs/jbd2/commit.c).

## Filesystem scope

The measurement applies to the Docker volume on the guest ext4 filesystem. A native NTFS comparison would use the same update stream, checkpoint count, resource limits where possible, and flush-before-acknowledgement rule. A Windows candidate is measuring the buffered flush path against aligned write-through I/O under the same durable-acknowledgement contract ([Microsoft file flush documentation](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers)). Windows file-flush and scheduler events can identify waiting phases; JBD2 transaction IDs are specific to the Linux measurement.

## Observer check

Two 512-update, 15-publication runs per mode and trace setting, on 10,000 rows. These short trials measure workload variation as well as observer overhead. Buffer overruns, dropped events and probe miss counters were checked in every capture.

| Mode | Measure | Plain mean | Traced mean | Change |
|---|---|---:|---:|---:|
{chr(10).join(overhead)}

[All six full runs](trials.csv) · [All eight pilot runs](pilot.csv). The per-run compressed CSVs preserve fsync roles, transaction IDs, completion and sleep timestamps, and classification. Times are monotonic guest nanoseconds. To reproduce, build benchmarks/scaled-gate/Dockerfile.fifo as catdb-journal-identity and prepare the observer image used by benchmarks/wal-sync-waits. Run run.py first with --pilot and a fresh output directory, then with a separate output directory for full runs. Pass both directories and a report directory to report.py. The observer requires Linux tracefs, the two named kernel functions and ext4/JBD2 trace events.
"""
    (args.out/"README.md").write_text(report,encoding="utf-8")
    print(result)
if __name__=="__main__":
    main()
