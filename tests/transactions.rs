use silverhamu::Snapshot;
use silverhamu::transactions::{
    ConflictKind, Database, Error, MAX_KEY_BYTES, MAX_TRANSACTION_WRITES, MAX_VALUE_BYTES,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;

struct Fixture(PathBuf);

#[cfg(feature = "lock-timing")]
#[test]
fn checkpoint_replace_trace_is_bounded_by_the_call_and_consumed_once() {
    use silverhamu::transactions::{
        diagnostic_now_ns, take_checkpoint_replace_trace, take_checkpoint_timing,
    };
    let fixture = Fixture::new();
    let db = fixture.create();
    let before = diagnostic_now_ns();
    db.checkpoint().unwrap();
    let after = diagnostic_now_ns();
    let [start, end] = take_checkpoint_replace_trace();
    assert!(before <= start && start <= end && end <= after);
    assert!(end - start <= take_checkpoint_timing()[4]);
    assert_eq!(take_checkpoint_replace_trace(), [0, 0]);
}
impl Fixture {
    fn new() -> Self {
        static ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        fs::create_dir_all(Path::new(env!("CARGO_MANIFEST_DIR")).join("artifacts")).unwrap();
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("artifacts")
            .join(format!("newdbms-tx-{}-{time}-{id}.wal", std::process::id()));
        Self(path)
    }
    fn create(&self) -> Database {
        Database::create(&self.0, [9; 16]).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let mut owner = self.0.as_os_str().to_owned();
        owner.push(".owner");
        let _ = fs::remove_file(PathBuf::from(owner));
    }
}
fn put(db: &Database, key: &[u8], value: &[u8]) -> u64 {
    let mut tx = db.begin().unwrap();
    tx.put(key, value).unwrap();
    tx.commit().unwrap()
}
fn value(db: &Database, key: &[u8]) -> Option<Vec<u8>> {
    db.begin().unwrap().get(key).unwrap()
}

#[test]
fn row_reservations_exclude_competing_commits_and_follow_savepoints() {
    let fixture = Fixture::new();
    let db = fixture.create();
    put(&db, b"a", b"before");
    let mut staged = db.begin().unwrap();
    staged.put(b"a", b"other").unwrap();
    let mut owner = db.begin().unwrap();
    owner.lock_keys(&[b"a"]).unwrap();
    owner.savepoint("first").unwrap();
    owner.lock_keys(&[b"b", b"a"]).unwrap();
    assert!(matches!(staged.commit(), Err(Error::Conflict(ConflictKind::Write))));
    let mut other = db.begin().unwrap();
    assert!(matches!(other.put(b"a", b"blocked"), Err(Error::Conflict(ConflictKind::Write))));
    assert!(matches!(other.lock_keys(&[b"free", b"b"]), Err(Error::Conflict(_))));
    owner.lock_keys(&[b"free"]).unwrap();
    owner.rollback_to_savepoint("first").unwrap();
    other.lock_keys(&[b"b", b"free"]).unwrap();
    assert!(matches!(other.lock_keys(&[b"a"]), Err(Error::Conflict(_))));
    drop(other);
    owner.put(b"a", b"after").unwrap();
    owner.commit().unwrap();
    assert_eq!(value(&db, b"a").as_deref(), Some(b"after".as_slice()));
    let mut stale = db.begin().unwrap();
    put(&db, b"a", b"newer");
    assert!(matches!(stale.lock_keys(&[b"a"]), Err(Error::Conflict(_))));
    db.begin().unwrap().lock_keys(&[b"a"]).unwrap();
    assert!(db.begin_read().unwrap().lock_keys(&[b"a"]).is_err());
}

#[test]
fn savepoint_undo_preserves_snapshot_and_releases_staging_budget() {
    let fixture = Fixture::new();
    let db = fixture.create();
    put(&db, b"stable", b"old");
    let mut tx = db.begin().unwrap();
    tx.put(b"first", b"before").unwrap();
    let baseline = db.resource_usage().unwrap().current[1];
    tx.savepoint("s").unwrap();
    tx.delete(b"first").unwrap();
    tx.put(b"new", b"after").unwrap();
    tx.savepoint("s").unwrap();
    tx.put(b"first", b"temporary").unwrap();
    tx.rollback_to_savepoint("s").unwrap();
    assert_eq!(tx.get(b"first").unwrap(), None);
    tx.release_savepoint("s").unwrap();
    put(&db, b"stable", b"concurrent");
    tx.rollback_to_savepoint("s").unwrap();
    assert_eq!(db.resource_usage().unwrap().current[1], baseline);
    assert_eq!(tx.get(b"new").unwrap(), None);
    assert_eq!(tx.get(b"first").unwrap().as_deref(), Some(&b"before"[..]));
    assert_eq!(tx.get(b"stable").unwrap().as_deref(), Some(&b"old"[..]));
    assert!(matches!(tx.commit(), Err(Error::Conflict(_))));
    assert_eq!(db.resource_usage().unwrap().current[1], 0);
}

#[test]
fn eq_binary_empty_values_and_deleted_values_are_distinct_and_durable() {
    let f = Fixture::new();
    let db = f.create();
    let mut tx = db.begin().unwrap();
    tx.put(b"", b"").unwrap();
    tx.put("日本語".as_bytes(), &[0, 255, 0]).unwrap();
    tx.delete(b"absent").unwrap();
    assert_eq!(tx.get(b"").unwrap(), Some(vec![]));
    assert_eq!(tx.get(b"absent").unwrap(), None);
    assert_eq!(tx.commit().unwrap(), 1);
    assert_eq!(db.id(), [9; 16]);
    drop(db);
    let db = Database::open(&f.0).unwrap();
    assert_eq!(value(&db, b""), Some(vec![]));
    assert_eq!(value(&db, "日本語".as_bytes()), Some(vec![0, 255, 0]));
    assert_eq!(db.stats().unwrap().version, 1);
}

#[test]
fn st_rollback_drop_and_read_only_commit_do_not_append_or_publish() {
    let f = Fixture::new();
    let db = f.create();
    let original = db.stats().unwrap().wal_bytes;
    let mut a = db.begin().unwrap();
    a.put(b"a", b"rolled back").unwrap();
    a.rollback();
    {
        let mut a = db.begin().unwrap();
        a.put(b"a", b"dropped").unwrap();
    }
    assert_eq!(db.begin().unwrap().commit().unwrap(), 0);
    assert_eq!(value(&db, b"a"), None);
    assert_eq!(db.stats().unwrap().wal_bytes, original);
    assert_eq!(db.stats().unwrap().active_transactions, 0);
}

#[test]
fn st_eight_transactions_stage_together_and_commit_disjoint_records() {
    let f = Fixture::new();
    let db = f.create();
    let staged = Arc::new(Barrier::new(9));
    let publish = Arc::new(Barrier::new(9));
    let workers: Vec<_> = (0_u8..8)
        .map(|id| {
            let db = db.clone();
            let staged = staged.clone();
            let publish = publish.clone();
            thread::spawn(move || {
                let mut tx = db.begin().unwrap();
                tx.put(&[id], &[id, 255]).unwrap();
                staged.wait();
                publish.wait();
                tx.commit().unwrap()
            })
        })
        .collect();
    staged.wait();
    assert_eq!(db.stats().unwrap().active_transactions, 8);
    assert_eq!(db.stats().unwrap().visible_records, 0);
    let mut old = db.begin().unwrap();
    publish.wait();
    let mut versions: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    versions.sort();
    assert_eq!(versions, (1..=8).collect::<Vec<_>>());
    assert!(old.scan(b"", None).unwrap().is_empty());
    drop(old);
    assert_eq!(db.stats().unwrap().visible_records, 8);
    assert_eq!(db.stats().unwrap().retained_versions, 8);
    drop(db);
    let db = Database::open(&f.0).unwrap();
    assert_eq!(db.stats().unwrap().version, 8);
}

#[test]
fn dt_same_record_race_has_exactly_one_winner_without_partial_writes() {
    let f = Fixture::new();
    let db = f.create();
    let barrier = Arc::new(Barrier::new(8));
    let workers: Vec<_> = (0_u8..8)
        .map(|id| {
            let db = db.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                let mut tx = db.begin().unwrap();
                tx.put(b"shared", &[id]).unwrap();
                tx.put(&[id], b"side effect").unwrap();
                barrier.wait();
                tx.commit()
            })
        })
        .collect();
    let results: Vec<_> = workers.into_iter().map(|w| w.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::Conflict(ConflictKind::Write))))
            .count(),
        7
    );
    assert_eq!(db.stats().unwrap().visible_records, 2);
}

