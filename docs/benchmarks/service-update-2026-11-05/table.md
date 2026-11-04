| Workload | Clients | Variant | ops/s | p50 ms | p95 ms | p99 ms |
|---|---:|---|---:|---:|---:|---:|
| Key lookup | 1 | Before | 2,984.7 | 0.324 | 0.443 | 0.514 |
| Key lookup | 1 | After | 3,047.7 | 0.319 | 0.428 | 0.510 |
| Key lookup | 4 | Before | 2,679.2 | 1.466 | 2.089 | 2.418 |
| Key lookup | 4 | After | 2,651.5 | 1.468 | 2.145 | 2.442 |
| Read 100 consecutive records | 1 | Before | 1,528.7 | 0.643 | 0.792 | 0.883 |
| Read 100 consecutive records | 1 | After | 1,517.6 | 0.646 | 0.801 | 0.892 |
| Read 100 consecutive records | 4 | Before | 1,883.5 | 2.085 | 2.887 | 3.307 |
| Read 100 consecutive records | 4 | After | 1,878.2 | 2.101 | 2.888 | 3.318 |
| Replace one value + durable commit | 1 | Before | 266.6 | 3.845 | 4.485 | 5.544 |
| Replace one value + durable commit | 1 | After | 264.1 | 3.866 | 4.517 | 5.516 |
| Replace one value + durable commit | 4 | Before | 324.1 | 12.411 | 14.130 | 16.984 |
| Replace one value + durable commit | 4 | After | 629.9 | 6.246 | 7.000 | 8.827 |
