use std::cell::Cell;
use std::time::Instant;

thread_local! {
    static ORIGIN: Cell<Option<Instant>> = const { Cell::new(None) };
    static EVENTS: Cell<[u64;5]> = const { Cell::new([0;5]) };
}

pub(super) fn enable(origin: Instant) {
    ORIGIN.set(Some(origin));
}
pub(super) fn mark(index: usize) {
    if let Some(origin) = ORIGIN.get() {
        let mut e = EVENTS.get();
        e[index] = origin.elapsed().as_nanos() as u64;
        EVENTS.set(e);
    }
}
pub(super) fn size(bytes: usize) {
    if ORIGIN.get().is_some() {
        let mut e = EVENTS.get();
        e[4] = bytes as u64;
        EVENTS.set(e);
    }
}
pub(super) fn take() -> [u64; 5] {
    EVENTS.replace([0; 5])
}

thread_local! {
    static HANDOFF_CLOCK: Cell<Option<Instant>> = const { Cell::new(None) };
    static HOLDS: std::cell::RefCell<Vec<[u64;4]>> = const { std::cell::RefCell::new(Vec::new()) };
}

pub(super) fn enable_handoffs(clock: Instant, capacity: usize) {
    HOLDS.with_borrow_mut(|holds| holds.reserve(capacity));
    HANDOFF_CLOCK.set(Some(clock));
}

pub(super) fn stamp() -> u64 {
    HANDOFF_CLOCK.get().map_or(0, |clock| clock.elapsed().as_nanos() as u64)
}

pub(super) struct Observed<G> {
    guard: Option<G>,
    start: u64,
    acquired: u64,
}

pub(super) fn observe<G>(start:u64, guard:G) -> Observed<G> {
    Observed { guard:Some(guard),start,acquired:stamp() }
}

impl<G> Drop for Observed<G> {
    fn drop(&mut self) {
        if self.start != 0 {
            let before=stamp();
            drop(self.guard.take());
            let after=stamp();
            HOLDS.with_borrow_mut(|v| v.push([self.start,self.acquired,before,after]));
        }
    }
}

pub(super) fn take_holds()->Vec<[u64;4]> { HOLDS.take() }
