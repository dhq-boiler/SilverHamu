# Backup and restore measurement

## Dense database backups

On a closed format-4 database, the updated `silverhamu backup` outperformed a synchronized OS copy at both measured data densities on Windows and Linux. The table shows **median backup time** over 18 trials per method, including process startup, file synchronization and publication. Occupancy is the share of the physical database covered by nonzero 64 KiB blocks.

| Environment | Occupancy | OS copy | Previous archive | Updated archive | Time saved vs. copy | Faster than copy |
|---|---:|---:|---:|---:|---:|---:|
| Windows / NTFS | 89.11% | 56.98 ms | 68.18 ms | 42.05 ms | 26.2% | 18/18 |
| Windows / NTFS | 98.93% | 89.91 ms | 94.04 ms | 66.79 ms | 25.7% | 18/18 |
| Linux / WSL2 | 89.11% | 34.00 ms | 51.90 ms | 31.49 ms | 7.4% | 15/18 |
| Linux / WSL2 | 98.93% | 34.08 ms | 57.04 ms | 29.89 ms | 12.3% | 18/18 |

The dense and near-full fixtures occupy 64.25 MiB and contain 57,344 and 63,744 live rows, respectively. Each archive has the same size and SHA-256 as the archive produced by commit `9c11ba0`. A third fixture with 1.80% occupancy exercised the sequential encoder. Its previous and updated backup medians were 50.68 and 49.44 ms on Windows, and 57.78 and 58.15 ms on Linux. [Windows trials](dense-windows-results.json) · [Linux trials](dense-linux-results.json)

### Where the time went

In the previous encoder, reading, processing and writing each batch ran sequentially. On the dense fixture the median times spent reading, writing and synchronizing the archive were 10.34, 11.39 and 13.72 ms on Windows, and 19.04, 11.21 and 21.80 ms on Linux. CRC32C took 1.73 and 1.83 ms respectively. These timings came from a separate instrumented build. [Phase timings](dense-profile.json)

For files of at least 8 MiB, the encoder samples three 4 KiB pages to choose the I/O path. If at least two contain data, two readers prepare 1 MiB batches while the writer emits the payload in source order, using aligned 2 MiB direct writes when supported. Every block is still examined. Each stored block retains its CRC32C, the exact byte length is preserved, and the completed file is synchronized before publication.

Restore uses the existing v1 decoder. Each measured restore had to match the closed source's SHA-256 before opening, pass checks for every live row, the database ID, commit version and WAL capacity, and retain a new write across a restart. In the immediate backup-then-restore sequence on Linux, median restore time **increased** from 37.05 to 47.54 ms at 89.11% occupancy, and from 35.89 to 46.01 ms at 98.93%. The benchmark uses normal filesystem caching, and the new backup writes most dense payload bytes through direct I/O. In a separate dense restore check, each CLI read the same archive after its bytes had been read fully into the filesystem cache. The medians were 34.50 ms with the previous CLI and 34.19 ms with the updated CLI (12 restores each); all output hashes matched. This is consistent with cache state contributing to the immediate-restore difference. [Restore trials with the same input and cache preparation](warm-restore-linux.json)

### Reproduce the three-way comparison

Build both versions into separate directories. From the updated checkout:

```sh
git worktree add --detach ../catdb-backup-reference 9c11ba0
cargo build --manifest-path ../catdb-backup-reference/Cargo.toml --locked --release --bin silverhamu --target-dir ../catdb-backup-reference/target
cargo build --locked --release --bin silverhamu --example backup-restore-fixture --target-dir target
python benchmarks/backup-restore/run.py --catdb target/release/silverhamu --baseline-catdb ../catdb-backup-reference/target/release/silverhamu --fixture target/release/examples/backup-restore-fixture --work-dir artifacts/dense-work --out artifacts/dense-times.json --profiles dense near-full spare --reps 18
```

Add `.exe` to the executable paths on Windows. Each method runs once as a warm-up. The next 18 repetitions cycle through all six orders three times. Each method reads the same closed source and writes a new, synchronized destination on the same filesystem. Windows used NTFS and Rust 1.99.1. Linux used the Docker overlay filesystem on a WSL2 virtual disk with Rust 1.89.0 on the same Ryzen 9 5950X host. The OS measurements ran separately. On Windows, the OS copy and previous-archive trials used the same extended fixture source built against the reference engine (`--baseline-fixture`); Linux used one fixture executable for all three methods. Executable hashes are recorded in each result file.

