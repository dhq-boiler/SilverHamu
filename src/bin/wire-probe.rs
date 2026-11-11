use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
struct Alloc;
static ENABLED: AtomicBool = AtomicBool::new(false);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Alloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if ENABLED.load(Ordering::Relaxed) && !p.is_null() {
            CALLS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(l.size(), Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, n) };
        if ENABLED.load(Ordering::Relaxed) && !q.is_null() {
            CALLS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(n, Ordering::Relaxed);
        }
        q
    }
}
#[global_allocator]
static ALLOC: Alloc = Alloc;
fn main() {
    let n = 10000;
    let (reads, writes) = silverhamu::service::wire_probe(
        n,
        || ENABLED.store(true, Ordering::Relaxed),
        || ENABLED.store(false, Ordering::Relaxed),
    )
    .unwrap();
    println!(
        "{}",
        serde_json::json!({"requests":n,"read_calls":reads,"write_calls":writes,"allocation_calls":CALLS.load(Ordering::Relaxed),"requested_bytes":BYTES.load(Ordering::Relaxed),"wire_buffer":cfg!(feature="wire-buffer")})
    );
}
