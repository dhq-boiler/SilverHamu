use newdbms::transactions::{
    ConflictKind, Database, Error, MAX_SCAN_PAGE_BYTES, MAX_SCAN_PAGE_ROWS, MAX_SCAN_PAGE_VISITS,
    Rows, ScanLimits, Transaction,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("artifacts");
        std::fs::create_dir_all(&directory).unwrap();
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(directory.join(format!(
            "bounded-scan-{}-{time}-{}.wal",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
    fn create(&self) -> Database {
        Database::create(&self.0, [27; 16]).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_file(self.0.with_extension("wal.owner"));
    }
}

fn limits(rows: usize, bytes: usize, visits: usize) -> ScanLimits {
    ScanLimits {
        max_rows: rows,
        max_bytes: bytes,
        max_visits: visits,
    }
}

fn collect(tx: &mut Transaction, start: &[u8], end: Option<&[u8]>, budget: ScanLimits) -> Rows {
    let mut after: Option<Vec<u8>> = None;
    let mut rows = Vec::new();
    for _ in 0..10_000 {
        let page = tx.scan_page(start, end, after.as_deref(), budget).unwrap();
        assert!(page.rows.len() <= budget.max_rows);
        assert!(page.payload_bytes <= budget.max_bytes);
        assert!(page.visited_entries <= budget.max_visits);
        assert_eq!(
            page.payload_bytes,
            page.rows
                .iter()
                .map(|(key, value)| key.len() + value.len())
                .sum::<usize>()
        );
        rows.extend(page.rows);
        match page.next_after {
            None => return rows,
            Some(next) => {
                assert!(after.as_ref().is_none_or(|previous| previous < &next));
                after = Some(next);
            }
        }
    }
    panic!("continuation did not terminate");
}

#[test]
fn eq_empty_range_binary_keys_and_empty_values_preserve_presence() {
    let f = Fixture::new();
    let db = f.create();
    let mut tx = db.begin().unwrap();
    let page = tx
        .scan_page(b"", None, None, ScanLimits::default())
        .unwrap();
    assert!(page.rows.is_empty());
    assert_eq!(page.visited_entries, 0);
    assert_eq!(page.next_after, None);
    for key in [b"".as_slice(), b"\0", b"\xff"] {
        tx.put(key, b"").unwrap();
    }
    let first = tx.scan_page(b"", None, None, limits(1, 1, 1)).unwrap();
    assert_eq!(first.rows, vec![(vec![], vec![])]);
    assert_eq!(first.next_after, Some(vec![]));
    assert_eq!(collect(&mut tx, b"", None, limits(1, 1, 1)).len(), 3);
    assert!(collect(&mut tx, b"\0", Some(b"\0"), limits(1, 1, 1)).is_empty());
    assert_eq!(
        collect(&mut tx, b"\0", Some(b"\xff"), limits(1, 1, 1)),
        vec![(vec![0], vec![])]
    );
}

#[test]
fn eq_staged_insert_replace_delete_and_rollback_match_full_scan() {
    let f = Fixture::new();
    let db = f.create();
    let mut seed = db.begin().unwrap();
    for key in [b"a", b"c", b"e", b"g"] {
        seed.put(key, b"old").unwrap();
    }
    seed.commit().unwrap();
    let mut tx = db.begin().unwrap();
    tx.put(b"b", b"new").unwrap();
    tx.put(b"c", b"edit").unwrap();
    tx.delete(b"e").unwrap();
    tx.delete(b"f").unwrap();
    let expected = tx.scan(b"a", Some(b"g")).unwrap();
    assert_eq!(
        collect(&mut tx, b"a", Some(b"g"), limits(2, 9, 2)),
        expected
    );
    assert_eq!(
        expected
            .iter()
            .map(|row| row.0.as_slice())
            .collect::<Vec<_>>(),
        vec![b"a", b"b", b"c"]
    );
    tx.rollback();
    assert_eq!(db.begin().unwrap().scan(b"", None).unwrap().len(), 4);
}

#[test]
fn bv_row_budget_stops_before_copying_an_extra_result() {
    let f = Fixture::new();
    let db = f.create();
    let mut tx = db.begin().unwrap();
    for key in [b"a", b"b", b"c"] {
        tx.put(key, b"x").unwrap();
    }
    for count in [1, 2, 3, 4, MAX_SCAN_PAGE_ROWS] {
        let page = tx
            .scan_page(b"", None, None, limits(count, 100, 100))
            .unwrap();
        assert_eq!(page.rows.len(), count.min(3));
        assert_eq!(page.visited_entries, count.min(3));
        assert_eq!(page.next_after.is_some(), count < 3);
    }
}

#[test]
fn bv_byte_budget_includes_keys_and_defers_without_losing_a_record() {
    let f = Fixture::new();
    let db = f.create();
    let mut tx = db.begin().unwrap();
    tx.put(b"a", b"12").unwrap();
    tx.put(b"b", b"34").unwrap();
    for bytes in [1, 2] {
        assert!(matches!(
            tx.scan_page(b"", None, None, limits(10, bytes, 10)),
            Err(Error::Limit(_))
        ));
    }
    for bytes in [3, 4, 5, 6, 7, MAX_SCAN_PAGE_BYTES] {
        let page = tx
            .scan_page(b"", None, None, limits(10, bytes, 10))
            .unwrap();
        assert_eq!(page.rows.len(), if bytes < 6 { 1 } else { 2 });
        assert_eq!(
            page.next_after,
            if bytes < 6 { Some(b"a".to_vec()) } else { None }
        );
        assert_eq!(
            collect(&mut tx, b"", None, limits(10, bytes, 10)),
            tx.scan(b"", None).unwrap()
        );
    }
}

#[test]
fn bv_invalid_budgets_are_rejected_before_reading() {
    let f = Fixture::new();
    let db = f.create();
    let mut tx = db.begin().unwrap();
    for budget in [
        limits(0, 1, 1),
        limits(MAX_SCAN_PAGE_ROWS + 1, 1, 1),
        limits(1, 0, 1),
        limits(1, MAX_SCAN_PAGE_BYTES + 1, 1),
        limits(1, 1, 0),
        limits(1, 1, MAX_SCAN_PAGE_VISITS + 1),
        limits(usize::MAX, usize::MAX, usize::MAX),
    ] {
        assert!(matches!(
            tx.scan_page(b"", None, None, budget),
            Err(Error::Limit(_))
        ));
    }
    assert!(
        tx.scan_page(
            b"",
            None,
            None,
            limits(
                MAX_SCAN_PAGE_ROWS,
                MAX_SCAN_PAGE_BYTES,
                MAX_SCAN_PAGE_VISITS
            )
        )
        .is_ok()
    );
}

#[test]
fn er_invalid_ranges_and_continuations_are_rejected_without_panics() {
    let f = Fixture::new();
    let db = f.create();
    let mut tx = db.begin().unwrap();
    for (start, end, after) in [
        (b"z".as_slice(), Some(b"a".as_slice()), None),
        (b"b", Some(b"d"), Some(b"a".as_slice())),
        (b"b", Some(b"d"), Some(b"d".as_slice())),
        (b"b", Some(b"d"), Some(b"e".as_slice())),
        (b"b", Some(b"b"), Some(b"b".as_slice())),
    ] {
        assert!(matches!(
            tx.scan_page(start, end, after, ScanLimits::default()),
            Err(Error::Invalid(_))
        ));
    }
    let long = vec![0; newdbms::transactions::MAX_KEY_BYTES + 1];
    assert!(matches!(
        tx.scan_page(b"", None, Some(&long), ScanLimits::default()),
        Err(Error::Limit(_))
    ));
    assert!(
        tx.scan_page(b"", None, Some(b"missing"), ScanLimits::default())
            .unwrap()
            .rows
            .is_empty()
    );
}

#[test]
fn er_newer_invisible_keys_consume_visit_budget_and_empty_pages_advance() {
    let f = Fixture::new();
    let db = f.create();
    let mut old = db.begin().unwrap();
    let mut seed = db.begin().unwrap();
    for key in 0..8u8 {
        seed.put(&[key], b"x").unwrap();
    }
    seed.commit().unwrap();
    for (after, visits, next) in [
        (None, 3, Some(vec![2])),
        (Some(vec![2]), 3, Some(vec![5])),
        (Some(vec![5]), 2, None),
    ] {
        let page = old
            .scan_page(b"", None, after.as_deref(), limits(10, 100, 3))
            .unwrap();
        assert!(page.rows.is_empty());
        assert_eq!(page.visited_entries, visits);
        assert_eq!(page.next_after, next);
    }
    assert!(collect(&mut old, b"", None, limits(1, 10, 1)).is_empty());
}

#[test]
fn er_tombstones_and_staged_deletes_are_counted_once_per_key() {
    let f = Fixture::new();
    let db = f.create();
    let mut seed = db.begin().unwrap();
    for key in 0..6u8 {
        seed.put(&[key], b"x").unwrap();
    }
    seed.commit().unwrap();
    let _pin = db.begin().unwrap();
    let mut delete = db.begin().unwrap();
    for key in 0..5u8 {
        delete.delete(&[key]).unwrap();
    }
    delete.commit().unwrap();
    let mut tx = db.begin().unwrap();
    for key in 0..5u8 {
        tx.delete(&[key]).unwrap();
    }
    let page = tx.scan_page(b"", None, None, limits(10, 100, 5)).unwrap();
    assert!(page.rows.is_empty());
    assert_eq!(page.visited_entries, 5);
    assert_eq!(page.next_after, Some(vec![4]));
    assert_eq!(
        collect(&mut tx, b"", None, limits(1, 10, 1)),
        vec![(vec![5], b"x".to_vec())]
    );
}

#[test]
fn st_continuation_preserves_snapshot_across_commits_and_checkpoint() {
    let f = Fixture::new();
    let db = f.create();
    let mut seed = db.begin().unwrap();
    for key in [b"a", b"c", b"e"] {
        seed.put(key, b"old").unwrap();
    }
    seed.commit().unwrap();
    let mut old = db.begin().unwrap();
    let first = old.scan_page(b"", None, None, limits(1, 100, 10)).unwrap();
    let mut newer = db.begin().unwrap();
    newer.put(b"b", b"new").unwrap();
    newer.put(b"c", b"edited").unwrap();
    newer.delete(b"e").unwrap();
    newer.commit().unwrap();
    db.checkpoint().unwrap();
    let rest = old
        .scan_page(b"", None, first.next_after.as_deref(), limits(10, 100, 10))
        .unwrap();
    assert_eq!(
        rest.rows,
        vec![
            (b"c".to_vec(), b"old".to_vec()),
            (b"e".to_vec(), b"old".to_vec())
        ]
    );
    assert_eq!(rest.next_after, None);
    drop(old);
    drop(db);
    let db = Database::open(&f.0).unwrap();
    let mut read = db.begin().unwrap();
    assert_eq!(
        collect(&mut read, b"", None, limits(1, 100, 2)),
        read.scan(b"", None).unwrap()
    );
}

#[test]
fn cc_entire_requested_range_is_protected_beyond_the_returned_page_and_cursor() {
    for (after, changed) in [
        (None, b"y".as_slice()),
        (Some(b"m".as_slice()), b"b".as_slice()),
    ] {
        let f = Fixture::new();
        let db = f.create();
        let mut seed = db.begin().unwrap();
        seed.put(b"m", b"x").unwrap();
        seed.put(b"n", b"x").unwrap();
        seed.commit().unwrap();
        let mut tx = db.begin().unwrap();
        tx.scan_page(b"a", Some(b"z"), after, limits(1, 100, 1))
            .unwrap();
        let mut other = db.begin().unwrap();
        other.put(changed, b"new").unwrap();
        other.commit().unwrap();
        tx.put(b"outside", b"side effect").unwrap();
        assert!(matches!(
            tx.commit(),
            Err(Error::Conflict(ConflictKind::Range))
        ));
    }
}

#[test]
fn cc_empty_pages_detect_insert_delete_aba_but_ignore_upper_bound() {
    for key in [b"b", b"z"] {
        let f = Fixture::new();
        let db = f.create();
        let mut tx = db.begin().unwrap();
        tx.scan_page(b"a", Some(b"z"), None, limits(1, 100, 1))
            .unwrap();
        let mut insert = db.begin().unwrap();
        insert.put(key, b"v").unwrap();
        insert.commit().unwrap();
        let mut delete = db.begin().unwrap();
        delete.delete(key).unwrap();
        delete.commit().unwrap();
        tx.put(b"0", b"side effect").unwrap();
        let result = tx.commit();
        if key == b"b" {
            assert!(matches!(result, Err(Error::Conflict(ConflictKind::Range))));
        } else {
            result.unwrap();
        }
    }
}

#[test]
fn eq_all_pages_match_legacy_scan_with_many_versions_and_local_changes() {
    let f = Fixture::new();
    let db = f.create();
    let mut seed = db.begin().unwrap();
    for id in 0..2048u32 {
        seed.put(
            &id.to_be_bytes(),
            &vec![(id % 251) as u8; (id % 31) as usize],
        )
        .unwrap();
    }
    seed.commit().unwrap();
    let _pin = db.begin().unwrap();
    let mut changes = db.begin().unwrap();
    for id in (0..2048u32).step_by(3) {
        changes.delete(&id.to_be_bytes()).unwrap();
    }
    changes.commit().unwrap();
    let mut read = db.begin().unwrap();
    for id in (0..2200u32).step_by(5) {
        read.put(&id.to_be_bytes(), b"local").unwrap();
    }
    for id in (0..2048u32).step_by(7) {
        read.delete(&id.to_be_bytes()).unwrap();
    }
    let expected = read
        .scan(&20u32.to_be_bytes(), Some(&2180u32.to_be_bytes()))
        .unwrap();
    for budget in [limits(1, 40, 1), limits(13, 100, 17), ScanLimits::default()] {
        assert_eq!(
            collect(
                &mut read,
                &20u32.to_be_bytes(),
                Some(&2180u32.to_be_bytes()),
                budget
            ),
            expected
        );
    }
}
