| Rows | Updates | Case | Before p99 µs | After p99 µs | Before max µs¹ | After max µs¹ |
|---:|---|---|---:|---:|---:|---:|
| 100,000 | local | stable_wide | 1711.800 | 0.300 | 2469.500 | 0.300 |
| 100,000 | local | stable_narrow | 0.600 | 0.100 | 1.300 | 0.100 |
| 100,000 | local | stale_upper_half | 365.200 | 820.200 | 797.700 | 894.100 |
| 100,000 | local | stale_narrow | 0.400 | 0.800 | 0.400 | 1.000 |
| 100,000 | local | prepare | 57.800 | 50.400 | 77.200 | 53.400 |
| 100,000 | local | commit | 5276.000 | 3239.400 | 10425.500 | 4063.000 |
| 100,000 | spread | stable_wide | 1433.100 | 0.400 | 1597.700 | 0.400 |
| 100,000 | spread | stable_narrow | 0.500 | 0.100 | 0.500 | 0.100 |
| 100,000 | spread | stale_upper_half | 0.200 | 0.300 | 0.200 | 0.400 |
| 100,000 | spread | stale_narrow | 1.000 | 0.800 | 1.100 | 1.200 |
| 100,000 | spread | prepare | 42.600 | 42.200 | 60.900 | 234.800 |
| 100,000 | spread | commit | 4621.300 | 2239.800 | 13471.200 | 3524.900 |
| 1,000,000 | local | stable_wide | 23976.800 | 0.500 | 26337.600 | 0.500 |
| 1,000,000 | local | stable_narrow | 0.800 | 0.100 | 1.800 | 0.100 |
| 1,000,000 | local | stale_upper_half | 11963.000 | 10868.500 | 15357.600 | 15557.100 |
| 1,000,000 | local | stale_narrow | 1.300 | 1.100 | 6.600 | 1.200 |
| 1,000,000 | local | prepare | 90.700 | 44.400 | 213.900 | 59.000 |
| 1,000,000 | local | commit | 25200.100 | 3131.000 | 27562.800 | 4122.000 |
| 1,000,000 | spread | stable_wide | 22020.000 | 1.000 | 31059.800 | 1.000 |
| 1,000,000 | spread | stable_narrow | 0.800 | 0.100 | 0.900 | 0.100 |
| 1,000,000 | spread | stale_upper_half | 6.200 | 1.500 | 8.300 | 1.800 |
| 1,000,000 | spread | stale_narrow | 0.500 | 0.900 | 0.600 | 4.400 |
| 1,000,000 | spread | prepare | 53.600 | 46.000 | 212.500 | 48.900 |
| 1,000,000 | spread | commit | 24685.500 | 3055.300 | 29893.700 | 5767.700 |

p99 columns are medians of three trial p99s. ¹Max columns are maxima across all three trials; they are not medians.