#[test]
fn st_snapshot_reads_remain_stable_and_old_versions_are_collected_after_release() {
    let f = Fixture::new();
    let db = f.create();
    put(&db, b"a", b"old");
    let mut old = db.begin().unwrap();
    for i in 0_u8..10 {
        put(&db, b"a", &[i]);
    }
    assert_eq!(old.get(b"a").unwrap(), Some(b"old".to_vec()));
    assert_eq!(db.stats().unwrap().retained_versions, 11);
    assert_eq!(old.commit().unwrap(), 1);
    assert_eq!(db.stats().unwrap().retained_versions, 1);
    assert_eq!(value(&db, b"a"), Some(vec![9]));
}

#[test]
fn dt_read_write_dependency_rejects_write_skew() {
    let f = Fixture::new();
    let db = f.create();
    let mut seed = db.begin().unwrap();
    seed.put(b"a", b"1").unwrap();
    seed.put(b"b", b"1").unwrap();
    seed.commit().unwrap();
    let mut a = db.begin().unwrap();
    let mut b = db.begin().unwrap();
    assert_eq!(a.get(b"b").unwrap(), Some(b"1".to_vec()));
    assert_eq!(b.get(b"a").unwrap(), Some(b"1".to_vec()));
    a.put(b"a", b"0").unwrap();
    b.put(b"b", b"0").unwrap();
    a.commit().unwrap();
    assert!(matches!(
        b.commit(),
        Err(Error::Conflict(ConflictKind::Read))
    ));
    assert_eq!(value(&db, b"b"), Some(b"1".to_vec()));
}

