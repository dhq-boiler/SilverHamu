import gzip, importlib.util, json, statistics as stats, sys
from pathlib import Path
probe = Path(__file__).resolve().parents[1] / 'commit-breakdown' / 'report.py'
spec = importlib.util.spec_from_file_location('decomp', probe)
decomp = importlib.util.module_from_spec(spec)
spec.loader.exec_module(decomp)
root = Path(sys.argv[1])
results = json.loads((root / 'results.json').read_text())
all_metrics = []
for row in results:
    raw = json.load(gzip.open(root / row['profile_file'],'rt'))
    assert raw['dropped'] == 0 and len(raw['records']) == raw['limit'] == row['completed']
    stages = [decomp.phases(record) for record in raw['records']]
    groups = {}
    for record, split in zip(raw['records'], stages):
        identity = tuple(record['diagnostic']['trace'][1:])
        groups.setdefault(identity,[]).append(split)
    def average_stage(name):
        return stats.mean(s[name] for s in stages)/1e6
    syncs = [traces[0]['log sync']/1e6 for traces in groups.values()]
    from collections import Counter
    duration = row['elapsed_s'] * 1000
    groups_nonoverlap = sorted((g[4],g[5]) for g in groups)
    assert all(start < end for start,end in groups_nonoverlap)
    assert all(left[1] <= right[0] for left,right in zip(groups_nonoverlap,groups_nonoverlap[1:]))
    metrics = {
        'rep': row['rep'], 'clients': row['clients'], 'ops_s':row['ops_s'],
        'syncs': len(groups), 'updates_per_sync': len(stages) / len(groups),
        'group_size_histogram': dict(sorted(Counter(len(s) for s in groups.values()).items())),
        'sync_ms_per_group': stats.mean(syncs),
        'sync_p99_ms_per_group': decomp.percentile(syncs,.99),
        'percent_wall_in_sync':100*sum(syncs)/duration,
        'response_mean_ms':stats.mean(record['request_ns'] for record in raw['records'])/1e6,
        'phases_ms_per_request':{name:average_stage(name) for name in stages[0]},
    }
    assert 0 <= metrics['percent_wall_in_sync'] <= 100
    all_metrics.append(metrics)
print(json.dumps(all_metrics,indent=2))
