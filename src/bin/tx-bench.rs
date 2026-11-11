use silverhamu::Snapshot;
use silverhamu::transactions::Database;
use std::error::Error;
use std::sync::{Arc, Barrier};
use std::time::Instant;

fn payload(worker: u32, item: u32, role: u8) -> Vec<u8> {
    let mut value = vec![role; 256];
    value[..4].copy_from_slice(&worker.to_le_bytes());
    value[4..8].copy_from_slice(&item.to_le_bytes());
    value
}
fn key(worker: u32, item: u32, role: u8) -> Vec<u8> {
    let mut value = Vec::new();
    value.extend_from_slice(&worker.to_be_bytes());
    value.extend_from_slice(&item.to_be_bytes());
    value.push(role);
    value
}
fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    if !(2..=5).contains(&arguments.len()) {
        return Err(
            "usage: tx-bench <new-output-directory> [workers=8] [commits-per-worker=50] [single|group]".into(),
        );
    }
    let workers: u32 = arguments.get(2).map_or(Ok(8), |v| v.parse())?;
    let commits: u32 = arguments.get(3).map_or(Ok(50), |v| v.parse())?;
    let mode = arguments.get(4).map_or("single", String::as_str);
    if !matches!(mode, "single" | "group") {
        return Err("mode must be single or group".into());
    }
    if !(1..=64).contains(&workers)
        || commits == 0
        || u64::from(workers) * u64::from(commits) > 100_000
    {
        return Err("workers must be 1..64 and total commits 1..100000".into());
    }
    let directory = std::path::PathBuf::from(&arguments[1]);
    std::fs::create_dir(&directory)?;
    let path = directory.join("transactions.wal");
    let db = Database::create(&path, [12; 16])?;
    db.set_group_commit(mode == "group");
    let mut old = db.begin()?;
    let staged = Arc::new(Barrier::new(workers as usize + 1));
    let publish = Arc::new(Barrier::new(workers as usize + 1));
    let handles: Vec<_> = (0..workers)
        .map(|worker| {
            let db = db.clone();
            let staged = staged.clone();
            let publish = publish.clone();
            std::thread::spawn(move || -> silverhamu::transactions::Result<Vec<f64>> {
                let mut timings = Vec::new();
                for item in 0..commits {
                    let mut tx = db.begin()?;
                    for role in 0..2 {
                        tx.put(&key(worker, item, role), &payload(worker, item, role))?;
                    }
                    if item == 0 {
                        staged.wait();
                        publish.wait();
                    }
                    let start = Instant::now();
                    tx.commit()?;
                    timings.push(start.elapsed().as_secs_f64() * 1000.0);
                }
                Ok(timings)
            })
        })
        .collect();
    staged.wait();
    let simultaneously_active = db.stats()?.active_transactions;
    let started = Instant::now();
    publish.wait();
    let mut latencies = Vec::new();
    for handle in handles {
        latencies.extend(handle.join().map_err(|_| "worker panicked")??);
    }
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    let syncs = db.commit_sync_attempts();
    if !old.scan(b"", None)?.is_empty() {
        return Err("snapshot changed during concurrent writes".into());
    }
    drop(old);
    let stats = db.stats()?;
    let export = Instant::now();
    let snapshot_path = directory.join("values.ndb");
    let info = db.export_values(&snapshot_path, [6; 16], b"", None)?;
    let export_ms = export.elapsed().as_secs_f64() * 1000.0;
    if info.version != u64::from(workers) * u64::from(commits) {
        return Err("commit count mismatch".into());
    }
    drop(db);
    let recovery = Instant::now();
    let db = Database::open(&path)?;
    let recovery_ms = recovery.elapsed().as_secs_f64() * 1000.0;
    let mut tx = db.begin()?;
    for worker in 0..workers {
        for item in 0..commits {
            for role in 0..2 {
                if tx.get(&key(worker, item, role))? != Some(payload(worker, item, role)) {
                    return Err("recovered value mismatch".into());
                }
            }
        }
    }
    let rows = tx.scan(b"", None)?;
    let mut snapshot = Snapshot::open(&snapshot_path, [6; 16])?;
    if snapshot.len() != rows.len() as u64 {
        return Err("snapshot count mismatch".into());
    }
    for start in (0..rows.len()).step_by(1000) {
        for (offset, record) in snapshot.read_page(start as u64, 1000)?.iter().enumerate() {
            if record != &rows[start + offset].1 {
                return Err("snapshot record mismatch".into());
            }
        }
    }
    latencies.sort_by(f64::total_cmp);
    let total = workers * commits;
    println!(
        "{{\"mode\":\"{mode}\",\"workers\":{workers},\"commits\":{total},\"syncs\":{syncs},\"records\":{},\"simultaneously_active_including_reader\":{simultaneously_active},\"elapsed_ms\":{elapsed:.3},\"commits_per_second\":{:.3},\"commit_p50_ms\":{:.3},\"commit_p95_ms\":{:.3},\"commit_p99_ms\":{:.3},\"recovery_ms\":{recovery_ms:.3},\"export_ms\":{export_ms:.3},\"wal_bytes\":{},\"all_recovered_values_verified\":true,\"all_exported_values_verified\":true}}",
        rows.len(),
        f64::from(total) * 1000.0 / elapsed,
        latencies[latencies.len() / 2],
        latencies[(latencies.len() - 1) * 95 / 100],
        latencies[(latencies.len() - 1) * 99 / 100],
        stats.wal_bytes
    );
    Ok(())
}
