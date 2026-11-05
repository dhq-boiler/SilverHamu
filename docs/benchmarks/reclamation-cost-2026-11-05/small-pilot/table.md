Medians of three runs, except worst read is the maximum across all repeats.

| Scenario | Variant | Read p99 ms | Worst read across repeats ms | Write p99 ms | Write ops/s | Complete GC ms | Max hold ms | Peak heap MiB | Drained heap MiB | Drained retained MiB |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| gc | before | 0.025 | 1.672 | 0.000 | 0.000 | 314.837 | 0.200 | 108.013 | 76.641 | 50.354 |
| gc | after | 0.030 | 0.821 | 0.000 | 0.000 | 237.064 | 0.212 | 108.013 | 76.641 | 50.354 |
| gc | before-light | 0.021 | 1.403 | 0.000 | 0.000 | 216.762 | 0.094 | 108.013 | 76.641 | 50.354 |
| gc | after-light | 0.031 | 0.650 | 0.000 | 0.000 | 163.778 | 0.067 | 108.013 | 76.641 | 50.354 |
