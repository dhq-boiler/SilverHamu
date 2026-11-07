use std::collections::VecDeque;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LockResult, Mutex, MutexGuard, PoisonError};
#[cfg(test)]
use std::sync::{TryLockError, TryLockResult};

#[derive(Default, Debug)]
pub(super) struct WriterMutex {
    state: Mutex<Queue>,
    poisoned: AtomicBool,
}

#[derive(Default, Debug)]
struct Queue {
    held: bool,
    waiters: VecDeque<Arc<Waiter>>,
}

#[derive(Debug, Default)]
struct Waiter {
    wake: Condvar,
    #[cfg(feature = "aged-writer")]
    queued: Enqueued,
}

#[cfg(feature = "aged-writer")]
#[derive(Debug)]
struct Enqueued(std::time::Instant);

#[cfg(feature = "aged-writer")]
impl Default for Enqueued {
    fn default() -> Self {
        Self(std::time::Instant::now())
    }
}

impl Queue {
    fn must_handoff(&self) -> bool {
        #[cfg(feature = "aged-writer")]
        {
            self.waiters
                .front()
                .is_some_and(|head| Self::aged(head.queued.0.elapsed()))
        }
        #[cfg(not(feature = "aged-writer"))]
        {
            !self.waiters.is_empty()
        }
    }

    #[cfg(feature = "aged-writer")]
    fn aged(wait: std::time::Duration) -> bool {
        wait >= std::time::Duration::from_millis(16)
    }
}

#[derive(Debug)]
pub(super) struct WriterGuard<'a> {
    lock: &'a WriterMutex,
    entered_panicking: bool,
    _thread_bound: PhantomData<MutexGuard<'a, ()>>,
}

impl WriterMutex {
    pub(super) fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }

    fn acquire(&self, state: &mut Queue) -> LockResult<WriterGuard<'_>> {
        state.held = true;
        let guard = WriterGuard {
            lock: self,
            entered_panicking: std::thread::panicking(),
            _thread_bound: PhantomData,
        };
        if self.is_poisoned() {
            Err(PoisonError::new(guard))
        } else {
            Ok(guard)
        }
    }

    pub(super) fn lock(&self) -> LockResult<WriterGuard<'_>> {
        let mut state = self.state.lock().unwrap();
        if state.held || state.must_handoff() {
            let wake = Arc::new(Waiter::default());
            state.waiters.push_back(wake.clone());
            while state.held
                || !state
                    .waiters
                    .front()
                    .is_some_and(|head| Arc::ptr_eq(head, &wake))
            {
                state = wake.wake.wait(state).unwrap();
            }
            state.waiters.pop_front();
        }
        self.acquire(&mut state)
    }

    #[cfg(test)]
    pub(super) fn wait_queued(&self, depth: usize) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while self.state.lock().unwrap().waiters.len() != depth {
            assert!(
                std::time::Instant::now() < deadline,
                "queue depth did not reach {depth}"
            );
            std::thread::yield_now();
        }
    }

    #[cfg(test)]
    pub(super) fn try_lock(&self) -> TryLockResult<WriterGuard<'_>> {
        let mut state = self
            .state
            .try_lock()
            .map_err(|_| TryLockError::WouldBlock)?;
        if state.held || state.must_handoff() {
            return Err(TryLockError::WouldBlock);
        }
        self.acquire(&mut state).map_err(TryLockError::Poisoned)
    }
}

impl Drop for WriterGuard<'_> {
    fn drop(&mut self) {
        let mut state = self.lock.state.lock().unwrap();
        if !self.entered_panicking && std::thread::panicking() {
            self.lock.poisoned.store(true, Ordering::Release);
        }
        state.held = false;
        if let Some(head) = state.waiters.front() {
            head.wake.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "aged-writer")]
    #[test]
    fn only_the_oldest_waiter_controls_barging_at_the_threshold() {
        use std::time::{Duration, Instant};
        let limit = Duration::from_millis(16);
        assert!(!Queue::aged(limit - Duration::from_nanos(1)));
        assert!(Queue::aged(limit));
        assert!(Queue::aged(limit + Duration::from_nanos(1)));
        let lock = WriterMutex::default();
        let held = lock.lock().unwrap();
        let mut state = lock.state.lock().unwrap();
        state.waiters.push_back(Arc::new(Waiter::default()));
        drop(state);
        drop(held);
        drop(lock.try_lock().expect("young head permits a new arrival"));
        let mut state = lock.state.lock().unwrap();
        Arc::get_mut(state.waiters.front_mut().unwrap())
            .unwrap()
            .queued = Enqueued(Instant::now() - 2 * limit);
        drop(state);
        assert!(matches!(lock.try_lock(), Err(TryLockError::WouldBlock)));
    }

    #[test]
    fn queued_workers_are_admitted_in_registration_order() {
        let lock = WriterMutex::default();
        let guard = lock.lock().unwrap();
        let observed = Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for id in 0..8 {
                let (lock, observed) = (&lock, &observed);
                scope.spawn(move || {
                    let _guard = lock.lock().unwrap();
                    observed.lock().unwrap().push(id);
                });
                lock.wait_queued(id + 1);
            }
            assert!(matches!(lock.try_lock(), Err(TryLockError::WouldBlock)));
            #[cfg(feature = "aged-writer")]
            std::thread::sleep(std::time::Duration::from_millis(17));
            drop(guard);
            let _guard = lock.lock().unwrap();
            assert_eq!(*observed.lock().unwrap(), (0..8).collect::<Vec<_>>());
        });
        assert!(lock.try_lock().is_ok());
    }

    #[test]
    fn panic_poisons_the_lock_and_wakes_every_queued_worker() {
        let lock = WriterMutex::default();
        std::thread::scope(|scope| {
            let panic = std::panic::catch_unwind(|| {
                let _guard = lock.lock().unwrap();
                for depth in 1..=4 {
                    scope.spawn(|| assert!(lock.lock().is_err()));
                    lock.wait_queued(depth);
                }
                panic!("injected while holding writer lock");
            });
            assert!(panic.is_err());
        });
        assert!(lock.is_poisoned());
        assert!(matches!(lock.try_lock(), Err(TryLockError::Poisoned(_))));
    }
}
