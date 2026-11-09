use super::*;
use crate::transactions::{FRAME, PREFIX_BYTES};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let path = temporary_path(&std::env::temp_dir().join("catdb-online-backup"));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn online_backup_captures_one_generation_across_chunks_and_restores_all_keys() {
    let dir = Scratch::new();
    let source = Database::create(dir.file("live"), [8; 16]).unwrap();
    let mut tx = source.begin().unwrap();
    tx.put(b"", b"").unwrap();
    for i in 0..600u16 {
        tx.put(&i.to_be_bytes(), b"before").unwrap();
    }
    let version = tx.commit().unwrap();
    let backup = dir.file("backup");
    let mut wrote = false;
    let captured = source
        .backup_with(&backup, |_| {
            if !wrote {
                let mut tx = source.begin().unwrap();
                tx.put(&550u16.to_be_bytes(), b"after").unwrap();
                tx.delete(&551u16.to_be_bytes()).unwrap();
                tx.put(b"later", b"").unwrap();
                tx.commit().unwrap();
                wrote = true;
            }
            Ok(())
        })
        .unwrap();
    assert!(wrote);
    assert_eq!((captured.version, captured.records), (version, 601));
    let copy = dir.file("copy");
    let restored = Database::restore(&backup, &copy).unwrap();
    assert_eq!((restored.version, restored.records), (version, 601));
    let db = Database::open(copy).unwrap();
    assert_eq!(db.id(), source.id());
    #[cfg(feature = "fixed-log")]
    assert_eq!(
        db.fixed_log_capacity().unwrap(),
        source.fixed_log_capacity().unwrap()
    );
    // The established offline restore command/API also understands this archive.
    let offline_copy = dir.file("offline-copy");
    restore(&backup, &offline_copy).unwrap();
    assert_eq!(
        Database::open(offline_copy)
            .unwrap()
            .current_version()
            .unwrap(),
        version
    );
    let mut tx = db.begin().unwrap();
    assert_eq!(tx.get(b"").unwrap(), Some(vec![]));
    for i in 0..600u16 {
        assert_eq!(
            tx.get(&i.to_be_bytes()).unwrap().as_deref(),
            Some(&b"before"[..])
        );
    }
    assert_eq!(tx.get(b"later").unwrap(), None);
    tx.put(b"new", b"copy-only").unwrap();
    assert_eq!(tx.commit().unwrap(), version + 1);
    assert_eq!(source.begin().unwrap().get(b"new").unwrap(), None);
}

#[test]
fn incomplete_damaged_and_extended_backups_fail_without_publication() {
    let dir = Scratch::new();
    let db = Database::create(dir.file("live"), [1; 16]).unwrap();
    let mut tx = db.begin().unwrap();
    tx.put(b"key", b"value").unwrap();
    tx.commit().unwrap();
    let backup = dir.file("backup");
    db.backup(&backup).unwrap();
    let original = fs::read(&backup).unwrap();
    let mut damaged = original.clone();
    damaged[HEADER_BYTES + PREFIX_BYTES + 12] ^= 0x80;
    let mut extended = original.clone();
    extended.extend_from_slice(&FRAME[..3]);
    let target = dir.file("copy");
    for bytes in [original[..original.len() - 1].to_vec(), damaged, extended] {
        fs::write(&backup, &bytes).unwrap();
        assert!(Database::restore(&backup, &target).is_err());
        assert!(!target.exists());
        assert_eq!(fs::read(&backup).unwrap(), bytes);
    }
    let failed = dir.file("failed");
    assert!(
        db.backup_with(&failed, |_| Err(io::Error::other("injected failure")))
            .is_err()
    );
    assert!(!failed.exists());
    assert!(!failed.exists());
}

#[test]
fn restore_validation_obeys_budget_and_can_be_retried_to_the_same_path() {
    let dir = Scratch::new();
    let db = Database::create(dir.file("live"), [5; 16]).unwrap();
    let mut tx = db.begin().unwrap();
    tx.put(b"k", b"value").unwrap();
    tx.commit().unwrap();
    let backup = dir.file("backup");
    let copy = dir.file("copy");
    db.backup(&backup).unwrap();
    let limits = ResourceLimits {
        max_buffer_bytes: 1,
        ..Default::default()
    };
    assert!(matches!(
        Database::restore_with_limits(&backup, &copy, limits),
        Err(Error::ResourceExhausted(_))
    ));
    assert!(!copy.exists());
    assert_eq!(Database::restore(&backup, &copy).unwrap().records, 1);
}

#[test]
fn empty_backups_no_clobber_and_late_destination_creation() {
    let dir = Scratch::new();
    let source = dir.file("live");
    let db = Database::create(&source, [7; 16]).unwrap();
    let backup = dir.file("backup");
    assert_eq!(db.backup(&backup).unwrap().records, 0);
    assert_eq!(
        Database::restore(&backup, dir.file("copy"))
            .unwrap()
            .version,
        0
    );
    assert!(db.backup(&backup).is_err());
    assert!(Database::restore(&backup, &source).is_err());
    let race = dir.file("race");
    let mut tx = db.begin().unwrap();
    tx.put(b"k", b"v").unwrap();
    tx.commit().unwrap();
    assert!(
        db.backup_with(&race, |_| fs::write(&race, b"keep"))
            .is_err()
    );
    assert_eq!(fs::read(&race).unwrap(), b"keep");
}

#[test]
fn legacy_checkpoint_accepted_only_with_complete_frames_and_no_tail() {
    let dir = Scratch::new();
    let image = dir.file("legacy");
    let db = Database::create_append(&image, [12; 16]).unwrap();
    let mut tx = db.begin().unwrap();
    tx.put(b"old", b"db").unwrap();
    tx.commit().unwrap();
    db.checkpoint().unwrap();
    drop(db);
    let target = dir.file("readable");
    let info = Database::restore(&image, &target).unwrap();
    assert_eq!((info.version, info.records), (1, 1));
    assert_eq!(
        Database::open(&target)
            .unwrap()
            .begin_read()
            .unwrap()
            .get(b"old")
            .unwrap(),
        Some(b"db".to_vec()),
    );
    let bytes = fs::read(&image).unwrap();
    fs::write(&image, [&bytes[..], b"NDT"].concat()).unwrap();
    assert!(Database::restore(&image, dir.file("bad")).is_err());
    fs::write(&image, &bytes[..bytes.len() - 1]).unwrap();
    assert!(Database::restore(&image, dir.file("bad")).is_err());
    assert!(!dir.file("bad").exists());
}

#[test]
fn online_api_refuses_account_files_and_reserved_destinations() {
    let dir = Scratch::new();
    let path = dir.file("enrolled");
    let db = Database::create_append(&path, [0; 16]).unwrap();
    let marker = with_suffix(&path, ".account-owner");
    fs::write(&marker, b"service").unwrap();
    assert!(db.backup(dir.file("backup")).is_err());
    assert!(!dir.file("backup").exists());
    fs::remove_file(marker).unwrap();
    assert!(db.backup(dir.file("x.accounts")).is_err());
    let archive = dir.file("backup");
    db.backup(&archive).unwrap();
    fs::write(dir.file("restored.accounts"), b"service").unwrap();
    assert!(Database::restore(&archive, dir.file("restored")).is_err());
    assert!(!dir.file("restored").exists());
}
