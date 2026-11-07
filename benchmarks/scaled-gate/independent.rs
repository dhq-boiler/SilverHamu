#[path = "sync_clock.rs"]
mod sync_clock;
use crate::transactions::independent_trace;
#[test]
#[ignore = "independent production checkpoint campaign"]
fn independent_campaign() {
    let n: usize = std::env::var("SCALED_ROWS").unwrap().parse().unwrap();
    let ops: usize = std::env::var("SCALED_OPS").unwrap().parse().unwrap();
    let cap: usize = std::env::var("SCALED_CAP").unwrap().parse().unwrap();
    assert_eq!(
        fs::read_to_string("/sys/fs/cgroup/memory.max")
            .unwrap()
            .trim(),
        "4294967296"
    );
    let q = fs::read_to_string("/sys/fs/cgroup/cpu.max").unwrap();
    let q: Vec<u64> = q.split_whitespace().map(|p| p.parse().unwrap()).collect();
    assert_eq!(q[0], 2 * q[1]);
    let enabled = std::env::var("SCALED_VARIANT").unwrap() == "prefix-on";
    let path = dir().join("independent.db");
    let db = Database::create_with_limits(
        &path,
        [87; 16],
        ResourceLimits {
            max_transactions: 64,
            max_staged_bytes: 32 * PAGE,
            max_dependency_bytes: 32 * PAGE,
            max_retained_bytes: 2 * 1024 * PAGE,
            max_buffer_bytes: 64 * PAGE,
        },
    )
    .unwrap();
    for start in (0..n).step_by(1000) {
        let mut t = db.begin().unwrap();
        for i in start..(start + 1000).min(n) {
            t.put(&key(i), &value(i, 0)).unwrap();
        }
        t.commit().unwrap();
    }
    db.checkpoint().unwrap();
    let mut old = db.begin_read().unwrap();
    let base = old.version();
    let clock = Instant::now();
    let os_tag = std::env::var("SYNC_OS_TAG").ok();
    if os_tag.is_some() {
        sync_clock::clock_anchor(clock);
    }
    let now = || clock.elapsed().as_nanos() as u64;
    let (send, recv) = std::sync::mpsc::channel::<usize>();
    let barrier = Barrier::new(5);
    let exact = std::env::var_os("EXACT_PUBLICATIONS").is_some();
    let holds = Mutex::new(Vec::new());
    let worker_holds = &holds;
    let (attempts, publications) = std::thread::scope(|scope| {
        let worker_db = &db;
        let worker = scope.spawn(move || {
            let db = worker_db;
            if let Ok(tag) = std::env::var("JOURNAL_WORKER_TAG") {
                sync_clock::identify(&tag, 4);
            }
            independent_trace::enable(clock);
            let mut result = Vec::new();
            if exact {
                independent_trace::enable_handoffs(clock, 2 * ops / cap);
            }
            let mut pending = std::collections::BTreeSet::new();
            let mut last = 0;
            while last < ops / cap - 1 {
                let start = now();
                let mut prefix = enabled.then(|| db.prepare_checkpoint_prefix(2048).unwrap());
                let ready = now();
                let due = if exact {
                    next_publication(&recv, &mut pending, last, ops / cap - 1)
                } else {
                    let mut due = recv.recv().unwrap();
                    while due <= last {
                        due = recv.recv().unwrap();
                    }
                    for signal in recv.try_iter() {
                        due = due.max(signal);
                    }
                    due
                };
                let checkpoint = now();
                let mut stages = [0; 5];
                let mut observe = |stage| {
                    match stage {
                        CheckpointStage::Written => stages[0] = now(),
                        CheckpointStage::Synced => stages[1] = now(),
                        CheckpointStage::TailCopied => stages[2] = now(),
                        CheckpointStage::TailSynced => stages[3] = now(),
                        CheckpointStage::Published => stages[4] = now(),
                        _ => {}
                    }
                    Ok(())
                };
                let info = match prefix.as_mut() {
                    Some(p) => db.finish_checkpoint(p.draft.take(), false, None, &mut observe),
                    None => db.checkpoint_with(&mut observe),
                }
                .unwrap();
                let end = now();
                let [lock, release, ..] = independent_trace::take();
                result.push((
                    due,
                    [
                        start, ready, checkpoint, stages[0], stages[1], lock, stages[2], stages[3],
                        stages[4], release, end,
                    ],
                    info.after_bytes + 48,
                ));
                last = due;
            }
            if exact {
                assert!(pending.is_empty());
                assert!(recv.recv().is_err(), "duplicate final checkpoint trigger");
                worker_holds
                    .lock()
                    .unwrap()
                    .push((4, independent_trace::take_holds()));
            }
            result
        });
        let mut threads = Vec::new();
        for lane in 0..4 {
            let db = &db;
            let send = send.clone();
            let barrier = &barrier;
            let tag = os_tag.as_deref();
            let holds = &holds;
            threads.push(scope.spawn(move || {
                if let Some(tag) = tag {
                    sync_clock::identify(tag, lane);
                }
                independent_trace::enable(clock);
                if exact {
                    independent_trace::enable_handoffs(clock, ops / 4 + 128);
                }
                let mut records = Vec::new();
                barrier.wait();
                for id in (lane..ops).step_by(4) {
                    let received = now();
                    let w = changes(id, n);
                    loop {
                        let begin = now();
                        let mut t = db.begin().unwrap();
                        for (k, v) in &w {
                            match v {
                                Some(v) => t.put(k, v).unwrap(),
                                None => t.delete(k).unwrap(),
                            }
                        }
                        let commit = now();
                        let outcome = t.commit();
                        let end = now();
                        let e = independent_trace::take();
                        let seq = match outcome {
                            Ok(s) => s,
                            Err(TxError::Conflict(_)) => 0,
                            Err(e) => panic!("{e:?}"),
                        };
                        records.push((id, received, begin, commit, e, end, seq));
                        if seq > 0 {
                            let pos = (seq - base) as usize;
                            if pos % cap == 0 && pos < ops {
                                if exact {
                                    send.send(pos / cap).unwrap();
                                } else {
                                    let _ = send.send(pos / cap);
                                }
                            }
                            break;
                        }
                    }
                }
                if exact {
                    holds
                        .lock()
                        .unwrap()
                        .push((lane, independent_trace::take_holds()));
                }
                records
            }));
        }
        drop(send);
        barrier.wait();
        let rows = threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect::<Vec<_>>();
        (rows, worker.join().unwrap())
    });
    if exact {
        assert_eq!(
            publications.iter().map(|p| p.0).collect::<Vec<_>>(),
            (1..ops / cap).collect::<Vec<_>>()
        );
        for (lane, events) in holds.into_inner().unwrap() {
            for [start, acq, before, after] in events {
                println!("HOLD,{lane},{start},{acq},{before},{after}");
            }
        }
    }
    if os_tag.is_some() {
        sync_clock::clock_anchor(clock);
    }
    let mut expected = BTreeMap::new();
    let mut committed = Vec::new();
    for (id, received, begin, commit, [l, a, s, f, b], end, seq) in attempts {
        println!("ATTEMPT,{id},{received},{begin},{commit},{l},{a},{s},{f},{b},{end},{seq}");
        if seq > 0 {
            committed.push((seq, id));
        }
    }
    committed.sort_unstable();
    assert_eq!(committed.len(), ops);
    for (index, (seq, id)) in committed.into_iter().enumerate() {
        assert_eq!(seq, base + 1 + index as u64);
        for (k, v) in changes(id, n) {
            expected.insert(u64::from_be_bytes(k[..8].try_into().unwrap()) as usize, v);
        }
    }
    for (due, t, b) in publications {
        println!(
            "PUBLICATION,{due},{},{},{},{},{},{},{},{},{},{},{},{b}",
            t[0], t[1], t[2], t[3], t[4], t[5], t[6], t[7], t[8], t[9], t[10]
        );
    }
    check_tx(&mut old, n, &BTreeMap::new());
    drop(old);
    check_tx(&mut db.begin_read().unwrap(), n, &expected);
    drop(db);
    let reopened = Database::open(&path).unwrap();
    let mut r = reopened.begin_read().unwrap();
    assert_eq!(r.version(), base + ops as u64);
    check_tx(&mut r, n, &expected);
    println!("INDEPENDENT_OK,{ops}");
}

