use silverhamu::transactions::{Database, Rows, ScanLimits};
use std::alloc::{GlobalAlloc, Layout, System};
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

struct MeasuredAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

fn account(size: usize) {
    let current = LIVE.fetch_add(size, Ordering::Relaxed) + size;
    PEAK.fetch_max(current, Ordering::Relaxed);
}

// SAFETY: Every allocation is forwarded unchanged to System; counters never own its memory.
unsafe impl GlobalAlloc for MeasuredAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            account(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            account(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, layout, size) };
        if !result.is_null() {
            if size >= layout.size() {
                account(size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - size, Ordering::Relaxed);
            }
        }
        result
    }
}

#[global_allocator]
static ALLOCATOR: MeasuredAllocator = MeasuredAllocator;

struct Sample {
    ms: f64,
    extra_peak: usize,
    visited: usize,
    payload: usize,
}

fn measure(db: &Database, bounded: bool, count: usize) -> Result<Sample, Box<dyn Error>> {
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    let started = Instant::now();
    let mut tx = db.begin()?;
    let (rows, visited, payload): (Rows, usize, usize) = if bounded {
        let page = tx.scan_page(
            b"",
            None,
            None,
            ScanLimits {
                max_rows: 100,
                ..ScanLimits::default()
            },
        )?;
        (page.rows, page.visited_entries, page.payload_bytes)
    } else {
        let rows = tx.scan(b"", None)?;
        let payload = rows
            .iter()
            .take(100)
            .map(|(key, value)| key.len() + value.len())
            .sum();
        (rows.into_iter().take(100).collect(), count, payload)
    };
    let sample = Sample {
        ms: started.elapsed().as_secs_f64() * 1000.0,
        extra_peak: PEAK.load(Ordering::Relaxed).saturating_sub(baseline),
        visited,
        payload,
    };
    if rows.len() != 100 {
        return Err("page count mismatch".into());
    }
    for (id, (key, value)) in rows.iter().enumerate() {
        if key != &(id as u64).to_be_bytes() || value.iter().any(|byte| *byte != (id % 251) as u8) {
            return Err("page content mismatch".into());
        }
    }
    Ok(sample)
}

fn median<T: Ord + Copy>(values: impl Iterator<Item = T>) -> T {
    let mut sorted: Vec<_> = values.collect();
    sorted.sort();
    sorted[sorted.len() / 2]
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 4 {
        return Err("usage: scan-bench <new-output-directory> <records> <value-bytes>".into());
    }
    let count: usize = args[2].parse()?;
    let value_bytes: usize = args[3].parse()?;
    if !(100..=200_000).contains(&count)
        || !(1..=4096).contains(&value_bytes)
        || count
            .checked_mul(value_bytes)
            .is_none_or(|size| size > 128 * 1024 * 1024)
    {
        return Err("records 100..200000, value bytes 1..4096, total values <=128MiB".into());
    }
    let directory = PathBuf::from(&args[1]);
    std::fs::create_dir(&directory)?;
    let db = Database::create(directory.join("source.wal"), [31; 16])?;
    for start in (0..count).step_by(1000) {
        let mut tx = db.begin()?;
        for id in start..(start + 1000).min(count) {
            tx.put(
                &(id as u64).to_be_bytes(),
                &vec![(id % 251) as u8; value_bytes],
            )?;
        }
        tx.commit()?;
    }
    measure(&db, false, count)?;
    measure(&db, true, count)?;
    let mut full = Vec::with_capacity(7);
    let mut pages = Vec::with_capacity(7);
    for round in 0..7 {
        if round % 2 == 0 {
            full.push(measure(&db, false, count)?);
            pages.push(measure(&db, true, count)?);
        } else {
            pages.push(measure(&db, true, count)?);
            full.push(measure(&db, false, count)?);
        }
    }
    let live = LIVE.load(Ordering::Relaxed);
    let output = |name: &str, samples: &[Sample]| {
        let mut times: Vec<_> = samples.iter().map(|sample| sample.ms).collect();
        times.sort_by(f64::total_cmp);
        println!(
            "{{\"path\":\"{name}\",\"source_records\":{count},\"value_bytes\":{value_bytes},\"returned_records\":100,\"rounds\":7,\"median_ms\":{:.6},\"median_extra_live_allocation_peak_bytes\":{},\"visited_entries\":{},\"payload_bytes\":{},\"baseline_live_allocations_bytes\":{live},\"values_verified\":true}}",
            times[times.len() / 2],
            median(samples.iter().map(|sample| sample.extra_peak)),
            samples[0].visited,
            samples[0].payload,
        );
    };
    output("legacy_full_scan_then_take_100", &full);
    output("bounded_scan_100", &pages);
    Ok(())
}
