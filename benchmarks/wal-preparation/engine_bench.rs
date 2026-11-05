use super::*;
use crate::work_elision_metrics as meter;
use std::time::Instant;

#[test]
#[ignore = "explicit bounded WAL preparation experiment"]
fn wal_preparation_benchmark() {
    let dir = PathBuf::from(std::env::var_os("CATDB_WAL_BENCH_DIR").unwrap());
    std::fs::create_dir(&dir).unwrap();
    let shape = std::env::var("CATDB_WAL_BENCH_SHAPE").unwrap();
    assert!(["small", "large", "hot"].contains(&shape.as_str()));
    let value_size = if shape == "large" { 65536 } else { 256 };
    let iterations = 128usize;
    let limits = ResourceLimits {
        max_retained_bytes: 512 * 1024 * 1024,
        max_buffer_bytes: 512 * 1024 * 1024,
        ..ResourceLimits::default()
    };
    let path = dir.join("db.wal");
    let db = Database::create_with_limits(&path, [114; 16], limits).unwrap();
    let mut expected = BTreeMap::new();
    for start in (0..100000u64).step_by(1024) {
        let mut tx = db.begin().unwrap();
        for i in start..(start + 1024).min(100000) {
            let k = i.to_be_bytes().to_vec();
            let v = vec![i as u8; 256];
            tx.put(&k, &v).unwrap();
            expected.insert(k, v);
        }
        tx.commit().unwrap();
    }
    let mut old = db.begin_read().unwrap();
    db.set_group_commit(true);
    db.set_group_commit_wake_on_four(true);
    let barrier = Arc::new(std::sync::Barrier::new(4));
    let start = Instant::now();
    let mut workers = Vec::new();
    for worker in 0..4u64 {
        let db = db.clone();
        let barrier = barrier.clone();
        let hot = shape == "hot";
        workers.push(std::thread::spawn(move || {
            let mut results = Vec::new();
            for iteration in 0..iterations {
                let key = if hot {
                    0
                } else {
                    worker * 1000 + iteration as u64
                };
                let mut value = vec![worker as u8; value_size];
                value[..8].copy_from_slice(&(iteration as u64).to_le_bytes());
                let mut tx = db.begin().unwrap();
                tx.put(&key.to_be_bytes(), &value).unwrap();
                barrier.wait();
                let begin = Instant::now();
                meter::start();
                #[cfg(feature = "wal-prepare")]
                let (prep_ns, prepared_bytes) = {
                    let t = Instant::now();
                    tx.prepare_log().unwrap();
                    (
                        t.elapsed().as_nanos(),
                        tx.prepared.as_ref().unwrap().frame.len(),
                    )
                };
                #[cfg(not(feature = "wal-prepare"))]
                let (prep_ns, prepared_bytes) = (0u128, 0usize);
                let result = tx.commit();
                let elapsed = begin.elapsed().as_nanos();
                let (calls, bytes) = meter::stop();
                let trace = take_commit_trace();
                let lock_ns = trace[4].saturating_sub(trace[3]);
                let seq = match result {
                    Ok(s) => s,
                    Err(Error::Conflict(_)) if hot => 0,
                    other => panic!("unexpected outcome {other:?}"),
                };
                results.push((
                    worker,
                    iteration,
                    key,
                    value,
                    seq,
                    elapsed,
                    prep_ns,
                    prepared_bytes,
                    calls,
                    bytes,
                    lock_ns,
                ));
            }
            results
        }));
    }
    let mut results = Vec::new();
    for worker in workers {
        results.extend(worker.join().unwrap());
    }
    let elapsed = start.elapsed().as_nanos();
    let mut raw = std::io::BufWriter::new(File::create(dir.join("samples.csv")).unwrap());
    writeln!(raw,"worker,iteration,sequence,ns,prepare_ns,prepared_bytes,discarded_bytes,calls,bytes,writer_preparation_ns").unwrap();
    let mut latest = BTreeMap::new();
    for (worker, iteration, key, value, seq, ns, prep_ns, prepared_bytes, calls, bytes, lock_ns) in
        results
    {
        writeln!(raw,"{worker},{iteration},{seq},{ns},{prep_ns},{prepared_bytes},{},{calls},{bytes},{lock_ns}",if seq==0{prepared_bytes}else{0}).unwrap();
        if seq > 0 && latest.get(&key).is_none_or(|(prior, _)| seq > *prior) {
            latest.insert(key, (seq, value));
        }
    }
    assert_eq!(
        old.scan(b"", None).unwrap(),
        expected
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Rows>()
    );
    drop(old);
    for (key, (_, value)) in latest {
        expected.insert(key.to_be_bytes().to_vec(), value);
    }
    assert_eq!(
        db.begin_read().unwrap().scan(b"", None).unwrap(),
        expected
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<Rows>()
    );
    assert_eq!(
        db.resource_usage().unwrap().current[crate::resources::BUFFER],
        0
    );
    let usage = db.resource_usage().unwrap();
    let syncs = db.commit_sync_attempts();
    drop(db);
    let db = Database::open_with_limits(&path, limits).unwrap();
    assert_eq!(
        db.begin_read().unwrap().scan(b"", None).unwrap(),
        expected.into_iter().collect::<Rows>()
    );
    let mut meta = File::create(dir.join("metadata.json")).unwrap();
    write!(meta,"{{\"elapsed_ns\":{elapsed},\"attempted\":512,\"sync_attempts_including_seed\":{syncs},\"buffer_peak\":{},\"counted\":{}}}",usage.peak[crate::resources::BUFFER],meter::counted()).unwrap();
    let mut writes = BTreeMap::new();
    writes.insert(b"key".to_vec(), Some(vec![1; value_size]));
    let mut encoding = File::create(dir.join("encoding.csv")).unwrap();
    writeln!(encoding, "ns,calls,bytes").unwrap();
    for _ in 0..128 {
        meter::start();
        let begin = Instant::now();
        let frame = encode_frame(1, &writes);
        let ns = begin.elapsed().as_nanos();
        let (calls, bytes) = meter::stop();
        assert_eq!(frame, encode_frame_legacy(1, &writes));
        writeln!(encoding, "{ns},{calls},{bytes}").unwrap();
    }
    raw.flush().unwrap();
    println!(
        "verified 100000 rows, {shape}, old snapshot, latest state, restart and released buffers"
    );
}
