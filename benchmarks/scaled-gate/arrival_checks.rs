fn wait_count(counter: &AtomicUsize, n: usize) {
    let until = Instant::now() + Duration::from_secs(8);
    while counter.load(Ordering::Acquire) < n {
        assert!(Instant::now() < until, "arrival timeout");
        std::thread::yield_now();
    }
}
#[test]
#[ignore = "explicit arrival group"]
fn arrival_gate_conflict_and_bounds() {
    assert!(std::env::var_os("CATDB_TEST_ARRIVALS").is_some());
    for case in ["point", "range", "write", "bytes", "failure"] {
        let p = dir().join("arrivals.bin");
        let entered = Arc::new(AtomicUsize::new(0));
        let ordinal = Arc::new(AtomicUsize::new(0));
        let barrier = Arc::new(Barrier::new(2));
        let hook: Fault = Some(Arc::new({
            let entered = entered.clone();
            let ordinal = ordinal.clone();
            let barrier = barrier.clone();
            move |stage| {
                if stage == "arrival.admitted" {
                    entered.fetch_add(1, Ordering::Release);
                }
                if stage == "append.before-sync" {
                    match ordinal.fetch_add(1, Ordering::AcqRel) {
                        0 => {
                            barrier.wait();
                            barrier.wait();
                        }
                        1 => {
                            barrier.wait();
                            barrier.wait();
                            if case == "failure" {
                                return Err(err("injected group sync"));
                            }
                        }
                        _ => {}
                    }
                }
                Ok(())
            }
        }));
        let db = MvccPages::create_direct(
            &p,
            four_config(),
            (0..20).map(|i| (key(i), value(i, 0))),
            hook,
        );
        let mut old = db.begin_read().unwrap();
        let mut leader = db.begin().unwrap();
        leader.put(b"L", b"ok").unwrap();
        let mut a = db.begin().unwrap();
        a.put(b"x", b"A").unwrap();
        let mut b = db.begin().unwrap();
        match case {
            "point" => {
                assert_eq!(b.get(b"x").unwrap(), None);
            }
            "range" => {
                assert!(b.scan(b"w", Some(b"z")).unwrap().is_empty());
            }
            "write" => b.put(b"x", b"B").unwrap(),
            _ => {}
        }
        b.put(b"B", b"B").unwrap();
        let mut c = db.begin().unwrap();
        c.put(b"C", b"C").unwrap();
        if case == "bytes" {
            for i in 0..6 {
                a.put(&[b'a', i], &vec![1; 48000]).unwrap();
                b.put(&[b'b', i], &vec![2; 48000]).unwrap();
            }
        }
        std::thread::scope(|s| {
            let l = s.spawn(move || leader.commit());
            barrier.wait();
            let one = s.spawn(move || a.commit());
            wait_count(&entered, 2);
            let two = s.spawn(move || b.commit());
            wait_count(&entered, 3);
            let three = s.spawn(move || c.commit());
            wait_count(&entered, 4);
            barrier.wait();
            assert_eq!(l.join().unwrap().unwrap(), 1);
            barrier.wait();
            let mut reader = db.begin_read().unwrap();
            assert_eq!(reader.version(), 1);
            assert_eq!(reader.get(b"x").unwrap(), None);
            barrier.wait();
            let ans = [
                one.join().unwrap(),
                two.join().unwrap(),
                three.join().unwrap(),
            ];
            if case == "failure" {
                assert!(
                    ans.iter()
                        .all(|r| matches!(r, Err(TxError::CommitUncertain(_))))
                );
                return;
            }
            assert_eq!(*ans[0].as_ref().unwrap(), 2);
            if case == "bytes" {
                assert_eq!(*ans[1].as_ref().unwrap(), 3);
                assert_eq!(*ans[2].as_ref().unwrap(), 4);
            } else {
                assert!(matches!(ans[1], Err(TxError::Conflict(_))));
                assert_eq!(*ans[2].as_ref().unwrap(), 3);
            }
            assert_eq!(old.get(b"L").unwrap(), None);
        });
        drop(old);
        if case != "failure" {
            assert_eq!(
                db.commit_sync_attempts(),
                if case == "bytes" { 3 } else { 2 }
            );
            db.checkpoint().unwrap();
        }
        drop(db);
        let reopened = MvccPages::open_direct(&p, four_config(), None);
        let mut r = reopened.begin_read().unwrap();
        assert_eq!(r.get(b"L").unwrap(), Some(b"ok".to_vec()));
        if case == "failure" {
            assert!([1, 4].contains(&r.version()));
            assert_eq!(r.get(b"x").unwrap().is_some(), r.version() == 4);
        } else {
            assert_eq!(r.version(), if case == "bytes" { 4 } else { 3 });
            assert!(r.get(b"C").unwrap().is_some());
        }
    }
}
#[test]
#[ignore = "child process for a multi-commit group"]
fn arrivals_kill_worker() {
    let p = PathBuf::from(std::env::var_os("ARRIVAL_FILE").unwrap());
    let target = std::env::var("ARRIVAL_STAGE").unwrap();
    let entered = Arc::new(AtomicUsize::new(0));
    let hook: Fault = Some(Arc::new({
        let entered = entered.clone();
        let p = p.clone();
        let batch = AtomicUsize::new(0);
        move |event| {
            if event == "arrival.admitted" {
                entered.fetch_add(1, Ordering::Release);
            }
            if event == "append.prepared" {
                batch.fetch_add(1, Ordering::AcqRel);
                if batch.load(Ordering::Acquire) == 2 {
                    let until = Instant::now() + Duration::from_secs(8);
                    while !p.with_extension("ack").exists() {
                        assert!(Instant::now() < until);
                        std::thread::yield_now();
                    }
                }
            }
            if event == "append.before-sync" && batch.load(Ordering::Acquire) == 1 {
                wait_count(&entered, 4);
            }
            if event == target
                && entered.load(Ordering::Acquire) == 4
                && p.with_extension("ack").exists()
            {
                fs::write(p.with_extension("ready"), event)?;
                loop {
                    std::thread::park();
                }
            }
            Ok(())
        }
    }));
    let db = MvccPages::create_direct(&p, four_config(), std::iter::empty(), hook);
    std::thread::scope(|scope| {
        let mut jobs = Vec::new();
        for i in 0..4 {
            let db = &db;
            let p = &p;
            jobs.push(scope.spawn(move || {
                let mut tx = db.begin().unwrap();
                tx.put(&[i], &[i]).unwrap();
                let ver = tx.commit().unwrap();
                if i == 0 {
                    assert_eq!(ver, 1);
                    fs::write(p.with_extension("ack"), "1").unwrap();
                }
            }));
            wait_count(&entered, i as usize + 1);
        }
        for job in jobs {
            job.join().unwrap();
        }
    });
    db.checkpoint().unwrap();
}
#[test]
#[ignore = "explicit arrival group"]
fn arrival_gate_process_exit() {
    for point in [
        "append-0.half",
        "append.before-sync",
        "append.synced",
        "selector.synced",
    ] {
        let p = dir().join("kill.bin");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "transactions::scaled_gate::checks::timing::arrivals_kill_worker",
                "--exact",
                "--ignored",
            ])
            .env("ARRIVAL_FILE", &p)
            .env("ARRIVAL_STAGE", point)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(12);
        while !p.with_extension("ready").exists() {
            assert!(child.try_wait().unwrap().is_none(), "child exited {point}");
            if Instant::now() > until {
                let _ = child.kill();
                let _ = child.wait();
                panic!("timeout {point}");
            }
            std::thread::sleep(Duration::from_millis(3));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        let db = MvccPages::open_direct(&p, four_config(), None);
        let mut r = db.begin_read().unwrap();
        assert!([1, 4].contains(&r.version()));
        if matches!(point, "append.synced" | "selector.synced") {
            assert_eq!(r.version(), 4);
        }
        assert_eq!(r.get(&[0]).unwrap(), Some(vec![0]));
        for i in 1..4 {
            assert_eq!(r.get(&[i]).unwrap(), (r.version() == 4).then(|| vec![i]));
        }
    }
}

