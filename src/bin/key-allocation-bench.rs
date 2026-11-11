use silverhamu::transactions::{Database, ResourceLimits};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
struct Alloc;
static CALLS: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Alloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if cfg!(feature = "allocation-metrics") && !p.is_null() {
            CALLS.fetch_add(1, Ordering::Relaxed);
            LIVE.fetch_add(l.size(), Ordering::Relaxed);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if cfg!(feature = "allocation-metrics") && !p.is_null() {
            CALLS.fetch_add(1, Ordering::Relaxed);
            LIVE.fetch_add(l.size(), Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        if cfg!(feature = "allocation-metrics") {
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, n) };
        if cfg!(feature = "allocation-metrics") && !q.is_null() {
            LIVE.fetch_add(n, Ordering::Relaxed);
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        }
        q
    }
}
#[global_allocator]
static ALLOC: Alloc = Alloc;

fn key(i: usize, n: usize) -> Vec<u8> {
    let mut k = vec![0; n];
    k[..8].copy_from_slice(&(i as u64).to_be_bytes());
    k
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let n: usize = a[3].parse().unwrap();
    let len: usize = a[4].parse().unwrap();
    assert!((8..=4096).contains(&len));
    let limits = ResourceLimits {
        max_retained_bytes: 640 * 1024 * 1024,
        ..ResourceLimits::default()
    };
    if a[1] == "seed" {
        let db = Database::create_with_limits(&a[2], [81; 16], limits).unwrap();
        for g in [1u8, 2] {
            for first in (0..n).step_by(1000) {
                let mut tx = db.begin().unwrap();
                for i in first..(first + 1000).min(n) {
                    tx.put(&key(i, len), &[g; 256]).unwrap();
                }
                tx.commit().unwrap();
                while db.collect_garbage().unwrap() {}
            }
        }
        return;
    }
    let before = LIVE.load(Ordering::Relaxed);
    let calls = CALLS.load(Ordering::Relaxed);
    let t = std::time::Instant::now();
    let db = Database::open_with_limits(&a[2], limits).unwrap();
    let ns = t.elapsed().as_nanos();
    let live = LIVE.load(Ordering::Relaxed) - before;
    let allocations = CALLS.load(Ordering::Relaxed) - calls;
    println!(
        "{}",
        serde_json::json!({"rows":n,"key_bytes":len,"open_ns":ns,"live_bytes":live,"allocation_calls":allocations,"rss":std::fs::read_to_string("/proc/self/status").unwrap().lines().find(|s|s.starts_with("VmRSS:")).unwrap(),"metrics":db.history_metrics().unwrap().key_capacity_bytes})
    );
    let mut tx = db.begin_read().unwrap();
    for i in 0..n {
        assert_eq!(tx.get(&key(i, len)).unwrap(), Some(vec![2; 256]));
    }
    println!("verified");
}
