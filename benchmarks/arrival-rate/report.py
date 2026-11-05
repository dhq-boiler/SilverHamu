"""Validate offered arrivals and correlate queue-inclusive tails with commit phases."""
import gzip
import importlib.util
import json
import math
import shutil
import statistics as st
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('commit_phases', HERE.parent / 'commit-breakdown/report.py')
phase_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(phase_module)

def quantile(xs, p):
    return sorted(xs)[max(0, math.ceil(len(xs) * p) - 1)]

def validate_events(run, events):
    assert len(events) == run['attempted']
    origin = run['schedule']['origin_ns']
    for i, e in enumerate(events):
        assert e['id'] == i and e['worker'] == i % run['clients']
        assert e['sequence'] == i // run['clients']
        assert e['planned_ns'] == origin + i * 1_000_000_000 // run['rate']
        assert e['planned_ns'] <= e['admitted_ns'] <= e['finished_ns']
        assert e['status'] in ('completed', 'rejected', 'failed')
        if e['status'] != 'rejected':
            assert e['admitted_ns'] <= e['sent_ns'] <= e['finished_ns']
    assert sum(e['status'] == 'completed' for e in events) == run['completed']
    assert sum(e['status'] == 'rejected' for e in events) == run['rejected']
    assert sum(e['status'] == 'failed' for e in events) == len(run['errors'])
    for worker in range(run['clients']):
        sent = [e for e in events if e['worker'] == worker and 'sent_ns' in e]
        assert all(a['finished_ns'] <= b['sent_ns'] for a, b in zip(sent, sent[1:]))
    counters = run['schedule']['counters']
    assert int(counters['after']['version']) - int(counters['before']['version']) == run['completed']
    assert counters['after']['sync_attempts'] - counters['before']['sync_attempts'] == run['sync_attempts']
    assert all(n <= run['schedule']['queue_capacity'] // run['clients'] for n in run['schedule']['queue_high_water'])

def attribute(run, events, raw):
    assert raw['limit'] == run['attempted'] and raw['dropped'] == 0
    seen = set()
    resolved = []
    for row in raw['records']:
        assert row['id'] not in seen
        seen.add(row['id'])
        e = events[row['id']]
        assert e['status'] == 'completed'
        assert (e['worker'], e['sequence']) == (row['worker'], row['sequence'])
        assert row['operation_ns'] == e['finished_ns'] - e['sent_ns']
        assert row['response_ns'] == e['finished_ns'] - e['planned_ns']
        assert not row.get('probe_error') and row['diagnostic']['commit_ok']
        assert row['request_index'] == row['diagnostic']['request_index']
        phases = phase_module.phases(row)
        phases['outside protocol request'] = row['operation_ns'] - row['request_ns']
        phases['producer scheduling delay'] = e['admitted_ns'] - e['planned_ns']
        phases['client queue'] = e['sent_ns'] - e['admitted_ns']
        assert all(x >= 0 for x in phases.values())
        assert sum(phases.values()) == row['response_ns']
        resolved.append({**row, 'phases': phases, 'largest_phase': max(phases, key=phases.get)})
    completed = [e for e in events if e['status'] == 'completed']
    for worker in range(run['clients']):
        subset = [e for e in completed if e['worker'] == worker]
        for start in ['planned_ns', 'sent_ns']:
            maximum = max(subset, key=lambda e: e['finished_ns'] - e[start])
            assert maximum['id'] in seen
    assert all(e['id'] in seen for e in completed if e['finished_ns'] - e['planned_ns'] >= 10_000_000)
    return resolved

def cadence(run, events, width_ns):
    origin = run['schedule']['origin_ns']
    end = max(e['finished_ns'] for e in events)
    count = (end - origin) // width_ns + 1
    bins = [{'begin_s': i * width_ns / 1e9, 'planned': 0, 'admitted': 0, 'sent': 0, 'finished': 0,
             'completed': 0, 'rejected': 0, 'failed': 0} for i in range(count)]
    for e in events:
        for name in ['planned', 'admitted', 'sent', 'finished']:
            if name + '_ns' in e:
                bins[(e[name + '_ns'] - origin) // width_ns][name] += 1
        bins[(e['finished_ns'] - origin) // width_ns][e['status']] += 1
    return bins

def main():
    source, out = map(Path, sys.argv[1:3])
    out.mkdir(parents=True, exist_ok=True)
    rows = json.loads((source / 'results.json').read_text())
    env = json.loads((source / 'environment.json').read_text())
    assert len(rows) == 36 and len(env['runs']) == 12
    assert all(r['status'] == 'passed' for r in env['runs'])
    assert sum(len(r['checks']) for r in env['runs']) == 60
    for f in source.iterdir():
        if f.is_file():
            shutil.copy2(f, out / f.name)
    trials, traces = [], []
    for r in rows:
        events = json.load(gzip.open(source / r['events_file']))
        validate_events(r, events)
        assert r['attempted'] == 8000 and not r['errors']
        complete = [e for e in events if e['status'] == 'completed']
        t = {k: r[k] for k in ['variant', 'clients', 'rate', 'rep', 'ops_s', 'syncs_s', 'sync_attempts', 'updates_per_sync', 'rejected', 'completed', 'response', 'operation', 'queue']}
        sent = [e['sent_ns'] for e in complete]
        t['mean_send_rate'] = (len(sent) - 1) * 1e9 / (max(sent) - min(sent))
        t['producer_p99_ms'] = quantile([e['admitted_ns'] - e['planned_ns'] for e in events], .99) / 1e6
        t['client_queue_p99_ms'] = quantile([e['sent_ns'] - e['admitted_ns'] for e in complete], .99) / 1e6
        maximum = max(complete, key=lambda e: e['finished_ns'] - e['planned_ns'])
        t['response_maximum_event'] = {**maximum,
            'producer_ms': (maximum['admitted_ns'] - maximum['planned_ns']) / 1e6,
            'client_queue_ms': (maximum['sent_ns'] - maximum['admitted_ns']) / 1e6,
            'operation_ms': (maximum['finished_ns'] - maximum['sent_ns']) / 1e6}
        for width in [100_000_000, 1_000_000_000]:
            bins = cadence(r, events, width)
            name = f"{r['variant']}-r{r['rep']}-c{r['clients']}-rate{r['rate']}-bins{width//1_000_000}ms.json.gz"
            with gzip.open(out / name, 'wt', encoding='utf-8') as f:
                json.dump(bins, f)
            full = [b['sent'] for b in bins if b['begin_s'] + width / 1e9 <= (r['attempted'] - 1) / r['rate']]
            t[f'sends_per_{width//1_000_000}ms'] = {'min': min(full), 'max': max(full), 'median': st.median(full)}
        if 'profile_file' in r:
            resolved = attribute(r, events, json.load(gzip.open(source / r['profile_file'])))
            t['probe_count'] = len(resolved)
            t['probe_total_ms'] = sum(x['probe_ns'] for x in resolved) / 1e6
            t['maximum_record'] = max(resolved, key=lambda x: x['response_ns'])
            slow = [x for x in resolved if x['response_ns'] > 10_000_000]
            t['tail_largest_phase'] = {name: sum(x['largest_phase'] == name for x in slow) for name in sorted({x['largest_phase'] for x in slow})}
            traces.extend({'variant': r['variant'], 'clients': r['clients'], 'rate': r['rate'], 'rep': r['rep'], **x} for x in resolved)
        trials.append(t)
    summary = []
    for c, rate in [(1, 200), (1, 250), (4, 400), (4, 800)]:
        for v in ['before', 'candidate', 'before-light', 'candidate-light']:
            group = [t for t in trials if (t['clients'], t['rate'], t['variant']) == (c, rate, v)]
            if not group:
                continue
            assert len(group) == 3
            s = {'clients': c, 'rate': rate, 'variant': v, 'rejected': sum(t['rejected'] for t in group)}
            for metric in ['ops_s', 'syncs_s', 'updates_per_sync', 'mean_send_rate', 'producer_p99_ms', 'client_queue_p99_ms']:
                vals = [t[metric] for t in group]
                s[metric] = dict(median=st.median(vals), min=min(vals), max=max(vals))
            for kind in ['response', 'operation', 'queue']:
                s[kind] = {}
                for metric in ['p99_ms', 'max_ms']:
                    vals = [t[kind][metric] for t in group]
                    s[kind][metric] = dict(median=st.median(vals), min=min(vals), max=max(vals))
                s[kind]['above_ms'] = {str(n): sum(t[kind]['above_ms'][str(n)] for t in group) for n in [10, 20, 50]}
            summary.append(s)
    (out / 'summary.json').write_text(json.dumps({'summary': summary, 'trials': trials}, indent=2) + '\n', encoding='utf-8')
    with gzip.open(out / 'attributed-records.json.gz', 'wt', encoding='utf-8') as f:
        json.dump(traces, f)
    table = ['| Clients | Offered/s | Mode | Completed/s | Syncs/s | Updates/sync | Response p99 ms | Worst response ms | Operation p99 ms | Worst operation ms | >50ms responses / 24k | Rejected |', '|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|']
    for s in summary:
        table.append(f"| {s['clients']} | {s['rate']} | {s['variant']} | {s['ops_s']['median']:.1f} | {s['syncs_s']['median']:.1f} | {s['updates_per_sync']['median']:.2f} | {s['response']['p99_ms']['median']:.3f} | {s['response']['max_ms']['max']:.3f} | {s['operation']['p99_ms']['median']:.3f} | {s['operation']['max_ms']['max']:.3f} | {s['response']['above_ms']['50']} | {s['rejected']} |")
    (out / 'table.md').write_text('\n'.join(table) + '\n', encoding='utf-8')
    render(summary, trials, out)
    print('\n'.join(table))

def render(summary, trials, out):
    import matplotlib
    matplotlib.use('Agg')
    import matplotlib.pyplot as plt
    cases = [(1, 200), (1, 250), (4, 400), (4, 800)]
    colors = ['#667085', '#007f85']
    fig, axes = plt.subplots(2, 2, figsize=(12, 8), layout='constrained')
    panels = [(lambda s: s['response']['p99_ms'], 'Queue-inclusive response p99 (ms)'), (lambda s: s['response']['max_ms'], 'Per-run response maximum (ms)'), (lambda s: s['syncs_s'], 'Synchronization attempts / second'), (lambda s: s['operation']['p99_ms'], 'Send-start to completion p99 (ms)')]
    for ax, (select, title) in zip(axes.flat, panels):
        for j, v in enumerate(['before', 'candidate']):
            data = [select(next(s for s in summary if (s['clients'], s['rate'], s['variant']) == (c, r, v))) for c, r in cases]
            xs = [i + (j - .5) * .32 for i in range(4)]
            ax.bar(xs, [d['median'] for d in data], .30, label=v, color=colors[j], yerr=[[d['median'] - d['min'] for d in data], [d['max'] - d['median'] for d in data]], capsize=3)
        ax.set_xticks(range(4), [f'{c} conn.\n{r}/s' for c, r in cases])
        ax.set_title(title)
        ax.spines[['top', 'right']].set_visible(False)
    axes[0, 0].legend()
    fig.suptitle('Equal offered load: skipping sole-client collection wait')
    fig.supxlabel('Commit tracing OFF; median and range of 3 runs. 100k rows / 256B / 8,000 requests.\nFixed deadlines; 64 pending slots; actual sends can lag. Sync counters read outside the timed workload.', fontsize=9)
    for ext in ['png', 'svg']:
        fig.savefig(out / f'arrival-rate.{ext}', dpi=140)
    plt.close(fig)
    fig, axes = plt.subplots(1, 2, figsize=(12, 5.5), layout='constrained')
    for ax, clients, rate in zip(axes, [1, 4], [250, 800]):
        selected = [t for t in trials if t['clients'] == clients and t['rate'] == rate and 'maximum_record' in t]
        selected.sort(key=lambda t: (t['variant'], t['rep']))
        buckets = {'Producer + client queue': ['producer scheduling delay', 'client queue'], 'Log sync': ['log sync'], 'Other operation time': []}
        bottom = [0.] * len(selected)
        for name, color in zip(buckets, ['#e0a438', '#1b6d85', '#b4b9c2']):
            vals = []
            for t in selected:
                x = t['maximum_record']
                if name == 'Other operation time':
                    ns = x['operation_ns'] - x['phases']['log sync']
                else:
                    ns = sum(x['phases'][n] for n in buckets[name])
                vals.append(ns / 1e6)
            ax.bar(range(len(selected)), vals, bottom=bottom, label=name, color=color)
            bottom = [a + b for a, b in zip(bottom, vals)]
        ax.set_xticks(range(len(selected)), [('B' if t['variant'] == 'before-light' else 'C') + f" r{t['rep']}" for t in selected])
        ax.set_title(f'{clients} connection(s), {rate} offered/s')
        ax.set_ylabel('Milliseconds')
        ax.spines[['top', 'right']].set_visible(False)
    axes[0].legend(fontsize=8)
    fig.suptitle('The same maximum-response request: queue and operation time')
    fig.supxlabel('Separate observer cohorts, not primary maxima. B = before; C = candidate.\nProbe time is outside the sampled operation but delays later requests; sync elapsed includes scheduling.', fontsize=9)
    for ext in ['png', 'svg']:
        fig.savefig(out / f'maximum-breakdown.{ext}', dpi=140)
    plt.close(fig)
    primary = [t for t in trials if not t['variant'].endswith('light')]
    worst = max(primary, key=lambda t: t['response']['max_ms'])
    stem = f"{worst['variant']}-r{worst['rep']}"
    events = json.load(gzip.open(out / f"{stem}-update-c{worst['clients']}-rate{worst['rate']}-events.json.gz"))
    origin = events[0]['planned_ns']
    maximum = worst['response_maximum_event']
    left = max(0, (maximum['planned_ns'] - origin) / 1e9 - 2)
    right = (maximum['finished_ns'] - origin) / 1e9 + 1
    bins = json.load(gzip.open(out / f"{stem}-c{worst['clients']}-rate{worst['rate']}-bins100ms.json.gz"))
    bins = [b for b in bins if left <= b['begin_s'] <= right]
    fig, axes = plt.subplots(2, 1, figsize=(11, 7), layout='constrained')
    for field, color in [('planned', '#667085'), ('sent', '#007f85'), ('rejected', '#bc4141')]:
        axes[0].step([b['begin_s'] for b in bins], [b[field] for b in bins], where='post', label=field, color=color)
    axes[0].set_ylabel('Requests per 100 ms')
    axes[0].set_xlabel('Client-clock seconds from first planned arrival')
    chosen = [e for e in events if e['status'] == 'completed' and left <= (e['planned_ns'] - origin) / 1e9 <= right]
    xs = [(e['planned_ns'] - origin) / 1e9 for e in chosen]
    axes[1].plot(xs, [(e['sent_ns'] - e['planned_ns']) / 1e6 for e in chosen], label='Scheduled-to-send wait', color='#e0a438')
    axes[1].plot(xs, [(e['finished_ns'] - e['sent_ns']) / 1e6 for e in chosen], label='Send-to-finish operation', color='#1b6d85')
    axes[1].set_ylabel('Milliseconds')
    axes[1].set_xlabel('Request scheduled time (seconds from first planned arrival)')
    for ax in axes:
        ax.legend(fontsize=9)
        ax.spines[['top','right']].set_visible(False)
    fig.suptitle(f"Worst primary response: {worst['variant']} r{worst['rep']}, {worst['clients']} connection(s), {worst['rate']}/s")
    fig.supxlabel('Commit tracing OFF. Queue-inclusive maximum and nearby scheduled requests.\nSend starts are client-side timestamps, not server receipts or sync completions; no sync cause is inferred.', fontsize=9)
    for ext in ['png', 'svg']:
        fig.savefig(out / f'backlog.{ext}', dpi=140)
    plt.close(fig)
    for svg in out.glob('*.svg'):
        svg.write_text('\n'.join(line.rstrip() for line in svg.read_text().splitlines()) + '\n')

if __name__ == '__main__':
    main()
