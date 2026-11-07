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
    waiters: VecDeque<Arc<Condvar>>,
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
        if state.held || !state.waiters.is_empty() {
            let wake = Arc::new(Condvar::new());
            state.waiters.push_back(wake.clone());
            while state.held || !state.waiters.front().is_some_and(|head| Arc::ptr_eq(head, &wake)) {
                state = wake.wait(state).unwrap();
            }
            state.waiters.pop_front();
        }
        self.acquire(&mut state)
    }

    #[cfg(test)]
    pub(super) fn try_lock(&self) -> TryLockResult<WriterGuard<'_>> {
        let mut state = self.state.try_lock().map_err(|_| TryLockError::WouldBlock)?;
        if state.held || !state.waiters.is_empty() {return Err(TryLockError::WouldBlock);}
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
            head.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn wait_for_depth(lock: &WriterMutex, depth: usize) {
        let deadline=Instant::now()+Duration::from_secs(10);
        while lock.state.lock().unwrap().waiters.len()!=depth {
            assert!(Instant::now()<deadline,"queue depth did not reach {depth}");
            std::thread::yield_now();
        }
    }

    #[test]
    fn queued_workers_are_admitted_in_registration_order() {
        let lock=WriterMutex::default();
        let guard=lock.lock().unwrap();
        let observed=Mutex::new(Vec::new());
        std::thread::scope(|scope| {
            for id in 0..8 {
                let (lock,observed)=(&lock,&observed);
                scope.spawn(move || {let _guard=lock.lock().unwrap(); observed.lock().unwrap().push(id);});
                wait_for_depth(lock,id+1);
            }
            assert!(matches!(lock.try_lock(),Err(TryLockError::WouldBlock)));
            drop(guard);
            let _guard=lock.lock().unwrap();
            assert_eq!(*observed.lock().unwrap(),(0..8).collect::<Vec<_>>());
        });
        assert!(lock.try_lock().is_ok());
    }

    #[test]
    fn panic_poisons_the_lock_and_wakes_every_queued_worker() {
        let lock=WriterMutex::default();
        std::thread::scope(|scope| {
            let panic=std::panic::catch_unwind(|| {
                let _guard=lock.lock().unwrap();
                for depth in 1..=4 {
                    scope.spawn(|| assert!(lock.lock().is_err()));
                    wait_for_depth(&lock,depth);
                }
                panic!("injected while holding writer lock");
            });
            assert!(panic.is_err());
        });
        assert!(lock.is_poisoned());
        assert!(matches!(lock.try_lock(),Err(TryLockError::Poisoned(_))));
    }
}