## Original archive results

Measured on Windows build 26100, NTFS, AMD Ryzen 9 5950X, Rust 1.99.1. The reference engine was built from commit `78db7ab`. Both methods produced byte-for-byte restores in all 24 trials.

| Profile | Method | Archive MiB | Backup ms | Restore ms | Open and first read ms | Restore + first read ms |
|---|---|---:|---:|---:|---:|---:|
| Spare | OS copy / reference engine | 128.25 | 129.88 | 131.00 | 110.55 | 247.30 |
| Spare | SilverHamu archive / updated engine | 2.32 | 48.40 | 123.38 | 34.35 | 157.35 |
| Dense | OS copy / reference engine | 64.25 | 55.02 | 54.52 | 112.92 | 168.20 |
| Dense | SilverHamu archive / updated engine | 57.25 | 59.52 | 61.31 | 102.92 | 165.12 |

Times are medians of six trials. On the spare database the archive backup was 2.68× as fast by median, with times ranging from 44.47 to 212.95 ms. The reference copy ranged from 119.77 to 144.99 ms. Restoring this archive and reading a committed value took less time in all six comparisons. On the dense database the ordinary copy was faster at backup and at restore. Faster reopening brought the overall dense restore and first read within 2% of the reference. [Individual trials, ranges and executable hashes](windows-results.json)

### Linux

The same two datasets and 24 alternating trials ran in a Linux Docker container on the same Windows/WSL2 host with Rust 1.89. The comparison uses the container's local filesystem. All 24 restored databases matched their source byte for byte and passed the content checks.

| Profile | Method | Backup ms | Restore ms | Open and first read ms | Restore + first read ms |
|---|---|---:|---:|---:|---:|
| Spare | OS copy / reference engine | 55.07 | 63.10 | 62.08 | 128.57 |
| Spare | SilverHamu archive / updated engine | 49.61 | 48.22 | 13.16 | 62.55 |
| Dense | OS copy / reference engine | 47.97 | 44.71 | 101.91 | 145.96 |
| Dense | SilverHamu archive / updated engine | 53.26 | 34.80 | 95.11 | 130.38 |

The updated engine reads and checks the unused end of a format-4 WAL in larger batches when its memory budget permits. This also improves opening a regular database file with unused capacity. The separate timing columns show how much of the end-to-end change came from opening the restored database. [Linux trials and executable hashes](linux-results.json)

## Reproduce

Run the benchmark on the filesystem used for backups. It creates independent, closed, format-4 databases under a temporary directory on that filesystem. Six trials per method alternate the order of the archive and file-copy methods. Both methods write to a new file, flush it to storage, then publish it in the destination directory. The copy baseline uses Rust's operating-system file copy and the same destination volume. The archive CLI additionally checks its block checksums during restoration.

The **spare** database has a 128 MiB WAL data area and 2,048 rows. The **dense** database has a 64 MiB data area and 57,344 rows. Each row initially contains a deterministic 1 KiB binary value. Both fixtures then replace one value, delete one key, commit an empty value and roll back an uncommitted insertion. The two profiles use the same workload in every repetition.

```sh
cargo build --release --locked --bin silverhamu --example backup-restore-fixture
python benchmarks/backup-restore/run.py --catdb target/release/silverhamu --fixture target/release/examples/backup-restore-fixture --work-dir artifacts/backup-work --out artifacts/backup-times.json
```

On Windows add `.exe` to both executable paths. To compare engine startup changes with a prior version, build the same `fixture.rs` as an example in the prior checkout and pass the resulting executable as `--baseline-fixture`. The reference engine then creates both source databases and opens the ordinary restores. The candidate engine opens the archive restores.

Backup and restore timings include CLI process startup, data transfer, synchronization and publication. The separate first-read measurement starts just before opening the restored database and stops after reading a known value. The combined metric adds these two durations for each trial before taking the median. The benchmark runs sequentially with normal filesystem caching. It does not flush the host's page cache or simulate a different storage device.

Every restore must match the source's full-file SHA-256. The fixture checks every live key and value, the deleted and rolled-back keys, database identity, commit version and WAL capacity. It commits another write and reopens the database to read it back. The source is hashed again after both methods complete. The JSON output includes all individual timings, logical archive sizes, source hashes and executable hashes.
