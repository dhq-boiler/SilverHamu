fn prewrite(db: &Database, count: usize, bytes: u64) -> crate::transactions::Result<(usize, u64)> {
    db.inner
        .test_persistence
        .get()
        .unwrap()
        .prewrite(db, count, bytes)
}
#[test]
#[ignore = "bounded checkpoint prewrite pilot"]
fn prewrite_pilot() {
    let n = 12000;
    for rep in 0..3 {
        for hot in [false, true] {
            for enabled in if rep % 2 == 0 {
                [false, true]
            } else {
                [true, false]
            } {
                let path = dir().join("pilot.db");
                let cfg = Config {
                    checkpoints: 16,
                    logs: 64,
                };
                let db = MvccPages::create_direct(
                    &path,
                    cfg,
                    (0..n).map(|i| (key(i), value(i, 0))),
                    None,
                );
                let mut old = db.begin_read().unwrap();
                let mut expected = BTreeMap::new();
                let update =
                    |id: usize, v: u64, expected: &mut BTreeMap<usize, Option<Vec<u8>>>| {
                        let mut t = db.begin().unwrap();
                        let bytes = value(id, v);
                        t.put(&key(id), &bytes).unwrap();
                        t.commit().unwrap();
                        expected.insert(id, Some(bytes));
                    };
                for i in 0..16 {
                    update(600 * i, 1, &mut expected);
                }
                let before = Instant::now();
                let mut count = 0;
                let mut bytes = 0;
                if enabled {
                    assert_eq!(prewrite(&db, 16, PAIR - 1).unwrap(), (0, 0));
                    for _ in 0..4 {
                        let (k, b) = prewrite(&db, 16, 16 * PAIR).unwrap();
                        count += k;
                        bytes += b;
                    }
                    assert_eq!(count, 16);
                    assert_eq!(prewrite(&db, 16, 16 * PAIR).unwrap(), (0, 0));
                }
                let prep = before.elapsed();
                for i in 0..16 {
                    update(600 * i + if hot { 1 } else { 300 }, 2, &mut expected);
                }
                let before = Instant::now();
                assert_eq!(db.checkpoint().unwrap().records, n);
                let cp = before.elapsed();
                println!(
                    "PREWRITE_PILOT,{rep},{hot},{enabled},{count},{bytes},{},{},{}",
                    prep.as_nanos(),
                    cp.as_nanos(),
                    cp.as_nanos() + prep.as_nanos()
                );
                check_tx(&mut old, n, &BTreeMap::new());
                drop(old);
                drop(db);
                let db = MvccPages::open_direct(&path, cfg, None);
                let mut r = db.begin_read().unwrap();
                assert_eq!(r.version(), 32);
                check_tx(&mut r, n, &expected);
            }
        }
    }
}
#[test]
#[ignore = "prewrite shape changes and I/O failure"]
fn prewrite_shapes_and_failure() {
    let n = 700;
    let p = dir().join("shape.db");
    let cfg = four_config();
    let db = MvccPages::create_direct(&p, cfg, (0..n).map(|i| (key(i), value(i, 0))), None);
    drop(db);
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let hook: Fault = Some(Arc::new(move |s| {
        if s == "page.half" && fail.swap(false, Ordering::Relaxed) {
            return Err(err("injected prewrite failure"));
        }
        Ok(())
    }));
    let db = MvccPages::open_direct(&p, cfg, hook);
    let mut old = db.begin_read().unwrap();
    let mut tx = db.begin().unwrap();
    tx.put(&key(2), b"short").unwrap();
    tx.commit().unwrap();
    assert!(prewrite(&db, 1, PAIR).is_err());
    assert_eq!(prewrite(&db, 1, PAIR).unwrap().0, 1);
    let mut t = db.begin().unwrap();
    t.put(&key(2), &vec![7; 8000]).unwrap();
    t.commit().unwrap();
    db.checkpoint().unwrap();
    let mut t = db.begin().unwrap();
    t.put(&key(2), b"x").unwrap();
    t.commit().unwrap();
    assert_eq!(prewrite(&db, 1, PAIR).unwrap().0, 1);
    for lo in (0..n).step_by(100) {
        let mut t = db.begin().unwrap();
        for i in lo..(lo + 100).min(n) {
            t.delete(&key(i)).unwrap();
        }
        t.commit().unwrap();
    }
    assert_eq!(db.checkpoint().unwrap().records, 0);
    check_tx(&mut old, n, &BTreeMap::new());
    drop(old);
    drop(db);
    let db = MvccPages::open_direct(&p, cfg, None);
    let mut r = db.begin_read().unwrap();
    assert_eq!(r.version(), 10);
    for i in 0..n {
        assert!(r.get(&key(i)).unwrap().is_none());
    }
}

#[test]
#[ignore = "prewrite crash fixture"]
fn prepared_kill_worker() {
    let path = PathBuf::from(std::env::var_os("PATCH_FILE").unwrap());
    let phase = std::env::var("PATCH_POINT").unwrap();
    let db = MvccPages::create_direct(
        &path,
        four_config(),
        (0..500).map(|i| (key(i), value(i, 0))),
        None,
    );
    drop(db);

    let hook: Fault = Some(Arc::new({
        let path = path.clone();
        move |s| {
            if s == phase {
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
    assert_eq!(prewrite(&db, 1, PAIR).unwrap().0, 1);
    db.checkpoint().unwrap();
}

#[test]
#[ignore = "prewrite recovery boundaries"]
fn prewrite_crashes() {
    for point in [
        "page.half",
        "pages.before-sync",
        "pages.synced",
        "selector-0.half",
        "selector.synced",
    ] {
        let p = dir().join("crash.db");
        kill_at(
            &p,
            point,
            "transactions::scaled_gate::checks::timing::prepared_kill_worker",
        );
        let db = MvccPages::open_direct(&p, four_config(), None);
        let mut r = db.begin_read().unwrap();
        assert_eq!(r.version(), 1);
        check_tx(&mut r, 500, &[(2, Some(value(2, 1)))].into());
    }
}

#[test]
#[ignore = "invalidated slots are recycled and read conflicts survive staging"]
fn prewrite_recycling_and_conflict() {
    let p = dir().join("slots.db");
    let db = MvccPages::create_direct(
        &p,
        Config {
            checkpoints: 16,
            logs: 32,
        },
        (0..500).map(|i| (key(i), value(i, 0))),
        None,
    );
    let mut reader = db.begin().unwrap();
    reader.get(&key(2)).unwrap();
    let mut max_len = 0;
    for pass in 1..=12 {
        let mut tx = db.begin().unwrap();
        tx.put(&key(2), &value(2, pass)).unwrap();
        tx.commit().unwrap();
        assert_eq!(prewrite(&db, 1, PAIR).unwrap().0, 1);
        let len = fs::metadata(p.with_extension("pages")).unwrap().len();
        if pass == 1 {
            max_len = len;
        } else {
            assert_eq!(len, max_len);
        }
    }
    reader.put(&key(3), b"conflict").unwrap();
    assert!(matches!(reader.commit(), Err(TxError::Conflict(_))));
    db.checkpoint().unwrap();
    drop(db);
    let db = MvccPages::open_direct(
        &p,
        Config {
            checkpoints: 16,
            logs: 32,
        },
        None,
    );
    check_tx(
        &mut db.begin_read().unwrap(),
        500,
        &[(2, Some(value(2, 12)))].into(),
    );
}
