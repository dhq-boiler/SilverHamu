| Trial | First-second write p99 ms (200 requests) | Later write p99 ms (1,400 requests) | Before-call p99 ms | Before-call max ms | Call p99 ms | Call max ms |
|---|---:|---:|---:|---:|---:|---:|
| matched-1-explicit | 28.556 | 9.623 | 0.285 | 27.286 | 9.518 | 47.161 |
| matched-1-default | 7.471 | 7.937 | 0.210 | 0.961 | 7.755 | 11.640 |
| matched-2-default | 5.776 | 6.178 | 0.200 | 0.990 | 6.083 | 9.406 |
| matched-2-explicit | 5.589 | 5.979 | 0.153 | 3.433 | 5.815 | 6.827 |
| matched-3-explicit | 5.495 | 21.817 | 1.668 | 21.642 | 19.217 | 41.824 |
| matched-3-default | 6.463 | 7.466 | 0.165 | 0.750 | 7.374 | 18.871 |
| control-1-same-a | 4.479 | 6.896 | 0.160 | 0.238 | 6.655 | 13.254 |
| control-2-same-b | 9.211 | 10.728 | 0.189 | 5.179 | 10.331 | 25.387 |
| control-3-same-b | 7.564 | 6.359 | 0.197 | 0.490 | 6.731 | 13.187 |
| control-4-same-a | 8.772 | 8.234 | 0.169 | 0.264 | 8.214 | 12.482 |