#[test]
fn dt_missing_read_detects_insert_then_delete_even_when_current_value_is_absent() {
    let f = Fixture::new();
    let db = f.create();
    let mut tx = db.begin().unwrap();
    assert_eq!(tx.get(b"missing").unwrap(), None);
    put(&db, b"missing", b"temporary");
    let mut other = db.begin().unwrap();
    other.delete(b"missing").unwrap();
    other.commit().unwrap();
    tx.put(b"side", b"must not commit").unwrap();
    assert!(matches!(
        tx.commit(),
        Err(Error::Conflict(ConflictKind::Read))
    ));
    assert_eq!(value(&db, b"side"), None);
    assert_eq!(db.stats().unwrap().retained_versions, 0);
}

#[test]
fn dt_empty_range_detects_phantoms_and_deleted_phantoms() {
    for deleted in [false, true] {
        let f = Fixture::new();
        let db = f.create();
        let mut a = db.begin().unwrap();
        assert!(a.scan(b"a", Some(b"z")).unwrap().is_empty());
        put(&db, b"m", b"phantom");
        if deleted {
            let mut d = db.begin().unwrap();
            d.delete(b"m").unwrap();
            d.commit().unwrap();
        }
        a.put(b"zz", b"side").unwrap();
        assert!(matches!(
            a.commit(),
            Err(Error::Conflict(ConflictKind::Range))
        ));
        assert_eq!(value(&db, b"zz"), None);
    }
}

