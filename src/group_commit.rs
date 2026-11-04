use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

const MAX_BATCH: usize = 32;
const MAX_BATCH_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub(super) struct GroupCommit {
    enabled: AtomicBool,
    syncs: AtomicU64,
    pending: Mutex<Vec<Request>>,
    #[cfg(feature = "group-handoff")]
    leader: Mutex<()>,
    wake_on_four: AtomicBool,
    ready: std::sync::Condvar,
    timing: AtomicBool,
    timings: Mutex<Vec<[u64; 3]>>,
    #[cfg(feature = "lock-timing")]
    collection_enabled: AtomicBool,
    #[cfg(feature = "lock-timing")]
    collections: Mutex<Vec<[u64; 5]>>,
    #[cfg(any(test, feature = "sync-stall"))]
    stall: Mutex<Option<(Instant, Duration, Duration, Duration)>>,
}

struct Request {
    transaction: Transaction,
    response: Sender<Reply>,
    #[cfg(feature = "lock-timing")]
    submitted: u64,
    #[cfg(feature = "group-handoff")]
    queued: Instant,
}

struct Reply {
    result: Result<u64>,
    #[cfg(feature = "lock-timing")]
    trace: [u64; 8],
}

#[cfg(feature = "commit-window")]
pub struct CommitOutcome {
    pub result: Result<u64>,
    #[cfg(feature = "lock-timing")]
    pub trace: [u64; 8],
}

impl Database {
    #[cfg(feature = "lock-timing")]
    #[doc(hidden)]
    pub fn set_group_collection_timing(&self, enabled: bool) {
        self.inner
            .group
            .collection_enabled
            .store(enabled, Ordering::Relaxed);
    }

    #[cfg(feature = "lock-timing")]
    #[doc(hidden)]
    pub fn take_group_collection_timings(&self) -> Vec<[u64; 5]> {
        std::mem::take(&mut *self.inner.group.collections.lock().unwrap())
    }

    #[cfg(feature = "commit-window")]
    #[doc(hidden)]
    pub fn commit_batch(&self, transactions: Vec<Transaction>) -> Result<Vec<CommitOutcome>> {
        if transactions
            .iter()
            .any(|tx| !Arc::ptr_eq(&tx.database.inner, &self.inner) || tx.writes.is_empty())
        {
            return Err(Error::Invalid("batch requires writes from one database"));
        }
        self.inner.group.submit_many(transactions).map(|replies| {
            replies
                .into_iter()
                .map(|reply| CommitOutcome {
                    result: reply.result,
                    #[cfg(feature = "lock-timing")]
                    trace: reply.trace,
                })
                .collect()
        })
    }

    #[cfg(any(test, feature = "sync-stall"))]
    #[doc(hidden)]
    pub fn set_diagnostic_sync_stall(&self, start: Duration, duration: Duration, delay: Duration) {
        *self.inner.group.stall.lock().unwrap() = Some((Instant::now(), start, duration, delay));
    }
    /// Ends the batching wait early once four write requests are queued.
    pub fn set_group_commit_wake_on_four(&self, enabled: bool) {
        self.inner
            .group
            .wake_on_four
            .store(enabled, Ordering::Relaxed);
    }
    #[doc(hidden)]
    pub fn set_group_commit_timing(&self, enabled: bool) {
        self.inner.group.timing.store(enabled, Ordering::Relaxed);
    }

    #[doc(hidden)]
    pub fn take_group_commit_timings(&self) -> Vec<[u64; 3]> {
        std::mem::take(&mut *self.inner.group.timings.lock().unwrap())
    }

    /// Enables batching for subsequent write commits through all clones of this handle.
    pub fn set_group_commit(&self, enabled: bool) {
        self.inner.group.enabled.store(enabled, Ordering::Relaxed);
    }

    /// Number of commit log synchronization attempts on this open handle.
    pub fn commit_sync_attempts(&self) -> u64 {
        self.inner.group.syncs.load(Ordering::Relaxed)
    }
}

