#[cfg(feature = "service")]
mod probe {
    use newdbms::transactions::{Database, ScanLimits};
    #[cfg(feature = "lock-timing")]
    use newdbms::transactions::{take_state_hold_timing, take_thread_lock_wait_ns};
    #[cfg(not(feature = "lock-timing"))]
    fn take_state_hold_timing() -> [u64; 3] {
        [0; 3]
    }
    #[cfg(not(feature = "lock-timing"))]
    fn take_thread_lock_wait_ns() -> u64 {
        0
    }
    struct ThreadResult {
        ns: Vec<u64>,
        hold: [u64; 3],
        wait: u64,
        elapsed: f64,
    }
    fn encode(t: ThreadResult) -> Value {
        json!({"elapsed_s":t.elapsed,"latency":summary(t.ns),"hold":t.hold,"wait_ns":t.wait})
    }
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
        gate: Arc<[Barrier; 2]>,
        stop: Arc<AtomicBool>,
    ) -> ThreadResult {
        take_state_hold_timing();
        take_thread_lock_wait_ns();
        let mut ns = Vec::with_capacity(2_000_000);
        gate[0].wait();
        gate[1].wait();
        let start = Instant::now();
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
        ThreadResult {
            elapsed: start.elapsed().as_secs_f64(),
            ns,
            hold: take_state_hold_timing(),
            wait: take_thread_lock_wait_ns(),
        }
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
        let background = std::env::var("CATDB_BACKGROUND_RECLAMATION").as_deref() == Ok("1");

        for start in (0..rows).step_by(256) {
            let mut t = db.begin().unwrap();
            for i in start..(start + 256).min(rows) {
                t.put(&key(i), &value(i, bytes, 1)).unwrap();
            }
            t.commit().unwrap();
        }
        while db.collect_garbage().unwrap() {}
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

        let stop = Arc::new(AtomicBool::new(false));
        let gate = Arc::new([
            Barrier::new(if gc { 3 } else { 7 }),
            Barrier::new(if gc { 3 } else { 7 }),
        ]);
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
                writers.push(std::thread::spawn(move || {
                    take_state_hold_timing();
                    take_thread_lock_wait_ns();
                    let mut ns = Vec::with_capacity(updates);
                    g[0].wait();
                    g[1].wait();
                    let start = Instant::now();
                    for j in 0..updates {
                        let t = Instant::now();
                        let i = j * 4 + worker;
                        let mut w = d.begin().unwrap();
                        w.put(&key(i), &value(i, bytes, 2)).unwrap();
                        w.commit().unwrap();
                        ns.push(t.elapsed().as_nanos() as u64);
                    }
                    ThreadResult {
                        ns,
                        hold: take_state_hold_timing(),
                        wait: take_thread_lock_wait_ns(),
                        elapsed: start.elapsed().as_secs_f64(),
                    }
                }));
            }
        }
        take_state_hold_timing();
        take_thread_lock_wait_ns();
        gate[0].wait();
        db.set_background_reclamation(background).unwrap();
        let heap_start = LIVE.load(Ordering::Relaxed);
        PEAK.store(heap_start, Ordering::Relaxed);
        gate[1].wait();
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
            let deadline = Instant::now() + std::time::Duration::from_secs(10);
            while if background {
                db.reclamation_pending().unwrap()
            } else {
                db.collect_garbage().unwrap()
            } {
                assert!(Instant::now() < deadline, "drain timed out");
                steps += 1;
                if background {
                    std::thread::sleep(std::time::Duration::from_micros(100));
                } else {
                    std::thread::yield_now();
                }
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
        db.set_background_reclamation(false).unwrap();
        while db.collect_garbage().unwrap() {}
        let heap_drained = LIVE.load(Ordering::Relaxed);
        let budget_drained = db.resource_usage().unwrap();
        let worker = db.reclamation_status();
        let profile = db.reclamation_profile();
        let reader_results: Vec<_> = reader_results.into_iter().map(encode).collect();
        let writer_results: Vec<_> = writer_results.into_iter().map(encode).collect();
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
            json!({"scenario":scenario,"rows":rows,"value_bytes":bytes,"writers":writer_results,"readers":reader_results,"elapsed_s":elapsed,"drop_ns":drop_ns,"drain_ns":drain_ns,"drain_steps":steps,"main_hold":holds,"main_wait_ns":waits,"heap_start":heap_start,"heap_peak":heap_peak,"heap_end":heap_end,"budget_current":budget.current,"budget_peak":budget.peak,"verified":true,"background":background,"lock_timing":cfg!(feature="lock-timing"),"heap_drained":heap_drained,"budget_drained":budget_drained.current,"sample_capacity_bytes":32_000_000+if gc {0}else{updates*4*8},"worker_batches":worker.batches,"worker_reclaimed_bytes":worker.reclaimed_bytes,"worker_max_lock_hold_ns":worker.max_lock_hold_ns,"worker_profile":{"lock_misses":profile.lock_misses,"io_deferrals":profile.io_deferrals,"work_ns":profile.work_ns,"free_ns":profile.free_ns,"yield_ns":profile.yield_ns,"pause_requested_ns":profile.pause_requested_ns,"pause_elapsed_ns":profile.pause_elapsed_ns,"worker_elapsed_ns":profile.worker_elapsed_ns}})
        );
    }
}
#[cfg(feature = "service")]
fn main() {
    probe::run()
}
#[cfg(not(feature = "service"))]
fn main() {
    eprintln!("requires --features service,lock-timing")
}
