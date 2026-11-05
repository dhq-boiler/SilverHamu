| Connections | Variant | Successful ops/s | p99 ms | Worst run maximum ms |
|---:|---|---:|---:|---:|
| 1 | baseline | 268.8 | 5.615 | 85.026 |
| 1 | direct | 270.6 | 5.703 | 28.004 |
| 1 | prepare | 262.0 | 5.850 | 46.168 |
| 4 | baseline | 1011.1 | 5.910 | 13.680 |
| 4 | direct | 1015.5 | 5.861 | 26.283 |
| 4 | prepare | 983.6 | 6.089 | 13.855 |

Auxiliary Windows engine runs: instrumentation enabled, four barrier-synchronized writers. Medians of three trials. Not directly comparable to the service chart.

| Shape | Variant | Accepted ops/s | Accepted p99 ms | Writer preparation p99 µs | Encode allocations | Encode bytes | Discarded preparation bytes |
|---|---|---:|---:|---:|---:|---:|---:|
| small | baseline | 2621.5 | 2.295 | 120.1 | 5 | 648 | 0 |
| small | direct | 2605.7 | 2.448 | 84.9 | 1 | 312 | 0 |
| small | prepare | 2572.0 | 3.058 | 91.7 | 1 | 312 | 0 |
| large | baseline | 1627.9 | 4.185 | 1523.2 | 5 | 131208 | 0 |
| large | direct | 1530.3 | 4.462 | 1384.7 | 1 | 65592 | 0 |
| large | prepare | 1587.3 | 4.670 | 2101.0 | 1 | 65592 | 0 |
| hot | baseline | 682.6 | 2.201 | 19.2 | 5 | 648 | 0 |
| hot | direct | 673.0 | 2.273 | 11.8 | 1 | 312 | 0 |
| hot | prepare | 651.8 | 3.052 | 9.3 | 1 | 312 | 121728 |