#[test]
fn bv_half_open_ranges_exclude_end_and_include_own_changes() {
    let f = Fixture::new();
    let db = f.create();
    put(&db, b"a", b"old");
    put(&db, b"b", b"old");
    let mut tx = db.begin().unwrap();
    tx.delete(b"a").unwrap();
    tx.put(b"b", b"new").unwrap();
    tx.put(b"c", b"new").unwrap();
    assert_eq!(
        tx.scan(b"a", Some(b"c")).unwrap(),
        vec![(b"b".to_vec(), b"new".to_vec())]
    );
    assert!(tx.scan(b"b", Some(b"b")).unwrap().is_empty());
    assert!(matches!(tx.scan(b"z", Some(b"a")), Err(Error::Invalid(_))));
    put(&db, b"d", b"outside");
    tx.commit().unwrap();
    let mut empty = db.begin().unwrap();
    empty.scan(b"z", Some(b"z")).unwrap();
    put(&db, b"z", b"outside empty range");
    empty.put(b"zz", b"ok").unwrap();
    empty.commit().unwrap();
}

#[test]
fn bv_exclusive_end_insert_does_not_conflict() {
    let f = Fixture::new();
    let db = f.create();
    let mut tx = db.begin().unwrap();
    tx.scan(b"a", Some(b"z")).unwrap();
    put(&db, b"z", b"outside");
    tx.put(b"b", b"ok").unwrap();
    tx.commit().unwrap();
}

#[test]
fn st_deletion_retains_metadata_until_old_snapshots_release_then_reclaims_it() {
    let f = Fixture::new();
    let db = f.create();
    put(&db, b"seed", b"1");
    let pin = db.begin().unwrap();
    let mut tx = db.begin().unwrap();
    tx.delete(b"never existed").unwrap();
    tx.commit().unwrap();
    let pin2 = db.begin().unwrap();
    drop(pin);
    assert_eq!(db.stats().unwrap().retained_versions, 1);
    drop(pin2);
    assert_eq!(db.stats().unwrap().retained_versions, 1);
    let mut tx = db.begin().unwrap();
    tx.delete(b"seed").unwrap();
    tx.commit().unwrap();
    assert_eq!(db.stats().unwrap().retained_versions, 0);
}

#[test]
fn bv_key_value_and_operation_count_limits_preserve_previous_staged_value() {
    let f = Fixture::new();
    let db = f.create();
    let mut tx = db.begin().unwrap();
    tx.put(&vec![1; MAX_KEY_BYTES], b"ok").unwrap();
    assert!(matches!(
        tx.put(&vec![1; MAX_KEY_BYTES + 1], b"no"),
        Err(Error::Limit(_))
    ));
    let value = vec![7; MAX_VALUE_BYTES];
    tx.put(b"large", &value).unwrap();
    assert!(matches!(
        tx.put(b"large", &vec![8; MAX_VALUE_BYTES + 1]),
        Err(Error::Limit(_))
    ));
    assert_eq!(tx.get(b"large").unwrap().unwrap()[0], 7);
    tx.rollback();
    let mut tx = db.begin().unwrap();
    for i in 0..MAX_TRANSACTION_WRITES {
        tx.put(&(i as u32).to_be_bytes(), b"").unwrap();
    }
    assert!(matches!(tx.put(b"extra", b""), Err(Error::Limit(_))));
    tx.put(&0_u32.to_be_bytes(), b"replacement").unwrap();
    tx.rollback();
    assert_eq!(db.stats().unwrap().version, 0);
}

#[test]
fn st_existing_file_is_never_overwritten_and_owner_is_held_by_transactions() {
    let f = Fixture::new();
    let db = f.create();
    put(&db, b"a", b"v");
    assert!(matches!(Database::create(&f.0, [1; 16]), Err(Error::Io(_))));
    assert!(matches!(Database::open(&f.0), Err(Error::Locked(_))));
    let tx = db.begin().unwrap();
    drop(db);
    assert!(matches!(Database::open(&f.0), Err(Error::Locked(_))));
    drop(tx);
    assert_eq!(
        value(&Database::open(&f.0).unwrap(), b"a"),
        Some(b"v".to_vec())
    );
}

