# Offline backup and restore

Use this procedure for one standalone transaction database, including a service using a shared token. For a service with accounts, stop it and copy the primary database, attached databases, account catalog and ownership markers together. Keep the configured filenames when restoring that set. [Account storage and backup](accounts.md#authentication-and-storage)

## Back up and restore from the checkout

1. Build the CLI with `cargo build --locked --release --bin catdb`. The executable is `./target/release/catdb` on Linux and `.\target\release\catdb.exe` on Windows. Rust applications can call `newdbms::transactions::backup` and `newdbms::transactions::restore` directly.
2. Close all embedded database handles or stop the service. Use its configured database path. Choose an existing private backup directory on a separate device and a new archive filename. The command holds the ownership lock and a file lock during the copy and reports an error if the database is busy.
3. Restore to a new database filename in an existing directory with space for the **full original file**, including its reserved WAL capacity.

```sh
./target/release/catdb backup /data/state.ndb /backups/state.catbak
./target/release/catdb restore /backups/state.catbak /data/recovered.ndb
```

For an immediate restore drill, compare the full-file SHA-256 of the closed source and recovered database before opening either one. To check a backup after the source has changed, compare against the source hash recorded at backup time. Open the recovered path, read a known committed value, and verify a new write survives a restart before switching applications to it.

## Back up and restore the Compose service

The repository's Compose service uses a shared token and stores its database at `/data/state.ndb` in a named volume. Build the image from this checkout with `docker compose build catdb`. In this Linux host example, `/mnt/catdb-backups` is an existing private backup directory writable by the container user (UID 10001). Replace it with the path to your backup device.

```sh
docker compose stop catdb
docker compose run --rm --no-deps -v /mnt/catdb-backups:/backups --entrypoint catdb catdb backup /data/state.ndb /backups/state.catbak
docker compose run --rm --no-deps -v /mnt/catdb-backups:/backups:ro --entrypoint catdb catdb restore /backups/state.catbak /data/recovered.ndb
docker compose run --rm --no-deps --entrypoint sha256sum catdb /data/state.ndb /data/recovered.ndb
```

After a drill, `docker compose start catdb` resumes the original database. To serve the restored copy, set the service's `--db` argument to `/data/recovered.ndb`, retain its other server arguments, and recreate it with `docker compose up -d catdb`.

## Archive guarantees

Both commands publish to a new name. They write a temporary file in the destination directory, sync it, and publish without overwriting an existing file. Failed writes and checksum checks discard the temporary output. An interruption can leave an unpublished temporary file. If directory synchronization fails after publication, a complete destination file may already be visible. The database ownership sidecar (`.owner`) can remain after a failed restore and is reused by catdb.

Archive version 1 stores a checksummed header and ordered block directory, with CRC32C for the stored payload blocks. All-zero 64 KiB blocks occupy only a directory entry. Restore verifies the block types, checksums and exact archive length, and writes the entire database, including zeros in reserved WAL space. Opening the result runs catdb's normal recovery checks. The checksums detect accidental damage in transit or storage. Use private storage and appropriate access controls for archives containing sensitive data.

This physical copy preserves database bytes, including their existing condition. The WAL format must be supported by the build and the original database identity is retained. Save token files, configuration and TLS keys separately. Single-file backups refuse a database with account sidecars. Enrollment markers are bound to filenames, so use the configured service path and avoid hard-link or symlink aliases. [Measurements and reproduction](../benchmarks/backup-restore/README.md)
