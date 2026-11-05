| Scenario | Clients | Offered/s | Mode | Updates/s | Response p99 ms | Worst response ms | Rejected / 24k | Updates/sync | Recovery ms |
|---|---:|---:|---|---:|---:|---:|---:|---:|---:|
| normal | 4 | 800 | candidate | 799.6 | 12.048 | 35.892 | 0 | 3.52 | — |
| normal | 4 | 800 | candidate-window | 799.5 | 9.290 | 17.861 | 0 | 3.43 | — |
| burst | 4 | 800 | candidate | 787.4 | 90.253 | 102.338 | 366 | 3.76 | 16.756 (3/3) |
| burst | 4 | 800 | candidate-window | 799.7 | 49.919 | 79.943 | 0 | 3.80 | 136.086 (3/3) |
