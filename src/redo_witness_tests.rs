use super::redo_witness::{HEADER, SLOT_BYTES};
use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

const CAPACITY: u64 = 256 * 1024;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("artifacts");
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir.join(format!(
            "redo-test-{}-{}-{}.wal",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }

    fn create(&self) -> Database {
        let db = Database::create(&self.0, [39; 16]).unwrap();
        db.checkpoint_redo_log(CAPACITY).unwrap();
        db
    }

    fn image(&self) -> Vec<u8> {
        std::fs::read(&self.0).unwrap()
    }

    fn from_image(image: &[u8]) -> Self {
        let f = Self::new();
        std::fs::write(&f.0, image).unwrap();
        f
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let mut owner = self.0.as_os_str().to_owned();
        owner.push(".owner");
        let _ = std::fs::remove_file(PathBuf::from(owner));
    }
}

fn put(db: &Database, key: &[u8], value: &[u8]) {
    let mut tx = db.begin().unwrap();
    tx.put(key, value).unwrap();
    tx.commit().unwrap();
}

fn write_pair(db: &Database) -> Result<u64> {
    let mut tx = db.begin()?;
    tx.put(b"next-a", b"one")?;
    tx.put(b"next-b", b"two")?;
    tx.commit()
}

fn check_pair(db: &Database) -> bool {
    let mut tx = db.begin_read().unwrap();
    assert_eq!(
        tx.get(b"ack").unwrap().as_deref(),
        Some(b"stable".as_slice())
    );
    let a = tx.get(b"next-a").unwrap();
    let b = tx.get(b"next-b").unwrap();
    if a.is_some() {
        assert_eq!(a.as_deref(), Some(b"one".as_slice()));
        assert_eq!(b.as_deref(), Some(b"two".as_slice()));
        true
    } else {
        assert_eq!(b, None);
        false
    }
}

#[test]
fn torn_primary_and_witness_images_recover_the_acknowledged_prefix_and_whole_transaction() {
    let f = Fixture::new();
    let db = f.create();
    put(&db, b"ack", b"stable");
    let start = db.stats().unwrap().wal_bytes as usize;
    drop(db);
    let before = f.image();
    let db = Database::open(&f.0).unwrap();
    write_pair(&db).unwrap();
    let end = db.stats().unwrap().wal_bytes as usize;
    drop(db);
    let after = f.image();
    let slot = (0..2)
        .map(|i| (CAPACITY + i * SLOT_BYTES) as usize)
        .find(|&at| before[at..at + HEADER] != after[at..at + HEADER])
        .unwrap();
    let payload = end - start;
    let cuts = [
        0,
        7,
        HEADER - 1,
        HEADER,
        HEADER + payload / 2,
        HEADER + payload,
    ];
    let peer_slot = slot + 2 * SLOT_BYTES as usize;
    for data_cut in [0, 7, PREFIX_BYTES, payload / 2, payload - 1, payload] {
        for witness_cut in cuts {
            for peer_cut in cuts {
                let mut image = before.clone();
                for (at, cut) in [
                    (start, data_cut),
                    (slot, witness_cut),
                    (peer_slot, peer_cut),
                ] {
                    image[at..at + cut].copy_from_slice(&after[at..at + cut]);
                }
                let test = Fixture::from_image(&image);
                let reopened = Database::open(&test.0).unwrap();
                let visible = check_pair(&reopened);
                if data_cut == payload || [witness_cut, peer_cut].contains(&(HEADER + payload)) {
                    assert!(visible, "cuts {data_cut}/{witness_cut}/{peer_cut}");
                }
                put(&reopened, b"following", b"durable");
                drop(reopened);
                let again = Database::open(&test.0).unwrap();
                assert_eq!(check_pair(&again), visible);
                assert_eq!(
                    again
                        .begin_read()
                        .unwrap()
                        .get(b"following")
                        .unwrap()
                        .as_deref(),
                    Some(b"durable".as_slice())
                );
            }
        }
    }
}

