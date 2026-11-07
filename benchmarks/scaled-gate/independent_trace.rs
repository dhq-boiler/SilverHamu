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