#[test]
fn st_export_values_uses_existing_snapshot_format_and_excludes_uncommitted_writes() {
    let f = Fixture::new();
    let db = f.create();
    put(&db, b"catalog/a", b"first opaque payload");
    put(&db, b"catalog/b", b"second opaque payload");
    put(&db, b"other", b"not catalog");
    let mut pending = db.begin().unwrap();
    pending.put(b"catalog/a", b"uncommitted").unwrap();
    let out = Fixture::new();
    let result = db
        .export_values(&out.0, [4; 16], b"catalog/", Some(b"catalog0"))
        .unwrap();
    assert_eq!(result.version, 3);
    assert_eq!(result.records, 2);
    let mut snapshot = Snapshot::open(&out.0, [4; 16]).unwrap();
    assert_eq!(
        snapshot.read_page(0, 10).unwrap(),
        vec![
            b"first opaque payload".to_vec(),
            b"second opaque payload".to_vec()
        ]
    );
    drop(snapshot);
    assert!(db.export_values(&out.0, [4; 16], b"", None).is_err());
}

fn child(path: &Path, mode: &str) -> std::process::Output {
    std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "process_worker", "--nocapture"])
        .env("NEWDBMS_TX_CHILD_PATH", path)
        .env("NEWDBMS_TX_CHILD_MODE", mode)
        .output()
        .unwrap()
}

#[test]
#[ignore = "subprocess helper"]
fn process_worker() {
    let path = std::env::var_os("NEWDBMS_TX_CHILD_PATH").unwrap();
    let mode = std::env::var("NEWDBMS_TX_CHILD_MODE").unwrap();
    if mode == "lock" {
        assert!(matches!(Database::open(path), Err(Error::Locked(_))));
        std::process::exit(73);
    }
    let db = Database::open(path).unwrap();
    let mut tx = db.begin().unwrap();
    tx.put(b"new", b"after crash").unwrap();
    tx.put(b"paired", b"atomic").unwrap();
    if mode == "after" {
        tx.commit().unwrap();
        std::process::exit(72);
    }
    std::process::exit(71);
}

#[test]
fn er_os_ownership_lock_is_enforced_in_another_process() {
    let f = Fixture::new();
    let _db = f.create();
    let result = child(&f.0, "lock");
    assert_eq!(result.status.code(), Some(73), "{:?}", result);
}

#[test]
fn er_process_exit_before_and_after_commit_recovers_only_committed_changes() {
    for mode in ["before", "after"] {
        let f = Fixture::new();
        let db = f.create();
        put(&db, b"old", b"durable");
        drop(db);
        let result = child(&f.0, mode);
        assert_eq!(
            result.status.code(),
            Some(if mode == "after" { 72 } else { 71 }),
            "{:?}",
            result
        );
        let db = Database::open(&f.0).unwrap();
        assert_eq!(value(&db, b"old"), Some(b"durable".to_vec()));
        assert_eq!(value(&db, b"new").is_some(), mode == "after");
        assert_eq!(value(&db, b"paired").is_some(), mode == "after");
    }
}

#[test]
fn er_every_incomplete_tail_boundary_recovers_valid_prefix_and_accepts_new_commit() {
    let f = Fixture::new();
    let db = Database::create_append(&f.0, [9; 16]).unwrap();
    put(&db, b"old", b"one");
    let boundary = db.stats().unwrap().wal_bytes as usize;
    put(&db, b"new", b"two");
    drop(db);
    let bytes = fs::read(&f.0).unwrap();
    for length in boundary..bytes.len() {
        let copy = Fixture::new();
        fs::write(&copy.0, &bytes[..length]).unwrap();
        let db = Database::open(&copy.0).unwrap();
        assert_eq!(db.stats().unwrap().wal_bytes, boundary as u64);
        assert_eq!(value(&db, b"old"), Some(b"one".to_vec()));
        assert_eq!(value(&db, b"new"), None);
        put(&db, b"next", b"valid");
        drop(db);
        assert_eq!(
            value(&Database::open(&copy.0).unwrap(), b"next"),
            Some(b"valid".to_vec())
        );
    }
}

