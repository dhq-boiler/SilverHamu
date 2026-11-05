use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static TOTAL: Cell<(u64,u64)> = const { Cell::new((0,0)) };
}
struct Meter;
fn record(bytes: usize) {
    if option_env!("CATDB_ELISION_ALLOC").is_some() {
        let _ = ACTIVE.try_with(|active| {
            if active.get() {
                let _ = TOTAL.try_with(|total| {
                    let (n, b) = total.get();
                    total.set((n + 1, b + bytes as u64));
                });
            }
        });
    }
}
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            record(l.size());
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            record(l.size());
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, n) };
        if !q.is_null() {
            record(n);
        }
        q
    }
}
#[global_allocator]
static ALLOC: Meter = Meter;
pub(crate) fn start() {
    TOTAL.with(|v| v.set((0, 0)));
    ACTIVE.with(|v| v.set(true));
}
pub(crate) fn stop() -> (u64, u64) {
    ACTIVE.with(|v| v.set(false));
    TOTAL.with(Cell::get)
}
pub(crate) fn counted() -> bool {
    option_env!("CATDB_ELISION_ALLOC").is_some()
}
