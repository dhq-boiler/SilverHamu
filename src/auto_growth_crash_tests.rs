use super::*;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

thread_local! {
    static INJECT: Cell<Option<(CheckpointStage, bool)>> = const { Cell::new(None) };
}

pub(super) fn stage(at: CheckpointStage) -> io::Result<()> {
    if let Some((requested, exit)) = INJECT.with(Cell::get)
        && at == requested
    {
        INJECT.with(|mode| mode.set(None));
        if exit {
            std::process::exit(81);
        }
        return Err(io::Error::other("injected managed checkpoint failure"));
    }
    Ok(())
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "silverhamu-auto-cutover-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            N.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> PathBuf {
        self.0.join("state.ndb")
    }

    fn seed(&self) -> Database {
        let db = Database::create_with_log(
            self.path(),
            [78; 16],
            ResourceLimits::unlimited(),
            Some(CheckpointLog::ManagedRedo(4096)),
        )
        .unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"ack", b"before").unwrap();
        assert_eq!(tx.commit().unwrap(), 1);
        db
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn trigger(db: &Database) -> Transaction {
    let mut tx = db.begin().unwrap();
    tx.put(b"left", &[1; 2500]).unwrap();
    tx.put(b"right", &[2; 2500]).unwrap();
    tx
}

fn verify_retry(f: &Fixture, before_replacement: bool) {
    let db = Database::open(f.path()).unwrap();
    let expected = if before_replacement { 4096 } else { 8192 };
    assert_eq!(db.fixed_log_capacity().unwrap(), Some(expected));
    assert_eq!(db.current_version().unwrap(), 1);
    let mut snap = db.begin_read().unwrap();
    assert_eq!(snap.get(b"ack").unwrap().as_deref(), Some(&b"before"[..]));
    assert_eq!(snap.get(b"left").unwrap(), None);
    assert_eq!(snap.get(b"right").unwrap(), None);
    assert_eq!(trigger(&db).commit().unwrap(), 2);
    assert_eq!(snap.get(b"left").unwrap(), None);
    drop(snap);
    drop(db);
    let db = Database::open(f.path()).unwrap();
    let mut tx = db.begin_read().unwrap();
    assert_eq!(tx.get(b"left").unwrap().as_deref(), Some(&[1; 2500][..]));
    assert_eq!(tx.get(b"right").unwrap().as_deref(), Some(&[2; 2500][..]));
}

#[test]
fn failures_on_either_side_of_auto_growth_preserve_acknowledged_state_and_allow_retry() {
    for grouped in [false, true] {
        for at in [CheckpointStage::Retained, CheckpointStage::Published] {
            let f = Fixture::new();
            let db = f.seed();
            db.set_group_commit(grouped);
            INJECT.with(|mode| mode.set(Some((at, false))));
            let result = trigger(&db).commit();
            if at == CheckpointStage::Retained {
                assert!(matches!(result, Err(Error::Io(_))));
                assert_eq!(db.current_version().unwrap(), 1);
            } else {
                assert!(matches!(result, Err(Error::CheckpointUncertain(_))));
                assert!(matches!(db.begin(), Err(Error::Poisoned)));
            }
            drop(db);
            verify_retry(&f, at == CheckpointStage::Retained);
        }
    }
}

#[test]
#[ignore = "subprocess helper"]
fn crash_worker() {
    let path = std::env::var_os("CATDB_AUTO_CRASH_PATH").unwrap();
    let stage = if std::env::var("CATDB_AUTO_CRASH_STAGE").unwrap() == "before" {
        CheckpointStage::Retained
    } else {
        CheckpointStage::Published
    };
    let db = Database::open(path).unwrap();
    INJECT.with(|mode| mode.set(Some((stage, true))));
    let _ = trigger(&db).commit();
    panic!("growth cutover was not reached");
}

#[test]
fn process_exit_around_auto_growth_rename_recovers_only_acknowledged_rows() {
    for phase in ["before", "after"] {
        let f = Fixture::new();
        drop(f.seed());
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "transactions::auto_growth_crash_tests::crash_worker",
                "--ignored",
            ])
            .env("CATDB_AUTO_CRASH_STAGE", phase)
            .env("CATDB_AUTO_CRASH_PATH", f.path())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(81));
        verify_retry(&f, phase == "before");
    }
}

#[test]
fn replacement_does_not_acknowledge_the_update_before_its_own_sync() {
    let f = Fixture::new();
    let db = f.seed();
    let (reached, observed) = mpsc::channel();
    let (resume, wait) = mpsc::channel();
    let (reply, response) = mpsc::channel();
    std::thread::scope(|threads| {
        let db_handle = db.clone();
        threads.spawn(move || {
            let result = trigger(&db_handle).commit_with(|file, frame| {
                file.write_all(frame)?;
                reached.send(()).unwrap();
                wait.recv().unwrap();
                file.sync_all()
            });
            reply.send(result).unwrap();
        });
        observed
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        let visible = db.begin_read().unwrap().get(b"left").unwrap();
        let replied = response.try_recv();
        resume.send(()).unwrap();
        assert_eq!(visible, None);
        assert!(matches!(replied, Err(mpsc::TryRecvError::Empty)));
        assert_eq!(response.recv().unwrap().unwrap(), 2);
    });
}

#[test]
fn full_maximum_capacity_is_reclaimed_once_per_unchanged_database_version() {
    let max = fixed_log::MAX_REDO_CAPACITY;
    let live = max - PREFIX_BYTES as u64 - 5;
    assert_eq!(managed_capacity(max, live, 8).unwrap(), max);
    assert!(exceeds_managed_threshold(max, live, 8, false));
    assert!(!exceeds_managed_threshold(max, live, 8, true));
    assert!(matches!(
        managed_capacity(max, max, 1),
        Err(Error::ResourceExhausted(_))
    ));
    assert!(!exceeds_managed_threshold(max, live, 5, false));
}
