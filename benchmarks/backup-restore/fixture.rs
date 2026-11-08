use newdbms::transactions::Database;
use std::error::Error;
use std::fs::{self, File};
use std::io;
use std::path::Path;
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn Error>>;
const ID: [u8; 16] = *b"catdb-backup-001";
const MIB: u64 = 1024 * 1024;
const BATCH: u64 = 256;

struct Profile {
    rows: u64,
    capacity: u64,
}

impl Profile {
    fn parse(name: &str) -> Result<Self> {
        match name {
            "spare" => Ok(Self {
                rows: 2_048,
                capacity: 128 * MIB,
            }),
            "dense" => Ok(Self {
                rows: 57_344,
                capacity: 64 * MIB,
            }),
            _ => Err("expected spare or dense".into()),
        }
    }

    fn version(&self) -> u64 {
        self.rows.div_ceil(BATCH) + 1
    }
}

fn value(index: u64) -> [u8; 1024] {
    let mut bytes = [0; 1024];
    let mut state = index + 1;
    for chunk in bytes.chunks_exact_mut(8) {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        chunk.copy_from_slice(&state.wrapping_mul(0x2545_f491_4f6c_dd1d).to_le_bytes());
    }
    bytes
}

fn seed(path: &Path, profile: &Profile) -> Result<()> {
    let db = Database::create_append(path, ID)?;
    for start in (0..profile.rows).step_by(BATCH as usize) {
        let mut tx = db.begin()?;
        for index in start..profile.rows.min(start + BATCH) {
            tx.put(&index.to_be_bytes(), &value(index))?;
        }
        tx.commit()?;
    }
    db.checkpoint_redo_log(profile.capacity)?;
    let mut tx = db.begin()?;
    tx.put(&0u64.to_be_bytes(), b"updated")?;
    tx.delete(&(profile.rows - 1).to_be_bytes())?;
    tx.put(b"empty", b"")?;
    assert_eq!(tx.commit()?, profile.version());
    let mut pending = db.begin()?;
    pending.put(b"rolled-back", b"uncommitted")?;
    pending.rollback();
    Ok(())
}

fn first_read(path: &Path) -> Result<()> {
    let start = Instant::now();
    let db = Database::open(path)?;
    let result = db.begin_read()?.get(&0u64.to_be_bytes())?;
    let ns = start.elapsed().as_nanos();
    assert_eq!(result.as_deref(), Some(b"updated".as_slice()));
    println!("{ns}");
    Ok(())
}

fn verify(path: &Path, profile: &Profile) -> Result<()> {
    let db = Database::open(path)?;
    assert_eq!(db.id(), ID);
    assert_eq!(db.fixed_log_capacity()?, Some(profile.capacity));
    let stats = db.stats()?;
    assert_eq!(stats.visible_records as u64, profile.rows);
    assert_eq!(stats.version, profile.version());
    let mut tx = db.begin_read()?;
    assert_eq!(
        tx.get(&0u64.to_be_bytes())?.as_deref(),
        Some(b"updated".as_slice())
    );
    for index in 1..profile.rows - 1 {
        assert_eq!(
            tx.get(&index.to_be_bytes())?.as_deref(),
            Some(value(index).as_slice())
        );
    }
    assert_eq!(tx.get(&(profile.rows - 1).to_be_bytes())?, None);
    assert_eq!(tx.get(b"empty")?, Some(Vec::new()));
    assert_eq!(tx.get(b"rolled-back")?, None);
    drop(tx);
    let mut writer = db.begin()?;
    writer.put(b"post-restore", b"persisted")?;
    assert_eq!(writer.commit()?, profile.version() + 1);
    drop(db);
    let reopened = Database::open(path)?;
    assert_eq!(
        reopened.begin_read()?.get(b"post-restore")?.as_deref(),
        Some(b"persisted".as_slice())
    );
    Ok(())
}

#[cfg(unix)]
fn publish(temp: &Path, destination: &Path) -> io::Result<()> {
    fs::hard_link(temp, destination)?;
    fs::remove_file(temp)?;
    File::open(destination.parent().unwrap())?.sync_all()
}

#[cfg(windows)]
fn publish(temp: &Path, destination: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(source: *const u16, target: *const u16, flags: u32) -> i32;
    }
    let wide = |p: &Path| {
        p.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let source = wide(temp);
    let target = wide(destination);
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 8) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn plain_copy(source: &Path, destination: &Path) -> Result<()> {
    let temp = destination.with_extension("copying");
    drop(File::create_new(&temp)?);
    fs::copy(source, &temp)?;
    File::options().write(true).open(&temp)?.sync_all()?;
    publish(&temp, destination)?;
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [action, input, arg] = args.as_slice() else {
        return Err("expected ACTION INPUT PROFILE|DEST".into());
    };
    let input = Path::new(input);
    match action.to_str() {
        Some("copy") => plain_copy(input, Path::new(arg)),
        Some("first-read") => first_read(input),
        Some("seed") => seed(input, &Profile::parse(arg.to_str().unwrap_or(""))?),
        Some("verify") => verify(input, &Profile::parse(arg.to_str().unwrap_or(""))?),
        _ => Err("expected seed, first-read, verify or copy".into()),
    }
}
