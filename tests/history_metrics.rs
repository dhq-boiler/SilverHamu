#![cfg(feature = "history-metrics")]
use newdbms::transactions::{Database, Error, ResourceLimits};
use std::path::PathBuf;
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("artifacts");
        std::fs::create_dir_all(&p).unwrap();
        Self(
            p.join(format!(
                "history-metrics-{}-{}.wal",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            )),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("wal.owner"));
    }
}
#[test]
fn eq_st_metrics_observe_empty_growth_and_release_without_changing_snapshots() {
    let f = Fixture::new();
    let db = Database::create(&f.0, [81; 16]).unwrap();
    assert_eq!(db.history_metrics().unwrap().versions, 0);
    let mut snapshots = Vec::new();
    for n in 1..=9u8 {
        let mut tx = db.begin().unwrap();
        tx.put(b"key", &[n; 256]).unwrap();
        tx.commit().unwrap();
        snapshots.push(db.begin_read().unwrap());
        let before = db.current_version().unwrap();
        let m = db.history_metrics().unwrap();
        assert_eq!(m.records, 1);
        assert_eq!(m.versions, n as usize);
        assert!(m.capacity_slots >= m.versions);
        assert_eq!(m.value_bytes, 256 * n as usize);
        assert_eq!(db.current_version().unwrap(), before);
        for (i, old) in snapshots.iter_mut().enumerate() {
            assert_eq!(old.get(b"key").unwrap(), Some(vec![i as u8 + 1; 256]));
        }
    }
    drop(snapshots);
    while db.collect_garbage().unwrap() {}
    let m = db.history_metrics().unwrap();
    assert_eq!(m.versions, 1);
    assert_eq!(m.value_bytes, 256);
    assert_eq!(m.history_keys, 0);
    assert!(m.capacity_slots >= 1);
    drop(db);
    let reopened = Database::open(&f.0).unwrap();
    assert_eq!(
        reopened.begin_read().unwrap().get(b"key").unwrap(),
        Some(vec![9; 256])
    );
}
#[test]
fn bv_er_refused_update_keeps_metrics_and_data_and_release_restores_admission() {
    let f = Fixture::new();
    let db = Database::create_with_limits(
        &f.0,
        [82; 16],
        ResourceLimits {
            max_retained_bytes: 1000,
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    let key = 0u64.to_be_bytes();
    let mut tx = db.begin().unwrap();
    tx.put(&key, &[1; 256]).unwrap();
    tx.commit().unwrap();
    let mut old = db.begin_read().unwrap();
    let mut tx = db.begin().unwrap();
    tx.put(&key, &[2; 256]).unwrap();
    tx.commit().unwrap();
    let before = db.history_metrics().unwrap();
    let seq = db.current_version().unwrap();
    let mut tx = db.begin().unwrap();
    tx.put(&key, &[3; 256]).unwrap();
    assert!(matches!(tx.commit(), Err(Error::ResourceExhausted(_))));
    assert_eq!(db.history_metrics().unwrap(), before);
    assert_eq!(db.current_version().unwrap(), seq);
    assert_eq!(old.get(&key).unwrap(), Some(vec![1; 256]));
    drop(old);
    while db.collect_garbage().unwrap() {}
    let mut tx = db.begin().unwrap();
    tx.put(&key, &[3; 256]).unwrap();
    tx.commit().unwrap();
    assert_eq!(
        db.begin_read().unwrap().get(&key).unwrap(),
        Some(vec![3; 256])
    );
}
