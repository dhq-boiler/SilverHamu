| API | Clients | Workload | ops/s | p50 ms | p95 ms | p99 ms |
|---|---:|---|---:|---:|---:|---:|
| SilverHamu (C ABI) | 1 | Key lookup | 202,894.9 | 0.004 | 0.005 | 0.008 |
| SilverHamu (C ABI) | 4 | Key lookup | 15,927.3 | 0.193 | 0.660 | 0.965 |
| SQLite | 1 | Key lookup | 80,867.1 | 0.012 | 0.018 | 0.040 |
| SQLite | 4 | Key lookup | 7,539.7 | 0.503 | 1.021 | 1.288 |
| SilverHamu (C ABI) | 1 | Read 100 consecutive records | 16,811.6 | 0.055 | 0.075 | 0.194 |
| SilverHamu (C ABI) | 4 | Read 100 consecutive records | 8,673.7 | 0.419 | 0.941 | 1.371 |
| SQLite | 1 | Read 100 consecutive records | 14,138.0 | 0.066 | 0.097 | 0.116 |
| SQLite | 4 | Read 100 consecutive records | 561.8 | 7.104 | 10.154 | 11.622 |
| SilverHamu (C ABI) | 1 | Replace one value + durable commit | 303.5 | 3.398 | 3.920 | 5.143 |
| SilverHamu (C ABI) | 4 | Replace one value + durable commit | 787.4 | 5.768 | 6.424 | 8.133 |
| SQLite | 1 | Replace one value + durable commit | 596.5 | 1.270 | 3.289 | 3.570 |
| SQLite | 4 | Replace one value + durable commit | 583.1 | 1.290 | 3.344 | 3.846 |
| SilverHamu (1-request TCP) | 1 | Key lookup | 10,059.4 | 0.094 | 0.130 | 0.176 |
| SilverHamu (1-request TCP) | 4 | Key lookup | 8,046.5 | 0.461 | 0.890 | 1.147 |
| PostgreSQL | 1 | Key lookup | 3,131.3 | 0.308 | 0.407 | 0.497 |
| PostgreSQL | 4 | Key lookup | 2,553.9 | 1.523 | 2.304 | 2.696 |
| MySQL | 1 | Key lookup | 4,327.7 | 0.221 | 0.288 | 0.360 |
| MySQL | 4 | Key lookup | 2,435.8 | 1.589 | 2.534 | 3.049 |
| Oracle Free | 1 | Key lookup | 3,089.0 | 0.306 | 0.439 | 0.518 |
| Oracle Free | 4 | Key lookup | 6,026.3 | 0.636 | 0.946 | 1.123 |
| SilverHamu (1-request TCP) | 1 | Read 100 consecutive records | 2,448.5 | 0.398 | 0.499 | 0.580 |
| SilverHamu (1-request TCP) | 4 | Read 100 consecutive records | 3,612.9 | 1.072 | 1.668 | 1.982 |
| PostgreSQL | 1 | Read 100 consecutive records | 1,748.3 | 0.554 | 0.710 | 0.799 |
| PostgreSQL | 4 | Read 100 consecutive records | 1,843.1 | 2.122 | 3.026 | 3.491 |
| MySQL | 1 | Read 100 consecutive records | 1,430.9 | 0.684 | 0.827 | 0.938 |
| MySQL | 4 | Read 100 consecutive records | 194.2 | 20.549 | 25.300 | 27.492 |
| Oracle Free | 1 | Read 100 consecutive records | 1,764.2 | 0.551 | 0.694 | 0.780 |
| Oracle Free | 4 | Read 100 consecutive records | 3,027.6 | 1.286 | 1.813 | 2.111 |
| SilverHamu (1-request TCP) | 1 | Replace one value + durable commit | 294.2 | 3.546 | 4.024 | 5.198 |
| SilverHamu (1-request TCP) | 4 | Replace one value + durable commit | 1,133.3 | 3.588 | 4.223 | 5.078 |
| PostgreSQL | 1 | Replace one value + durable commit | 550.2 | 1.609 | 2.497 | 2.962 |
| PostgreSQL | 4 | Replace one value + durable commit | 1,297.9 | 3.147 | 3.739 | 5.547 |
| MySQL | 1 | Replace one value + durable commit | 164.7 | 5.979 | 8.080 | 10.121 |
| MySQL | 4 | Replace one value + durable commit | 374.2 | 10.309 | 15.045 | 19.081 |
| Oracle Free | 1 | Replace one value + durable commit | 525.9 | 1.701 | 2.613 | 3.166 |
| Oracle Free | 4 | Replace one value + durable commit | 1,280.0 | 3.226 | 3.763 | 5.371 |
