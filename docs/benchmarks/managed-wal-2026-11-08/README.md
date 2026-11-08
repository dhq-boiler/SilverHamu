# Managed WAL under sustained updates

A newly created database used WAL format 4 without storage settings. Each of four or sixteen connections waited for its own durable writes. Each run inserted and repeatedly updated 2,048 keys with 2 KiB values, for 10,240 operations. Three fresh volumes per client count were tested. An old reader stayed open throughout each run.

| Clients | All updates/s | Overall p99 ms | p99 without observed maintenance, ms | p99 in growth epochs, ms | p99 in compaction epochs, ms | Worst single update, ms |
|---:|---:|---:|---:|---:|---:|---:|
| 4 | 1,728.4 | 3.98 | 3.60 | 22.71 | 25.27 | 30.12 |
| 16 | 4,022.3 | 15.76 | 5.75 | 24.86 | 25.53 | 29.43 |

Rates and p99 values are medians of three run statistics. An epoch is a batch of 128 observations. The harness checked the WAL header and file size between epochs and classified the whole epoch by its observed transition. This grouping includes other responses in the same epoch. Client timing includes maintenance performed on the commit path; setup, inspections and restart checks were outside timed epochs.

The six runs completed 61,440 synchronous updates and observed 18 growth epochs and 30 same-capacity checkpoint epochs. The data area grew from 1 MiB to 8 MiB, plus 256 KiB for recovery records. Old-snapshot contents were checked 36 times. After forcing the service to exit, each of six reopens matched all 2,048 expected rows and accepted another write. Separate fault-injection tests stop during automatic replacement before and after publication and verify that the triggering transaction requires its own sync.

The first write needing maintenance can take longer than a regular write. At sixteen connections the overall p99 was 15.76 ms in this workload; the previous single-key peer comparison used different value sizes and a manually sized WAL. No Oracle or PostgreSQL trial was run as part of this maintenance workload.

[All per-run throughput, latency samples, observed transitions, and hashes](raw/) · [Aggregate statistics](raw/summary.json) · [Configuration and migration](../../service.md#default-wal-and-upgrades)

```sh
docker build -t catdb:local .
python benchmarks/managed-wal/run.py --image catdb:local --out artifacts/managed-repeat --clients 4 16 --reps 3
```
