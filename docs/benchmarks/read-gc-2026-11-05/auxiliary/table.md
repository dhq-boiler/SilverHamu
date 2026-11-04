Median of three run-level values (maxima are medians of per-run maxima).

| Scenario | Variant | Read p99 ms | Read max ms | Write p99 ms | Write ops/s | Max lock hold ms | Drop ms | Drop + drain ms | Peak heap MiB | Extra heap MiB |
|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| mixed | before | 0.018 | 0.350 | 4.576 | 1126.679 | 0.283 | 0.000 | 0.000 | 95.620 | 50.896 |
| mixed | after | 0.012 | 0.373 | 4.548 | 1108.962 | 0.305 | 0.000 | 0.000 | 103.812 | 57.690 |
| large | before | 0.552 | 4.042 | 14.682 | 456.770 | 0.393 | 0.000 | 0.000 | 282.877 | 26.045 |
| large | after | 0.051 | 10.706 | 10.612 | 716.530 | 1.435 | 0.000 | 0.000 | 283.241 | 26.352 |
| gc | before | 0.038 | 26.164 | 0.000 | 0.000 | 26.062 | 26.086 | 26.086 | 76.097 | 0.001 |
| gc | after | 0.090 | 22.519 | 0.000 | 0.000 | 1.913 | 0.066 | 58.599 | 77.495 | 0.001 |
