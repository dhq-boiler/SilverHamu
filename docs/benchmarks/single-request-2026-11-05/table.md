| Workload | Clients | Variant | ops/s | p50 ms | p95 ms | p99 ms |
|---|---:|---|---:|---:|---:|---:|
| Key lookup | 1 | Before / 3 requests | 3,000.9 | 0.322 | 0.420 | 0.492 |
| Key lookup | 1 | After / 3 requests | 3,101.4 | 0.308 | 0.428 | 0.526 |
| Key lookup | 1 | After / 1 request | 8,852.6 | 0.107 | 0.151 | 0.192 |
| Key lookup | 4 | Before / 3 requests | 2,490.4 | 1.567 | 2.271 | 2.629 |
| Key lookup | 4 | After / 3 requests | 2,620.6 | 1.495 | 2.148 | 2.478 |
| Key lookup | 4 | After / 1 request | 7,238.1 | 0.512 | 0.989 | 1.252 |
| Read 100 consecutive records | 1 | Before / 3 requests | 1,518.7 | 0.642 | 0.812 | 0.926 |
| Read 100 consecutive records | 1 | After / 3 requests | 1,454.6 | 0.675 | 0.831 | 0.940 |
| Read 100 consecutive records | 1 | After / 1 request | 2,321.7 | 0.419 | 0.537 | 0.625 |
| Read 100 consecutive records | 4 | Before / 3 requests | 1,763.9 | 2.223 | 3.084 | 3.473 |
| Read 100 consecutive records | 4 | After / 3 requests | 1,777.9 | 2.207 | 3.040 | 3.483 |
| Read 100 consecutive records | 4 | After / 1 request | 3,144.8 | 1.234 | 1.904 | 2.295 |
| Replace one value + durable commit | 1 | Before / 3 requests | 249.0 | 4.142 | 4.754 | 6.087 |
| Replace one value + durable commit | 1 | After / 3 requests | 250.0 | 4.115 | 4.771 | 5.902 |
| Replace one value + durable commit | 1 | After / 1 request | 265.6 | 3.885 | 4.475 | 5.824 |
| Replace one value + durable commit | 4 | Before / 3 requests | 794.1 | 5.015 | 6.020 | 7.334 |
| Replace one value + durable commit | 4 | After / 3 requests | 799.5 | 4.980 | 5.944 | 6.714 |
| Replace one value + durable commit | 4 | After / 1 request | 1,013.4 | 4.003 | 4.775 | 5.889 |
