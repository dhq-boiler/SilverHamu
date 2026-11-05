| Workload | Clients | Variant | ops/s | p50 ms | p95 ms | p99 ms |
|---|---:|---|---:|---:|---:|---:|
| Key lookup | 1 | Cooperative | 9,320.4 | 0.098 | 0.146 | 0.173 |
| Key lookup | 1 | Background | 9,041.4 | 0.103 | 0.148 | 0.197 |
| Key lookup | 4 | Cooperative | 7,472.0 | 0.498 | 0.945 | 1.200 |
| Key lookup | 4 | Background | 7,164.7 | 0.516 | 1.008 | 1.284 |
| Read 100 consecutive records | 1 | Cooperative | 2,212.8 | 0.441 | 0.548 | 0.618 |
| Read 100 consecutive records | 1 | Background | 2,178.2 | 0.448 | 0.559 | 0.635 |
| Read 100 consecutive records | 4 | Cooperative | 3,294.7 | 1.164 | 1.921 | 2.314 |
| Read 100 consecutive records | 4 | Background | 3,220.3 | 1.184 | 1.899 | 2.290 |
| Replace one value + durable commit | 1 | Cooperative | 269.8 | 3.851 | 4.362 | 5.662 |
| Replace one value + durable commit | 1 | Background | 272.5 | 3.815 | 4.352 | 5.552 |
| Replace one value + durable commit | 4 | Cooperative | 1,037.2 | 3.905 | 4.632 | 5.697 |
| Replace one value + durable commit | 4 | Background | 1,028.0 | 3.940 | 4.681 | 5.591 |
