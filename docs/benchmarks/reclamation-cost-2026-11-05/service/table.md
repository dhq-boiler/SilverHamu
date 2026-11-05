| Workload | Clients | Variant | ops/s | p50 ms | p95 ms | p99 ms |
|---|---:|---|---:|---:|---:|---:|
| Key lookup | 1 | Previous background | 8,492.9 | 0.114 | 0.155 | 0.182 |
| Key lookup | 1 | Candidate | 8,892.4 | 0.109 | 0.156 | 0.217 |
| Key lookup | 4 | Previous background | 7,011.3 | 0.526 | 1.033 | 1.306 |
| Key lookup | 4 | Candidate | 6,985.4 | 0.527 | 1.025 | 1.337 |
| Read 100 consecutive records | 1 | Previous background | 2,072.7 | 0.469 | 0.594 | 0.662 |
| Read 100 consecutive records | 1 | Candidate | 2,073.8 | 0.469 | 0.595 | 0.670 |
| Read 100 consecutive records | 4 | Previous background | 3,210.2 | 1.185 | 1.924 | 2.322 |
| Read 100 consecutive records | 4 | Candidate | 3,186.9 | 1.194 | 1.942 | 2.313 |
| Replace one value + durable commit | 1 | Previous background | 271.4 | 3.815 | 4.342 | 5.531 |
| Replace one value + durable commit | 1 | Candidate | 270.6 | 3.822 | 4.383 | 5.523 |
| Replace one value + durable commit | 4 | Previous background | 1,021.6 | 3.951 | 4.681 | 5.546 |
| Replace one value + durable commit | 4 | Candidate | 1,003.7 | 3.999 | 4.845 | 5.887 |
