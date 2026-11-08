"""Render the paired service study after verifying every timed and reopened fixture."""
import csv
import json
from pathlib import Path
from statistics import median
import subprocess
import sys

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

products = {
    "catdb-baseline": ("catdb before", "#83929a"),
    "catdb-service": ("catdb default", "#0072b2"),
    "catdb-experimental": ("catdb wide-cohort", "#00516c"),
    "postgres": ("PostgreSQL", "#397955"),
    "oracle": ("Oracle Free", "#9d514b"),
}
operations = {"get": "Key lookup", "range100": "100-row range", "update": "Durable update"}


def main(raw):
    subprocess.run([sys.executable, str(Path(__file__).resolve().parents[1] / "validate.py"), str(raw)], check=True)
    records = json.loads((raw / "results.json").read_text())
    trials = json.loads((raw / "environment.json").read_text())["reps"]
    plt.rcParams.update({"svg.hashsalt": "catdb-grouped-writes", "font.size": 10})
    fig, axes = plt.subplots(2, 3, figsize=(13, 6.1), layout="constrained", sharey=True)
    summary = []
    for column, (operation, title) in enumerate(operations.items()):
        ceiling = 0
        for row, clients in enumerate([4, 16]):
            ax = axes[row, column]
            for index, (engine, (label, color)) in enumerate(products.items()):
                samples = [r for r in records if (r["engine"], r["workload"], r["clients"]) == (engine, operation, clients)]
                assert len(samples) == trials
                result = {"engine": engine, "workload": operation, "clients": clients}
                for metric in ["ops_s", "p50_ms", "p95_ms", "p99_ms"]:
                    values = [r[metric] for r in samples]
                    result[metric] = median(values)
                    result[metric + "_min"] = min(values)
                    result[metric + "_max"] = max(values)
                result["retries"] = sum(r["retries"] for r in samples)
                summary.append(result)
                low, center, high = (result[k] for k in ("ops_s_min", "ops_s", "ops_s_max"))
                ax.hlines(-index, low, high, color=color, linewidth=2)
                ax.plot(center, -index, "o", color=color, markersize=7)
                ceiling = max(ceiling, high)
            ax.set_title(f"{title} · {clients} clients")
            ax.set_yticks(range(0, -len(products), -1), [v[0] for v in products.values()])
            ax.grid(axis="x", alpha=.2)
            ax.spines[["right", "top"]].set_visible(False)
            ax.margins(y=.18)
        for ax in axes[:, column]:
            ax.set_xlim(0, ceiling * 1.06)
    fig.supxlabel(f"Completed operations/s   |   Dot = median of {trials} runs, line = minimum to maximum", fontsize=11)
    fig.savefig(raw.parent / "throughput.svg", metadata={"Date": None})
    plt.close(fig)
    with (raw.parent / "summary.csv").open("w", newline="", encoding="utf-8") as fp:
        writer = csv.DictWriter(fp, summary[0].keys())
        writer.writeheader()
        writer.writerows(summary)


if __name__ == "__main__":
    main(Path(sys.argv[1]))
