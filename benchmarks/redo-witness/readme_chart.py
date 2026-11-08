"""Generate the comparison shown on the project README from the audited release trials."""
import json
from pathlib import Path

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.ticker import FuncFormatter

root = Path(__file__).resolve().parents[2] / "docs/benchmarks/durable-redo-2026-11-08"
audit = json.loads((root / "comparison/independent-audit.json").read_text())
products = [("catdb-service", "catdb"), ("postgres", "PostgreSQL 18.6"), ("oracle", "Oracle 26ai Free")]
metrics = [
    ("throughput", "Durable updates / second", "Higher is better", 5550, [0, 2000, 4000]),
    ("p99_ms", "p99 latency (ms)", "Lower is better", 18.5, [0, 5, 10, 15]),
]

ink, muted, accent, gray = "#172b37", "#475e68", "#087464", "#8398a4"
plt.rcParams.update({"font.family": "DejaVu Sans", "font.size": 12})
fig, axes = plt.subplots(1, 2, figsize=(10.2, 4.9), facecolor="white")
fig.subplots_adjust(left=.205, right=.975, top=.80, bottom=.18, wspace=.20)

for column, (metric, title, direction, limit, ticks) in enumerate(metrics):
    ax = axes[column]
    ax.set(xlim=(0, limit), ylim=(-.6, 7.6), xticks=ticks, yticks=[])
    ax.set_axisbelow(True)
    ax.grid(axis="x", linewidth=.7, color="#dbe2e5")
    for spine in ax.spines.values():
        spine.set_visible(False)
    ax.tick_params(axis="x", labelsize=10, colors=muted, length=0)
    ax.xaxis.set_major_formatter(FuncFormatter(lambda x, _: f"{x:,.0f}"))
    ax.text(0, 1.14, title, transform=ax.transAxes, color=ink, fontsize=16, weight="bold")
    ax.text(0, 1.06, direction, transform=ax.transAxes, color=muted, fontsize=11)
    for group_index, group in enumerate(audit["workloads"]):
        top = 6 - 4 * group_index
        if column == 0:
            ax.text(-.035, top + .84, f"{group['clients']} clients", transform=ax.get_yaxis_transform(),
                    ha="right", va="center", fontsize=13, weight="bold", color=ink)
        for index, (product, label) in enumerate(products):
            y = top - index
            value = group["engines"][product][metric]
            highlight = product == "catdb-service"
            ax.barh(y, value, height=.53, color=accent if highlight else gray)
            weight = "bold" if highlight else "normal"
            if column == 0:
                ax.text(-.035, y, label, transform=ax.get_yaxis_transform(), ha="right",
                        va="center", weight=weight, color=ink)
            number = f"{value:,.0f}" if metric == "throughput" else f"{value:.2f}"
            ax.text(value + limit * .018, y, number, ha="left", va="center", color=ink, weight=weight)
    ax.axhline(3.55, color="#dbe2e5", lw=.8)

fig.text(.033, .067, "Median of 3 runs  ﾂｷ  100,000 keys  ﾂｷ  256-byte values  ﾂｷ  1 durable commit per update",
         fontsize=10.5, color=muted)
fig.text(.033, .025, "catdb format 4  ·  64 MiB preallocated WAL  ·  No checkpoint during timed updates",
         fontsize=10.5, color=muted)
fig.savefig(root / "readme-comparison.png", dpi=160, facecolor="white")
plt.close(fig)
