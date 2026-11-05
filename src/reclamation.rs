use super::{State, wrap_state};
use std::io;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Default)]
pub(super) struct Reclaimer {
    counters: Arc<Counters>,
    enabled: AtomicBool,
    worker: Mutex<Option<Worker>>,
}
#[derive(Default)]
struct Counters {
    busy: AtomicBool,
    batches: AtomicU64,
    bytes: AtomicU64,
    hold_max_ns: AtomicU64,
    lock_misses: AtomicU64,
    io_deferrals: AtomicU64,
    work_ns: AtomicU64,
    free_ns: AtomicU64,
    yield_ns: AtomicU64,
    pause_requested_ns: AtomicU64,
    pause_elapsed_ns: AtomicU64,
    worker_elapsed_ns: AtomicU64,
}
#[derive(Debug, Clone, Copy)]
pub struct ReclamationStatus {
    pub enabled: bool,
    pub busy: bool,
    pub batches: u64,
    pub reclaimed_bytes: u64,
    pub max_lock_hold_ns: u64,
}
#[derive(Debug, Clone, Copy)]
pub struct ReclamationProfile {
    pub lock_misses: u64,
    pub io_deferrals: u64,
    pub work_ns: u64,
    pub free_ns: u64,
    pub yield_ns: u64,
    pub pause_requested_ns: u64,
    pub pause_elapsed_ns: u64,
    pub worker_elapsed_ns: u64,
}
struct Busy<'a>(&'a AtomicBool);
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
struct Worker {
    stop: Arc<(Mutex<bool>, Condvar)>,
    handle: JoinHandle<()>,
}
impl Worker {
    fn stop(self) {
        let (lock, wake) = &*self.stop;
        *lock.lock().unwrap_or_else(|e| e.into_inner()) = true;
        wake.notify_one();
        let _ = self.handle.join();
    }
}
impl Reclaimer {
    pub fn status(&self) -> ReclamationStatus {
        ReclamationStatus {
            enabled: self.enabled(),
            busy: self.counters.busy.load(Ordering::Acquire),
            batches: self.counters.batches.load(Ordering::Relaxed),
            reclaimed_bytes: self.counters.bytes.load(Ordering::Relaxed),
            max_lock_hold_ns: self.counters.hold_max_ns.load(Ordering::Relaxed),
        }
    }
    pub fn profile(&self) -> ReclamationProfile {
        ReclamationProfile {
            lock_misses: self.counters.lock_misses.load(Ordering::Relaxed),
            io_deferrals: self.counters.io_deferrals.load(Ordering::Relaxed),
            work_ns: self.counters.work_ns.load(Ordering::Relaxed),
            free_ns: self.counters.free_ns.load(Ordering::Relaxed),
            yield_ns: self.counters.yield_ns.load(Ordering::Relaxed),
            pause_requested_ns: self.counters.pause_requested_ns.load(Ordering::Relaxed),
            pause_elapsed_ns: self.counters.pause_elapsed_ns.load(Ordering::Relaxed),
            worker_elapsed_ns: self.counters.worker_elapsed_ns.load(Ordering::Relaxed),
        }
    }
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Acquire)
    }
    pub fn set(&self, enabled: bool, state: &Arc<Mutex<State>>) -> io::Result<()> {
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| io::Error::other("reclaimer control poisoned"))?;
        if enabled {
            if worker.is_some() {
                return Ok(());
            }
            #[cfg(test)]
            if FAIL_START.with(|f| f.replace(false)) {
                return Err(io::Error::other("injected worker start failure"));
            }
            let stop = Arc::new((Mutex::new(false), Condvar::new()));
            let signal = stop.clone();
            // Owning Inner would retain the owner file indefinitely or join this very thread.
            let shared = state.clone();
            let counters = self.counters.clone();
            let handle = thread::Builder::new()
                .name("catdb-reclaim".into())
                .spawn(move || run(shared, signal, counters))?;
            *worker = Some(Worker { stop, handle });
            self.enabled.store(true, Ordering::Release);
        } else {
            self.enabled.store(false, Ordering::Release);
            if let Some(worker) = worker.take() {
                worker.stop();
            }
        }
        Ok(())
    }
}
impl Drop for Reclaimer {
    fn drop(&mut self) {
        if let Some(worker) = self
            .worker
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            worker.stop();
        }
    }
}
#[cfg(feature = "lock-timing")]
struct WorkerClock<'a> {
    counters: &'a Counters,
    started: std::time::Instant,
}
#[cfg(feature = "lock-timing")]
impl Drop for WorkerClock<'_> {
    fn drop(&mut self) {
        self.counters
            .worker_elapsed_ns
            .fetch_add(self.started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
}
fn run(state: Arc<Mutex<State>>, stop: Arc<(Mutex<bool>, Condvar)>, counters: Arc<Counters>) {
    #[cfg(feature = "lock-timing")]
    let _clock = WorkerClock {
        counters: &counters,
        started: std::time::Instant::now(),
    };
    let mut burst = 0;
    loop {
        if *stop.0.lock().unwrap_or_else(|e| e.into_inner()) {
            return;
        }
        counters.busy.store(true, Ordering::Release);
        let busy = Busy(&counters.busy);
        let mut contended = false;
        let pending = match state.try_lock() {
            Ok(guard) => {
                let started = std::time::Instant::now();
                let mut guard = wrap_state(guard);
                if guard.poisoned {
                    return;
                }
                if guard.file.is_none() {
                    #[cfg(feature = "lock-timing")]
                    counters.io_deferrals.fetch_add(1, Ordering::Relaxed);
                    None
                } else {
                    let retired = guard.collect_with_keys(16);
                    let pending = guard.gc_pending();
                    let charge = retired.charge;
                    counters
                        .hold_max_ns
                        .fetch_max(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    counters.batches.fetch_add(1, Ordering::Relaxed);
                    drop(guard);
                    #[cfg(feature = "lock-timing")]
                    counters
                        .work_ns
                        .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    #[cfg(feature = "lock-timing")]
                    let freeing = std::time::Instant::now();
                    drop(retired);
                    #[cfg(feature = "lock-timing")]
                    counters
                        .free_ns
                        .fetch_add(freeing.elapsed().as_nanos() as u64, Ordering::Relaxed);
                    counters.bytes.fetch_add(charge as u64, Ordering::Relaxed);
                    Some(pending)
                }
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                contended = true;
                #[cfg(feature = "lock-timing")]
                counters.lock_misses.fetch_add(1, Ordering::Relaxed);
                None
            }
            Err(std::sync::TryLockError::Poisoned(_)) => return,
        };
        drop(busy);
        if pending == Some(true) && burst < 15 {
            burst += 1;
            #[cfg(feature = "lock-timing")]
            let yielding = std::time::Instant::now();
            thread::yield_now();
            #[cfg(feature = "lock-timing")]
            counters
                .yield_ns
                .fetch_add(yielding.elapsed().as_nanos() as u64, Ordering::Relaxed);
            continue;
        }
        burst = 0;
        // Contending for the lock must not put maintenance ahead of queued requests.
        let pause = match pending {
            Some(false) => Duration::from_millis(10),
            None if contended => Duration::from_micros(50),
            None => Duration::from_millis(1),
            Some(true) => Duration::from_micros(100),
        };
        let stopped = stop.0.lock().unwrap_or_else(|e| e.into_inner());
        if *stopped {
            return;
        }
        #[cfg(feature = "lock-timing")]
        let sleeping = std::time::Instant::now();
        let _ = stop
            .1
            .wait_timeout_while(stopped, pause, |stopped| !*stopped)
            .unwrap_or_else(|e| e.into_inner());
        #[cfg(feature = "lock-timing")]
        {
            counters
                .pause_requested_ns
                .fetch_add(pause.as_nanos() as u64, Ordering::Relaxed);
            counters
                .pause_elapsed_ns
                .fetch_add(sleeping.elapsed().as_nanos() as u64, Ordering::Relaxed);
        }
    }
}
#[cfg(test)]
thread_local! { pub(super) static FAIL_START: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