impl GroupCommit {
    #[cfg(any(test, feature = "sync-stall"))]
    fn stall_sync(&self) {
        let delay =
            self.stall
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|(origin, start, duration, delay)| {
                    let elapsed = origin.elapsed();
                    (elapsed >= *start && elapsed < *start + *duration).then_some(*delay)
                });
        if let Some(delay) = delay {
            std::thread::sleep(delay);
        }
    }

    #[cfg(feature = "commit-window")]
    fn submit_many(&self, transactions: Vec<Transaction>) -> Result<Vec<Reply>> {
        if transactions.is_empty() {
            return Ok(Vec::new());
        }
        let mut pending = self.pending.lock().map_err(|_| Error::Poisoned)?;
        let bytes: usize = pending
            .iter()
            .map(|r| r.transaction.write_bytes)
            .sum::<usize>()
            + transactions.iter().map(|tx| tx.write_bytes).sum::<usize>();
        if pending.len() + transactions.len() > MAX_BATCH || bytes > MAX_BATCH_BYTES {
            return Err(Error::ResourceExhausted("commit batch capacity"));
        }
        let leader = pending.is_empty();
        let mut receivers = Vec::with_capacity(transactions.len());
        for transaction in transactions {
            let (response, receiver) = mpsc::channel();
            receivers.push(receiver);
            pending.push(Request {
                transaction,
                response,
                #[cfg(feature = "lock-timing")]
                submitted: diagnostic_now_ns(),
                queued: Instant::now(),
            });
        }
        let wake = self.wake_on_four.load(Ordering::Relaxed);
        if wake && pending.len() >= 4 {
            self.ready.notify_one();
        }
        if leader {
            self.drive(pending, wake)?;
        } else {
            drop(pending);
        }
        receivers
            .into_iter()
            .map(|r| {
                r.recv()
                    .map_err(|e| Error::CommitUncertain(io::Error::other(e)))
            })
            .collect()
    }
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn record_sync(&self) {
        self.syncs.fetch_add(1, Ordering::Relaxed);
    }

    fn drive<'a>(
        &'a self,
        mut pending: MutexGuard<'a, Vec<Request>>,
        wake_on_four: bool,
    ) -> Result<()> {
        #[cfg(feature = "group-handoff")]
        let _leader = {
            drop(pending);
            // A poisoned coordinator owns no durable state; writer/state poisoning
            // still rejects persistence, while queued callers must receive replies.
            let guard = self
                .leader
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            pending = self.pending.lock().map_err(|_| Error::Poisoned)?;
            guard
        };
        #[cfg(feature = "lock-timing")]
        let collection = self
            .collection_enabled
            .load(Ordering::Relaxed)
            .then(|| [diagnostic_now_ns(), 0, pending.len() as u64, 0, 0]);
        let start = self.timing.load(Ordering::Relaxed).then(Instant::now);
        let delay = Duration::from_micros(200);
        #[cfg(feature = "group-handoff")]
        let delay = delay.saturating_sub(pending[0].queued.elapsed());
        #[cfg(feature = "commit-window")]
        let target = if pending.len() > 4 { MAX_BATCH } else { 4 };
        #[cfg(not(feature = "commit-window"))]
        let target = 4;
        #[cfg(feature = "commit-window")]
        let delay = if target == MAX_BATCH {
            Duration::from_micros(200)
        } else {
            delay
        };
        #[cfg(feature = "adaptive-window")]
        let delay = if wake_on_four && pending.len() <= 4 {
            // Idle arrivals cannot fill a large group; an extra timed wake adds
            // latency while persistence already provides a collection interval.
            Duration::ZERO
        } else {
            delay
        };
        if wake_on_four && !delay.is_zero() {
            pending = self
                .ready
                .wait_timeout_while(pending, delay, |requests| requests.len() < target)
                .map_err(|_| Error::Poisoned)?
                .0;
        } else if !delay.is_zero() {
            drop(pending);
            std::thread::sleep(delay);
            pending = self.pending.lock().map_err(|_| Error::Poisoned)?;
        }
        let mut requests = std::mem::take(&mut *pending);
        drop(pending);
        #[cfg(feature = "lock-timing")]
        trace_mark(1);
        #[cfg(feature = "lock-timing")]
        if let Some(mut collection) = collection {
            collection[1] = COMMIT_TRACE.with(|trace| trace.get()[1]);
            collection[3] = requests.len() as u64;
            collection[4] = delay.as_nanos() as u64;
            self.collections.lock().unwrap().push(collection);
        }
        let database = requests[0].transaction.database.clone();
        let mut timing = start.map(|start| [start.elapsed().as_nanos() as u64, 0, 0]);
        let results = persist_group(&database, &mut requests, |file, frames| {
            let write = timing.as_ref().map(|_| Instant::now());
            for frame in frames {
                file.write_all(frame)?;
            }
            #[cfg(feature = "lock-timing")]
            trace_mark(5);
            self.record_sync();
            let sync = write.map(|write| write.elapsed().as_nanos() as u64);
            let start_sync = timing.as_ref().map(|_| Instant::now());
            #[cfg(any(test, feature = "sync-stall"))]
            self.stall_sync();
            #[cfg(test)]
            crash_group_at("before-sync");
            let result = file.sync_all();
            #[cfg(test)]
            crash_group_at("after-sync");
            #[cfg(feature = "lock-timing")]
            trace_mark(6);
            if let (Some(timing), Some(write), Some(start_sync)) = (&mut timing, sync, start_sync) {
                timing[1] = write;
                timing[2] = start_sync.elapsed().as_nanos() as u64;
            }
            result
        });
        if let Some(timing) = timing.filter(|timing| timing[2] != 0) {
            self.timings.lock().unwrap().push(timing);
        }
        #[cfg(feature = "lock-timing")]
        let trace = take_commit_trace();
        for (request, result) in requests.into_iter().zip(results) {
            drop(request.transaction);
            #[cfg(feature = "lock-timing")]
            let trace = {
                let mut v = trace;
                v[0] = request.submitted;
                v
            };
            let _ = request.response.send(Reply {
                result,
                #[cfg(feature = "lock-timing")]
                trace,
            });
        }
        Ok(())
    }

    pub fn submit(&self, transaction: Transaction) -> Result<u64> {
        let (response, receiver) = mpsc::channel();
        let mut pending = self.pending.lock().map_err(|_| Error::Poisoned)?;
        let bytes: usize = pending
            .iter()
            .map(|entry| entry.transaction.write_bytes)
            .sum();
        if pending.len() == MAX_BATCH || bytes + transaction.write_bytes > MAX_BATCH_BYTES {
            drop(pending);
            return transaction.commit_single();
        }
        let leader = pending.is_empty();
        pending.push(Request {
            transaction,
            response,
            #[cfg(feature = "lock-timing")]
            submitted: diagnostic_now_ns(),
            #[cfg(feature = "group-handoff")]
            queued: Instant::now(),
        });
        let wake_on_four = self.wake_on_four.load(Ordering::Relaxed);
        if wake_on_four && pending.len() == 4 {
            self.ready.notify_one();
        }

        if leader {
            self.drive(pending, wake_on_four)?;
        } else {
            drop(pending);
        }
        let reply = receiver
            .recv()
            .map_err(|error| Error::CommitUncertain(io::Error::other(error)))?;
        #[cfg(feature = "lock-timing")]
        COMMIT_TRACE.with(|trace| trace.set(reply.trace));
        reply.result
    }
}

