use super::{
    Error, HEADER_BYTES, MAX_WAL_BYTES, Result, backup_archive, ownership, publish_new_file,
    temporary_path, validate_wal_header, with_suffix,
};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek};
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
    write_new(&target, |output| {
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
    write_new(&target, |output| {
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

fn write_new(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    let temporary = temporary_path(path);
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(&temporary)?;
    let result = write(&mut output).and_then(|()| Ok(output.sync_all()?));
    drop(output);
    result
        .and_then(|()| Ok(publish_new_file(&temporary, path)?))
        .inspect_err(|_| {
            let _ = fs::remove_file(&temporary);
        })
}
