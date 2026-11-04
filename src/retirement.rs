use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct RetirementStatus {
    pub enabled: bool,
    pub max_bytes: u64,
    pub retained: u64,
    pub reclaimed: u64,
    pub fallbacks: u64,
    pub last_error: Option<String>,
    pub link_ns: u64,
    pub reclaim_ns: u64,
}

pub(super) struct Retirement {
    path: PathBuf,
    retired: PathBuf,
    receipt: PathBuf,
    id: [u8; 16],
    prepared: Option<[u64; 2]>,
    pub status: RetirementStatus,
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

fn plain(path: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(io::Error::other("reparse point is not a retirement file"));
        }
    }
    if !meta.is_file() || meta.is_symlink() {
        return Err(io::Error::other("not a regular retirement file"));
    }
    Ok(())
}

#[cfg(windows)]
fn identity(path: &Path) -> io::Result<[u64; 2]> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct Information {
        fields: [u32; 13],
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetFileInformationByHandle(handle: *mut std::ffi::c_void, info: *mut Information)
        -> i32;
    }
    plain(path)?;
    // Data access is unnecessary for identity and can trigger filesystem filter work.
    let file = OpenOptions::new().access_mode(0).open(path)?;
    let mut info = Information { fields: [0; 13] };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok([
        info.fields[7] as u64,
        ((info.fields[11] as u64) << 32) | info.fields[12] as u64,
    ])
}

#[cfg(unix)]
fn identity(path: &Path) -> io::Result<[u64; 2]> {
    use std::os::unix::fs::MetadataExt;
    plain(path)?;
    let meta = fs::metadata(path)?;
    Ok([meta.dev(), meta.ino()])
}

#[cfg(not(any(unix, windows)))]
fn identity(_: &Path) -> io::Result<[u64; 2]> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "file identity unavailable",
    ))
}

impl Retirement {
    pub fn new(path: PathBuf, id: [u8; 16]) -> Self {
        Self {
            retired: sidecar(&path, ".retired"),
            receipt: sidecar(&path, ".retired.meta"),
            path,
            id,
            prepared: None,
            status: RetirementStatus {
                enabled: false,
                max_bytes: super::MAX_WAL_BYTES,
                ..Default::default()
            },
        }
    }

    fn receipt_bytes(&self, identity: [u64; 2]) -> Vec<u8> {
        let mut bytes = b"NDRET001".to_vec();
        bytes.extend_from_slice(&self.id);
        for word in identity {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        bytes.extend_from_slice(&crate::crc32(&bytes).to_le_bytes());
        bytes
    }

    pub fn cleanup(&mut self) {
        self.prepared = None;
        let started = std::time::Instant::now();
        let result = (|| -> io::Result<()> {
            match plain(&self.receipt) {
                Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
                other => other?,
            }
            let mut bytes = Vec::new();
            File::open(&self.receipt)?
                .take(45)
                .read_to_end(&mut bytes)?;
            if bytes.len() != 44
                || &bytes[..8] != b"NDRET001"
                || bytes[8..24] != self.id
                || crate::crc32(&bytes[..40]).to_le_bytes() != bytes[40..44]
            {
                return Err(io::Error::other("unrecognized retirement receipt"));
            }
            let expected = [
                u64::from_le_bytes(bytes[24..32].try_into().unwrap()),
                u64::from_le_bytes(bytes[32..40].try_into().unwrap()),
            ];
            match identity(&self.retired) {
                Ok(actual) if actual == expected => {
                    fs::remove_file(&self.retired)?;
                    self.status.reclaimed += 1;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => (),
                _ => return Err(io::Error::other("retirement file identity mismatch")),
            }
            fs::remove_file(&self.receipt)
        })();
        self.status.reclaim_ns = started.elapsed().as_nanos() as u64;
        if let Err(error) = result {
            self.status.last_error = Some(error.to_string());
        }
    }

    pub fn prepare(&mut self) {
        self.status.link_ns = 0;
        if !self.status.enabled {
            return;
        }
        let result = (|| -> io::Result<[u64; 2]> {
            if self.retired.try_exists()? {
                return Err(io::Error::other("retirement slot occupied"));
            }
            let id = identity(&self.path)?;
            let mut receipt = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&self.receipt)?;
            receipt.write_all(&self.receipt_bytes(id))?;
            receipt.sync_all()?;
            Ok(id)
        })();
        match result {
            Ok(id) => self.prepared = Some(id),
            Err(e) => self.status.last_error = Some(e.to_string()),
        }
    }

    pub fn retain(&mut self, charged_bytes: u64) {
        if !self.status.enabled {
            return;
        }
        let started = std::time::Instant::now();
        let result = (|| -> io::Result<()> {
            let expected = self
                .prepared
                .ok_or_else(|| io::Error::other("retirement not prepared"))?;
            if charged_bytes > self.status.max_bytes {
                return Err(io::Error::other("retirement byte limit"));
            }
            if identity(&self.path)? != expected {
                return Err(io::Error::other("WAL identity changed"));
            }
            fs::hard_link(&self.path, &self.retired)
        })();
        self.status.link_ns = started.elapsed().as_nanos() as u64;
        match result {
            Ok(()) => self.status.retained += 1,
            Err(e) => {
                self.status.fallbacks += 1;
                self.status.last_error = Some(e.to_string());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn er_receipt_identity_checksum_and_database_id_protect_unrelated_files() {
        for mode in ["identity", "checksum", "database"] {
            let root = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("artifacts")
                .join(format!(
                    "retirement-receipt-{}-{mode}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
            fs::create_dir_all(&root).unwrap();
            let path = root.join("wal");
            fs::write(&path, b"original").unwrap();
            let mut retirement = Retirement::new(path.clone(), [3; 16]);
            retirement.status.enabled = true;
            retirement.prepare();
            assert!(retirement.prepared.is_some());
            if mode == "identity" {
                fs::write(&retirement.retired, b"unrelated").unwrap();
            } else {
                retirement.retain(8);
                assert_eq!(retirement.status.retained, 1);
                if mode == "checksum" {
                    let mut bytes = fs::read(&retirement.receipt).unwrap();
                    bytes[40] ^= 1;
                    fs::write(&retirement.receipt, bytes).unwrap();
                } else {
                    retirement.id = [4; 16];
                }
            }
            retirement.cleanup();
            assert!(retirement.status.last_error.is_some());
            assert!(retirement.retired.exists());
            assert_eq!(fs::read(&path).unwrap(), b"original");
            fs::remove_file(&retirement.retired).unwrap();
            fs::remove_file(&retirement.receipt).unwrap();
            fs::remove_file(&path).unwrap();
            fs::remove_dir(&root).unwrap();
        }
    }
}
