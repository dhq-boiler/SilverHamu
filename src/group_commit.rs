use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

const MAX_BATCH: usize = 32;
const MAX_BATCH_BYTES: usize = 4 * 1024 * 1024;

#[cfg(feature = "handoff-refill")]
fn refill_delay(
    wake_on_four: bool,
    initial_delay: Duration,
    handed_off: bool,
    queued: usize,
) -> Duration {
    if wake_on_four && initial_delay.is_zero() && handed_off && (1..4).contains(&queued) {
        // Draining an old partial group immediately can strand the previous
        // group's clients in another sync; idle and full groups need no refill.
        Duration::from_micros(1000)
    } else {
        Duration::ZERO
    }
}

#[cfg(feature = "handoff-refill")]
fn cohort_delay(
    wake_on_four: bool,
    initial: Duration,
    previous: Option<(Instant, usize)>,
    now: Instant,
) -> Duration {
    if wake_on_four
        && previous.is_some_and(|(ended, count)| {
            count >= 4 && now.saturating_duration_since(ended) < Duration::from_millis(2)
        })
    {
        Duration::from_micros(1000)
    } else {
        initial
    }
}

#[derive(Default)]
pub(super) struct GroupCommit {
    enabled: AtomicBool,
    #[cfg(feature = "single-client-commit")]
    clients: std::sync::OnceLock<Arc<std::sync::atomic::AtomicUsize>>,
    syncs: AtomicU64,
    pending: Mutex<Vec<Request>>,
    #[cfg(feature = "group-handoff")]
    leader: Mutex<()>,
    #[cfg(feature = "handoff-refill")]
    recent_group: Mutex<Option<(Instant, usize)>>,
    wake_on_four: AtomicBool,
    ready: std::sync::Condvar,
    timing: AtomicBool,
    timings: Mutex<Vec<[u64; 3]>>,
    #[cfg(feature = "lock-timing")]
    collection_enabled: AtomicBool,
    #[cfg(feature = "lock-timing")]
    collections: Mutex<Vec<[u64; 5]>>,
    #[cfg(any(test, feature = "sync-stall"))]
    stall: Mutex<Option<SyncStall>>,
    #[cfg(any(test, feature = "sync-stall"))]
    stall_count: AtomicU64,
    #[cfg(any(test, feature = "sync-stall"))]
    stall_elapsed: AtomicU64,
    #[cfg(any(test, feature = "sync-stall"))]
    stall_first: AtomicU64,
    #[cfg(any(test, feature = "sync-stall"))]
    stall_last: AtomicU64,
    #[cfg(test)]
    sync_gate: Mutex<Option<Arc<[std::sync::Barrier; 2]>>>,
}

#[cfg(any(test, feature = "sync-stall"))]
struct SyncStall {
    origin: Instant,
    start: Duration,
    duration: Duration,
    delay: Duration,
    remaining: Option<u64>,
}

struct Request {
    transaction: Transaction,
    response: Sender<Reply>,
    #[cfg(feature = "lock-timing")]
    submitted: u64,
    #[cfg(feature = "lock-timing")]
    entry: [u64; 2],
    #[cfg(feature = "group-handoff")]
    queued: Instant,
}

struct Reply {
    result: Result<u64>,
    #[cfg(feature = "lock-timing")]
    trace: [u64; 8],
    #[cfg(feature = "lock-timing")]
    detail: [u64; 8],
    #[cfg(feature = "lock-timing")]
    costs: [u64; 2],
}

#[cfg(any(feature = "commit-window", feature = "backlog-batch"))]
pub struct CommitOutcome {
    pub result: Result<u64>,
    #[cfg(feature = "lock-timing")]
    pub trace: [u64; 8],
}

impl Database {
    #[cfg(feature = "single-client-commit")]
    pub(crate) fn set_service_clients(&self, clients: Arc<std::sync::atomic::AtomicUsize>) {
        let _ = self.inner.group.clients.set(clients);
    }

    #[cfg(test)]
    pub(crate) fn hold_test_sync(&self, gate: Arc<[std::sync::Barrier; 2]>) {
        *self.inner.group.sync_gate.lock().unwrap() = Some(gate);
    }

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

