| Workload | Clients | Variant | ops/s | p50 ms | p95 ms | p99 ms |
|---|---:|---|---:|---:|---:|---:|
| Key lookup | 1 | Previous background | 8,801.5 | 0.107 | 0.148 | 0.206 |
| Key lookup | 1 | Candidate | 8,595.0 | 0.112 | 0.155 | 0.201 |
| Key lookup | 4 | Previous background | 7,235.3 | 0.510 | 0.998 | 1.316 |
| Key lookup | 4 | Candidate | 7,145.0 | 0.516 | 1.018 | 1.306 |
| Read 100 consecutive records | 1 | Previous background | 2,067.4 | 0.470 | 0.593 | 0.678 |
| Read 100 consecutive records | 1 | Candidate | 2,202.6 | 0.443 | 0.555 | 0.622 |
| Read 100 consecutive records | 4 | Previous background | 3,270.9 | 1.161 | 1.878 | 2.262 |
| Read 100 consecutive records | 4 | Candidate | 3,319.4 | 1.176 | 1.802 | 2.167 |
| Replace one value + durable commit | 1 | Previous background | 271.7 | 3.783 | 4.435 | 5.618 |
| Replace one value + durable commit | 1 | Candidate | 273.0 | 3.789 | 4.345 | 5.643 |
| Replace one value + durable commit | 4 | Previous background | 1,027.6 | 3.907 | 4.745 | 5.706 |
| Replace one value + durable commit | 4 | Candidate | 998.7 | 4.036 | 4.839 | 5.903 |
