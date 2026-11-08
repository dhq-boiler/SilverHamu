# Backup and restore measurement

## Results

Measured on Windows build 26100, NTFS, AMD Ryzen 9 5950X, Rust 1.99.1. The reference engine was built from commit `78db7ab`. Both methods produced byte-for-byte restores in all 24 trials.

| Profile | Method | Archive MiB | Backup ms | Restore ms | Open and first read ms | Restore + first read ms |
|---|---|---:|---:|---:|---:|---:|
| Spare | OS copy / reference engine | 128.25 | 129.88 | 131.00 | 110.55 | 247.30 |
| Spare | catdb archive / updated engine | 2.32 | 48.40 | 123.38 | 34.35 | 157.35 |
| Dense | OS copy / reference engine | 64.25 | 55.02 | 54.52 | 112.92 | 168.20 |
| Dense | catdb archive / updated engine | 57.25 | 59.52 | 61.31 | 102.92 | 165.12 |

Times are medians of six trials. On the spare database the archive backup was 2.68× as fast by median, with times ranging from 44.47 to 212.95 ms. The reference copy ranged from 119.77 to 144.99 ms. Restoring this archive and reading a committed value took less time in all six comparisons. On the dense database the ordinary copy was faster at backup and at restore. Faster reopening brought the overall dense restore and first read within 2% of the reference. [Individual trials, ranges and executable hashes](windows-results.json)

### Linux

The same two datasets and 24 alternating trials ran in a Linux Docker container on the same Windows/WSL2 host with Rust 1.89. The comparison uses the container's local filesystem. All 24 restored databases matched their source byte for byte and passed the content checks.

| Profile | Method | Backup ms | Restore ms | Open and first read ms | Restore + first read ms |
|---|---|---:|---:|---:|---:|
| Spare | OS copy / reference engine | 55.07 | 63.10 | 62.08 | 128.57 |
| Spare | catdb archive / updated engine | 49.61 | 48.22 | 13.16 | 62.55 |
| Dense | OS copy / reference engine | 47.97 | 44.71 | 101.91 | 145.96 |
| Dense | catdb archive / updated engine | 53.26 | 34.80 | 95.11 | 130.38 |

The updated engine reads and checks the unused end of a format-4 WAL in larger batches when its memory budget permits. This also improves opening a regular database file with unused capacity. The separate timing columns show how much of the end-to-end change came from opening the restored database. [Linux trials and executable hashes](linux-results.json)

## Reproduce

Run the benchmark on the filesystem used for backups. It creates independent, closed, format-4 databases under a temporary directory on that filesystem. Six trials per method alternate the order of the archive and file-copy methods. Both methods write to a new file, flush it to storage, then publish it in the destination directory. The copy baseline uses Rust's operating-system file copy and the same destination volume. The archive CLI additionally checks its block checksums during restoration.

The **spare** database has a 128 MiB WAL data area and 2,048 rows. The **dense** database has a 64 MiB data area and 57,344 rows. Each row initially contains a deterministic 1 KiB binary value. Both fixtures then replace one value, delete one key, commit an empty value and roll back an uncommitted insertion. The two profiles use the same workload in every repetition.

```sh
cargo build --release --locked --bin catdb --example backup-restore-fixture
python benchmarks/backup-restore/run.py --catdb target/release/catdb --fixture target/release/examples/backup-restore-fixture --work-dir artifacts/backup-work --out artifacts/backup-times.json
```

On Windows add `.exe` to both executable paths. To compare engine startup changes with a prior version, build the same `fixture.rs` as an example in the prior checkout and pass the resulting executable as `--baseline-fixture`. The reference engine then creates both source databases and opens the ordinary restores. The candidate engine opens the archive restores.

Backup and restore timings include CLI process startup, data transfer, synchronization and publication. The separate first-read measurement starts just before opening the restored database and stops after reading a known value. The combined metric adds these two durations for each trial before taking the median. The benchmark runs sequentially with normal filesystem caching. It does not flush the host's page cache or simulate a different storage device.

Every restore must match the source's full-file SHA-256. The fixture checks every live key and value, the deleted and rolled-back keys, database identity, commit version and WAL capacity. It commits another write and reopens the database to read it back. The source is hashed again after both methods complete. The JSON output includes all individual timings, logical archive sizes, source hashes and executable hashes.
