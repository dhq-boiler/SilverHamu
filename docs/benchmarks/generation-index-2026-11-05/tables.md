| Rows | Updates | Method | Build ms | Update p99 µs | Rank100 p99 µs | Current MiB | With old root MiB | Allocated/update KiB |
|---:|---|---|---:|---:|---:|---:|---:|---:|
| 100,000 | local | count_only | 7.807 | 1.900 | 5.200 | 6.905 | 6.973 | 1.182 |
| 100,000 | local | count_max | 9.008 | 1.700 | 6.000 | 7.672 | 7.748 | 1.313 |
| 100,000 | spread | count_only | 7.860 | 3.700 | 5.000 | 6.921 | 7.902 | 1.606 |
| 100,000 | spread | count_max | 8.403 | 4.200 | 6.300 | 7.690 | 8.780 | 1.785 |
| 1,000,000 | local | count_only | 83.217 | 2.900 | 8.200 | 68.717 | 69.102 | 1.607 |
| 1,000,000 | local | count_max | 97.534 | 3.600 | 9.300 | 76.353 | 76.780 | 1.785 |
| 1,000,000 | spread | count_only | 80.654 | 6.900 | 7.300 | 68.719 | 70.433 | 1.848 |
| 1,000,000 | spread | count_max | 98.083 | 7.500 | 8.300 | 76.355 | 78.259 | 2.053 |

| Rows | Updates | Range case | Count-only p99 µs | Global guard p99 µs | Count+max p99 µs | BTreeMap p99 µs |
|---:|---|---|---:|---:|---:|---:|
| 100,000 | local | stable_wide | 2792.700 | 0.100 | 0.200 | 220.200 |
| 100,000 | local | stable_narrow | 13.500 | 0.100 | 0.200 | 0.700 |
| 100,000 | local | stale_wide | 740.100 | 641.300 | 0.600 | 59.800 |
| 100,000 | local | stale_narrow | 5.100 | 1.600 | 0.200 | 0.600 |
| 100,000 | local | stale_upper_half | 916.200 | 490.600 | 0.200 | 118.800 |
| 100,000 | spread | stable_wide | 4648.800 | 0.100 | 0.100 | 236.100 |
| 100,000 | spread | stable_narrow | 7.700 | 0.100 | 0.100 | 0.800 |
| 100,000 | spread | stale_wide | 1.000 | 0.300 | 3.500 | 0.300 |
| 100,000 | spread | stale_narrow | 0.300 | 0.200 | 1.900 | 0.300 |
| 100,000 | spread | stale_upper_half | 2.500 | 0.400 | 1.500 | 0.300 |
| 1,000,000 | local | stable_wide | 40061.500 | 0.700 | 0.800 | 7844.900 |
| 1,000,000 | local | stable_narrow | 11.900 | 0.100 | 0.100 | 3.400 |
| 1,000,000 | local | stale_wide | 10889.000 | 10535.900 | 1.600 | 1688.200 |
| 1,000,000 | local | stale_narrow | 7.300 | 5.100 | 0.900 | 4.300 |
| 1,000,000 | local | stale_upper_half | 18267.000 | 22439.400 | 1.000 | 3775.600 |
| 1,000,000 | spread | stable_wide | 38735.500 | 0.600 | 0.800 | 7852.200 |
| 1,000,000 | spread | stable_narrow | 22.700 | 0.100 | 0.100 | 3.300 |
| 1,000,000 | spread | stale_wide | 1.700 | 1.200 | 3.600 | 3.800 |
| 1,000,000 | spread | stale_narrow | 1.100 | 0.500 | 1.700 | 1.600 |
| 1,000,000 | spread | stale_upper_half | 13.400 | 6.200 | 2.400 | 2.800 |

Sorted-vector rank100 p99 (µs; reference only): 100,000/local: 0.500, 100,000/spread: 0.400, 1,000,000/local: 0.700, 1,000,000/spread: 0.700