#[test]
fn er_complete_header_prefix_payload_and_seal_corruption_is_rejected_without_modifying_file() {
    let f = Fixture::new();
    let db = Database::create_append(&f.0, [9; 16]).unwrap();
    put(&db, b"key", b"value");
    drop(db);
    let bytes = fs::read(&f.0).unwrap();
    for at in [
        0,
        8,
        12,
        16,
        32,
        44,
        48,
        56,
        60,
        64,
        68,
        bytes.len() - 12,
        bytes.len() - 1,
    ] {
        let copy = Fixture::new();
        let mut corrupt = bytes.clone();
        corrupt[at] ^= 128;
        fs::write(&copy.0, &corrupt).unwrap();
        assert!(
            matches!(Database::open(&copy.0), Err(Error::Invalid(_))),
            "offset {at}"
        );
        assert_eq!(fs::read(&copy.0).unwrap(), corrupt);
    }
}

#[test]
fn st_concurrent_readers_never_observe_half_of_a_committed_pair() {
    let f = Fixture::new();
    let db = f.create();
    let mut seed = db.begin().unwrap();
    seed.put(b"a", b"0").unwrap();
    seed.put(b"b", b"0").unwrap();
    seed.commit().unwrap();
    let writer = db.clone();
    let handle = thread::spawn(move || {
        for i in 0_u32..100 {
            let mut tx = writer.begin().unwrap();
            tx.put(b"a", &i.to_le_bytes()).unwrap();
            tx.put(b"b", &i.to_le_bytes()).unwrap();
            tx.commit().unwrap();
        }
    });
    for _ in 0..200 {
        let mut tx = db.begin().unwrap();
        assert_eq!(tx.get(b"a").unwrap(), tx.get(b"b").unwrap());
    }
    handle.join().unwrap();
    assert_eq!(db.stats().unwrap().version, 101);
}

#[test]
fn bv_transaction_body_limit_allows_exact_maximum_and_rejects_next_byte() {
    use silverhamu::transactions::MAX_TRANSACTION_BYTES;
    let f = Fixture::new();
    let db = f.create();
    let mut tx = db.begin().unwrap();
    let large = vec![5; MAX_VALUE_BYTES];
    for key in 0..4_u8 {
        tx.put(&[key], &large).unwrap();
    }
    let remaining = MAX_TRANSACTION_BYTES - (12 + 4 * (10 + MAX_VALUE_BYTES)) - 10;
    tx.put(&[4], &vec![6; remaining]).unwrap();
    assert!(matches!(
        tx.put(&[4], &vec![7; remaining + 1]),
        Err(Error::Limit(_))
    ));
    assert_eq!(tx.get(&[4]).unwrap(), Some(vec![6; remaining]));
    tx.commit().unwrap();
    drop(db);
    let db = Database::open(&f.0).unwrap();
    assert_eq!(value(&db, &[0]).unwrap().len(), MAX_VALUE_BYTES);
    assert_eq!(db.stats().unwrap().visible_records, 5);
}

#[test]
fn st_checkpoint_shrinks_history_preserves_active_transactions_and_commit_sequence() {
    let f = Fixture::new();
    let db = f.create();
    put(&db, b"key", b"old");
    let mut old = db.begin().unwrap();
    for _ in 0..40 {
        put(&db, b"key", b"latest");
    }
    let mut staged = db.begin().unwrap();
    staged.put(b"new", b"pending").unwrap();
    let info = db.checkpoint().unwrap();
    assert_eq!(info.version, 41);
    assert!(info.after_bytes < info.before_bytes / 4);
    assert_eq!(old.get(b"key").unwrap(), Some(b"old".to_vec()));
    staged.commit().unwrap();
    old.put(b"side", b"reject").unwrap();
    assert!(matches!(
        old.commit(),
        Err(Error::Conflict(ConflictKind::Read))
    ));
    assert!(matches!(Database::open(&f.0), Err(Error::Locked(_))));
    drop(db);
    let db = Database::open(&f.0).unwrap();
    assert_eq!(db.stats().unwrap().version, 42);
    assert_eq!(value(&db, b"new"), Some(b"pending".to_vec()));
    assert_eq!(value(&db, b"key"), Some(b"latest".to_vec()));
    assert_eq!(put(&db, b"next", b"43"), 43);
}

