use silverhamu::transactions::{Database, Error, ResourceLimits, ScanLimits};
use std::path::PathBuf;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("artifacts")
            .join(format!(
                "read-gc-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
        std::fs::create_dir_all(&p).unwrap();
        Self(p)
    }
    fn db(&self, limits: ResourceLimits) -> Database {
        Database::create_with_limits(self.0.join("db"), [19; 16], limits).unwrap()
    }
}

fn drain(db: &Database) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while db.reclamation_pending().unwrap() {
        assert!(
            std::time::Instant::now() < deadline,
            "background reclamation stalled"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}
#[test]
fn eq_st_background_preserves_all_old_values_through_updates_deletes_and_checkpoint() {
    let f = Fixture::new();
    let db = f.db(ResourceLimits::default());
    db.set_background_reclamation(true).unwrap();
    let mut seed = db.begin().unwrap();
    for i in 0u32..512 {
        seed.put(&i.to_be_bytes(), &[1; 256]).unwrap();
    }
    seed.commit().unwrap();
    let mut old = db.begin_read().unwrap();
    let mut conflict = db.begin().unwrap();
    conflict.get(&1u32.to_be_bytes()).unwrap();
    let mut range = db.begin().unwrap();
    range
        .scan_page(&[], None, None, ScanLimits::default())
        .unwrap();
    let mut w = db.begin().unwrap();
    for i in 0u32..512 {
        if i % 2 == 0 {
            w.delete(&i.to_be_bytes()).unwrap();
        } else {
            w.put(&i.to_be_bytes(), &[2; 256]).unwrap();
        }
    }
    w.put(&512u32.to_be_bytes(), &[3; 256]).unwrap();
    w.commit().unwrap();
    db.checkpoint().unwrap();
    assert_eq!(
        old.scan(&[], None).unwrap(),
        (0u32..512)
            .map(|i| (i.to_be_bytes().to_vec(), vec![1; 256]))
            .collect::<Vec<_>>()
    );
    conflict.put(b"read-conflict", b"x").unwrap();
    assert!(matches!(conflict.commit(), Err(Error::Conflict(_))));
    range.put(b"range-conflict", b"x").unwrap();
    assert!(matches!(range.commit(), Err(Error::Conflict(_))));
    drop(old);
    drain(&db);
    let expected: Vec<_> = (0u32..513)
        .filter(|i| i % 2 == 1 || *i == 512)
        .map(|i| {
            (
                i.to_be_bytes().to_vec(),
                vec![if i == 512 { 3 } else { 2 }; 256],
            )
        })
        .collect();
    assert_eq!(db.begin_read().unwrap().scan(&[], None).unwrap(), expected);
    drop(db);
    let db = Database::open(f.0.join("db")).unwrap();
    assert_eq!(db.begin_read().unwrap().scan(&[], None).unwrap(), expected);
}
#[test]
fn er_retained_limit_rejects_pinned_updates_then_recovers_without_read_assistance() {
    let f = Fixture::new();
    let db = f.db(ResourceLimits {
        max_retained_bytes: 324,
        ..ResourceLimits::default()
    });
    db.set_background_reclamation(true).unwrap();
    let mut w = db.begin().unwrap();
    w.put(b"k", &[1]).unwrap();
    w.commit().unwrap();
    let mut old = db.begin_read().unwrap();
    let mut w = db.begin().unwrap();
    w.put(b"k", &[2]).unwrap();
    w.commit().unwrap();
    let wal = db.stats().unwrap().wal_bytes;
    let mut w = db.begin().unwrap();
    w.put(b"k", &[3]).unwrap();
    assert!(matches!(w.commit(), Err(Error::ResourceExhausted(_))));
    assert_eq!(wal, db.stats().unwrap().wal_bytes);
    assert_eq!(old.get(b"k").unwrap(), Some(vec![1]));
    drop(old);
    drain(&db);
    assert_eq!(db.resource_usage().unwrap().current[3], 259);
    let mut w = db.begin().unwrap();
    w.put(b"k", &[3]).unwrap();
    w.commit().unwrap();
    drain(&db);
    assert_eq!(db.begin_read().unwrap().get(b"k").unwrap(), Some(vec![3]));
}
#[test]
fn st_concurrent_enable_disable_and_last_close_release_database_ownership() {
    let f = Fixture::new();
    let db = f.db(ResourceLimits::default());
    std::thread::scope(|s| {
        for _ in 0..4 {
            let d = db.clone();
            s.spawn(move || {
                for _ in 0..20 {
                    d.set_background_reclamation(true).unwrap();
                    d.begin_read().unwrap();
                    d.set_background_reclamation(false).unwrap();
                }
            });
        }
    });
    db.set_background_reclamation(true).unwrap();
    db.set_background_reclamation(true).unwrap();
    assert!(db.background_reclamation_enabled());
    drop(db);
    let reopened = Database::open(f.0.join("db")).unwrap();
    assert!(!reopened.background_reclamation_enabled());
    assert_eq!(reopened.stats().unwrap().visible_records, 0);
}
#[test]
fn bv_empty_hot_key_and_large_value_reclamation_finish_while_idle() {
    for bytes in [0, 1024 * 1024, 1024 * 1024 + 1] {
        let f = Fixture::new();
        let db = f.db(ResourceLimits::default());
        db.set_background_reclamation(true).unwrap();
        drain(&db);
        let mut w = db.begin().unwrap();
        w.put(b"k", &vec![1; bytes]).unwrap();
        w.commit().unwrap();
        let mut old = db.begin_read().unwrap();
        for _ in 0..4 {
            let mut w = db.begin().unwrap();
            w.put(b"k", &vec![2; bytes]).unwrap();
            w.commit().unwrap();
        }
        assert_eq!(old.get(b"k").unwrap(), Some(vec![1; bytes]));
        drop(old);
        drain(&db);
        assert_eq!(db.stats().unwrap().retained_versions, 1);
        assert_eq!(db.resource_usage().unwrap().current[3], 258 + bytes);
    }
}

#[test]
#[ignore]
fn child_abrupt_exit_with_background_reclamation() {
    let Ok(path) = std::env::var("CATDB_BG_CRASH_PATH") else {
        return;
    };
    let mode = std::env::var("CATDB_BG_CRASH_MODE").unwrap();
    let db = Database::create(&path, [41; 16]).unwrap();
    db.set_background_reclamation(true).unwrap();
    let mut w = db.begin().unwrap();
    w.put(b"k", b"seed").unwrap();
    w.commit().unwrap();
    let _old = db.begin_read().unwrap();
    let mut w = db.begin().unwrap();
    w.put(b"k", b"committed").unwrap();
    if mode != "before" {
        w.commit().unwrap();
    }
    if mode == "checkpoint" {
        db.checkpoint().unwrap();
    }
    std::process::exit(23);
}
#[test]
fn er_abrupt_exit_recovers_only_confirmed_values_with_background_reclamation() {
    for mode in ["before", "after", "checkpoint"] {
        let f = Fixture::new();
        let path = f.0.join("crash");
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "child_abrupt_exit_with_background_reclamation",
            ])
            .env("CATDB_BG_CRASH_PATH", &path)
            .env("CATDB_BG_CRASH_MODE", mode)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(23),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let db = Database::open(path).unwrap();
        assert_eq!(
            db.begin_read().unwrap().get(b"k").unwrap(),
            Some(if mode == "before" {
                b"seed".to_vec()
            } else {
                b"committed".to_vec()
            })
        );
    }
}
