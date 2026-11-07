#[test]
#[ignore = "explicit copy-patch safety"]
fn pagepatch_fallback_repair_and_old_reader() {
    use std::sync::atomic::AtomicUsize;
    let path = dir().join("patch.db");
    let cfg = four_config();
    let db = MvccPages::create_direct(&path, cfg, (0..700).map(|i| (key(i), value(i, 0))), None);
    drop(db);
    let initial = Dirty::open(&path, cfg, &mut noop).unwrap();
    let offset = u64::from(initial.pages.values().next().unwrap().slot) * PAIR;
    drop(initial);
    let hits = Arc::new(AtomicUsize::new(0));
    let fallback = Arc::new(AtomicUsize::new(0));
    let hook: Fault = Some(Arc::new({
        let hits = hits.clone();
        let fallback = fallback.clone();
        move |stage| {
            if stage == "patch.used" {
                hits.fetch_add(1, Ordering::Relaxed);
            }
            if stage == "patch.fallback" {
                fallback.fetch_add(1, Ordering::Relaxed);
            }
            Ok(())
        }
    }));
    let db = MvccPages::open_direct(&path, cfg, hook);
    let mut old = db.begin_read().unwrap();
    overwrite(&path.with_extension("pages"), offset, &[0; HEAD]);
    let mut tx = db.begin().unwrap();
    tx.put(&key(10), &value(10, 1)).unwrap();
    tx.commit().unwrap();
    assert_eq!(db.checkpoint().unwrap().records, 700);
    assert_eq!(hits.load(Ordering::Relaxed), 1);
    for step in 0..3 {
        let mut tx = db.begin().unwrap();
        match step {
            0 => tx.put(&key(10), b"short").unwrap(),
            1 => tx.delete(&key(10)).unwrap(),
            _ => tx.put(&key(10), b"again").unwrap(),
        }
        tx.commit().unwrap();
        db.checkpoint().unwrap();
    }
    assert_eq!(fallback.load(Ordering::Relaxed), 3);
    assert_eq!(old.get(&key(10)).unwrap(), Some(value(10, 0)));
    drop(old);
    drop(db);
    let db = MvccPages::open_direct(&path, cfg, None);
    assert_eq!(
        db.begin_read().unwrap().get(&key(10)).unwrap(),
        Some(b"again".to_vec())
    );
}
#[test]
#[ignore = "child for patch-specific checkpoint termination"]
fn patch_kill_worker() {
    let path = PathBuf::from(std::env::var_os("PATCH_FILE").unwrap());
    let phase = std::env::var("PATCH_POINT").unwrap();
    let db = MvccPages::create_direct(
        &path,
        four_config(),
        (0..500).map(|i| (key(i), value(i, 0))),
        None,
    );
    drop(db);
    let used = std::sync::atomic::AtomicBool::new(false);
    let hook: Fault = Some(Arc::new({
        let path = path.clone();
        move |s| {
            if s == "patch.validated" {
                used.store(true, Ordering::Release);
            }
            if s == phase && used.load(Ordering::Acquire) {
                fs::write(path.with_extension("ready"), s)?;
                loop {
                    std::thread::park()
                }
            }
            Ok(())
        }
    }));
    let db = MvccPages::open_direct(&path, four_config(), hook);
    let mut tx = db.begin().unwrap();
    tx.put(&key(2), &value(2, 1)).unwrap();
    tx.commit().unwrap();
    db.checkpoint().unwrap();
}
#[test]
#[ignore = "explicit copy-patch safety"]
fn pagepatch_crashes_preserve_acknowledged_update() {
    for point in [
        "page.half",
        "pages.synced",
        "selector-0.half",
        "selector.synced",
    ] {
        let p = dir().join("kill.db");
        kill_at(&p, point, "transactions::scaled_gate::checks::timing::patch_kill_worker");
        let db = MvccPages::open_direct(&p, four_config(), None);
        let mut r = db.begin_read().unwrap();
        assert_eq!(r.version(), 1);
        check_tx(&mut r, 500, &[(2, Some(value(2, 1)))].into());
    }
}

fn kill_at(p:&Path,point:&str,worker:&str){
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                worker,
                "--exact",
                "--ignored",
            ])
            .env("PATCH_FILE", p)
            .env("PATCH_POINT", point)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let until = Instant::now() + Duration::from_secs(12);
        while !p.with_extension("ready").exists() {
            assert!(child.try_wait().unwrap().is_none(), "child exited {point}");
            if Instant::now() > until {
                let _ = child.kill();
                let _ = child.wait();
                panic!("missed {point}");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        child.kill().unwrap();
        child.wait().unwrap();
}
