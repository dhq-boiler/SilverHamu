| API | Clients | Workload | ops/s | p50 ms | p95 ms | p99 ms |
|---|---:|---|---:|---:|---:|---:|
| SilverHamu (C ABI) | 1 | Key lookup | 190,940.9 | 0.005 | 0.006 | 0.009 |
| SilverHamu (C ABI) | 4 | Key lookup | 14,549.8 | 0.202 | 0.744 | 1.068 |
| SQLite | 1 | Key lookup | 85,105.6 | 0.012 | 0.018 | 0.026 |
| SQLite | 4 | Key lookup | 7,440.4 | 0.498 | 1.080 | 1.345 |
| SilverHamu (C ABI) | 1 | Read 100 consecutive records | 19,101.6 | 0.050 | 0.065 | 0.079 |
| SilverHamu (C ABI) | 4 | Read 100 consecutive records | 8,060.2 | 0.411 | 1.086 | 1.616 |
| SQLite | 1 | Read 100 consecutive records | 11,454.6 | 0.073 | 0.131 | 0.177 |
| SQLite | 4 | Read 100 consecutive records | 590.1 | 6.737 | 9.821 | 11.257 |
| SilverHamu (C ABI) | 1 | Replace one value + durable commit | 292.9 | 3.561 | 4.094 | 5.269 |
| SilverHamu (C ABI) | 4 | Replace one value + durable commit | 647.4 | 6.175 | 7.117 | 8.937 |
| SQLite | 1 | Replace one value + durable commit | 554.9 | 1.367 | 3.516 | 3.881 |
| SQLite | 4 | Replace one value + durable commit | 540.0 | 1.400 | 3.528 | 4.085 |
| SilverHamu (TCP) | 1 | Key lookup | 2,858.9 | 0.344 | 0.440 | 0.520 |
| SilverHamu (TCP) | 4 | Key lookup | 2,649.2 | 1.478 | 2.119 | 2.467 |
| PostgreSQL | 1 | Key lookup | 2,602.7 | 0.377 | 0.482 | 0.546 |
| PostgreSQL | 4 | Key lookup | 2,504.6 | 1.542 | 2.397 | 2.821 |
| MySQL | 1 | Key lookup | 3,610.8 | 0.269 | 0.353 | 0.424 |
| MySQL | 4 | Key lookup | 2,223.3 | 1.726 | 2.828 | 3.382 |
| Oracle Free | 1 | Key lookup | 2,943.0 | 0.335 | 0.439 | 0.521 |
| Oracle Free | 4 | Key lookup | 5,671.0 | 0.674 | 1.018 | 1.235 |
| SilverHamu (TCP) | 1 | Read 100 consecutive records | 1,513.7 | 0.648 | 0.804 | 0.904 |
| SilverHamu (TCP) | 4 | Read 100 consecutive records | 1,885.6 | 2.092 | 2.866 | 3.244 |
| PostgreSQL | 1 | Read 100 consecutive records | 1,525.6 | 0.641 | 0.793 | 0.906 |
| PostgreSQL | 4 | Read 100 consecutive records | 1,783.7 | 2.188 | 3.177 | 3.646 |
| MySQL | 1 | Read 100 consecutive records | 1,285.7 | 0.760 | 0.942 | 1.048 |
| MySQL | 4 | Read 100 consecutive records | 183.3 | 21.666 | 26.792 | 29.124 |
| Oracle Free | 1 | Read 100 consecutive records | 1,594.9 | 0.617 | 0.755 | 0.841 |
| Oracle Free | 4 | Read 100 consecutive records | 2,693.3 | 1.439 | 2.061 | 2.405 |
| SilverHamu (TCP) | 1 | Replace one value + durable commit | 265.3 | 3.875 | 4.468 | 5.423 |
| SilverHamu (TCP) | 4 | Replace one value + durable commit | 335.1 | 12.374 | 14.153 | 16.319 |
| PostgreSQL | 1 | Replace one value + durable commit | 508.4 | 1.752 | 2.701 | 3.572 |
| PostgreSQL | 4 | Replace one value + durable commit | 1,297.7 | 3.137 | 3.739 | 5.124 |
| MySQL | 1 | Replace one value + durable commit | 155.3 | 6.299 | 8.527 | 11.926 |
| MySQL | 4 | Replace one value + durable commit | 355.4 | 10.687 | 16.139 | 26.799 |
| Oracle Free | 1 | Replace one value + durable commit | 484.2 | 1.864 | 2.844 | 3.931 |
| Oracle Free | 4 | Replace one value + durable commit | 1,195.7 | 3.416 | 4.088 | 5.645 |
