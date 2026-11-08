use newdbms::transactions::{Database, backup, restore};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Barrier;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "catdb-backup-{}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run_cli(command: &str, source: &Path, destination: &Path) {
    let result = Command::new(env!("CARGO_BIN_EXE_catdb"))
        .arg(command)
        .arg(source)
        .arg(destination)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{command} failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn cli_round_trip_preserves_every_byte_and_restored_commits_are_independent() {
    let f = Fixture::new();
    let source = f.path("original.ndb");
    let archive = f.path("backup.catbak");
    let destination = f.path("restored.ndb");
    let id = [0xc7; 16];
    {
        let db = Database::create(&source, id).unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"", b"").unwrap();
        tx.put(b"key", b"old").unwrap();
        tx.put(b"deleted", b"old").unwrap();
        assert_eq!(tx.commit().unwrap(), 1);
        let mut tx = db.begin().unwrap();
        tx.put(b"key", &[0xff, 0, 0x80]).unwrap();
        tx.delete(b"deleted").unwrap();
        assert_eq!(tx.commit().unwrap(), 2);
        let mut pending = db.begin().unwrap();
        pending.put(b"uncommitted", b"value").unwrap();
        pending.rollback();
    }

    let original = fs::read(&source).unwrap();
    run_cli("backup", &source, &archive);
    run_cli("restore", &archive, &destination);
    assert_eq!(fs::read(&destination).unwrap(), original);

    {
        let db = Database::open(&destination).unwrap();
        assert_eq!(db.id(), id);
        assert_eq!(db.current_version().unwrap(), 2);
        let mut tx = db.begin().unwrap();
        assert_eq!(
            tx.scan(b"", None).unwrap(),
            vec![(vec![], vec![]), (b"key".to_vec(), vec![0xff, 0, 0x80])]
        );
        tx.put(b"restored only", b"accepted").unwrap();
        assert_eq!(tx.commit().unwrap(), 3);
    }
    let reopened = Database::open(&destination).unwrap();
    assert_eq!(reopened.current_version().unwrap(), 3);
    assert_eq!(
        reopened
            .begin_read()
            .unwrap()
            .get(b"restored only")
            .unwrap()
            .as_deref(),
        Some(b"accepted".as_slice())
    );
    assert_eq!(fs::read(&source).unwrap(), original);
    let second = f.path("second restore.ndb");
    restore(&archive, &second).unwrap();
    assert_eq!(fs::read(second).unwrap(), original);
}

#[test]
fn empty_and_nonempty_append_databases_retain_their_identity_and_layout() {
    for populated in [false, true] {
        let f = Fixture::new();
        let source = f.path("old.ndb");
        let archive = f.path("backup");
        let destination = f.path("new.ndb");
        let db = Database::create_append(&source, [7; 16]).unwrap();
        if populated {
            let mut tx = db.begin().unwrap();
            tx.put(b"key", b"old format").unwrap();
            tx.commit().unwrap();
        }
        drop(db);
        backup(&source, &archive).unwrap();
        restore(&archive, &destination).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), fs::read(&source).unwrap());
        let db = Database::open(destination).unwrap();
        assert_eq!(db.id(), [7; 16]);
        assert_eq!(db.current_version().unwrap(), u64::from(populated));
        assert_eq!(
            db.begin_read().unwrap().get(b"key").unwrap().as_deref(),
            populated.then_some(b"old format".as_slice())
        );
    }
}

#[test]
fn historic_v1_and_fixed_v3_restore_the_original_bytes_and_commit_sequence() {
    let id = std::array::from_fn(|i| i as u8);
    let mut v1_header = [0u8; 48];
    v1_header[..8].copy_from_slice(b"NDTXN001");
    v1_header[8] = 1;
    v1_header[12] = 48;
    v1_header[16..32].copy_from_slice(&id);
    v1_header[44..].copy_from_slice(&0x7748_6538_u32.to_le_bytes());
    for version in [
        1_u32,
        #[cfg(feature = "fixed-log")]
        3,
    ] {
        let f = Fixture::new();
        let source = f.path("historical.wal");
        let archive = f.path("historical.catbak");
        let target = f.path("restored.wal");
        fs::write(&source, v1_header).unwrap();
        let db = Database::open(&source).unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"early", b"original").unwrap();
        assert_eq!(tx.commit().unwrap(), 1);
        #[cfg(feature = "fixed-log")]
        if version == 3 {
            db.checkpoint_fixed_log(64 * 1024).unwrap();
        }
        let mut tx = db.begin().unwrap();
        tx.put(b"late", b"next").unwrap();
        assert_eq!(tx.commit().unwrap(), 2);
        drop(db);

        let bytes = fs::read(&source).unwrap();
        assert_eq!(bytes[8..12], version.to_le_bytes());
        backup(&source, &archive).unwrap();
        restore(&archive, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), bytes);
        let recovered = Database::open(&target).unwrap();
        assert_eq!(recovered.id(), id);
        assert_eq!(recovered.current_version().unwrap(), 2);
        assert_eq!(
            recovered.begin_read().unwrap().scan(b"", None).unwrap(),
            vec![
                (b"early".to_vec(), b"original".to_vec()),
                (b"late".to_vec(), b"next".to_vec())
            ]
        );
    }
}

