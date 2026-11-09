use super::{
    Database, Error, ExportInfo, HEADER_BYTES, MAX_WAL_BYTES, ResourceLimits, Result,
    backup_archive, ownership, publish_new_file, temporary_path, validate_wal_header, with_suffix,
};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};

/// Creates a compact physical archive of a closed, standalone transaction database.
/// The archive must be a new file. It retains the database identity and storage policy.
pub fn backup(database: impl AsRef<Path>, archive: impl AsRef<Path>) -> Result<()> {
    standalone(database.as_ref())?;
    let source = fs::canonicalize(database.as_ref())?;
    let target = new_destination(archive.as_ref())?;
    let _owner = ownership(&source)?;
    standalone(database.as_ref())?;
    standalone(&source)?;
    let mut input = regular_file(&source)?;
    input
        .try_lock_shared()
        .map_err(|e| Error::Locked(e.to_string()))?;
    check_database(&mut input)?;
    write_new(&target, |output, _| {
        Ok(backup_archive::encode(&mut input, output)?)
    })
}

/// Restores a physical archive to a new standalone database and syncs it before returning.
/// A failed archive check cannot publish a partially restored file.
pub fn restore(archive: impl AsRef<Path>, database: impl AsRef<Path>) -> Result<()> {
    let mut input = regular_file(archive.as_ref())?;
    let target = new_destination(database.as_ref())?;
    standalone(&target)?;
    let _owner = ownership(&target)?;
    standalone(&target)?;
    write_new(&target, |output, _| {
        backup_archive::decode(&mut input, output)?;
        check_database(output)
    })
}

fn regular_file(path: &Path) -> Result<File> {
    if !fs::metadata(path)?.is_file() {
        return Err(Error::Invalid("expected a regular file"));
    }
    Ok(File::open(path)?)
}

fn check_database(file: &mut File) -> Result<()> {
    if !(HEADER_BYTES as u64..=MAX_WAL_BYTES).contains(&file.metadata()?.len()) {
        return Err(Error::Invalid("WAL length"));
    }
    file.rewind()?;
    let mut header = [0; HEADER_BYTES];
    file.read_exact(&mut header)?;
    validate_wal_header(&header)?;
    file.rewind()?;
    Ok(())
}