#[test]
fn acknowledged_primary_damage_beyond_the_witness_window_fails_closed() {
    let f = Fixture::new();
    let db = f.create();
    let start = db.stats().unwrap().wal_bytes as usize;
    for i in 0..4 {
        put(&db, &[i], &[i; 128]);
    }
    drop(db);
    let mut image = f.image();
    image[start + PREFIX_BYTES + 27] ^= 0x80;
    let test = Fixture::from_image(&image);
    assert!(Database::open(&test.0).is_err());
}

#[test]
fn oversized_commit_survives_reopen_and_rejects_corrupt_confirmed_payload() {
    let f = Fixture::new();
    let db = f.create();
    let start = db.stats().unwrap().wal_bytes as usize;
    let value = vec![0x5a; SLOT_BYTES as usize + 7];
    put(&db, b"large", &value);
    drop(db);
    let image = f.image();
    let db = Database::open(&f.0).unwrap();
    assert_eq!(
        db.begin_read().unwrap().get(b"large").unwrap().unwrap(),
        value
    );
    drop(db);
    let mut damaged = image;
    damaged[start + PREFIX_BYTES + 4000] ^= 1;
    let test = Fixture::from_image(&damaged);
    assert!(Database::open(&test.0).is_err());
}

#[test]
fn interrupted_oversized_and_small_commits_do_not_acknowledge_or_expose_unstable_versions() {
    for (stage, large) in [
        ("redo-before-witness", false),
        ("redo-before-sync", false),
        ("redo-after-sync", false),
        ("redo-before-payload-sync", true),
        ("redo-after-payload-sync", true),
    ] {
        let f = Fixture::new();
        let db = f.create();
        put(&db, b"ack", b"stable");
        FAIL.with(|v| v.set(Some(stage)));
        let mut tx = db.begin().unwrap();
        let value = vec![7; if large { SLOT_BYTES as usize + 1 } else { 8 }];
        tx.put(b"attempt", &value).unwrap();
        let outcome = tx.commit();
        FAIL.with(|v| v.set(None));
        assert!(matches!(outcome, Err(Error::CommitUncertain(_))), "{stage}");
        assert!(matches!(db.begin(), Err(Error::Poisoned)), "{stage}");
        drop(db);
        let reopened = Database::open(&f.0).unwrap();
        let mut reader = reopened.begin_read().unwrap();
        assert_eq!(
            reader.get(b"ack").unwrap().as_deref(),
            Some(b"stable".as_slice())
        );
        if let Some(recovered) = reader.get(b"attempt").unwrap() {
            assert_eq!(recovered, value, "{stage}");
        }
    }
}

#[test]
#[ignore = "child-process entry point for the redo crash test"]
fn redo_crash_worker() {
    let db = Database::open(std::env::var_os("CATDB_FIXED_PATH").unwrap()).unwrap();
    db.set_group_commit(true);
    if std::env::var_os("CATDB_REDO_LARGE_CRASH").is_some() {
        let mut tx = db.begin().unwrap();
        tx.put(b"next-a", b"one").unwrap();
        tx.put(b"next-b", b"two").unwrap();
        tx.put(b"large", &vec![0xa7; SLOT_BYTES as usize]).unwrap();
        tx.commit().unwrap();
    } else {
        write_pair(&db).unwrap();
    }
    panic!("crash point missed");
}