#[test]
fn independent_admission_during_scan_and_publication() {
    use std::sync::mpsc::sync_channel;
    use std::time::Duration;
    for enabled in [false, true] {
        let path = dir().join("check.db");
        let db = Database::create(&path, [62; 16]).unwrap();
        let mut seed = db.begin().unwrap();
        seed.put(b"original", b"one").unwrap();
        seed.commit().unwrap();
        let mut old = db.begin_read().unwrap();
        let mut prepared = enabled.then(|| db.prepare_checkpoint_prefix(1).unwrap());
        let (in_stage, arrived) = sync_channel(0);
        let (resume, continued) = sync_channel(0);
        std::thread::scope(|scope| {
            let db_ref = &db;
            let cp = scope.spawn(move || {
                let mut hook = |s| {
                    if matches!(s, CheckpointStage::Written | CheckpointStage::TailSynced) {
                        in_stage.send(s).unwrap();
                        continued.recv_timeout(Duration::from_secs(10)).unwrap();
                    }
                    Ok(())
                };
                match prepared.as_mut() {
                    None => db_ref.checkpoint_with(&mut hook),
                    Some(p) => db_ref.finish_checkpoint(p.draft.take(), false, None, &mut hook),
                }
                .unwrap();
            });
            assert!(
                arrived.recv_timeout(Duration::from_secs(10)).unwrap() == CheckpointStage::Written
            );
            let mut during = db.begin().unwrap();
            during.put(b"original", b"two").unwrap();
            assert_eq!(during.commit().unwrap(), 2);
            resume.send(()).unwrap();
            assert!(
                arrived.recv_timeout(Duration::from_secs(10)).unwrap()
                    == CheckpointStage::TailSynced
            );
            let mut next = db.begin().unwrap();
            next.put(b"new", b"three").unwrap();
            assert!(matches!(
                db.inner.writer.try_lock(),
                Err(std::sync::TryLockError::WouldBlock)
            ));
            let w = scope.spawn(move || next.commit().unwrap());
            resume.send(()).unwrap();
            cp.join().unwrap();
            assert_eq!(w.join().unwrap(), 3);
        });
        assert_eq!(old.get(b"original").unwrap(), Some(b"one".to_vec()));
        assert!(old.get(b"new").unwrap().is_none());
        drop(old);
        drop(db);
        let db = Database::open(&path).unwrap();
        let mut r = db.begin_read().unwrap();
        assert_eq!(r.version(), 3);
        assert_eq!(r.get(b"original").unwrap(), Some(b"two".to_vec()));
        assert_eq!(r.get(b"new").unwrap(), Some(b"three".to_vec()));
    }
}