fn path_occupied(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

fn reject_sidecar_name(path: &Path) -> Result<()> {
    let extension = path.extension().unwrap_or_default().as_encoded_bytes();
    if [b"owner".as_slice(), b"accounts", b"account-owner"]
        .iter()
        .any(|name| extension.eq_ignore_ascii_case(name))
    {
        return Err(Error::Invalid("reserved database sidecar filename"));
    }
    Ok(())
}

fn standalone(path: &Path) -> Result<()> {
    reject_sidecar_name(path)?;
    if path_occupied(&with_suffix(path, ".accounts"))?
        || path_occupied(&with_suffix(path, ".account-owner"))?
    {
        return Err(Error::Invalid(
            "account-enrolled databases require a backup of the complete service file set",
        ));
    }
    Ok(())
}

fn new_destination(path: &Path) -> Result<PathBuf> {
    reject_sidecar_name(path)?;
    if path_occupied(path)? {
        return Err(
            io::Error::new(io::ErrorKind::AlreadyExists, "destination already exists").into(),
        );
    }
    let name = path
        .file_name()
        .ok_or(Error::Invalid("missing destination filename"))?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    Ok(fs::canonicalize(parent)?.join(name))
}

fn write_new<T>(path: &Path, write: impl FnOnce(&mut File, &Path) -> Result<T>) -> Result<T> {
    let temporary = temporary_path(path);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(&temporary)?;
    let result = write(&mut output, &temporary).and_then(|value| {
        output.sync_all()?;
        Ok(value)
    });
    drop(output);
    result
        .and_then(|value| {
            publish_new_file(&temporary, path)?;
            Ok(value)
        })
        .inspect_err(|_| {
            let _ = fs::remove_file(&temporary);
        })
}

impl Database {
    /// Archives one pinned committed generation while writers continue. The archive uses
    /// the same CATDBBAK format as offline backup, including the source WAL policy.
    pub fn backup(&self, archive: impl AsRef<Path>) -> Result<ExportInfo> {
        self.backup_with(archive.as_ref(), |_| Ok(()))
    }

    fn backup_with(
        &self,
        archive: &Path,
        mut hook: impl FnMut(super::CheckpointStage) -> io::Result<()>,
    ) -> Result<ExportInfo> {
        let source = self.state()?.path.clone();
        standalone(&source)?;
        let target = new_destination(archive)?;
        let mut draft = self.start_checkpoint(0)?;
        let snapshot = draft.file.as_mut().unwrap();
        let mut buffer = self.inner.budget.buffer();
        while self.write_checkpoint_chunk(&mut draft.cursor, snapshot, &mut buffer, &mut hook)? {}
        buffer.clear();
        let end = snapshot.stream_position()?;
        if end > MAX_WAL_BYTES {
            return Err(Error::Limit("backup bytes"));
        }
        let version = draft.cursor.pinned.version();
        snapshot.rewind()?;
        snapshot.write_all(&super::wal_header(self.id(), version, draft.cursor.chunks))?;
        #[cfg(feature = "fixed-log")]
        if let Some(layout) = draft.layout {
            use super::{CheckpointLog, fixed_log};
            buffer.add(32768)?;
            match layout {
                CheckpointLog::Fixed(capacity) => fixed_log::initialize(snapshot, end, capacity)?,
                #[cfg(feature = "redo-witness")]
                CheckpointLog::Redo(capacity) => {
                    fixed_log::initialize_redo(snapshot, end, capacity, &self.inner.budget)?
                }
                #[cfg(feature = "redo-witness")]
                CheckpointLog::ManagedRedo(capacity) => fixed_log::initialize_managed_redo(
                    snapshot,
                    end,
                    super::managed_capacity(capacity, end, 0)?,
                    &self.inner.budget,
                )?,
            }
        }
        buffer.clear();
        // The source DB has not been checkpointed or replaced. Only the detached
        // checkpoint is encoded; later commits never enter this archive.
        let info = ExportInfo {
            version,
            records: draft.cursor.records,
        };
        write_new(&target, |output, _| {
            standalone(&source)?;
            backup_archive::encode(snapshot, output)?;
            Ok(info)
        })
    }

    /// Restores a CATDBBAK archive or a legacy NewDBMS compact checkpoint. Validate
    /// and recover the staged file before publication; never modify the backup.
    pub fn restore(archive: impl AsRef<Path>, destination: impl AsRef<Path>) -> Result<ExportInfo> {
        Self::restore_with_limits(archive, destination, ResourceLimits::default())
    }

    pub fn restore_with_limits(
        archive: impl AsRef<Path>,
        destination: impl AsRef<Path>,
        limits: ResourceLimits,
    ) -> Result<ExportInfo> {
        let limits = limits.validate()?;
        standalone(archive.as_ref())?;
        let source = fs::canonicalize(archive.as_ref())?;
        standalone(&source)?;
        let mut input = regular_file(&source)?;
        input
            .try_lock_shared()
            .map_err(|e| Error::Locked(e.to_string()))?;
        let target = new_destination(destination.as_ref())?;
        standalone(&target)?;
        let _owner = ownership(&target)?;
        standalone(&target)?;
        let mut magic = [0; 8];
        input.read_exact(&mut magic)?;
        input.rewind()?;
        write_new(&target, |output, temporary| {
            let legacy = &magic == super::MAGIC;
            if legacy {
                let length = input.metadata()?.len();
                if !(HEADER_BYTES as u64..=MAX_WAL_BYTES).contains(&length)
                    || io::copy(&mut (&mut input).take(length + 1), output)? != length
                {
                    return Err(Error::Invalid("checkpoint backup length"));
                }
            } else if &magic == backup_archive::MAGIC {
                backup_archive::decode(&mut input, output)?;
            } else {
                return Err(Error::Invalid("unknown backup format"));
            }
            let validated =
                Self::open_internal(temporary, limits, legacy).and_then(|db| db.stats());
            let _ = fs::remove_file(with_suffix(temporary, ".owner"));
            let stats = validated?;
            Ok(ExportInfo {
                version: stats.version,
                records: stats.visible_records,
            })
        })
    }
}

#[cfg(test)]
#[path = "online_backup_tests.rs"]
mod online_tests;