#[cfg(test)]
fn crash_group_at(stage: &str) {
    if std::env::var("NEWDBMS_GROUP_CRASH_PHASE").as_deref() == Ok(stage) {
        std::process::exit(77);
    }
}

fn persist_group(
    database: &Database,
    requests: &mut [Request],
    persist: impl FnOnce(&mut File, &[Vec<u8>]) -> io::Result<()>,
) -> Vec<Result<u64>> {
    #[cfg(feature = "lock-timing")]
    trace_mark(2);
    let Ok(_writer) = database.inner.writer.lock() else {
        return requests.iter().map(|_| Err(Error::Poisoned)).collect();
    };
    #[cfg(feature = "lock-timing")]
    trace_mark(3);
    let Ok(mut state) = database.state() else {
        return requests.iter().map(|_| Err(Error::Poisoned)).collect();
    };
    let mut results = Vec::with_capacity(requests.len());
    let mut accepted = Vec::new();
    let mut frames = Vec::new();
    let mut reservations = Vec::new();
    let mut earlier = BTreeSet::new();
    let mut sequence = state.sequence;
    let mut wal_bytes = state.wal_bytes;
    let mut retained = 0usize;
    for (index, request) in requests.iter().enumerate() {
        let transaction = &request.transaction;
        let result = (|| {
            transaction.validate_commit(&state, &earlier)?;
            let next = sequence
                .checked_add(1)
                .ok_or(Error::Limit("commit sequence"))?;
            let length = transaction.write_bytes + PREFIX_BYTES + TRAILER_BYTES;
            let next_bytes = wal_bytes
                .checked_add(length as u64)
                .filter(|bytes| *bytes <= MAX_WAL_BYTES)
                .ok_or(Error::Limit("WAL bytes"))?;
            let growth = retained
                .checked_add(state.retained_growth(&transaction.writes))
                .ok_or(Error::ResourceExhausted("group retained bytes"))?;
            state.budget.check_add(RETAINED, growth)?;
            let permit = transaction.reserve_buffer(
                transaction.write_bytes * 4
                    + PREFIX_BYTES
                    + TRAILER_BYTES
                    + 64 * transaction.writes.len(),
            )?;
            frames.push(encode_frame(next, &transaction.writes));
            reservations.push(permit);
            earlier.extend(transaction.writes.keys().map(Vec::as_slice));
            sequence = next;
            wal_bytes = next_bytes;
            retained = growth;
            accepted.push((index, next));
            Ok(next)
        })();
        results.push(result);
    }
    drop(earlier);
    if accepted.is_empty() {
        return results;
    }
    state.reserve_log(wal_bytes);
    #[cfg(feature = "lock-timing")]
    trace_mark(4);
    let mut state = match database.persist_wal(state, |file| persist(file, &frames)) {
        Ok(state) => state,
        Err(error) => {
            for (index, _) in accepted {
                results[index] = Err(Error::CommitUncertain(io::Error::new(
                    error.kind(),
                    error.to_string(),
                )));
            }
            return results;
        }
    };
    let outcome = (|| {
        for &(index, version) in &accepted {
            state
                .apply(
                    version,
                    std::mem::take(&mut requests[index].transaction.writes),
                )
                .map_err(io::Error::other)?;
        }
        state.wal_bytes = wal_bytes;
        Ok::<_, io::Error>(())
    })();
    if let Err(error) = outcome {
        state.poisoned = true;
        for (index, _) in accepted {
            results[index] = Err(Error::CommitUncertain(io::Error::new(
                error.kind(),
                error.to_string(),
            )));
        }
    }
    #[cfg(feature = "lock-timing")]
    trace_mark(7);
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let directory = Path::new(env!("CARGO_MANIFEST_DIR")).join("artifacts");
            std::fs::create_dir_all(&directory).unwrap();
            Self(temporary_path(&directory.join("group-test.wal")))
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(with_suffix(&self.0, ".owner"));
        }
    }

    fn write(database: &Database, key: &[u8]) -> Transaction {
        let mut transaction = database.begin().unwrap();
        transaction.put(key, b"value").unwrap();
        transaction
    }

    fn request(transaction: Transaction) -> Request {
        Request {
            transaction,
            response: mpsc::channel().0,
            #[cfg(feature = "lock-timing")]
            submitted: diagnostic_now_ns(),
            #[cfg(feature = "group-handoff")]
            queued: Instant::now(),
        }
    }

    #[cfg(feature = "commit-window")]
    #[test]
    fn eq_window_preserves_independent_conflicts_and_old_reads() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [61; 16]).unwrap();
        db.set_group_commit_wake_on_four(true);
        let mut old = db.begin().unwrap();
        let first = write(&db, b"a");
        let duplicate = write(&db, b"a");
        let mut read = write(&db, b"r");
        assert_eq!(read.get(b"a").unwrap(), None);
        let mut range = write(&db, b"s");
        assert!(range.scan(b"a", Some(b"b")).unwrap().is_empty());
        let last = write(&db, b"z");
        let result = db
            .commit_batch(vec![first, duplicate, read, range, last])
            .unwrap();
        assert!(matches!(result[0].result, Ok(1)));
        assert!(matches!(
            result[1].result,
            Err(Error::Conflict(ConflictKind::Write))
        ));
        assert!(matches!(
            result[2].result,
            Err(Error::Conflict(ConflictKind::Read))
        ));
        assert!(matches!(
            result[3].result,
            Err(Error::Conflict(ConflictKind::Range))
        ));
        assert!(matches!(result[4].result, Ok(2)));
        assert!(old.scan(b"", None).unwrap().is_empty());
        let mut edit = db.begin().unwrap();
        edit.put(b"a", b"edited").unwrap();
        let mut delete = db.begin().unwrap();
        delete.delete(b"z").unwrap();
        assert!(
            db.commit_batch(vec![edit, delete])
                .unwrap()
                .iter()
                .all(|r| r.result.is_ok())
        );
        drop(old);
        db.checkpoint().unwrap();
        drop(db);
        assert_eq!(
            Database::open(&f.0)
                .unwrap()
                .begin()
                .unwrap()
                .scan(b"", None)
                .unwrap(),
            vec![(b"a".to_vec(), b"edited".to_vec())]
        );
    }

    #[cfg(all(feature = "adaptive-window", feature = "lock-timing"))]
    #[test]
    fn st_light_and_backlogged_windows_preserve_values_without_idle_collection_wait() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [70; 16]).unwrap();
        db.set_group_commit_wake_on_four(true);
        db.set_group_collection_timing(true);
        let mut old = db.begin().unwrap();
        let counts = [1_u8, 4, 5, 31, 32, 4, 1];
        let mut expected = BTreeMap::new();
        for (round, count) in counts.into_iter().enumerate() {
            let txs = (0..count)
                .map(|key| {
                    let mut tx = db.begin().unwrap();
                    tx.put(&[key], &[round as u8]).unwrap();
                    expected.insert(vec![key], vec![round as u8]);
                    tx
                })
                .collect();
            let outcomes = db.commit_batch(txs).unwrap();
            assert_eq!(outcomes.len(), usize::from(count));
            assert!(outcomes.iter().all(|r| r.result.is_ok()));
            let timing = db.take_group_collection_timings();
            assert_eq!(timing.len(), 1);
            assert_eq!(timing[0][2], u64::from(count));
            assert_eq!(timing[0][3], u64::from(count));
            assert_eq!(timing[0][4], if count <= 4 { 0 } else { 200_000 });
            assert!(old.scan(b"", None).unwrap().is_empty());
            assert_eq!(
                db.begin().unwrap().scan(b"", None).unwrap(),
                expected.clone().into_iter().collect::<Vec<_>>()
            );
        }
        drop(old);
        db.checkpoint().unwrap();
        drop(db);
        assert_eq!(
            Database::open(&f.0)
                .unwrap()
                .begin()
                .unwrap()
                .scan(b"", None)
                .unwrap(),
            expected.into_iter().collect::<Vec<_>>()
        );
    }

    #[cfg(feature = "commit-window")]
    #[test]
    fn bv_window_rejects_excess_count_and_bytes_without_partial_submission() {
        for count in [0, 1, 31, 32, 33] {
            let f = Fixture::new();
            let db = Database::create(&f.0, [62; 16]).unwrap();
            let txs = (0_u64..count)
                .map(|k| write(&db, &k.to_be_bytes()))
                .collect();
            let outcome = db.commit_batch(txs);
            if count > 32 {
                assert!(matches!(outcome, Err(Error::ResourceExhausted(_))));
                assert_eq!(db.stats().unwrap().version, 0);
            } else {
                assert_eq!(outcome.unwrap().len(), count as usize);
                assert_eq!(db.stats().unwrap().version, count);
            }
        }
        for bytes in [MAX_BATCH_BYTES - 1, MAX_BATCH_BYTES, MAX_BATCH_BYTES + 1] {
            let f = Fixture::new();
            let db = Database::create(&f.0, [63; 16]).unwrap();
            let mut tx = db.begin().unwrap();
            tx.put(b"a", b"").unwrap();
            tx.put(b"a", &vec![9; bytes - tx.write_bytes]).unwrap();
            let outcome = db.commit_batch(vec![tx]);
            if bytes > MAX_BATCH_BYTES {
                assert!(matches!(outcome, Err(Error::ResourceExhausted(_))));
                assert_eq!(db.stats().unwrap().version, 0);
            } else {
                assert!(outcome.unwrap()[0].result.is_ok());
            }
        }
    }

    #[cfg(feature = "commit-window")]
    #[test]
    fn er_window_rejects_mixed_databases_and_read_only_entries_before_any_write() {
        let a = Fixture::new();
        let b = Fixture::new();
        let db = Database::create(&a.0, [64; 16]).unwrap();
        let other = Database::create(&b.0, [65; 16]).unwrap();
        assert!(matches!(
            db.commit_batch(vec![write(&db, b"a"), write(&other, b"b")]),
            Err(Error::Invalid(_))
        ));
        assert!(matches!(
            db.commit_batch(vec![write(&db, b"a"), db.begin().unwrap()]),
            Err(Error::Invalid(_))
        ));
        assert_eq!(db.stats().unwrap().version, 0);
        assert_eq!(other.stats().unwrap().version, 0);
    }

    #[cfg(feature = "commit-window")]
    #[test]
    fn st_window_keeps_all_successes_hidden_until_delayed_sync_finishes() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [66; 16]).unwrap();
        db.set_diagnostic_sync_stall(
            Duration::ZERO,
            Duration::from_secs(10),
            Duration::from_millis(28),
        );
        let mut old = db.begin().unwrap();
        std::thread::scope(|scope| {
            let stall = db.inner.group.stall.lock().unwrap();
            let (send, recv) = mpsc::channel();
            let database = &db;
            scope.spawn(move || {
                let txs = (0_u64..8)
                    .map(|k| write(database, &k.to_be_bytes()))
                    .collect();
                send.send(
                    database
                        .commit_batch(txs)
                        .unwrap()
                        .into_iter()
                        .map(|r| r.result)
                        .collect::<Vec<_>>(),
                )
                .unwrap();
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            while db.commit_sync_attempts() == 0 {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            assert!(db.begin().unwrap().scan(b"", None).unwrap().is_empty());
            assert!(matches!(recv.try_recv(), Err(mpsc::TryRecvError::Empty)));
            drop(stall);
            assert!(
                recv.recv_timeout(Duration::from_secs(10))
                    .unwrap()
                    .iter()
                    .all(Result::is_ok)
            );
        });
        assert!(old.scan(b"", None).unwrap().is_empty());
        drop(old);
        assert_eq!(db.commit_sync_attempts(), 1);
        drop(db);
        assert_eq!(
            Database::open(&f.0)
                .unwrap()
                .begin()
                .unwrap()
                .scan(b"", None)
                .unwrap()
                .len(),
            8
        );
    }

    #[cfg(feature = "commit-window")]
    #[test]
    fn st_four_windows_share_sync_without_stranding_requests() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [67; 16]).unwrap();
        db.set_group_commit_wake_on_four(true);
        let mut old = db.begin().unwrap();
        for round in 0_u8..8 {
            std::thread::scope(|scope| {
                let gate = db.inner.group.leader.lock().unwrap();
                let (send, recv) = mpsc::channel();
                for lane in 0_u8..4 {
                    let database = &db;
                    let send = send.clone();
                    scope.spawn(move || {
                        let txs = (0_u8..8)
                            .map(|key| {
                                let mut tx = database.begin().unwrap();
                                tx.put(&[lane, key], &[round]).unwrap();
                                tx
                            })
                            .collect();
                        send.send(
                            database
                                .commit_batch(txs)
                                .unwrap()
                                .into_iter()
                                .map(|r| r.result)
                                .collect::<Vec<_>>(),
                        )
                        .unwrap();
                    });
                }
                wait_pending(&db, 32);
                drop(gate);
                for _ in 0..4 {
                    assert!(
                        recv.recv_timeout(Duration::from_secs(10))
                            .unwrap()
                            .iter()
                            .all(Result::is_ok)
                    );
                }
            });
            assert!(old.scan(b"", None).unwrap().is_empty());
        }
        assert_eq!(db.commit_sync_attempts(), 8);
        drop(old);
        db.checkpoint().unwrap();
        drop(db);
        let db = Database::open(&f.0).unwrap();
        let rows = db.begin().unwrap().scan(b"", None).unwrap();
        assert_eq!(rows.len(), 32);
        assert!(rows.iter().all(|(_, v)| *v == vec![7]));
    }

    #[cfg(feature = "commit-window")]
    #[test]
    fn er_window_write_failure_replies_to_all_and_preserves_acknowledged_data() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [68; 16]).unwrap();
        write(&db, b"ack").commit().unwrap();
        let txs = (0_u8..8).map(|k| write(&db, &[k])).collect();
        db.inner.state.lock().unwrap().file = Some(File::open(&f.0).unwrap());
        let result = db.commit_batch(txs).unwrap();
        assert_eq!(result.len(), 8);
        assert!(
            result
                .iter()
                .all(|r| matches!(r.result, Err(Error::CommitUncertain(_))))
        );
        assert!(matches!(db.begin(), Err(Error::Poisoned)));
        drop(db);
        let db = Database::open(&f.0).unwrap();
        assert_eq!(
            db.begin().unwrap().scan(b"", None).unwrap(),
            vec![(b"ack".to_vec(), b"value".to_vec())]
        );
        assert!(
            db.commit_batch(vec![write(&db, b"next")]).unwrap()[0]
                .result
                .is_ok()
        );
    }

    #[cfg(feature = "commit-window")]
    #[test]
    #[ignore = "subprocess helper"]
    fn window_crash_worker() {
        let db = Database::open(std::env::var_os("NEWDBMS_GROUP_CRASH_PATH").unwrap()).unwrap();
        let mut paired = write(&db, b"a");
        paired.put(b"b", b"value").unwrap();
        let _ = db.commit_batch(vec![paired, write(&db, b"c")]);
        panic!("window crash point not reached");
    }

    #[cfg(feature = "commit-window")]
    #[test]
    fn er_window_process_exit_around_sync_preserves_atomic_transactions_and_acknowledged_data() {
        for phase in ["before-sync", "after-sync"] {
            let f = Fixture::new();
            let db = Database::create(&f.0, [69; 16]).unwrap();
            write(&db, b"ack").commit().unwrap();
            drop(db);
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "transactions::group_commit::tests::window_crash_worker",
                ])
                .env("NEWDBMS_GROUP_CRASH_PATH", &f.0)
                .env("NEWDBMS_GROUP_CRASH_PHASE", phase)
                .output()
                .unwrap();
            assert_eq!(result.status.code(), Some(77), "{result:?}");
            let db = Database::open(&f.0).unwrap();
            let mut tx = db.begin().unwrap();
            assert_eq!(tx.get(b"ack").unwrap(), Some(b"value".to_vec()));
            assert_eq!(tx.get(b"a").unwrap(), tx.get(b"b").unwrap());
            if tx.get(b"c").unwrap().is_some() {
                assert!(tx.get(b"a").unwrap().is_some());
            }
            if phase == "after-sync" {
                assert_eq!(tx.get(b"c").unwrap(), Some(b"value".to_vec()));
            }
        }
    }

    #[cfg(feature = "group-handoff")]
    fn wait_pending(database: &Database, count: usize) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while database.inner.group.pending.lock().unwrap().len() != count {
            assert!(Instant::now() < deadline, "queued requests did not arrive");
            std::thread::yield_now();
        }
    }

    #[cfg(feature = "group-handoff")]
    fn held_batch(database: &Database, transactions: Vec<Transaction>, queued: usize) {
        let count = transactions.len();
        std::thread::scope(|scope| {
            let gate = database.inner.group.leader.lock().unwrap();
            let (send, recv) = mpsc::channel();
            for tx in transactions {
                let send = send.clone();
                scope.spawn(move || send.send(tx.commit()).unwrap());
            }
            wait_pending(database, queued);
            for _ in queued..count {
                recv.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
            }
            assert_eq!(database.inner.group.pending.lock().unwrap().len(), queued);
            drop(gate);
            for _ in 0..queued {
                recv.recv_timeout(Duration::from_secs(10)).unwrap().unwrap();
            }
        });
    }

    #[cfg(feature = "group-handoff")]
    #[test]
    fn bv_handoff_count_limit_keeps_all_requests_and_bounds_each_batch() {
        for count in [MAX_BATCH - 1, MAX_BATCH, MAX_BATCH + 1] {
            let f = Fixture::new();
            let db = Database::create(&f.0, [1; 16]).unwrap();
            db.set_group_commit(true);
            db.set_group_commit_wake_on_four(true);
            let mut old = db.begin().unwrap();
            let transactions = (0..count).map(|k| write(&db, &k.to_be_bytes())).collect();
            held_batch(&db, transactions, count.min(MAX_BATCH));
            assert_eq!(
                db.commit_sync_attempts(),
                if count > MAX_BATCH { 2 } else { 1 }
            );
            assert!(old.scan(b"", None).unwrap().is_empty());
            assert_eq!(db.begin().unwrap().scan(b"", None).unwrap().len(), count);
            drop(old);
            drop(db);
            assert_eq!(
                Database::open(&f.0).unwrap().stats().unwrap().version,
                count as u64
            );
        }
    }

    #[cfg(feature = "group-handoff")]
    #[test]
    fn bv_handoff_byte_limit_keeps_values_below_at_and_above_boundary() {
        for total in [MAX_BATCH_BYTES - 1, MAX_BATCH_BYTES, MAX_BATCH_BYTES + 1] {
            let f = Fixture::new();
            let db = Database::create(&f.0, [2; 16]).unwrap();
            db.set_group_commit(true);
            let mut transactions = Vec::new();
            for (key, bytes) in [
                (b"a", MAX_BATCH_BYTES / 2),
                (b"b", total - MAX_BATCH_BYTES / 2),
            ] {
                let mut tx = db.begin().unwrap();
                tx.put(key, b"").unwrap();
                let value = vec![42; bytes - tx.write_bytes];
                tx.put(key, &value).unwrap();
                assert_eq!(tx.write_bytes, bytes);
                transactions.push(tx);
            }
            held_batch(
                &db,
                transactions,
                if total > MAX_BATCH_BYTES { 1 } else { 2 },
            );
            assert_eq!(
                db.commit_sync_attempts(),
                if total > MAX_BATCH_BYTES { 2 } else { 1 }
            );
            for key in [b"a", b"b"] {
                assert!(
                    db.begin()
                        .unwrap()
                        .get(key)
                        .unwrap()
                        .unwrap()
                        .iter()
                        .all(|b| *b == 42)
                );
            }
        }
    }

    #[cfg(feature = "group-handoff")]
    #[test]
    fn st_handoff_repeats_crud_without_stranding_waiters_or_changing_old_reads() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [3; 16]).unwrap();
        db.set_group_commit(true);
        db.set_group_commit_wake_on_four(true);
        let mut old = db.begin().unwrap();
        for round in 0..12 {
            let mut transactions = Vec::new();
            for key in [b"a", b"b", b"c", b"d"] {
                let mut tx = db.begin().unwrap();
                if round % 3 == 2 {
                    tx.delete(key).unwrap();
                } else {
                    tx.put(key, &[round]).unwrap();
                }
                transactions.push(tx);
            }
            held_batch(&db, transactions, 4);
            assert!(old.scan(b"", None).unwrap().is_empty());
            for key in [b"a", b"b", b"c", b"d"] {
                assert_eq!(
                    db.begin().unwrap().get(key).unwrap(),
                    if round % 3 == 2 {
                        None
                    } else {
                        Some(vec![round])
                    }
                );
            }
        }
        assert_eq!(db.commit_sync_attempts(), 12);
        drop(old);
        drop(db);
        assert!(
            Database::open(&f.0)
                .unwrap()
                .begin()
                .unwrap()
                .scan(b"", None)
                .unwrap()
                .is_empty()
        );
    }

    #[cfg(feature = "group-handoff")]
    #[test]
    fn er_handoff_replies_to_every_queued_caller_after_database_is_poisoned() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [4; 16]).unwrap();
        write(&db, b"ack").commit().unwrap();
        db.set_group_commit(true);
        std::thread::scope(|scope| {
            let gate = db.inner.group.leader.lock().unwrap();
            let (send, recv) = mpsc::channel();
            for key in [b"a", b"b", b"c", b"d"] {
                let tx = write(&db, key);
                let send = send.clone();
                scope.spawn(move || send.send(tx.commit()).unwrap());
            }
            wait_pending(&db, 4);
            db.inner.state.lock().unwrap().poisoned = true;
            drop(gate);
            for _ in 0..4 {
                assert!(matches!(
                    recv.recv_timeout(Duration::from_secs(10)).unwrap(),
                    Err(Error::Poisoned)
                ));
            }
        });
        drop(db);
        assert_eq!(
            Database::open(&f.0)
                .unwrap()
                .begin()
                .unwrap()
                .get(b"ack")
                .unwrap(),
            Some(b"value".to_vec())
        );
    }

    #[test]
    #[ignore = "subprocess helper"]
    fn group_crash_worker() {
        let db = Database::open(std::env::var_os("NEWDBMS_GROUP_CRASH_PATH").unwrap()).unwrap();
        db.set_group_commit(true);
        db.set_group_commit_wake_on_four(true);
        let mut tx = write(&db, b"a");
        tx.put(b"b", b"value").unwrap();
        let _ = tx.commit();
        panic!("group crash point not reached");
    }

    #[test]
    fn er_group_process_exit_around_sync_preserves_acknowledged_data_and_atomic_updates() {
        for phase in ["before-sync", "after-sync"] {
            let f = Fixture::new();
            let db = Database::create(&f.0, [5; 16]).unwrap();
            write(&db, b"ack").commit().unwrap();
            drop(db);
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "transactions::group_commit::tests::group_crash_worker",
                ])
                .env("NEWDBMS_GROUP_CRASH_PATH", &f.0)
                .env("NEWDBMS_GROUP_CRASH_PHASE", phase)
                .output()
                .unwrap();
            assert_eq!(result.status.code(), Some(77), "{result:?}");
            let db = Database::open(&f.0).unwrap();
            let mut tx = db.begin().unwrap();
            assert_eq!(tx.get(b"ack").unwrap(), Some(b"value".to_vec()));
            assert_eq!(tx.get(b"a").unwrap(), tx.get(b"b").unwrap());
            if phase == "after-sync" {
                assert_eq!(tx.get(b"a").unwrap(), Some(b"value".to_vec()));
            }
        }
    }

    #[test]
    fn wake_on_four_commits_a_lone_request_after_the_wait() {
        let fixture = Fixture::new();
        let database = Database::create(&fixture.0, [0; 16]).unwrap();
        database.set_group_commit(true);
        database.set_group_commit_wake_on_four(true);
        assert_eq!(write(&database, b"one").commit().unwrap(), 1);
        assert_eq!(database.commit_sync_attempts(), 1);
        assert_eq!(
            database.begin().unwrap().get(b"one").unwrap(),
            Some(b"value".to_vec())
        );
    }

    #[test]
    fn sync_precedes_publication_and_only_nonconflicting_frames_are_written() {
        let fixture = Fixture::new();
        let database = Database::create(&fixture.0, [0; 16]).unwrap();
        let first = write(&database, b"a");
        let same = write(&database, b"a");
        let mut read = write(&database, b"x");
        assert_eq!(read.get(b"a").unwrap(), None);
        let mut range = write(&database, b"y");
        assert!(range.scan(b"a", Some(b"b")).unwrap().is_empty());
        let last = write(&database, b"z");
        let mut snapshot = database.begin().unwrap();
        let mut requests = [first, same, read, range, last].map(request);
        let results = persist_group(&database, &mut requests, |file, frames| {
            assert_eq!(frames.len(), 2);
            assert!(database.inner.state.try_lock().is_ok());
            assert!(
                database
                    .begin()
                    .unwrap()
                    .scan(b"", None)
                    .unwrap()
                    .is_empty()
            );
            for frame in frames {
                file.write_all(frame)?;
            }
            file.sync_all()
        });
        assert!(matches!(
            &results[..],
            [
                Ok(1),
                Err(Error::Conflict(ConflictKind::Write)),
                Err(Error::Conflict(ConflictKind::Read)),
                Err(Error::Conflict(ConflictKind::Range)),
                Ok(2)
            ]
        ));
        assert!(snapshot.scan(b"", None).unwrap().is_empty());
        drop(snapshot);
        drop(requests);
        drop(database);
        let database = Database::open(&fixture.0).unwrap();
        let mut snapshot = database.begin().unwrap();
        assert_eq!(
            snapshot.scan(b"", None).unwrap(),
            vec![
                (b"a".to_vec(), b"value".to_vec()),
                (b"z".to_vec(), b"value".to_vec())
            ]
        );
    }

    #[test]
    fn failed_sync_marks_group_uncertain_and_recovery_keeps_complete_prefix() {
        let fixture = Fixture::new();
        let database = Database::create(&fixture.0, [0; 16]).unwrap();
        write(&database, b"confirmed").commit().unwrap();
        let mut requests = [
            request(write(&database, b"a")),
            request(write(&database, b"b")),
        ];
        let results = persist_group(&database, &mut requests, |file, frames| {
            file.write_all(&frames[0])?;
            file.write_all(&frames[1][..frames[1].len() - 1])?;
            Err(io::Error::other("injected persistence failure"))
        });
        assert!(
            results
                .iter()
                .all(|result| matches!(result, Err(Error::CommitUncertain(_))))
        );
        assert!(matches!(database.begin(), Err(Error::Poisoned)));
        drop(requests);
        drop(database);
        let database = Database::open(&fixture.0).unwrap();
        let mut reader = database.begin().unwrap();
        assert_eq!(reader.get(b"confirmed").unwrap(), Some(b"value".to_vec()));
        assert_eq!(reader.get(b"a").unwrap(), Some(b"value".to_vec()));
        assert_eq!(reader.get(b"b").unwrap(), None);
        assert_eq!(write(&database, b"new").commit().unwrap(), 3);
    }

    #[test]
    fn group_admission_counts_cumulative_retained_bytes_and_releases_buffers() {
        let fixture = Fixture::new();
        let database = Database::create_with_limits(
            &fixture.0,
            [0; 16],
            ResourceLimits {
                max_retained_bytes: 263,
                ..ResourceLimits::default()
            },
        )
        .unwrap();
        let mut requests = [
            request(write(&database, b"a")),
            request(write(&database, b"b")),
        ];
        let results = persist_group(&database, &mut requests, |file, frames| {
            assert_eq!(frames.len(), 1);
            file.write_all(&frames[0])?;
            file.sync_all()
        });
        assert!(matches!(
            &results[..],
            [Ok(1), Err(Error::ResourceExhausted(_))]
        ));
        drop(requests);
        let usage = database.resource_usage().unwrap();
        assert_eq!(usage.current, [0, 0, 0, 263, 0]);
    }
}
