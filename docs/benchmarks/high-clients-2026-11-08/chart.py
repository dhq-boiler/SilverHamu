"""Plot the four-concurrency durable update comparison from the measured cases."""

import argparse
import json
import math
from pathlib import Path
from statistics import median

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
from matplotlib.patches import Patch
from matplotlib.ticker import MaxNLocator, StrMethodFormatter


HERE = Path(__file__).resolve().parent
CLIENTS = (4, 16, 32, 64)
SERIES = {
    "catdb-service": ("silverhamu", "#166A68", ""),
    "oracle": ("Oracle 26ai Free", "#A35136", "///"),
    "postgres": ("PostgreSQL 18.6", "#496786", ".."),
}
INK = "#182D36"
MUTED = "#485E66"


def read_cases(source):
    groups = {}
    for case in json.loads(source.read_text(encoding="utf-8")):
        key = (case["engine"], case["clients"], case["rep"])
        if key in groups:
            raise ValueError(f"Duplicate case {key}")
        if (
            case["workload"] != "update"
            or case["rows"] != 100_000
            or case["value_bytes"] != 256
            or case["attempted"] != 8_000
            or case["completed"] != case["attempted"]
            or case["errors"]
        ):
            raise ValueError(f"Incomplete or unexpected workload in {key}")
        if any(
            not math.isfinite(case[metric]) or case[metric] <= 0
            for metric in ("ops_s", "p99_ms")
        ):
            raise ValueError(f"Invalid metric in {key}")
        groups[key] = case
    expected = {
        (engine, clients, rep)
        for engine in SERIES
        for clients in CLIENTS
        for rep in (1, 2, 3)
    }
    if set(groups) != expected:
        raise ValueError(f"Missing {expected - set(groups)}; unexpected {set(groups) - expected}")
    return groups


def draw_panel(ax, cases, metric, title, direction, fmt):
    largest = 0
    for offset, (engine, (_, color, hatch)) in enumerate(SERIES.items(), start=-1):
        y = [
            median(cases[(engine, clients, rep)][metric] for rep in (1, 2, 3))
            for clients in CLIENTS
        ]
        largest = max(largest, *y)
        x = [group + 0.24 * offset for group in range(len(CLIENTS))]
        bars = ax.bar(x, y, width=0.205, color=color, hatch=hatch, edgecolor="white",
                      linewidth=0.4, zorder=2)
        ax.bar_label(bars, labels=[format(value, fmt) for value in y], padding=3,
                     fontsize=10, color=INK, fontweight="bold" if offset == -1 else "normal")
    ax.set_title(title, loc="left", fontsize=15, fontweight="bold", color=INK, pad=11)
    ax.set_title(direction, loc="right", fontsize=10.2, color=MUTED, pad=12)
    ax.set_xlim(-0.5, len(CLIENTS) - 0.5)
    ax.set_ylim(0, largest * 1.17)
    ax.set_xticks(range(len(CLIENTS)), [f"{count} clients" for count in CLIENTS])
    ax.tick_params(axis="x", labelsize=11, colors=INK, pad=5, length=0)
    ax.yaxis.set_major_locator(MaxNLocator(nbins=4, min_n_ticks=3, integer=True))
    ax.yaxis.set_major_formatter(StrMethodFormatter("{x:,.0f}"))
    ax.tick_params(axis="y", labelsize=9, colors=MUTED, length=0, pad=5)
    ax.grid(axis="y", color="#DAE2E3", linewidth=0.8, zorder=0)
    ax.spines[["top", "right", "left"]].set_visible(False)
    ax.spines["bottom"].set_color("#87979B")


def plot(cases, output):
    plt.rcParams.update({"font.family": "DejaVu Sans", "hatch.linewidth": 0.65})
    fig, axes = plt.subplots(2, 1, figsize=(10.0, 8.0))
    fig.patch.set_facecolor("white")
    fig.subplots_adjust(top=0.759, bottom=0.116, left=0.085, right=0.975, hspace=0.57)
    fig.text(0.045, 0.948, "Synchronous single-key updates", fontsize=24,
             color=INK, fontweight="bold", va="center")
    fig.text(0.047, 0.897,
             "100,000 keys  ·  256-byte values  ·  8,000 updates per run  ·  Median of 3 runs",
             fontsize=11, color=MUTED, va="center")
    legend = [Patch(facecolor=color, edgecolor="white", hatch=hatch, label=name)
              for name, color, hatch in SERIES.values()]
    fig.legend(handles=legend, loc="center left", bbox_to_anchor=(0.045, 0.843),
               ncol=3, frameon=False, handlelength=1.6, handleheight=1.2, handletextpad=0.45,
               columnspacing=1.5, borderaxespad=0, fontsize=11, labelcolor=INK)
    draw_panel(axes[0], cases, "ops_s", "Throughput (updates/s)", "Higher is better", ",.0f")
    draw_panel(axes[1], cases, "p99_ms", "p99 latency (ms)", "Lower is better", ".2f")
    fig.text(0.047, 0.035,
             "2 CPUs and 4 GiB each for client and server  ·  silverhamu format 4, 64 MiB preallocated WAL",
             fontsize=10, color=MUTED, va="center")
    fig.savefig(output, dpi=160, facecolor="white")
    plt.close(fig)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, nargs="?", default=HERE / "comparison/results.json")
    parser.add_argument("output", type=Path, nargs="?", default=HERE / "comparison.png")
    args = parser.parse_args()
    plot(read_cases(args.source), args.output)
