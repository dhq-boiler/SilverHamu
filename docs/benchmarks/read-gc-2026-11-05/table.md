| Workload | Clients | Variant | ops/s | p50 ms | p95 ms | p99 ms |
|---|---:|---|---:|---:|---:|---:|
| Key lookup | 1 | Before | 8,376.1 | 0.115 | 0.159 | 0.192 |
| Key lookup | 1 | After | 8,081.6 | 0.120 | 0.161 | 0.187 |
| Key lookup | 4 | Before | 6,997.6 | 0.524 | 1.029 | 1.307 |
| Key lookup | 4 | After | 6,814.5 | 0.536 | 1.062 | 1.409 |
| Read 100 consecutive records | 1 | Before | 2,182.4 | 0.447 | 0.557 | 0.617 |
| Read 100 consecutive records | 1 | After | 2,104.3 | 0.465 | 0.578 | 0.668 |
| Read 100 consecutive records | 4 | Before | 3,082.2 | 1.241 | 1.959 | 2.386 |
| Read 100 consecutive records | 4 | After | 3,185.1 | 1.199 | 1.924 | 2.318 |
| Replace one value + durable commit | 1 | Before | 264.4 | 3.910 | 4.455 | 5.741 |
| Replace one value + durable commit | 1 | After | 264.5 | 3.934 | 4.450 | 5.752 |
| Replace one value + durable commit | 4 | Before | 990.8 | 4.071 | 4.906 | 5.997 |
| Replace one value + durable commit | 4 | After | 981.2 | 4.106 | 4.934 | 5.991 |
