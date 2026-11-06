use newdbms::transactions::{Database, ResourceLimits, ScanLimits, Transaction};
use serde_json::json;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Instant;
struct Alloc;
static LIVE: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Alloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if cfg!(feature = "allocation-metrics") && !p.is_null() {
            LIVE.fetch_add(l.size(), Ordering::Relaxed);
        }
        p
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if cfg!(feature = "allocation-metrics") && !p.is_null() {
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
fn key(i: usize) -> [u8; 8] {
    (i as u64).to_be_bytes()
}
fn value(i: usize, g: u8) -> [u8; 256] {
    let mut v = [g; 256];
    v[..8].copy_from_slice(&key(i));
    v
}
fn limits() -> ResourceLimits {
    ResourceLimits {
        max_retained_bytes: 640 * 1024 * 1024,
        ..ResourceLimits::default()
    }
}
fn rss() -> Option<usize> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find(|l| l.starts_with("VmRSS:"))?
        .split_whitespace()
        .nth(1)?
        .parse::<usize>()
        .ok()
        .map(|v| v * 1024)
}
fn capture(db: &Database, phase: &str, observe: bool) {
    let t = Instant::now();
    let m = if observe {
        Some(db.history_metrics().unwrap())
    } else {
        None
    };
    let ns = t.elapsed().as_nanos() as u64;
    let live = cfg!(feature = "allocation-metrics").then(|| LIVE.load(Ordering::Relaxed));
    println!(
        "{}",
        json!({"event":"snapshot","phase":phase,"live_requested_bytes":live,"rss_bytes":rss(),"metric_scan_ns":ns,"logical_retained_bytes":db.resource_usage().unwrap().current[3],"metrics":m.map(|m|json!({"records":m.records,"versions":m.versions,"capacity_slots":m.capacity_slots,"version_size":m.version_size,"version_capacity_bytes":m.capacity_slots*m.version_size,"unused_version_bytes":(m.capacity_slots-m.versions)*m.version_size,"key_capacity_bytes":m.key_capacity_bytes,"value_bytes":m.value_bytes,"history_keys":m.history_keys}))})
    );
}
fn drain(db: &Database) {
    let t = Instant::now();
    let mut steps = 0;
    while db.collect_garbage().unwrap() {
        steps += 1;
        assert!(t.elapsed().as_secs() < 60, "GC deadline");
    }
    println!(
        "{}",
        json!({"event":"drain","steps":steps,"ns":t.elapsed().as_nanos() as u64})
    );
}
fn update(db: &Database, start: usize, end: usize, g: u8) {
    let t = Instant::now();
    for first in (start..end).step_by(1000) {
        let mut tx = db.begin().unwrap();
        for i in first..(first + 1000).min(end) {
            tx.put(&key(i), &value(i, g)).unwrap();
        }
        tx.commit().unwrap();
        while db.collect_garbage().unwrap() {}
    }
    println!(
        "{}",
        json!({"event":"aging","first":start,"end":end,"generation":g,"elapsed_ns":t.elapsed().as_nanos() as u64,"batch_rows":1000})
    );
}
fn verify_tx(tx: &mut Transaction, rows: usize, changed: usize, g: u8, label: &str) {
    let mut after = None;
    let mut count = 0;
    loop {
        let page = tx
            .scan_page(
                &[],
                None,
                after.as_deref(),
                ScanLimits {
                    max_rows: 1024,
                    max_bytes: 1024 * 1024,
                    max_visits: 4096,
                },
            )
            .unwrap();
        for (k, v) in &page.rows {
            assert_eq!(k.as_slice(), key(count));
            assert_eq!(
                v.as_slice(),
                value(count, if count < changed { g } else { 0 })
            );
            count += 1;
        }
        after = page.next_after.clone();
        if after.is_none() {
            break;
        }
    }
    assert_eq!(count, rows);
    assert!(tx.get(&key(rows)).unwrap().is_none());
    println!(
        "{}",
        json!({"event":"verified","label":label,"rows":count,"changed":changed,"generation":g})
    );
}
fn verify(db: &Database, rows: usize, changed: usize, g: u8, label: &str) {
    verify_tx(&mut db.begin_read().unwrap(), rows, changed, g, label);
}
fn summary(mut ns: Vec<u64>) -> serde_json::Value {
    ns.sort_unstable();
    json!({"count":ns.len(),"p99_ns":ns[(ns.len()*99).div_ceil(100)-1],"max_ns":ns.last().unwrap()})
}
fn latency(db: &Database, rows: usize, changed: usize, g: u8, phase: &str) {
    let mut reads = Vec::with_capacity(1000);
    let mut rng = 739u64;
    for _ in 0..1000 {
        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
        let i = (rng as usize) % rows;
        let t = Instant::now();
        let got = db.begin_read().unwrap().get(&key(i)).unwrap();
        reads.push(t.elapsed().as_nanos() as u64);
        assert_eq!(
            got,
            Some(value(i, if i < changed { g } else { 0 }).to_vec())
        );
    }
    let gate = Arc::new(Barrier::new(5));
    let mut threads = Vec::new();
    for i in 0..4 {
        let db = db.clone();
        let gate = gate.clone();
        threads.push(std::thread::spawn(move || {
            let mut ns = Vec::with_capacity(100);
            gate.wait();
            for _ in 0..100 {
                let t = Instant::now();
                let mut tx = db.begin().unwrap();
                tx.put(&key(i), &value(i, g)).unwrap();
                tx.commit().unwrap();
                ns.push(t.elapsed().as_nanos() as u64);
            }
            ns
        }));
    }
    let t = Instant::now();
    gate.wait();
    let writes: Vec<_> = threads
        .into_iter()
        .flat_map(|t| t.join().unwrap())
        .collect();
    println!(
        "{}",
        json!({"event":"latency","phase":phase,"read":summary(reads),"write":summary(writes),"write_elapsed_ns":t.elapsed().as_nanos() as u64,"closed_loop_writers":4,"embedded_not_service":true})
    );
    drain(db);
}
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn trim() -> i32 {
    unsafe extern "C" {
        fn malloc_trim(pad: usize) -> i32;
    }
    unsafe { malloc_trim(0) }
}
#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn trim() -> i32 {
    -1
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert_eq!(args.len(), 6, "MODE PATH ROWS PATTERN OBSERVE");
    let mode = &args[1];
    let path = &args[2];
    let rows = args[3].parse::<usize>().unwrap();
    assert!((100..=1_000_000).contains(&rows) && rows % 100 == 0);
    let pattern = &args[4];
    assert!(pattern == "spread" || pattern == "hot");
    let observe = args[5] == "1";
    assert!(args[5] == "0" || observe);
    println!(
        "{}",
        json!({"event":"config","mode":mode,"rows":rows,"pattern":pattern,"observe":observe,"allocation_metrics":cfg!(feature="allocation-metrics")})
    );
    if mode == "init" {
        let db = Database::create_with_limits(path, [91; 16], limits()).unwrap();
        update(&db, 0, rows, 0);
        db.checkpoint().unwrap();
        verify(&db, rows, 0, 0, "seed");
        return;
    }
    let open = Instant::now();
    let db = Database::open_with_limits(path, limits()).unwrap();
    println!(
        "{}",
        json!({"event":"opened","ns":open.elapsed().as_nanos() as u64})
    );
    capture(&db, "opened", observe);
    let (changed, g) = if pattern == "spread" {
        (rows, 1)
    } else {
        (rows / 100, 16)
    };
    if mode == "verify" || mode == "checkpoint" {
        verify(&db, rows, changed, g, "reopen");
        if mode == "checkpoint" {
            db.checkpoint().unwrap();
        }
        capture(&db, "reopened-verified", observe);
        return;
    }
    assert_eq!(mode, "run");
    verify(&db, rows, 0, 0, "initial");
    drain(&db);
    capture(&db, "initial", observe);
    if pattern == "spread" {
        let mut start = 0;
        for pct in [25, 50, 100] {
            let end = rows * pct / 100;
            update(&db, start, end, 1);
            drain(&db);
            let phase = format!("spread-{pct}");
            capture(&db, &phase, observe);
            verify(&db, rows, end, 1, &phase);
            latency(&db, rows, end, 1, &phase);
            capture(&db, &format!("{phase}-after-latency"), observe);
            start = end;
        }
    } else {
        let mut old = db.begin_read().unwrap();
        for g in 1..=16 {
            update(&db, 0, changed, g);
        }
        capture(&db, "hot-held", observe);
        verify_tx(&mut old, rows, 0, 0, "old-generation");
        verify(&db, rows, changed, g, "latest-held");
        let t = Instant::now();
        drop(old);
        println!(
            "{}",
            json!({"event":"release","ns":t.elapsed().as_nanos() as u64})
        );
        capture(&db, "hot-released", observe);
        drain(&db);
        capture(&db, "hot-drained", observe);
        verify(&db, rows, changed, g, "latest-drained");
        latency(&db, rows, changed, g, "hot-drained");
        capture(&db, "hot-after-latency", observe);
    }
    capture(&db, "before-trim", observe);
    let t = Instant::now();
    let result = trim();
    println!(
        "{}",
        json!({"event":"diagnostic-trim","result":result,"ns":t.elapsed().as_nanos() as u64})
    );
    capture(&db, "after-trim", observe);
    verify(&db, rows, changed, g, "final");
    drop(db);
    println!(
        "{}",
        json!({"event":"complete","pattern":pattern,"rows":rows})
    );
}
