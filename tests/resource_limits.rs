use newdbms::transactions::{Database, Error, ResourceLimits, ScanLimits};
use std::path::PathBuf;
use std::sync::{Arc, Barrier};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("artifacts");
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory.join(format!(
                "resource-{}-{}-{}.wal",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )))
    }
    fn create(&self, limits: ResourceLimits) -> Database {
        Database::create_with_limits(&self.0, [42; 16], limits).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("wal.owner"));
    }
}

#[test]
fn st_repeated_old_generation_pressure_recovers_without_leaking_budget_or_losing_commits() {
    let f = Fixture::new();
    let db = f.create(ResourceLimits {
        max_retained_bytes: 324,
        ..ResourceLimits::default()
    });
    let mut first = db.begin().unwrap();
    first.put(b"k", &[0]).unwrap();
    first.commit().unwrap();
    for value in 1_u8..=32 {
        let mut old = db.begin().unwrap();
        let mut update = db.begin().unwrap();
        update.put(b"k", &[value]).unwrap();
        update.commit().unwrap();
        let bytes = db.stats().unwrap().wal_bytes;
        let mut rejected = db.begin().unwrap();
        rejected.put(b"k", &[255]).unwrap();
        assert!(matches!(
            rejected.commit(),
            Err(Error::ResourceExhausted(_))
        ));
        assert_eq!(db.stats().unwrap().wal_bytes, bytes);
        assert_eq!(old.get(b"k").unwrap(), Some(vec![value - 1]));
        drop(old);
        db.checkpoint().unwrap();
        let usage = db.resource_usage().unwrap();
        assert_eq!(usage.current[0], 0);
        assert_eq!(usage.current[3], 259);
        assert_eq!(usage.peak[3], 324);
    }
    drop(db);
    let reopened = Database::open_with_limits(
        &f.0,
        ResourceLimits {
            max_retained_bytes: 324,
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    assert_eq!(reopened.begin().unwrap().get(b"k").unwrap(), Some(vec![32]));
}

#[test]
fn bv_active_limit_rejects_next_and_drop_releases_slot() {
    let f = Fixture::new();
    let db = f.create(ResourceLimits {
        max_transactions: 2,
        ..ResourceLimits::default()
    });
    let a = db.begin().unwrap();
    let b = db.begin().unwrap();
    assert!(matches!(db.begin(), Err(Error::ResourceExhausted(_))));
    assert_eq!(db.resource_usage().unwrap().current[0], 2);
    drop(a);
    let c = db.begin().unwrap();
    drop(b);
    drop(c);
    assert_eq!(db.resource_usage().unwrap().current[0], 0);
}

#[test]
fn bv_staged_shared_budget_replacement_and_rejection_preserve_values() {
    let f = Fixture::new();
    let db = f.create(ResourceLimits {
        max_staged_bytes: 260,
        ..ResourceLimits::default()
    });
    let mut a = db.begin().unwrap();
    let mut b = db.begin().unwrap();
    a.put(b"a", b"x").unwrap();
    b.put(b"b", b"y").unwrap();
    assert_eq!(db.resource_usage().unwrap().current[1], 260);
    assert!(matches!(
        a.put(b"a", b"xx"),
        Err(Error::ResourceExhausted(_))
    ));
    assert_eq!(a.get(b"a").unwrap(), Some(b"x".to_vec()));
    b.rollback();
    a.put(b"a", b"xx").unwrap();
    a.commit().unwrap();
    assert_eq!(db.resource_usage().unwrap().current[1], 0);
    assert_eq!(db.begin().unwrap().get(b"a").unwrap(), Some(b"xx".to_vec()));
}

#[test]
fn bv_dependency_shared_budget_deduplicates_and_releases_on_drop() {
    let f = Fixture::new();
    let db = f.create(ResourceLimits {
        max_dependency_bytes: 194,
        ..ResourceLimits::default()
    });
    let mut a = db.begin().unwrap();
    let mut b = db.begin().unwrap();
    a.get(b"a").unwrap();
    a.get(b"a").unwrap();
    b.get(b"b").unwrap();
    assert_eq!(db.resource_usage().unwrap().current[2], 194);
    assert!(matches!(a.get(b"c"), Err(Error::ResourceExhausted(_))));
    drop(b);
    a.scan_page(b"", None, None, ScanLimits::default()).unwrap();
    let expected = db.resource_usage().unwrap().current[2];
    a.scan_page(b"", None, None, ScanLimits::default()).unwrap();
    assert_eq!(db.resource_usage().unwrap().current[2], expected);
    drop(a);
    assert_eq!(db.resource_usage().unwrap().current[2], 0);
}

#[test]
fn bv_retained_limit_rejects_before_wal_and_reopen_checks_same_budget() {
    let f = Fixture::new();
    let db = f.create(ResourceLimits {
        max_retained_bytes: 259,
        ..ResourceLimits::default()
    });
    let mut tx = db.begin().unwrap();
    tx.put(b"a", b"x").unwrap();
    tx.commit().unwrap();
    drop(db);
    let before = std::fs::read(&f.0).unwrap();
    let db = Database::open_with_limits(
        &f.0,
        ResourceLimits {
            max_retained_bytes: 259,
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    let mut tx = db.begin().unwrap();
    tx.put(b"b", b"y").unwrap();
    assert!(matches!(tx.commit(), Err(Error::ResourceExhausted(_))));
    assert_eq!(db.resource_usage().unwrap().current[3], 259);
    drop(db);
    assert_eq!(std::fs::read(&f.0).unwrap(), before);
    assert!(matches!(
        Database::open_with_limits(
            &f.0,
            ResourceLimits {
                max_retained_bytes: 258,
                ..ResourceLimits::default()
            }
        ),
        Err(Error::ResourceExhausted(_))
    ));
    assert_eq!(std::fs::read(&f.0).unwrap(), before);
    let db = Database::open_with_limits(
        &f.0,
        ResourceLimits {
            max_retained_bytes: 259,
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    assert_eq!(db.resource_usage().unwrap().current[3], 259);
}

#[test]
fn st_old_reader_keeps_history_charged_until_release() {
    let f = Fixture::new();
    let db = f.create(ResourceLimits::default());
    let mut tx = db.begin().unwrap();
    tx.put(b"a", b"x").unwrap();
    tx.commit().unwrap();
    let old = db.begin().unwrap();
    let mut tx = db.begin().unwrap();
    tx.put(b"a", b"yy").unwrap();
    tx.commit().unwrap();
    assert_eq!(db.resource_usage().unwrap().current[3], 325);
    drop(old);
    assert_eq!(db.resource_usage().unwrap().current[3], 260);
    let mut tx = db.begin().unwrap();
    tx.delete(b"a").unwrap();
    tx.commit().unwrap();
    assert_eq!(db.resource_usage().unwrap().current[3], 0);
}

#[test]
fn bv_page_buffers_share_budget_and_release_with_results() {
    let f = Fixture::new();
    let db = f.create(ResourceLimits {
        max_buffer_bytes: 300,
        ..ResourceLimits::default()
    });
    let mut tx = db.begin().unwrap();
    tx.put(b"a", b"x").unwrap();
    tx.commit().unwrap();
    let mut a = db.begin().unwrap();
    let mut b = db.begin().unwrap();
    let first = a.scan_page(b"", None, None, ScanLimits::default()).unwrap();
    let second = b.scan_page(b"", None, None, ScanLimits::default()).unwrap();
    assert_eq!(db.resource_usage().unwrap().current[4], 196);
    let third = a.scan_page(b"", None, None, ScanLimits::default()).unwrap();
    assert!(matches!(
        b.scan_page(b"", None, None, ScanLimits::default()),
        Err(Error::ResourceExhausted(_))
    ));
    drop(first);
    drop(second);
    drop(third);
    assert_eq!(db.resource_usage().unwrap().current[4], 0);
}

#[test]
fn cc_many_threads_cannot_overbook_active_or_staged_resources() {
    let f = Fixture::new();
    let db = f.create(ResourceLimits {
        max_transactions: 4,
        max_staged_bytes: 520,
        ..ResourceLimits::default()
    });
    let start = Arc::new(Barrier::new(17));
    let finish = Arc::new(Barrier::new(17));
    let workers: Vec<_> = (0..16u8)
        .map(|id| {
            let db = db.clone();
            let start = start.clone();
            let finish = finish.clone();
            std::thread::spawn(move || {
                start.wait();
                let tx = db.begin();
                match tx {
                    Ok(mut tx) => {
                        tx.put(&[id], b"x").unwrap();
                        finish.wait();
                        true
                    }
                    Err(Error::ResourceExhausted(_)) => {
                        finish.wait();
                        false
                    }
                    Err(error) => panic!("{error}"),
                }
            })
        })
        .collect();
    start.wait();
    finish.wait();
    assert_eq!(
        workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .filter(|accepted| *accepted)
            .count(),
        4
    );
    let usage = db.resource_usage().unwrap();
    assert_eq!(usage.current[0], 0);
    assert_eq!(usage.current[1], 0);
    assert_eq!(usage.peak[0], 4);
    assert_eq!(usage.peak[1], 520);
}

#[test]
fn er_recovery_buffer_refusal_does_not_truncate_existing_store() {
    let f = Fixture::new();
    let db = f.create(ResourceLimits::default());
    let mut tx = db.begin().unwrap();
    tx.put(b"a", &[1; 1024]).unwrap();
    tx.commit().unwrap();
    drop(db);
    let before = std::fs::read(&f.0).unwrap();
    assert!(matches!(
        Database::open_with_limits(
            &f.0,
            ResourceLimits {
                max_buffer_bytes: 100,
                ..ResourceLimits::default()
            }
        ),
        Err(Error::ResourceExhausted(_))
    ));
    assert_eq!(std::fs::read(&f.0).unwrap(), before);
    Database::open(&f.0).unwrap();
}

#[test]
fn st_limited_checkpoint_reopens_and_retains_all_values() {
    let f = Fixture::new();
    let limits = ResourceLimits {
        max_buffer_bytes: 4096,
        ..ResourceLimits::default()
    };
    let db = f.create(limits);
    let mut tx = db.begin().unwrap();
    for id in 0..10u8 {
        tx.put(&[id], &[id; 64]).unwrap();
    }
    tx.commit().unwrap();
    db.checkpoint().unwrap();
    drop(db);
    let db = Database::open_with_limits(&f.0, limits).unwrap();
    assert_eq!(db.begin().unwrap().scan(b"", None).unwrap().len(), 10);
    assert_eq!(db.resource_usage().unwrap().current[0], 0);
}

#[test]
fn bv_zero_limits_are_invalid_and_do_not_create_files() {
    let f = Fixture::new();
    assert!(matches!(
        Database::create_with_limits(
            &f.0,
            [0; 16],
            ResourceLimits {
                max_transactions: 0,
                ..ResourceLimits::default()
            }
        ),
        Err(Error::Invalid(_))
    ));
    assert!(!f.0.exists());
}

#[test]
fn eq_st_history_growth_preserves_each_snapshot_across_delete_reinsert_and_reopen() {
    let f = Fixture::new();
    let db = f.create(ResourceLimits::default());
    let mut snapshots = Vec::new();
    for generation in 0_u8..9 {
        let mut tx = db.begin().unwrap();
        tx.put(b"k", &[generation]).unwrap();
        tx.commit().unwrap();
        snapshots.push(db.begin_read().unwrap());
        for (expected, snapshot) in snapshots.iter_mut().enumerate() {
            assert_eq!(snapshot.get(b"k").unwrap(), Some(vec![expected as u8]));
        }
    }
    let mut remove = db.begin().unwrap();
    remove.delete(b"k").unwrap();
    remove.commit().unwrap();
    let mut deleted = db.begin_read().unwrap();
    let mut reinsert = db.begin().unwrap();
    reinsert.put(b"k", b"new").unwrap();
    reinsert.commit().unwrap();
    assert_eq!(deleted.get(b"k").unwrap(), None);
    for (expected, snapshot) in snapshots.iter_mut().enumerate() {
        assert_eq!(snapshot.get(b"k").unwrap(), Some(vec![expected as u8]));
    }
    drop(deleted);
    drop(snapshots);
    db.checkpoint().unwrap();
    drop(db);
    let reopened = Database::open(&f.0).unwrap();
    assert_eq!(
        reopened.begin_read().unwrap().get(b"k").unwrap(),
        Some(b"new".to_vec())
    );
}

#[test]
fn bv_er_history_growth_refusal_preserves_versions_and_capacity_recovers() {
    for generations in [1_usize, 2, 4, 8] {
        let f = Fixture::new();
        let db = f.create(ResourceLimits {
            max_retained_bytes: 194 + 65 * generations,
            ..ResourceLimits::default()
        });
        let mut initial = db.begin().unwrap();
        initial.put(b"k", &[0]).unwrap();
        initial.commit().unwrap();
        let mut old = db.begin_read().unwrap();
        for generation in 1..generations {
            let mut tx = db.begin().unwrap();
            tx.put(b"k", &[generation as u8]).unwrap();
            tx.commit().unwrap();
        }
        let wal_bytes = db.stats().unwrap().wal_bytes;
        let mut rejected = db.begin().unwrap();
        rejected.put(b"k", &[255]).unwrap();
        assert!(matches!(
            rejected.commit(),
            Err(Error::ResourceExhausted(_))
        ));
        assert_eq!(db.stats().unwrap().wal_bytes, wal_bytes);
        assert_eq!(old.get(b"k").unwrap(), Some(vec![0]));
        assert_eq!(
            db.begin_read().unwrap().get(b"k").unwrap(),
            Some(vec![(generations - 1) as u8])
        );
        drop(old);
        db.checkpoint().unwrap();
        assert_eq!(db.resource_usage().unwrap().current[3], 259);
        // A one-version budget cannot admit an update before its old value is retired.
        if generations > 1 {
            let mut next = db.begin().unwrap();
            next.put(b"k", &[99]).unwrap();
            next.commit().unwrap();
        }
        drop(db);
        let reopened = Database::open(&f.0).unwrap();
        assert_eq!(
            reopened.begin_read().unwrap().get(b"k").unwrap(),
            Some(vec![if generations > 1 { 99 } else { 0 }])
        );
    }
}