    #[cfg(any(feature = "commit-window", feature = "backlog-batch"))]
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
        self.set_diagnostic_sync_stall_limit(start, duration, delay, None);
    }

    #[cfg(any(test, feature = "sync-stall"))]
    #[doc(hidden)]
    pub fn set_diagnostic_sync_stall_limit(
        &self,
        start: Duration,
        duration: Duration,
        delay: Duration,
        remaining: Option<u64>,
    ) {
        *self.inner.group.stall.lock().unwrap() = Some(SyncStall {
            origin: Instant::now(),
            start,
            duration,
            delay,
            remaining,
        });
        self.inner.group.stall_count.store(0, Ordering::Relaxed);
        self.inner.group.stall_elapsed.store(0, Ordering::Relaxed);
        self.inner.group.stall_first.store(0, Ordering::Relaxed);
        self.inner.group.stall_last.store(0, Ordering::Relaxed);
    }

    #[cfg(any(test, feature = "sync-stall"))]
    #[doc(hidden)]
    pub fn diagnostic_stall_count(&self) -> u64 {
        self.inner.group.stall_count.load(Ordering::Relaxed)
    }
    #[cfg(any(test, feature = "sync-stall"))]
    #[doc(hidden)]
    pub fn diagnostic_stall_timing(&self) -> [u64; 3] {
        [
            self.inner.group.stall_first.load(Ordering::Relaxed),
            self.inner.group.stall_last.load(Ordering::Relaxed),
            self.inner.group.stall_elapsed.load(Ordering::Relaxed),
        ]
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

    /// Number of commit durability attempts on this open handle.
    /// The experimental fixed-length log uses two sync calls per attempt.
    pub fn commit_sync_attempts(&self) -> u64 {
        self.inner.group.syncs.load(Ordering::Relaxed)
    }
}

impl GroupCommit {
    #[cfg(any(test, feature = "sync-stall"))]
    fn stall_sync(&self) {
        #[cfg(test)]
        if let Some(gate) = self.sync_gate.lock().unwrap().take() {
            gate[0].wait();
            gate[1].wait();
        }
        let delay = self.stall.lock().unwrap().as_mut().and_then(|stall| {
            let elapsed = stall.origin.elapsed();
            if elapsed < stall.start
                || elapsed >= stall.start + stall.duration
                || stall.remaining == Some(0)
            {
                return None;
            }
            if let Some(remaining) = &mut stall.remaining {
                *remaining -= 1;
            }
            Some((stall.delay, stall.origin))
        });
        if let Some((delay, origin)) = delay {
            let started = origin.elapsed().as_nanos() as u64;
            if self.stall_count.fetch_add(1, Ordering::Relaxed) == 0 {
                self.stall_first.store(started, Ordering::Relaxed);
            }
            std::thread::sleep(delay);
            let ended = origin.elapsed().as_nanos() as u64;
            self.stall_elapsed
                .fetch_add(ended - started, Ordering::Relaxed);
            self.stall_last.store(ended, Ordering::Relaxed);
        }
    }