#[test]
fn process_exit_around_witness_sync_preserves_acknowledged_data_and_atomicity() {
    for (large, stage) in [
        (false, "redo-before-witness"),
        (false, "redo-before-sync"),
        (false, "redo-after-sync"),
        (true, "redo-before-payload-sync"),
        (true, "redo-after-payload-sync"),
        (true, "redo-after-sync"),
    ] {
        let f = Fixture::new();
        let db = f.create();
        put(&db, b"ack", b"stable");
        drop(db);
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        if large {
            child.env("CATDB_REDO_LARGE_CRASH", "1");
        }
        let result = child
            .args([
                "--exact",
                "transactions::fixed_log::redo_witness_tests::redo_crash_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("CATDB_FIXED_PATH", &f.0)
            .env("CATDB_FIXED_CRASH", stage)
            .status()
            .unwrap();
        assert_eq!(result.code(), Some(87), "{stage}");
        let reopened = Database::open(&f.0).unwrap();
        let visible = check_pair(&reopened);
        if large {
            let value = reopened.begin_read().unwrap().get(b"large").unwrap();
            assert_eq!(value.is_some(), visible);
            if let Some(value) = value {
                assert_eq!(value, vec![0xa7; SLOT_BYTES as usize]);
            }
        }
        if stage == "redo-after-sync" || stage == "redo-after-payload-sync" {
            assert!(visible);
        }
    }
}

#[test]
fn checkpoint_during_witness_persistence_preserves_format_snapshot_and_accepted_writes() {
    let f = Fixture::new();
    let db = f.create();
    put(&db, b"ack", b"stable");
    let mut old = db.begin_read().unwrap();
    let gate = Arc::new([Barrier::new(2), Barrier::new(2)]);
    std::thread::scope(|scope| {
        let d = db.clone();
        let g = gate.clone();
        let (send, recv) = std::sync::mpsc::channel();
        let writer = scope.spawn(move || {
            GATE.with(|saved| *saved.borrow_mut() = Some(("redo-before-sync", g)));
            send.send(write_pair(&d)).unwrap();
        });
        gate[0].wait();
        let no_response = recv.try_recv().is_err();
        let before_sync = db.begin_read().unwrap().get(b"next-a").unwrap();
        let d = db.clone();
        let checkpoint = scope.spawn(move || d.checkpoint());
        gate[1].wait();
        assert!(no_response);
        assert_eq!(before_sync, None);
        recv.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
        writer.join().unwrap();
        checkpoint.join().unwrap().unwrap();
    });
    assert_eq!(old.get(b"next-a").unwrap(), None);
    assert!(check_pair(&db));
    drop(old);
    drop(db);
    assert_eq!(read_u32(&f.image(), 8), 4);
    assert!(check_pair(&Database::open(&f.0).unwrap()));
}

#[test]
fn the_largest_inline_frame_uses_one_barrier_and_the_next_byte_selects_the_fallback() {
    let empty = BTreeMap::from([(b"size".to_vec(), Some(Vec::new()))]);
    let overhead = encode_frame(1, &empty).len();
    let boundary = SLOT_BYTES as usize - HEADER - overhead;
    for size in [boundary - 1, boundary, boundary + 1] {
        let f = Fixture::new();
        let db = f.create();
        let value = vec![91; size];
        let mut tx = db.begin().unwrap();
        tx.put(b"size", &value).unwrap();
        FAIL.with(|hook| hook.set(Some("redo-before-payload-sync")));
        let result = tx.commit();
        FAIL.with(|hook| hook.set(None));
        if size <= boundary {
            result.unwrap();
            drop(db);
            let db = Database::open(&f.0).unwrap();
            assert_eq!(
                db.begin_read().unwrap().get(b"size").unwrap().unwrap(),
                value
            );
        } else {
            assert!(matches!(result, Err(Error::CommitUncertain(_))));
        }
    }
}

#[test]
fn recovery_repairs_the_primary_before_a_subsequent_witness_loss() {
    let f = Fixture::new();
    let db = f.create();
    put(&db, b"ack", b"stable");
    let start = db.stats().unwrap().wal_bytes as usize;
    drop(db);
    let before = f.image();
    let db = Database::open(&f.0).unwrap();
    write_pair(&db).unwrap();
    let end = db.stats().unwrap().wal_bytes as usize;
    drop(db);
    let original = f.image();
    let slot = (0..2)
        .map(|i| (CAPACITY + i * SLOT_BYTES) as usize)
        .find(|&at| before[at..at + HEADER] != original[at..at + HEADER])
        .unwrap();
    let peer_slot = slot + 2 * SLOT_BYTES as usize;
    for (bad, healthy) in [(slot, peer_slot), (peer_slot, slot)] {
        let mut damaged = original.clone();
        damaged[start..end].fill(0);
        damaged[bad + HEADER + 9] ^= 0x80;
        let test = Fixture::from_image(&damaged);
        assert!(check_pair(&Database::open(&test.0).unwrap()));
        let mut repaired = test.image();
        assert_eq!(repaired[start..end], original[start..end]);
        assert_eq!(
            repaired[bad..bad + HEADER + end - start],
            original[bad..bad + HEADER + end - start]
        );
        repaired[start..end].fill(0);
        repaired[healthy..healthy + HEADER].fill(0);
        let test = Fixture::from_image(&repaired);
        assert!(check_pair(&Database::open(&test.0).unwrap()));
    }
}

#[test]
fn converting_an_existing_fixed_database_preserves_snapshots_conflicts_and_checkpoint_contents() {
    let f = Fixture::new();
    let db = Database::create(&f.0, [39; 16]).unwrap();
    put(&db, b"book", b"first");
    db.checkpoint_fixed_log(CAPACITY).unwrap();
    let mut reader = db.begin_read().unwrap();
    let mut stale_writer = db.begin().unwrap();
    assert_eq!(
        stale_writer.get(b"book").unwrap().as_deref(),
        Some(b"first".as_slice())
    );
    db.checkpoint_redo_log(CAPACITY).unwrap();
    db.set_group_commit(true);
    put(&db, b"book", b"second");
    stale_writer.put(b"book", b"stale").unwrap();
    assert!(matches!(stale_writer.commit(), Err(Error::Conflict(_))));
    assert_eq!(
        reader.get(b"book").unwrap().as_deref(),
        Some(b"first".as_slice())
    );
    db.checkpoint().unwrap();
    assert_eq!(
        reader.get(b"book").unwrap().as_deref(),
        Some(b"first".as_slice())
    );
    drop(reader);
    drop(db);
    assert_eq!(read_u32(&f.image(), 8), 4);
    let db = Database::open(&f.0).unwrap();
    assert_eq!(
        db.begin_read().unwrap().get(b"book").unwrap().as_deref(),
        Some(b"second".as_slice())
    );
    assert_eq!(db.id(), [39; 16]);
}

#[test]
fn exact_capacity_can_reject_a_group_without_losing_its_last_accepted_commit() {
    let f = Fixture::new();
    let db = Database::create(&f.0, [39; 16]).unwrap();
    db.checkpoint_redo_log(4096).unwrap();
    db.set_group_commit(true);
    let empty = BTreeMap::from([(b"full".to_vec(), Some(Vec::new()))]);
    let size = 4096 - PREFIX_BYTES - HEADER_BYTES - encode_frame(1, &empty).len();
    let content = vec![41; size];
    put(&db, b"full", &content);
    let mut rejected = db.begin().unwrap();
    rejected.put(b"extra", b"x").unwrap();
    assert!(matches!(
        rejected.commit(),
        Err(Error::ResourceExhausted(_))
    ));
    db.checkpoint_redo_log(8192).unwrap();
    put(&db, b"extra", b"fits");
    drop(db);
    let db = Database::open(&f.0).unwrap();
    let mut reader = db.begin_read().unwrap();
    assert_eq!(reader.get(b"full").unwrap().unwrap(), content);
    assert_eq!(
        reader.get(b"extra").unwrap().as_deref(),
        Some(b"fits".as_slice())
    );
}

#[test]
fn contradictory_valid_primary_or_redo_records_are_rejected_without_modifying_the_file() {
    let f = Fixture::new();
    let db = f.create();
    put(&db, b"ack", b"stable");
    let start = db.stats().unwrap().wal_bytes as usize;
    drop(db);
    let common = f.image();
    let alternate = Fixture::from_image(&common);
    let db = Database::open(&f.0).unwrap();
    put(&db, b"choice", b"one");
    let end = db.stats().unwrap().wal_bytes as usize;
    drop(db);
    let db = Database::open(&alternate.0).unwrap();
    put(&db, b"choice", b"two");
    drop(db);
    let first = f.image();
    let second = alternate.image();
    let slot = (0..2)
        .map(|i| (CAPACITY + i * SLOT_BYTES) as usize)
        .find(|&at| first[at..at + HEADER] != common[at..at + HEADER])
        .unwrap();
    let len = HEADER + end - start;
    for (at, n) in [
        (start, end - start),
        (slot, len),
        (slot + 2 * SLOT_BYTES as usize, len),
    ] {
        let mut mixed = first.clone();
        mixed[at..at + n].copy_from_slice(&second[at..at + n]);
        let test = Fixture::from_image(&mixed);
        assert!(Database::open(&test.0).is_err());
        assert_eq!(test.image(), mixed);
    }
}

#[test]
fn opening_an_invalid_confirmed_prefix_does_not_repair_any_tail_bytes() {
    let f = Fixture::new();
    let db = f.create();
    let first = db.stats().unwrap().wal_bytes as usize;
    put(&db, b"first", b"old");
    put(&db, b"second", b"previous");
    let last = db.stats().unwrap().wal_bytes as usize;
    put(&db, b"third", b"latest");
    let end = db.stats().unwrap().wal_bytes as usize;
    drop(db);
    let mut bytes = f.image();
    bytes[first + PREFIX_BYTES + 10] ^= 0x80;
    bytes[last..end].fill(0);
    let test = Fixture::from_image(&bytes);
    assert!(Database::open(&test.0).is_err());
    assert_eq!(bytes, test.image());
}

#[test]
fn reopened_generation_zero_survives_a_torn_first_commit_after_loss_of_even_witness_slots() {
    let f = Fixture::new();
    let db = Database::create(&f.0, [39; 16]).unwrap();
    put(&db, b"ack", b"stable");
    db.checkpoint_redo_log(CAPACITY).unwrap();
    let start = db.stats().unwrap().wal_bytes as usize;
    drop(db);
    let initial = f.image();
    let offset = |index: u64| (CAPACITY + index * SLOT_BYTES) as usize;
    let mut damaged = initial.clone();
    for slot in [0, 2] {
        damaged[offset(slot)..offset(slot) + HEADER].fill(0);
    }
    let f = Fixture::from_image(&damaged);
    let db = Database::open(&f.0).unwrap();
    assert!(!check_pair(&db));
    drop(db);
    let recovered = f.image();
    for slot in [0, 2] {
        assert_eq!(
            recovered[offset(slot)..offset(slot) + HEADER],
            initial[offset(slot)..offset(slot) + HEADER]
        );
    }
    let db = Database::open(&f.0).unwrap();
    write_pair(&db).unwrap();
    drop(db);
    let complete = f.image();
    let mut torn = recovered;
    torn[start..start + 4].copy_from_slice(&complete[start..start + 4]);
    for slot in [1, 3] {
        torn[offset(slot)..offset(slot) + HEADER + 8]
            .copy_from_slice(&complete[offset(slot)..offset(slot) + HEADER + 8]);
    }
    let f = Fixture::from_image(&torn);
    assert!(!check_pair(&Database::open(&f.0).unwrap()));
}

#[test]
fn a_small_database_can_convert_and_reopen_under_a_one_kib_buffer_budget() {
    let f = Fixture::new();
    let limits = ResourceLimits {
        max_buffer_bytes: 1024,
        ..ResourceLimits::default()
    };
    let db = Database::create_with_limits(&f.0, [39; 16], limits).unwrap();
    db.checkpoint_redo_log(CAPACITY).unwrap();
    put(&db, b"k", b"v");
    drop(db);
    let db = Database::open_with_limits(&f.0, limits).unwrap();
    assert_eq!(
        db.begin_read().unwrap().get(b"k").unwrap().as_deref(),
        Some(b"v".as_slice())
    );
    let usage = db.resource_usage().unwrap();
    assert_eq!(usage.current[crate::resources::BUFFER], 0);
    assert!(usage.peak[crate::resources::BUFFER] <= 1024);
}

#[test]
fn oversized_recovery_buffers_are_refused_before_a_damaged_primary_is_repaired() {
    let f = Fixture::new();
    let db = f.create();
    let start = db.stats().unwrap().wal_bytes as usize;
    put(&db, b"large", &[0x3a; 4096]);
    drop(db);
    let mut bytes = f.image();
    bytes[start + PREFIX_BYTES + 24] ^= 0x80;
    let test = Fixture::from_image(&bytes);
    let limits = ResourceLimits {
        max_buffer_bytes: 1024,
        ..ResourceLimits::default()
    };
    assert!(matches!(
        Database::open_with_limits(&test.0, limits),
        Err(Error::ResourceExhausted(_))
    ));
    assert_eq!(test.image(), bytes);
    let db = Database::open(&test.0).unwrap();
    assert_eq!(
        db.begin_read().unwrap().get(b"large").unwrap().unwrap(),
        [0x3a; 4096]
    );
}
