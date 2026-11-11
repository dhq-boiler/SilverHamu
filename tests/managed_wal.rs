#![cfg(feature = "redo-witness")]

use newdbms::transactions::{Database, Error};
use std::fs;
use std::path::PathBuf;
use std::sync::Barrier;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "silverhamu-managed-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> PathBuf {
        self.0.join("state.ndb")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn first_transaction_larger_than_the_initial_capacity_is_atomic_and_survives_reopen() {
    for grouped in [false, true] {
        let fixture = Fixture::new();
        let db = Database::create(fixture.path(), [3; 16]).unwrap();
        db.set_group_commit(grouped);
        let initial = db.fixed_log_capacity().unwrap().unwrap();
        let value = vec![0x7e; initial as usize / 2 + 1];
        let mut old = db.begin_read().unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"left", &value).unwrap();
        tx.put(b"right", &value).unwrap();
        assert_eq!(tx.commit().unwrap(), 1);
        assert!(db.fixed_log_capacity().unwrap().unwrap() > initial);
        assert_eq!(old.get(b"left").unwrap(), None);
        assert_eq!(old.get(b"right").unwrap(), None);
        drop(old);
        drop(db);

        let reopened = Database::open(fixture.path()).unwrap();
        assert_eq!(reopened.stats().unwrap().version, 1);
        let mut tx = reopened.begin_read().unwrap();
        for key in [b"left".as_slice(), b"right"] {
            assert_eq!(tx.get(key).unwrap().as_deref(), Some(value.as_slice()));
        }
    }
}

#[test]
fn automatic_replacements_preserve_old_reads_conflicts_and_every_acknowledged_sequence() {
    let fixture = Fixture::new();
    let db = Database::create(fixture.path(), [4; 16]).unwrap();
    let initial = db.fixed_log_capacity().unwrap().unwrap();
    db.set_group_commit(true);
    let mut seed = db.begin().unwrap();
    seed.put(b"sentinel", b"original").unwrap();
    assert_eq!(seed.commit().unwrap(), 1);
    let mut old = db.begin_read().unwrap();
    let mut stale = db.begin().unwrap();
    assert_eq!(
        stale.get(b"sentinel").unwrap().as_deref(),
        Some(b"original".as_slice())
    );
    let mut replace = db.begin().unwrap();
    replace.put(b"sentinel", b"new").unwrap();
    assert_eq!(replace.commit().unwrap(), 2);

    let gate = Barrier::new(5);
    let sequences = thread::scope(|scope| {
        let maintenance = scope.spawn(|| {
            gate.wait();
            for _ in 0..4 {
                db.checkpoint().unwrap();
                thread::yield_now();
            }
        });
        let handles = (0..4u8)
            .map(|lane| {
                let db = &db;
                let gate = &gate;
                scope.spawn(move || {
                    gate.wait();
                    let mut versions = Vec::new();
                    for generation in 0..5u8 {
                        for key in 0..8u8 {
                            let mut tx = db.begin().unwrap();
                            tx.put(&[lane, key], &vec![generation; 64 * 1024]).unwrap();
                            versions.push(tx.commit().unwrap());
                        }
                    }
                    versions
                })
            })
            .collect::<Vec<_>>();
        maintenance.join().unwrap();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap())
            .collect::<Vec<_>>()
    });
    let mut sequences = sequences;
    sequences.sort_unstable();
    assert_eq!(sequences, (3..=162).collect::<Vec<_>>());
    assert!(db.fixed_log_capacity().unwrap().unwrap() > initial);
    assert_eq!(
        old.get(b"sentinel").unwrap().as_deref(),
        Some(b"original".as_slice())
    );
    assert_eq!(old.get(&[0, 0]).unwrap(), None);
    stale.put(b"should-not-appear", b"x").unwrap();
    assert!(matches!(stale.commit(), Err(Error::Conflict(_))));
    drop(old);
    drop(db);

    let reopened = Database::open(fixture.path()).unwrap();
    assert_eq!(reopened.stats().unwrap().version, 162);
    let mut tx = reopened.begin_read().unwrap();
    assert_eq!(tx.get(b"should-not-appear").unwrap(), None);
    assert_eq!(
        tx.get(b"sentinel").unwrap().as_deref(),
        Some(b"new".as_slice())
    );
    let expected = vec![4; 64 * 1024];
    for lane in 0..4 {
        for key in 0..8 {
            assert_eq!(
                tx.get(&[lane, key]).unwrap().as_deref(),
                Some(expected.as_slice())
            );
        }
    }
}