fn next_publication(
    signals: &std::sync::mpsc::Receiver<usize>,
    pending: &mut std::collections::BTreeSet<usize>,
    last: usize,
    total: usize,
) -> usize {
    let next = last + 1;
    while !pending.remove(&next) {
        let signal = signals
            .recv()
            .expect("writer stopped before all checkpoint triggers");
        assert!(
            signal > last && signal <= total && pending.insert(signal),
            "invalid checkpoint trigger"
        );
    }
    next
}

#[test]
fn queued_signals_preserve_each_publication_even_when_notifications_arrive_out_of_order() {
    let (s, r) = std::sync::mpsc::channel();
    for id in [4, 2, 3, 1] {
        s.send(id).unwrap();
    }
    drop(s);
    let mut pending = std::collections::BTreeSet::new();
    for last in 0..4 {
        assert_eq!(next_publication(&r, &mut pending, last, 4), last + 1);
    }
    assert!(pending.is_empty());
}

#[test]
fn missing_and_duplicate_checkpoint_notifications_are_detected() {
    for signals in [vec![2], vec![2, 2, 1]] {
        assert!(
            std::panic::catch_unwind(move || {
                let (s, r) = std::sync::mpsc::channel();
                for id in signals {
                    s.send(id).unwrap();
                }
                drop(s);
                next_publication(&r, &mut std::collections::BTreeSet::new(), 0, 2);
            })
            .is_err()
        );
    }
}
