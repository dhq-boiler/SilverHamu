"""Plot all six paired cases, keeping regressions and repeat variability visible."""
import csv
import json
import pathlib
import statistics
import sys
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

plt.rcParams["svg.hashsalt"] = "catdb-service-after-v1"
root = pathlib.Path(sys.argv[1])
rows = json.loads((root / "results.json").read_text(encoding="utf-8"))
env = json.loads((root / "environment.json").read_text(encoding="utf-8"))
assert len(rows) == 36 and all(r["status"] == "passed" for r in env["runs"])
assert not env["profile"]
names = {"before": "Cooperative", "background": "Background"}
colors = {"before": "#767676", "background": "#0072B2"}
works = {"get": "Key lookup", "range100": "Read 100 consecutive records", "update": "Replace one value + durable commit"}
fig, axes = plt.subplots(3, 2, figsize=(13, 10), layout="constrained")
fig.suptitle("catdb service: bounded background reclamation\n100,000 records | 256-byte values | same Python client", fontsize=17, fontweight="bold")
summary = []
for i, (work, title) in enumerate(works.items()):
    cells = []
    for clients in (1, 4):
        for variant in names:
            subset = [r for r in rows if r["workload"] == work and r["clients"] == clients and r["variant"] == variant]
            assert len(subset) == 3 and all(not r["errors"] for r in subset)
            cell = {"workload": work, "clients": clients, "variant": variant}
            for metric in ("ops_s", "p50_ms", "p95_ms", "p99_ms", "max_ms"):
                values = [r[metric] for r in subset]
                cell[metric] = statistics.median(values)
                cell[metric + "_min"] = min(values)
                cell[metric + "_max"] = max(values)
            cells.append(cell)
            summary.append(cell)
    for col, metric in enumerate(("ops_s", "p99_ms")):
        ax = axes[i, col]
        values = [c[metric] for c in cells]
        low = [c[metric] - c[metric + "_min"] for c in cells]
        high = [c[metric + "_max"] - c[metric] for c in cells]
        bars = ax.barh(range(4), values, xerr=[low, high], color=[colors[c["variant"]] for c in cells], capsize=3, height=.65)
        for j, bar in enumerate(bars):
            if j >= 2:
                bar.set_hatch("//")
                bar.set_edgecolor("white")
        ax.set_yticks(range(4), [f"{names[c['variant']]} - {c['clients']} client" + ("s" if c["clients"] == 4 else "") for c in cells])
        ax.invert_yaxis()
        ax.set_title(title, loc="left", fontweight="bold", fontsize=11)
        ax.set_xlabel("Operations / second - higher is better" if metric == "ops_s" else "p99 response time (ms) - lower is better")
        maximum = max(c[metric + "_max"] for c in cells)
        ax.set_xlim(0, maximum * 1.24)
        ax.set_axisbelow(True)
        ax.grid(axis="x", alpha=.2)
        ax.spines[["top", "right"]].set_visible(False)
        for y, c in enumerate(cells):
            label = f"{c[metric]:,.0f}" if metric == "ops_s" else f"{c[metric]:.3f}"
            ax.text(c[metric + "_max"] + maximum * .015, y, label, va="center", fontsize=10)
fig.supxlabel("Median of 3 runs; whiskers = min/max. Before/after order alternated. Timing instrumentation disabled.\nWarm-cache API microbenchmarks; client overhead included. Experimental, not a general DBMS ranking. 2026-11-05 (JST)", fontsize=10)
fig.savefig(root / "reclamation.svg", metadata={"Date": env["utc"]})
fig.savefig(root / "reclamation.png", dpi=140)
plt.close(fig)
svg = root / "reclamation.svg"
svg.write_text("\n".join(line.rstrip() for line in svg.read_text(encoding="utf-8").splitlines()) + "\n", encoding="utf-8")
with (root / "summary.csv").open("w", newline="", encoding="utf-8") as stream:
    writer = csv.DictWriter(stream, fieldnames=list(summary[0]))
    writer.writeheader()
    writer.writerows(summary)
table = ["| Workload | Clients | Variant | ops/s | p50 ms | p95 ms | p99 ms |", "|---|---:|---|---:|---:|---:|---:|"]
for c in summary:
    table.append(f"| {works[c['workload']]} | {c['clients']} | {names[c['variant']]} | {c['ops_s']:,.1f} | {c['p50_ms']:.3f} | {c['p95_ms']:.3f} | {c['p99_ms']:.3f} |")
(root / "table.md").write_text("\n".join(table) + "\n", encoding="utf-8")
