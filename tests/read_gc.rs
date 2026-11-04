use newdbms::transactions::{Database, Error, ResourceLimits, ScanLimits};
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
#[test]
fn eq_read_only_tracks_snapshot_without_dependencies_and_rejects_all_writes() {
    let f = Fixture::new();
    let db = f.db(ResourceLimits {
        max_dependency_bytes: 1,
        ..ResourceLimits::default()
    });
    let mut w = db.begin().unwrap();
    for i in 0u8..10 {
        w.put(&[i], &[i]).unwrap();
    }
    w.commit().unwrap();
    let mut r = db.begin_read().unwrap();
    let v = r.version();
    assert_eq!(r.get(&[0]).unwrap(), Some(vec![0]));
    assert!(r.get(&[99]).unwrap().is_none());
    assert_eq!(r.scan(&[], None).unwrap().len(), 10);
    assert!(matches!(r.put(b"x", b"x"), Err(Error::Invalid(_))));
    assert!(matches!(r.delete(&[0]), Err(Error::Invalid(_))));
    let page = r
        .scan_page(
            &[],
            None,
            None,
            ScanLimits {
                max_rows: 3,
                ..ScanLimits::default()
            },
        )
        .unwrap();
    let mut w = db.begin().unwrap();
    w.delete(&[4]).unwrap();
    w.put(&[5], b"new").unwrap();
    w.commit().unwrap();
    let next = r
        .scan_page(&[], None, page.next_after.as_deref(), ScanLimits::default())
        .unwrap();
    assert_eq!(next.rows.len(), 7);
    assert_eq!(next.rows[1], (vec![4], vec![4]));
    assert_eq!(next.rows[2], (vec![5], vec![5]));
    assert_eq!(db.resource_usage().unwrap().current[2], 0);
    assert_eq!(r.commit().unwrap(), v);
    assert!(db.begin_read().unwrap().get(&[4]).unwrap().is_none());
}
#[test]
fn bv_gc_splits_many_keys_and_keeps_unfreed_versions_charged_until_progress() {
    let f = Fixture::new();
    let db = f.db(ResourceLimits::default());
    let mut w = db.begin().unwrap();
    for i in 0u32..200 {
        w.put(&i.to_be_bytes(), &[1; 16]).unwrap();
    }
    w.commit().unwrap();
    let original = db.resource_usage().unwrap().current[3];
    let mut old = db.begin_read().unwrap();
    let mut w = db.begin().unwrap();
    for i in 0u32..200 {
        w.put(&i.to_be_bytes(), &[2; 16]).unwrap();
    }
    w.commit().unwrap();
    assert_eq!(
        old.scan(&[], None).unwrap(),
        (0u32..200)
            .map(|i| (i.to_be_bytes().to_vec(), vec![1; 16]))
            .collect::<Vec<_>>()
    );
    assert_eq!(db.stats().unwrap().retained_versions, 400);
    drop(old);
    assert_eq!(db.stats().unwrap().retained_versions, 336);
    assert_eq!(db.resource_usage().unwrap().current[3], original + 136 * 80);
    let mut steps = 0;
    while db.collect_garbage().unwrap() {
        steps += 1;
        assert!(steps < 10);
    }
    assert_eq!(db.stats().unwrap().retained_versions, 200);
    assert_eq!(db.resource_usage().unwrap().current[3], original);
    drop(db);
    let db = Database::open(f.0.join("db")).unwrap();
    assert_eq!(
        db.begin_read().unwrap().scan(&[], None).unwrap(),
        (0u32..200)
            .map(|i| (i.to_be_bytes().to_vec(), vec![2; 16]))
            .collect::<Vec<_>>()
    );
}
#[test]
fn bv_one_hot_key_reclaims_at_most_128_versions_per_step() {
    let f = Fixture::new();
    let db = f.db(ResourceLimits::default());
    let mut w = db.begin().unwrap();
    w.put(b"k", b"old").unwrap();
    w.commit().unwrap();
    let mut old = db.begin_read().unwrap();
    for i in 0u32..500 {
        let mut w = db.begin().unwrap();
        w.put(b"k", &i.to_be_bytes()).unwrap();
        w.commit().unwrap();
    }
    assert_eq!(old.get(b"k").unwrap(), Some(b"old".to_vec()));
    drop(old);
    assert_eq!(db.stats().unwrap().retained_versions, 373);
    while db.collect_garbage().unwrap() {}
    assert_eq!(db.stats().unwrap().retained_versions, 1);
    assert_eq!(
        db.begin_read().unwrap().get(b"k").unwrap(),
        Some(499u32.to_be_bytes().to_vec())
    );
}
#[test]
fn bv_gc_byte_budget_and_oversized_value_both_make_progress() {
    for bytes in [1024 * 1024, 1024 * 1024 + 1] {
        let f = Fixture::new();
        let db = f.db(ResourceLimits::default());
        let mut w = db.begin().unwrap();
        w.put(b"k", &vec![1; bytes]).unwrap();
        w.commit().unwrap();
        let old = db.begin_read().unwrap();
        for _ in 0..3 {
            let mut w = db.begin().unwrap();
            w.put(b"k", &vec![2; bytes]).unwrap();
            w.commit().unwrap();
        }
        drop(old);
        assert_eq!(db.stats().unwrap().retained_versions, 3);
        while db.collect_garbage().unwrap() {}
        assert_eq!(db.stats().unwrap().retained_versions, 1);
    }
}

#[test]
fn er_partial_reclamation_pressure_rejects_without_logging_and_recovers_after_drain() {
    let f = Fixture::new();
    let db = f.db(ResourceLimits {
        max_retained_bytes: 72_000,
        ..ResourceLimits::default()
    });
    let mut seed = db.begin().unwrap();
    for i in 0u32..200 {
        seed.put(&i.to_be_bytes(), &[1; 16]).unwrap();
    }
    seed.commit().unwrap();
    let old = db.begin_read().unwrap();
    let mut update = db.begin().unwrap();
    for i in 0u32..200 {
        update.put(&i.to_be_bytes(), &[2; 16]).unwrap();
    }
    update.commit().unwrap();
    drop(old);
    let wal_bytes = db.stats().unwrap().wal_bytes;
    let mut rejected = db.begin().unwrap();
    for i in 0u32..200 {
        rejected.put(&i.to_be_bytes(), &[3; 16]).unwrap();
    }
    assert!(matches!(
        rejected.commit(),
        Err(Error::ResourceExhausted(_))
    ));
    assert_eq!(db.stats().unwrap().wal_bytes, wal_bytes);
    while db.collect_garbage().unwrap() {}
    assert_eq!(db.resource_usage().unwrap().current[3], 56_000);
    let mut read = db.begin_read().unwrap();
    assert!(
        read.scan(&[], None)
            .unwrap()
            .iter()
            .all(|(_, v)| v == &[2; 16])
    );
    drop(read);
    let mut retry = db.begin().unwrap();
    for i in 0u32..200 {
        retry.put(&i.to_be_bytes(), &[3; 16]).unwrap();
    }
    retry.commit().unwrap();
    while db.collect_garbage().unwrap() {}
    assert_eq!(db.resource_usage().unwrap().current[3], 56_000);
    drop(db);
    let db = Database::open(f.0.join("db")).unwrap();
    let values = db.begin_read().unwrap().scan(&[], None).unwrap();
    assert_eq!(values.len(), 200);
    assert!(values.iter().all(|(_, v)| v == &[3; 16]));
}