#[test]
#[ignore = "explicit arrival group"]
fn arrival_gate_count_limit_and_refusal() {
    let path = dir().join("bounds.db");
    let entered = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(Barrier::new(2));
    let first = std::sync::atomic::AtomicBool::new(true);
    let fault: Fault = Some(Arc::new({
        let entered = entered.clone();
        let barrier = barrier.clone();
        move |event| {
            if event == "arrival.admitted" {
                entered.fetch_add(1, Ordering::Release);
            }
            if event == "append.before-sync" && first.swap(false, Ordering::AcqRel) {
                barrier.wait();
                barrier.wait();
            }
            Ok(())
        }
    }));
    let db = MvccPages::create_direct(&path, four_config(), std::iter::empty(), fault);
    let mut oversized = db.begin().unwrap();
    for k in 0..9 {
        oversized.put(&[k], &vec![1; 60000]).unwrap();
    }
    assert!(matches!(
        oversized.commit(),
        Err(TxError::ResourceExhausted(_))
    ));
    assert_eq!(db.commit_sync_attempts(), 0);
    std::thread::scope(|s| {
        let mut threads = Vec::new();
        for i in 0u8..7 {
            let mut tx = db.begin().unwrap();
            tx.put(&[i], &[i]).unwrap();
            threads.push(s.spawn(move || tx.commit().unwrap()));
            wait_count(&entered, usize::from(i) + 1);
            if i == 0 {
                barrier.wait();
            }
        }
        barrier.wait();
        for (i, t) in threads.into_iter().enumerate() {
            assert_eq!(t.join().unwrap(), i as u64 + 1);
        }
    });
    assert_eq!(db.commit_sync_attempts(), 3);
    drop(db);
    assert_eq!(
        MvccPages::open_direct(&path, four_config(), None)
            .begin_read()
            .unwrap()
            .version(),
        7
    );
}
#[test]
#[ignore = "explicit arrival group"]
fn arrival_gate_queue_caps_do_not_orphan_admitted_work() {
    for (n, large) in [(32u8, false), (8, true)] {
        let path = dir().join("queue.db");
        let cfg = Config {
            logs: 64,
            ..four_config()
        };
        let entered = Arc::new(AtomicUsize::new(0));
        let paused = Arc::new(Barrier::new(2));
        let first = std::sync::atomic::AtomicBool::new(true);
        let hook: Fault = Some(Arc::new({
            let entered = entered.clone();
            let paused = paused.clone();
            move |s| {
                if s == "arrival.admitted" {
                    entered.fetch_add(1, Ordering::Release);
                }
                if s == "append.before-sync" && first.swap(false, Ordering::AcqRel) {
                    paused.wait();
                    paused.wait();
                }
                Ok(())
            }
        }));
        let db = MvccPages::create_direct(&path, cfg, std::iter::empty(), hook);
        let mut bad = db.begin().unwrap();
        bad.put(&vec![3; MAX_KEY_BYTES], b"v").unwrap();
        assert!(matches!(bad.commit(), Err(TxError::Limit(_))));
        let make = |i: u8| {
            let mut t = db.begin().unwrap();
            t.put(&[i], &[i]).unwrap();
            if large {
                for k in 0..8 {
                    t.put(&[i, k], &vec![i; 60000]).unwrap();
                }
            }
            t
        };
        std::thread::scope(|s| {
            let mut jobs = Vec::new();
            for i in 0..=n {
                let t = make(i);
                jobs.push(s.spawn(move || t.commit()));
                wait_count(&entered, usize::from(i) + 1);
                if i == 0 {
                    paused.wait();
                }
            }
            assert!(matches!(
                make(n + 1).commit(),
                Err(TxError::ResourceExhausted(_))
            ));
            paused.wait();
            for (i, job) in jobs.into_iter().enumerate() {
                assert_eq!(job.join().unwrap().unwrap(), i as u64 + 1);
            }
        });
        assert_eq!(make(n + 1).commit().unwrap(), u64::from(n) + 2);
        drop(db);
        let db = MvccPages::open_direct(&path, cfg, None);
        let mut r = db.begin_read().unwrap();
        for i in 0..=n + 1 {
            assert_eq!(r.get(&[i]).unwrap(), Some(vec![i]));
        }
    }
}
