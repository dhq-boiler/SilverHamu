Medians of three runs, except worst read is the maximum across all repeats.

| Scenario | Variant | Read p99 ms | Worst read across repeats ms | Write p99 ms | Write ops/s | Complete GC ms | Max hold ms | Peak heap MiB | Drained heap MiB | Drained retained MiB |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| gc | before | 0.022 | 0.942 | 0.000 | 0.000 | 352.366 | 0.089 | 108.013 | 76.641 | 50.354 |
| gc | after | 0.035 | 1.186 | 0.000 | 0.000 | 116.228 | 0.099 | 108.013 | 76.641 | 50.354 |
| gc | before-light | 0.023 | 0.902 | 0.000 | 0.000 | 244.828 | 0.096 | 108.013 | 76.641 | 50.354 |
| gc | after-light | 0.037 | 1.423 | 0.000 | 0.000 | 97.130 | 0.103 | 108.013 | 76.641 | 50.354 |
