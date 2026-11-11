#[cfg(feature = "lock-timing")]
mod probe {
    use silverhamu::transactions::{
        Database, ScanLimits, take_state_hold_timing, take_thread_lock_wait_ns,
    };
    use serde_json::{Value, json};
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::AtomicUsize;
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicBool, Ordering},
    };
    use std::time::Instant;
    struct Alloc;
    static LIVE: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);
    fn account(n: usize) {
        let live = LIVE.fetch_add(n, Ordering::Relaxed) + n;
        PEAK.fetch_max(live, Ordering::Relaxed);
    }
    unsafe impl GlobalAlloc for Alloc {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            let p = unsafe { System.alloc(l) };
            if !p.is_null() {
                account(l.size())
            }
            p
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            let p = unsafe { System.alloc_zeroed(l) };
            if !p.is_null() {
                account(l.size())
            }
            p
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            unsafe { System.dealloc(p, l) };
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            let q = unsafe { System.realloc(p, l, n) };
            if !q.is_null() {
                LIVE.fetch_sub(l.size(), Ordering::Relaxed);
                account(n)
            }
            q
        }
    }
    #[global_allocator]
    static ALLOC: Alloc = Alloc;
    fn key(i: usize) -> [u8; 8] {
        (i as u64).to_be_bytes()
    }
    fn value(i: usize, n: usize, version: u8) -> Vec<u8> {
        let mut v = vec![version; n];
        v[..8].copy_from_slice(&key(i));
        v
    }
    fn summary(mut ns: Vec<u64>) -> Value {
        ns.sort_unstable();
        let n = ns.len();
        json!({"count":n,"p99_ms":if n==0{0.0}else{ns[(n*99).div_ceil(100)-1] as f64/1e6},"max_ms":ns.last().copied().unwrap_or(0) as f64/1e6,"samples_ns":ns})
    }
    fn read_loop(
        db: Database,
        rows: usize,
        large: bool,
        gate: Arc<Barrier>,
        stop: Arc<AtomicBool>,
    ) -> Value {
        take_state_hold_timing();
        take_thread_lock_wait_ns();
        gate.wait();
        let start = Instant::now();
        let mut ns = Vec::new();
        let mut i = 0;
        while !stop.load(Ordering::Acquire) || i < 1000 {
            let t = Instant::now();
            let mut r = db.begin_read().unwrap();
            if large {
                let p = r
                    .scan_page(
                        &key((i * 17) % (rows - 4)),
                        None,
                        None,
                        ScanLimits {
                            max_rows: 4,
                            max_bytes: 512 * 1024,
                            ..ScanLimits::default()
                        },
                    )
                    .unwrap();
                assert_eq!(p.rows.len(), 4);
            } else {
                assert!(r.get(&key((i * 17) % rows)).unwrap().is_some());
            }
            drop(r);
            ns.push(t.elapsed().as_nanos() as u64);
            i += 1;
            assert!(i < 2_000_000, "read sample limit reached");
        }
        json!({"elapsed_s":start.elapsed().as_secs_f64(),"latency":summary(ns),"hold":take_state_hold_timing(),"wait_ns":take_thread_lock_wait_ns()})
    }
    pub fn run() {
        let args: Vec<_> = std::env::args().collect();
        let path = std::path::Path::new(&args[1]);
        std::fs::create_dir(path).unwrap();
        let scenario = &args[2];
        let large = scenario == "large";
        let gc = scenario == "gc";
        assert!(large || gc || scenario == "mixed");
        let rows = if large { 4096 } else { 100000 };
        let bytes = if large { 65536 } else { 256 };
        let updates = if large { 100 } else { 300 };
        let file = path.join("db.wal");
        let db = Database::create(&file, [32; 16]).unwrap();
        db.set_group_commit(true);
        for start in (0..rows).step_by(256) {
            let mut t = db.begin().unwrap();
            for i in start..(start + 256).min(rows) {
                t.put(&key(i), &value(i, bytes, 1)).unwrap();
            }
            t.commit().unwrap();
        }
        let mut old = db.begin_read().unwrap();
        if gc {
            for start in (0..rows).step_by(256) {
                let mut t = db.begin().unwrap();
                for i in start..(start + 256).min(rows) {
                    t.put(&key(i), &value(i, bytes, 2)).unwrap();
                }
                t.commit().unwrap();
            }
        }
        assert_eq!(old.get(&key(0)).unwrap().unwrap(), value(0, bytes, 1));
        let heap_start = LIVE.load(Ordering::Relaxed);
        PEAK.store(heap_start, Ordering::Relaxed);
        let stop = Arc::new(AtomicBool::new(false));
        let gate = Arc::new(Barrier::new(if gc { 3 } else { 7 }));
        let mut readers = Vec::new();
        for _ in 0..2 {
            let d = db.clone();
            let g = gate.clone();
            let done = stop.clone();
            readers.push(std::thread::spawn(move || {
                read_loop(d, rows, large, g, done)
            }));
        }
        let mut writers = Vec::new();
        if !gc {
            for worker in 0..4 {
                let d = db.clone();
                let g = gate.clone();
                writers.push(std::thread::spawn(move||{
  take_state_hold_timing();take_thread_lock_wait_ns();g.wait();let mut ns=Vec::new();
  for j in 0..updates{let t=Instant::now();let i=j*4+worker;let mut w=d.begin().unwrap();w.put(&key(i),&value(i,bytes,2)).unwrap();w.commit().unwrap();ns.push(t.elapsed().as_nanos() as u64);}
  json!({"latency":summary(ns),"hold":take_state_hold_timing(),"wait_ns":take_thread_lock_wait_ns()})
 }));
            }
        }
        take_state_hold_timing();
        take_thread_lock_wait_ns();
        gate.wait();
        let start = Instant::now();
        let mut drop_ns = 0;
        let mut drain_ns = 0;
        let mut steps = 0;
        let writer_results: Vec<_> = writers.into_iter().map(|w| w.join().unwrap()).collect();
        if gc {
            let t = Instant::now();
            drop(old);
            drop_ns = t.elapsed().as_nanos() as u64;
            let t = Instant::now();
            while db.collect_garbage().unwrap() {
                steps += 1;
                std::thread::yield_now();
            }
            drain_ns = t.elapsed().as_nanos() as u64;
        } else {
            assert_eq!(old.get(&key(0)).unwrap().unwrap(), value(0, bytes, 1));
            drop(old);
        }
        let elapsed = start.elapsed().as_secs_f64();
        stop.store(true, Ordering::Release);
        let reader_results: Vec<_> = readers.into_iter().map(|r| r.join().unwrap()).collect();
        let holds = take_state_hold_timing();
        let waits = take_thread_lock_wait_ns();
        let heap_peak = PEAK.load(Ordering::Relaxed);
        let heap_end = LIVE.load(Ordering::Relaxed);
        let budget = db.resource_usage().unwrap();
        while db.collect_garbage().unwrap() {}
        {
            let mut r = db.begin_read().unwrap();
            for i in 0..rows {
                assert_eq!(
                    r.get(&key(i)).unwrap().unwrap(),
                    value(i, bytes, if gc || i < updates * 4 { 2 } else { 1 })
                );
            }
        }
        drop(db);
        let reopened = Database::open(&file).unwrap();
        let mut r = reopened.begin_read().unwrap();
        for i in 0..rows {
            assert_eq!(
                r.get(&key(i)).unwrap().unwrap(),
                value(i, bytes, if gc || i < updates * 4 { 2 } else { 1 })
            );
        }
        println!(
            "{}",
            json!({"scenario":scenario,"rows":rows,"value_bytes":bytes,"writers":writer_results,"readers":reader_results,"elapsed_s":elapsed,"drop_ns":drop_ns,"drain_ns":drain_ns,"drain_steps":steps,"main_hold":holds,"main_wait_ns":waits,"heap_start":heap_start,"heap_peak":heap_peak,"heap_end":heap_end,"budget_current":budget.current,"budget_peak":budget.peak,"verified":true})
        );
    }
}
#[cfg(feature = "lock-timing")]
fn main() {
    probe::run()
}
#[cfg(not(feature = "lock-timing"))]
fn main() {
    eprintln!("requires --features service,lock-timing")
}
