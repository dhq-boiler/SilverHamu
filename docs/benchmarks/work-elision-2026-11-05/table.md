| Dataset | Operation | Dependencies / value bytes | Before p99 µs | After p99 µs | Before alloc calls | After alloc calls | Before allocated bytes | After allocated bytes |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| 100000 | validate | 1 | 0.700 | 0.100 | 0.000 | 0.000 | 0.000 | 0.000 |
| 100000 | validate | 100 | 32.400 | 0.400 | 0.000 | 0.000 | 0.000 | 0.000 |
| 100000 | validate | 10000 | 4901.000 | 35.500 | 0.000 | 0.000 | 0.000 | 0.000 |
| 100000 | commit | 1 | 3601.500 | 2241.100 | 7.000 | 7.000 | 217.000 | 217.000 |
| 100000 | commit | 100 | 3653.300 | 2566.300 | 7.000 | 7.000 | 217.000 | 217.000 |
| 100000 | commit | 10000 | 6280.900 | 3360.900 | 7.000 | 7.000 | 217.000 | 217.000 |
| 100000 | export | 0 | 48604.900 | 51166.400 | 216673.000 | 216672.000 | 17303336.000 | 17303142.000 |
| 1000000 | validate | 1 | 0.300 | 0.100 | 0.000 | 0.000 | 0.000 | 0.000 |
| 1000000 | validate | 100 | 35.600 | 0.500 | 0.000 | 0.000 | 0.000 | 0.000 |
| 1000000 | validate | 10000 | 7962.200 | 45.900 | 0.000 | 0.000 | 0.000 | 0.000 |
| 1000000 | commit | 1 | 2379.300 | 3141.600 | 7.000 | 7.000 | 217.000 | 217.000 |
| 1000000 | commit | 100 | 2344.800 | 3251.500 | 7.000 | 7.000 | 217.000 | 217.000 |
| 1000000 | commit | 10000 | 11749.500 | 3705.600 | 7.000 | 7.000 | 217.000 | 217.000 |
| 1000000 | export | 0 | 639843.100 | 646949.900 | 2166671.000 | 2166670.000 | 172959242.000 | 172959048.000 |
| service | put_commit | 8 | 3005.900 | 2223.200 | 29.000 | 20.000 | 2763.000 | 2088.000 |
| service | put_commit | 262144 | 8569.600 | 4334.200 | 45.000 | 36.000 | 2099881.000 | 1574934.000 |
| service | get_latest | 8 | 1.800 | 1.400 | 14.000 | 7.000 | 1341.000 | 687.000 |
| service | get_latest | 262144 | 1201.300 | 1187.000 | 14.000 | 7.000 | 1312021.000 | 1311367.000 |

Timing columns: median of three trial p99s, except export (one operation per trial; median elapsed time). Allocation columns: median of the per-trial median operation count/bytes, from separate instrumented builds. Values are cumulative allocated/requested bytes, not live or peak memory.