#[test]
fn st_checkpoint_of_empty_committed_database_keeps_version_and_allows_new_writes() {
    let f = Fixture::new();
    let db = f.create();
    put(&db, b"a", b"one");
    let mut tx = db.begin().unwrap();
    tx.delete(b"a").unwrap();
    tx.commit().unwrap();
    let cp = db.checkpoint().unwrap();
    assert_eq!(cp.version, 2);
    assert_eq!(cp.after_bytes, 48);
    drop(db);
    let db = Database::open(&f.0).unwrap();
    assert_eq!(db.stats().unwrap().version, 2);
    assert_eq!(value(&db, b"a"), None);
    assert_eq!(put(&db, b"b", b"three"), 3);
}

#[test]
fn st_checkpoint_splits_more_than_one_transaction_of_live_keys() {
    let f = Fixture::new();
    let db = f.create();
    let mut tx = db.begin().unwrap();
    for n in 0..MAX_TRANSACTION_WRITES {
        tx.put(&(n as u32).to_be_bytes(), b"v").unwrap();
    }
    tx.commit().unwrap();
    put(&db, b"extra", b"last");
    let cp = db.checkpoint().unwrap();
    assert_eq!(cp.records, MAX_TRANSACTION_WRITES + 1);
    assert_eq!(cp.version, 2);
    drop(db);
    let bytes = fs::read(&f.0).unwrap();
    let chunks = u32::from_le_bytes(bytes[40..44].try_into().unwrap()) & 0x7fff_ffff;
    assert!(chunks >= 2);
    let db = Database::open(&f.0).unwrap();
    assert_eq!(
        db.stats().unwrap().visible_records,
        MAX_TRANSACTION_WRITES + 1
    );
    assert_eq!(value(&db, b"extra"), Some(b"last".to_vec()));
    for n in 0..MAX_TRANSACTION_WRITES {
        assert_eq!(value(&db, &(n as u32).to_be_bytes()), Some(b"v".to_vec()));
    }
}

#[test]
fn st_checkpoint_and_parallel_updates_preserve_all_completed_writes() {
    let f = Fixture::new();
    let db = f.create();
    let barrier = Arc::new(Barrier::new(10));
    let workers: Vec<_> = (0_u8..8)
        .map(|id| {
            let db = db.clone();
            let barrier = barrier.clone();
            thread::spawn(move || {
                barrier.wait();
                for n in 0_u8..20 {
                    put(&db, &[id], &[n]);
                }
            })
        })
        .collect();
    let checkpoint_db = db.clone();
    let checkpoint_barrier = barrier.clone();
    let checkpointer = thread::spawn(move || {
        checkpoint_barrier.wait();
        for _ in 0..10 {
            checkpoint_db.checkpoint().unwrap();
        }
    });
    barrier.wait();
    for _ in 0..10 {
        db.checkpoint().unwrap();
    }
    for w in workers {
        w.join().unwrap();
    }
    checkpointer.join().unwrap();
    db.checkpoint().unwrap();
    drop(db);
    let db = Database::open(&f.0).unwrap();
    assert_eq!(db.stats().unwrap().version, 160);
    for id in 0..8_u8 {
        assert_eq!(value(&db, &[id]), Some(vec![19]));
    }
}

#[test]
fn er_truncated_checkpoint_is_not_mistaken_for_an_uncommitted_tail() {
    let f = Fixture::new();
    let db = Database::create_append(&f.0, [9; 16]).unwrap();
    put(&db, b"a", b"committed");
    db.checkpoint().unwrap();
    drop(db);
    let bytes = fs::read(&f.0).unwrap();
    for length in 48..bytes.len() {
        let copy = Fixture::new();
        fs::write(&copy.0, &bytes[..length]).unwrap();
        assert!(matches!(Database::open(&copy.0), Err(Error::Invalid(_))));
        assert_eq!(fs::read(&copy.0).unwrap(), bytes[..length]);
    }
    let copy = Fixture::new();
    let mut tail = bytes.clone();
    tail.extend_from_slice(b"NDTX");
    fs::write(&copy.0, &tail).unwrap();
    let db = Database::open(&copy.0).unwrap();
    assert_eq!(value(&db, b"a"), Some(b"committed".to_vec()));
    assert_eq!(db.stats().unwrap().wal_bytes, bytes.len() as u64);
}
