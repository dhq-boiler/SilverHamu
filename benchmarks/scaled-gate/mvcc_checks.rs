#[test]
fn mvcc_fifo_revalidates_and_publishes_only_after_sync() {
    let path = dir().join("mvcc.db");
    let paused = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let once = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let fault: Fault = Some(Arc::new({
        let paused = paused.clone();
        let resume = resume.clone();
        move |s| {
            if s == "append.before-sync" && once.swap(false, Ordering::AcqRel) {
                paused.wait();
                resume.wait();
            }
            Ok(())
        }
    }));
    let db = MvccPages::create(&path, four_config(), data(200), fault);
    let mut old = db.begin_read().unwrap();
    let mut stale = db.begin().unwrap();
    stale.put(&key(2), b"stale").unwrap();
    let mut point = db.begin().unwrap();
    assert_eq!(point.get(&key(2)).unwrap(), Some(value(2, 0)));
    point.put(&key(800), b"x").unwrap();
    let mut range = db.begin().unwrap();
    assert!(range.scan(&key(900), Some(&key(999))).unwrap().is_empty());
    range.put(&key(801), b"y").unwrap();
    let mut independent = db.begin().unwrap();
    independent.put(&key(1000), b"independent").unwrap();
    std::thread::scope(|scope| {
        let first = scope.spawn(|| {
            let mut t = db.begin().unwrap();
            t.delete(&key(2)).unwrap();
            t.put(&key(901), b"insert").unwrap();
            t.commit().unwrap()
        });
        paused.wait();
        let mut during = db.begin_read().unwrap();
        assert_eq!(during.version(), 0);
        assert_eq!(during.get(&key(2)).unwrap(), Some(value(2, 0)));
        assert_eq!(during.get(&key(901)).unwrap(), None);
        let second = scope.spawn(move || stale.commit());
        resume.wait();
        assert_eq!(first.join().unwrap(), 1);
        assert!(matches!(
            second.join().unwrap(),
            Err(TxError::Conflict(crate::transactions::ConflictKind::Write))
        ));
        assert_eq!(during.get(&key(901)).unwrap(), None);
    });
    assert_eq!(db.checkpoint().unwrap().version, 1);
    assert!(matches!(
        point.commit(),
        Err(TxError::Conflict(crate::transactions::ConflictKind::Read))
    ));
    assert!(matches!(
        range.commit(),
        Err(TxError::Conflict(crate::transactions::ConflictKind::Range))
    ));
    assert_eq!(independent.commit().unwrap(), 2);
    assert_eq!(old.get(&key(2)).unwrap(), Some(value(2, 0)));
    assert_eq!(old.get(&key(901)).unwrap(), None);
    drop(old);
    drop(db);
    let recovered = MvccPages::open(&path, four_config(), None);
    let mut r = recovered.begin_read().unwrap();
    assert_eq!(r.version(), 2);
    assert_eq!(r.get(&key(1000)).unwrap(), Some(b"independent".to_vec()));
    let mut t = recovered.begin().unwrap();
    t.put(&key(3), b"ok").unwrap();
    assert_eq!(t.commit().unwrap(), 3);
}
fn recovery_update(db: &Database) {
    let mut t = db.begin().unwrap();
    t.delete(&key(1)).unwrap();
    t.put(&key(504), b"new").unwrap();
    assert_eq!(t.commit().unwrap(), 2);
}
fn assert_recovered(path: &Path, must_have_second: bool) {
    let db = MvccPages::open(path, four_config(), None);
    let mut r = db.begin_read().unwrap();
    let seq = r.version();
    assert!(seq == 2 || (!must_have_second && seq == 1));
    let mut want = data(100);
    want.insert(key(2), b"ack".as_slice().into());
    if seq == 2 {
        want.remove(&key(1));
        want.insert(key(504), b"new".as_slice().into());
    }
    let actual: Data = r
        .scan(b"", None)
        .unwrap()
        .into_iter()
        .map(|(k, v)| (k, v.into()))
        .collect();
    assert_eq!(actual, want);
}
#[test]
fn mvcc_persistence_error_requires_reopen() {
    for failure in ["append-0.half", "pages.before-sync", "selector.before-sync"] {
        let path = dir().join("fail.db");
        let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let fault: Fault = Some(Arc::new({
            let armed = armed.clone();
            move |s| {
                if s == failure && armed.load(Ordering::Acquire) {
                    return Err(err("injected persistence error"));
                }
                Ok(())
            }
        }));
        let db = MvccPages::create(&path, four_config(), data(100), fault);
        let mut t = db.begin().unwrap();
        t.put(&key(2), b"ack").unwrap();
        t.commit().unwrap();
        armed.store(true, Ordering::Release);
        if failure.starts_with("append") {
            let mut t = db.begin().unwrap();
            t.delete(&key(1)).unwrap();
            t.put(&key(504), b"new").unwrap();
            assert!(matches!(t.commit(), Err(TxError::CommitUncertain(_))));
        } else {
            recovery_update(&db);
            assert!(db.checkpoint().is_err());
        }
        assert!(db.begin().is_err());
        drop(db);
        assert_recovered(&path, !failure.starts_with("append"));
    }
}
#[test]
#[ignore = "MVCC subprocess durability fixture"]
fn mvcc_kill_child() {
    let path = PathBuf::from(std::env::var_os("MVCC_PATH").unwrap());
    let point = std::env::var("MVCC_POINT").unwrap();
    let armed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop = {
        let path = path.clone();
        let armed = armed.clone();
        move |stage: &str| {
            if armed.load(Ordering::Acquire) && stage == point {
                fs::write(path.with_extension("ready"), stage)?;
                loop {
                    std::thread::park()
                }
            }
            Ok(())
        }
    };
    let hook: Arc<dyn Fn(&str) -> io::Result<()> + Send + Sync> = Arc::new(stop);
    let db = MvccPages::create(&path, four_config(), data(100), Some(hook.clone()));
    let mut t = db.begin().unwrap();
    t.put(&key(2), b"ack").unwrap();
    assert_eq!(t.commit().unwrap(), 1);
    armed.store(true, Ordering::Release);
    recovery_update(&db);
    hook("returned").unwrap();
    db.checkpoint().unwrap();
}
#[test]
fn mvcc_kills_recover_acknowledged_versions() {
    for point in [
        "append-0.half",
        "append.before-sync",
        "append.synced",
        "returned",
        "pages.synced",
        "selector-0.half",
        "selector.synced",
    ] {
        let path = dir().join("crash.db");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "transactions::scaled_gate::checks::timing::mvcc_kill_child",
                "--ignored",
                "--exact",
            ])
            .env("MVCC_PATH", &path)
            .env("MVCC_POINT", point)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(12);
        while !path.with_extension("ready").exists() {
            assert!(
                child.try_wait().unwrap().is_none(),
                "child exited at {point}"
            );
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("missed {point}");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        child.kill().unwrap();
        child.wait().unwrap();
        assert_recovered(
            &path,
            !matches!(point, "append-0.half" | "append.before-sync"),
        );
    }
}