#[test]
fn corrupt_payload_truncation_and_trailing_bytes_are_never_published() {
    let f = Fixture::new();
    let source = f.path("db");
    let archive = f.path("archive");
    let db = Database::create_append(&source, [2; 16]).unwrap();
    let mut tx = db.begin().unwrap();
    tx.put(b"checksummed key", b"payload used to detect corruption")
        .unwrap();
    tx.commit().unwrap();
    drop(db);
    backup(&source, &archive).unwrap();
    let good = fs::read(&archive).unwrap();
    let mut changed = good.clone();
    let middle = changed.len() / 2;
    changed[middle] ^= 0x80;
    let mut extra = good.clone();
    extra.push(0xff);
    let out_dir = f.path("out");
    fs::create_dir(&out_dir).unwrap();
    let destination = out_dir.join("restored");
    for bad in [changed, good[..good.len() - 1].to_vec(), extra, vec![]] {
        fs::write(&archive, &bad).unwrap();
        assert!(restore(&archive, &destination).is_err());
        assert!(!destination.exists());
        assert_eq!(fs::read(&archive).unwrap(), bad);
        assert!(
            fs::read_dir(&out_dir)
                .unwrap()
                .all(|entry| entry.unwrap().file_name() == "restored.owner")
        );
    }
}

#[test]
fn neither_existing_files_nor_the_input_itself_can_be_overwritten() {
    let f = Fixture::new();
    let source = f.path("db");
    let archive = f.path("archive");
    let occupied = f.path("existing");
    drop(Database::create_append(&source, [1; 16]).unwrap());
    let original = fs::read(&source).unwrap();
    backup(&source, &archive).unwrap();
    let snapshot = fs::read(&archive).unwrap();
    fs::write(&occupied, b"keep").unwrap();
    assert!(backup(&source, &occupied).is_err());
    assert!(restore(&archive, &occupied).is_err());
    assert!(backup(&source, &source).is_err());
    assert!(restore(&archive, &archive).is_err());
    assert!(restore(&archive, &source).is_err());
    assert_eq!(fs::read(occupied).unwrap(), b"keep");
    assert_eq!(fs::read(source).unwrap(), original);
    assert_eq!(fs::read(archive).unwrap(), snapshot);
}

#[test]
fn open_database_is_refused_until_its_last_handle_is_closed() {
    let f = Fixture::new();
    let source = f.path("db");
    let archive = f.path("archive");
    let db = Database::create(&source, [3; 16]).unwrap();
    let reader = db.begin_read().unwrap();
    drop(db);
    assert!(backup(&source, &archive).is_err());
    assert!(!archive.exists());
    drop(reader);
    backup(&source, &archive).unwrap();
}

#[test]
fn account_catalog_and_owner_markers_block_single_database_operations() {
    let f = Fixture::new();
    let source = f.path("source");
    let archive = f.path("unbound-archive");
    let destination = f.path("restored");
    drop(Database::create_append(&source, [6; 16]).unwrap());
    backup(&source, &archive).unwrap();
    for suffix in [".accounts", ".account-owner"] {
        let source_marker = f.path(&format!("source{suffix}"));
        let destination_marker = f.path(&format!("restored{suffix}"));
        fs::write(&source_marker, b"keep source sidecar").unwrap();
        fs::write(&destination_marker, b"keep destination sidecar").unwrap();
        let forbidden = f.path("incomplete");
        assert!(backup(&source, &forbidden).is_err(), "{suffix}");
        assert!(restore(&archive, &destination).is_err(), "{suffix}");
        assert!(!forbidden.exists());
        assert!(!destination.exists());
        assert_eq!(fs::read(&source_marker).unwrap(), b"keep source sidecar");
        assert_eq!(
            fs::read(&destination_marker).unwrap(),
            b"keep destination sidecar"
        );
        fs::remove_file(source_marker).unwrap();
        fs::remove_file(destination_marker).unwrap();
    }
}

#[test]
fn concurrent_restores_publish_one_complete_database() {
    let f = Fixture::new();
    let archives = [f.path("first.bak"), f.path("second.bak")];
    let mut originals = Vec::new();
    for (i, archive) in archives.iter().enumerate() {
        let source = f.path(&format!("db{i}"));
        drop(Database::create_append(&source, [i as u8; 16]).unwrap());
        backup(&source, archive).unwrap();
        originals.push(fs::read(source).unwrap());
    }
    let target = f.path("winner.ndb");
    let gate = Barrier::new(2);
    let results = thread::scope(|scope| {
        let gate = &gate;
        let target = &target;
        let handles = archives.each_ref().map(|archive| {
            scope.spawn(move || {
                gate.wait();
                restore(archive, target).is_ok()
            })
        });
        handles.map(|handle| handle.join().unwrap())
    });
    assert_ne!(results[0], results[1]);
    let winner = if results[0] { 0 } else { 1 };
    assert_eq!(fs::read(target).unwrap(), originals[winner]);
}