    #[cfg(any(feature = "commit-window", feature = "backlog-batch"))]
    fn submit_many(&self, transactions: Vec<Transaction>) -> Result<Vec<Reply>> {
        #[cfg(feature = "lock-timing")]
        {
            take_commit_trace();
            take_commit_detail();
            take_commit_costs();
            detail_mark(7);
        }
        #[cfg(feature = "wal-prepare")]
        let transactions = {
            let mut txs = transactions;
            for tx in &mut txs {
                tx.prepare_log()?;
            }
            txs
        };
        if transactions.is_empty() {
            return Ok(Vec::new());
        }
        #[cfg(feature = "lock-timing")]
        detail_mark(0);
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
                #[cfg(feature = "lock-timing")]
                entry: COMMIT_DETAIL.with(|v| [v.get()[7], v.get()[0]]),
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
    #[cfg(feature = "single-client-commit")]
    fn single_service_client(&self) -> bool {
        // A newly arriving connection can miss this cohort; it still serializes
        // through the writer and receives success only after its own durability step.
        self.clients
            .get()
            .is_some_and(|clients| clients.load(Ordering::Acquire) == 1)
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
        #[cfg(feature = "handoff-refill")]
        let mut handed_off = false;
        #[cfg(feature = "group-handoff")]
        let _leader = {
            drop(pending);
            // A poisoned coordinator owns no durable state; writer/state poisoning
            // still rejects persistence, while queued callers must receive replies.
            #[cfg(not(feature = "handoff-refill"))]
            let guard = self
                .leader
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            #[cfg(feature = "handoff-refill")]
            let guard = match self.leader.try_lock() {
                Ok(guard) => guard,
                Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
                Err(std::sync::TryLockError::WouldBlock) => {
                    handed_off = true;
                    self.leader
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                }
            };
            pending = self.pending.lock().map_err(|_| Error::Poisoned)?;
            guard
        };
        #[cfg(feature = "lock-timing")]
        detail_mark(1);
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
        #[cfg(feature = "handoff-refill")]
        let delay = cohort_delay(
            wake_on_four,
            delay,
            *self.recent_group.lock().map_err(|_| Error::Poisoned)?,
            Instant::now(),
        );
        #[cfg(feature = "single-client-commit")]
        let single_client = self.single_service_client();
        #[cfg(feature = "single-client-commit")]
        let delay = if single_client { Duration::ZERO } else { delay };
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
        #[cfg(feature = "handoff-refill")]
        {
            let refill = refill_delay(wake_on_four, delay, handed_off, pending.len());
            #[cfg(feature = "single-client-commit")]
            let refill = if single_client {
                Duration::ZERO
            } else {
                refill
            };
            if !refill.is_zero() {
                pending = self
                    .ready
                    .wait_timeout_while(pending, refill, |requests| requests.len() < 4)
                    .map_err(|_| Error::Poisoned)?
                    .0;
            }
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
        #[cfg(feature = "lock-timing")]
        let detail = take_commit_detail();
        #[cfg(feature = "lock-timing")]
        let costs = take_commit_costs();
        #[cfg(feature = "handoff-refill")]
        {
            let successful = results.iter().filter(|result| result.is_ok()).count();
            *self
                .recent_group
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some((Instant::now(), successful));
        }
        for (request, result) in requests.into_iter().zip(results) {
            drop(request.transaction);
            #[cfg(feature = "lock-timing")]
            let trace = {
                let mut v = trace;
                v[0] = request.submitted;
                v
            };
            #[cfg(feature = "lock-timing")]
            let detail = {
                let mut v = detail;
                v[0] = request.entry[1];
                v[7] = request.entry[0];
                v[5] = diagnostic_now_ns();
                v
            };
            let _ = request.response.send(Reply {
                result,
                #[cfg(feature = "lock-timing")]
                trace,
                #[cfg(feature = "lock-timing")]
                detail,
                #[cfg(feature = "lock-timing")]
                costs,
            });
        }
        Ok(())
    }

    pub fn submit(&self, transaction: Transaction) -> Result<u64> {
        #[cfg(feature = "wal-prepare")]
        let transaction = {
            let mut tx = transaction;
            tx.prepare_log()?;
            tx
        };
        let (response, receiver) = mpsc::channel();
        #[cfg(feature = "lock-timing")]
        detail_mark(0);
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
            #[cfg(feature = "lock-timing")]
            entry: COMMIT_DETAIL.with(|v| [v.get()[7], v.get()[0]]),
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
        {
            COMMIT_TRACE.with(|trace| trace.set(reply.trace));
            COMMIT_DETAIL.with(|trace| trace.set(reply.detail));
            COMMIT_COSTS.with(|value| value.set(reply.costs));
            detail_mark(6);
        }
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
    persist: impl FnOnce(&mut WalFile, &[Vec<u8>]) -> io::Result<()>,
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
    #[cfg(feature = "lock-timing")]
    detail_mark(2);
    let mut results = Vec::with_capacity(requests.len());
    let mut accepted = Vec::new();
    let mut frames = Vec::new();
    let mut reservations = Vec::new();
    let mut earlier = BTreeSet::new();
    let mut sequence = state.sequence;
    let mut wal_bytes = state.wal_bytes;
    let mut retained = 0usize;
    for (index, request) in requests.iter_mut().enumerate() {
        let transaction = &mut request.transaction;
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
            state.check_log_capacity(next_bytes)?;
            let growth = retained
                .checked_add(state.retained_growth(&transaction.writes))
                .ok_or(Error::ResourceExhausted("group retained bytes"))?;
            state.budget.check_add(RETAINED, growth)?;
            let (frame, permit) = transaction.commit_frame(next, 64 * transaction.writes.len())?;
            frames.push(frame);
            reservations.push(permit);
            sequence = next;
            wal_bytes = next_bytes;
            retained = growth;
            accepted.push((index, next));
            Ok(next)
        })();
        if result.is_ok() {
            earlier.extend(transaction.writes.keys().map(Vec::as_slice));
        }
        results.push(result);
    }
    drop(earlier);
    if accepted.is_empty() {
        return results;
    }
    #[cfg(feature = "lock-timing")]
    detail_mark(3);
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
    #[cfg(feature = "single-client-commit")]
    #[test]
    fn single_client_hint_tracks_zero_one_many_and_disconnect_without_affecting_embedded() {
        let group = GroupCommit::default();
        assert!(!group.single_service_client());
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        group.clients.set(Arc::clone(&count)).unwrap();
        for n in [0, 1, 2, 4, 256, 2, 1, 0] {
            count.store(n, Ordering::Release);
            assert_eq!(group.single_service_client(), n == 1);
        }
    }

    #[cfg(feature = "lock-timing")]
    #[test]
    fn diagnostic_failure_keeps_reached_phases_and_never_invents_sync_or_publication() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [119; 16]).unwrap();
        take_commit_trace();
        take_commit_detail();
        take_commit_costs();
        let mut requests = [request(write(&db, b"a"))];
        let results = persist_group(&db, &mut requests, |_, _| {
            trace_mark(5);
            Err(io::Error::other("injected sync failure"))
        });
        assert!(matches!(results[0], Err(Error::CommitUncertain(_))));
        let trace = take_commit_trace();
        assert!(trace[4] > 0 && trace[5] >= trace[4]);
        assert_eq!(trace[6..], [0, 0]);
        assert!(take_commit_detail()[4] >= trace[5]);
        assert!(take_commit_costs().iter().all(|n| *n > 0));
        assert_eq!(take_commit_trace(), [0; 8]);
        assert_eq!(take_commit_detail(), [0; 8]);
        assert_eq!(take_commit_costs(), [0; 2]);
    }

    #[cfg(feature = "wal-prepare")]
    #[test]
    fn wal_preparation_group_preserves_earlier_write_read_and_range_conflicts() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [113; 16]).unwrap();
        let mut old = db.begin_read().unwrap();
        let first = write(&db, b"a");
        let duplicate = write(&db, b"a");
        let mut read = write(&db, b"r");
        read.get(b"a").unwrap();
        let mut range = write(&db, b"s");
        range.scan(b"a", Some(b"b")).unwrap();
        let last = write(&db, b"z");
        let mut requests: Vec<_> = [first, duplicate, read, range, last]
            .into_iter()
            .map(|mut tx| {
                tx.prepare_log().unwrap();
                request(tx)
            })
            .collect();
        let outcome = persist_group(&db, &mut requests, |file, frames| {
            for frame in frames {
                file.write_all(frame)?;
            }
            file.sync_all()
        });
        assert!(matches!(outcome[0], Ok(1)));
        assert!(matches!(
            outcome[1],
            Err(Error::Conflict(ConflictKind::Write))
        ));
        assert!(matches!(
            outcome[2],
            Err(Error::Conflict(ConflictKind::Read))
        ));
        assert!(matches!(
            outcome[3],
            Err(Error::Conflict(ConflictKind::Range))
        ));
        assert!(matches!(outcome[4], Ok(2)));
        assert_eq!(old.get(b"a").unwrap(), None);
        drop(requests);
        drop(old);
        assert_eq!(
            db.resource_usage().unwrap().current[crate::resources::BUFFER],
            0
        );
    }

    #[test]
    fn global_guard_keeps_group_range_conflicts_for_insert_update_delete_and_boundaries() {
        for operation in ["insert", "update", "delete"] {
            for key in [b"a".as_slice(), b"m", b"z"] {
                let f = Fixture::new();
                let db = Database::create(&f.0, [84; 16]).unwrap();
                if operation != "insert" {
                    write(&db, key).commit().unwrap();
                }
                let old = db.begin_read().unwrap();
                let mut first = db.begin().unwrap();
                if operation == "delete" {
                    first.delete(key).unwrap();
                } else {
                    first.put(key, b"new").unwrap();
                }
                let mut second = write(&db, b"side");
                second.scan(b"a", Some(b"z")).unwrap();
                assert_eq!(second.snapshot, db.current_version().unwrap());
                let mut requests = [first, second].map(request);
                let results = persist_group(&db, &mut requests, |file, frames| {
                    for frame in frames {
                        file.write_all(frame)?;
                    }
                    file.sync_all()
                });
                assert!(results[0].is_ok());
                if key == b"z" {
                    assert!(results[1].is_ok());
                } else {
                    assert!(matches!(
                        results[1],
                        Err(Error::Conflict(ConflictKind::Range))
                    ));
                }
                drop(requests);
                drop(old);
                drop(db);
                let db = Database::open(&f.0).unwrap();
                assert_eq!(
                    db.begin_read().unwrap().get(b"side").unwrap().is_some(),
                    key == b"z"
                );
            }
        }
    }

    #[test]
    fn global_guard_keeps_same_group_point_dependencies_and_empty_ranges() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [85; 16]).unwrap();
        let first = write(&db, b"m");
        let same = write(&db, b"m");
        let mut read = write(&db, b"read-side");
        read.get(b"m").unwrap();
        let mut empty = write(&db, b"empty-side");
        empty.scan(b"m", Some(b"m")).unwrap();
        let mut requests = [first, same, read, empty].map(request);
        let results = persist_group(&db, &mut requests, |file, frames| {
            for frame in frames {
                file.write_all(frame)?;
            }
            file.sync_all()
        });
        assert!(matches!(
            &results[..],
            [
                Ok(_),
                Err(Error::Conflict(ConflictKind::Write)),
                Err(Error::Conflict(ConflictKind::Read)),
                Ok(_)
            ]
        ));
    }

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
            #[cfg(feature = "lock-timing")]
            entry: COMMIT_DETAIL.with(|v| [v.get()[7], v.get()[0]]),
            #[cfg(feature = "group-handoff")]
            queued: Instant::now(),
        }
    }

    #[cfg(any(feature = "commit-window", feature = "backlog-batch"))]
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

    #[cfg(any(feature = "commit-window", feature = "backlog-batch"))]
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

    #[cfg(any(feature = "commit-window", feature = "backlog-batch"))]
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

    #[cfg(any(feature = "commit-window", feature = "backlog-batch"))]
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

    #[cfg(any(feature = "commit-window", feature = "backlog-batch"))]
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

    #[cfg(any(feature = "commit-window", feature = "backlog-batch"))]
    #[test]
    fn er_window_write_failure_replies_to_all_and_preserves_acknowledged_data() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [68; 16]).unwrap();
        write(&db, b"ack").commit().unwrap();
        let txs = (0_u8..8).map(|k| write(&db, &[k])).collect();
        db.inner.state.lock().unwrap().file = Some(legacy_wal_file(File::open(&f.0).unwrap()));
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

    #[cfg(any(feature = "commit-window", feature = "backlog-batch"))]
    #[test]
    #[ignore = "subprocess helper"]
    fn window_crash_worker() {
        let db = Database::open(std::env::var_os("NEWDBMS_GROUP_CRASH_PATH").unwrap()).unwrap();
        let mut paired = write(&db, b"a");
        paired.put(b"b", b"value").unwrap();
        let _ = db.commit_batch(vec![paired, write(&db, b"c")]);
        panic!("window crash point not reached");
    }

    #[cfg(any(feature = "commit-window", feature = "backlog-batch"))]
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

    #[cfg(feature = "handoff-refill")]
    #[test]
    fn st_recent_full_group_can_reform_but_idle_or_small_groups_keep_the_original_budget() {
        let end = Instant::now();
        let original = Duration::from_micros(200);
        for count in [1, 2, 3] {
            assert_eq!(
                cohort_delay(true, original, Some((end, count)), end),
                original
            );
        }
        for age in [
            Duration::from_millis(2),
            Duration::from_millis(2) + Duration::from_nanos(1),
        ] {
            assert_eq!(
                cohort_delay(true, original, Some((end, 4)), end + age),
                original
            );
        }
        assert_eq!(cohort_delay(true, original, None, end), original);
        assert_eq!(cohort_delay(false, original, Some((end, 4)), end), original);
        assert_eq!(
            cohort_delay(
                true,
                original,
                Some((end, 4)),
                end + Duration::from_millis(2) - Duration::from_nanos(1)
            ),
            Duration::from_micros(1000)
        );
    }

    #[cfg(feature = "handoff-refill")]
    #[test]
    fn bv_refill_wait_requires_prior_writer_and_expired_partial_group() {
        for queued in [1, 2, 3, 4, MAX_BATCH] {
            assert_eq!(
                refill_delay(false, Duration::ZERO, true, queued),
                Duration::ZERO
            );
            assert_eq!(
                refill_delay(true, Duration::from_nanos(1), true, queued),
                Duration::ZERO
            );
        }
        for queued in [0, 4, MAX_BATCH] {
            assert_eq!(
                refill_delay(true, Duration::ZERO, true, queued),
                Duration::ZERO
            );
        }
        for queued in [1, 2, 3] {
            assert_eq!(
                refill_delay(true, Duration::ZERO, false, queued),
                Duration::ZERO
            );
            assert_eq!(
                refill_delay(true, Duration::ZERO, true, queued),
                Duration::from_micros(1000)
            );
        }
    }

    #[cfg(feature = "group-handoff")]
    #[test]
    fn st_handoff_collects_during_sync_without_early_ack_or_visibility() {
        for background in [false, true] {
            let fixture = Fixture::new();
            let db = Database::create(&fixture.0, [71; 16]).unwrap();
            db.set_background_reclamation(background).unwrap();
            db.set_group_commit(true);
            db.set_group_commit_wake_on_four(true);
            let mut old = db.begin().unwrap();
            let gate = Arc::new([std::sync::Barrier::new(2), std::sync::Barrier::new(2)]);
            *db.inner.group.sync_gate.lock().unwrap() = Some(Arc::clone(&gate));
            struct Release(Arc<[std::sync::Barrier; 2]>);
            impl Drop for Release {
                fn drop(&mut self) {
                    self.0[1].wait();
                }
            }
            std::thread::scope(|scope| {
                let (send, recv) = mpsc::channel();
                let first = write(&db, b"a");
                let first_send = send.clone();
                scope.spawn(move || first_send.send(first.commit()).unwrap());
                gate[0].wait();
                let release = Release(Arc::clone(&gate));
                for key in [b"b", b"c", b"d"] {
                    let tx = write(&db, key);
                    let send = send.clone();
                    scope.spawn(move || send.send(tx.commit()).unwrap());
                }
                wait_pending(&db, 3);
                assert_eq!(db.commit_sync_attempts(), 1);
                assert!(matches!(recv.try_recv(), Err(mpsc::TryRecvError::Empty)));
                assert!(old.scan(b"", None).unwrap().is_empty());
                assert!(db.begin().unwrap().scan(b"", None).unwrap().is_empty());
                drop(release);
                let mut versions: Vec<_> = (0..4)
                    .map(|_| recv.recv_timeout(Duration::from_secs(10)).unwrap().unwrap())
                    .collect();
                versions.sort();
                assert_eq!(versions, vec![1, 2, 3, 4]);
            });
            assert_eq!(db.commit_sync_attempts(), 2);
            assert!(old.scan(b"", None).unwrap().is_empty());
            drop(old);
            drop(db);
            let db = Database::open(&fixture.0).unwrap();
            let expected: Vec<_> = [b"a", b"b", b"c", b"d"]
                .into_iter()
                .map(|key| (key.to_vec(), b"value".to_vec()))
                .collect();
            assert_eq!(db.begin().unwrap().scan(b"", None).unwrap(), expected);
        }
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
        tx.scan(b"", None).unwrap();
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
        for background in [false, true] {
            let fixture = Fixture::new();
            let database = Database::create(&fixture.0, [0; 16]).unwrap();
            database.set_background_reclamation(background).unwrap();
            write(&database, b"confirmed").commit().unwrap();
            let mut first = write(&database, b"a");
            first.scan(b"q", Some(b"r")).unwrap();
            let mut requests = [request(first), request(write(&database, b"b"))];
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
