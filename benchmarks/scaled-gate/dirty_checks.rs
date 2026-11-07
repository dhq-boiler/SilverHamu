#[test]
fn dirty_split_delete_empty_pin_and_reuse() {
    let p = dir().join("dirty.db");
    let mut s = Dirty::create(&p, config(), data(2000)).unwrap();
    let pin = s.pin().unwrap();
    let original = pin.rows.clone();
    let live_slots: BTreeSet<_> = pin._pages.values().map(|p| p.slot).collect();
    let before = s.control.costs.writes["page"];
    s.switch(&mut noop).unwrap();
    assert_eq!(before, s.control.costs.writes["page"]);
    let mut expected = original.clone();
    for round in 0..5 {
        let mut w = WriteSet::new();
        for i in 0..500 {
            w.insert(
                key(i),
                if round == 2 {
                    None
                } else {
                    Some(vec![round; 700])
                },
            );
        }
        w.insert(key(1910), Some(vec![round; 300]));
        apply(&mut expected, w.clone());
        s.append(&w, &mut noop).unwrap();
        s.switch(&mut noop).unwrap();
        assert_eq!(s.rows, expected);
        assert_eq!(pin.rows, original);
        for page in s.pages.values().filter(|p| p.generation > 2) {
            assert!(!live_slots.contains(&page.slot));
        }
    }
    drop(s);
    assert!(Dirty::open(&p, config(), &mut noop).is_err());
    drop(pin);
    let mut s = Dirty::open(&p, config(), &mut noop).unwrap();
    assert_eq!(s.rows, expected);
    let w = s.rows.keys().map(|k| (k.clone(), None)).collect();
    s.append(&w, &mut noop).unwrap();
    s.switch(&mut noop).unwrap();
    assert!(s.pages.is_empty());
    drop(s);
    assert!(
        Dirty::open(&p, config(), &mut noop)
            .unwrap()
            .rows
            .is_empty()
    );
}

#[test]
#[ignore = "subprocess crash fixture"]
fn dirty_kill_child() {
    let p = PathBuf::from(std::env::var_os("DIRTY_PATH").unwrap());
    let point = std::env::var("DIRTY_POINT").unwrap();
    let mut stop = |event: &str| {
        if event == point {
            fs::write(p.with_extension("ready"), event)?;
            loop {
                std::thread::park();
            }
        }
        Ok(())
    };
    let mut s = Dirty::open(&p, config(), &mut stop).unwrap();
    s.switch(&mut stop).unwrap();
}
#[test]
fn dirty_crash_publish_reclaim_and_repair() {
    for point in [
        "page.half",
        "pages.synced",
        "checkpoint.synced",
        "selector-0.half",
        "selector.synced",
        "reclaim.before",
        "reclaim.after",
        "repair-page.half",
        "recovery-pages.synced",
    ] {
        let p = dir().join("crash.db");
        let mut s = Dirty::create(&p, config(), data(600)).unwrap();
        s.append(&change(), &mut noop).unwrap();
        let expected = s.rows.clone();
        let offset = u64::from(s.pages.values().next().unwrap().slot) * PAIR;
        drop(s);
        if point.starts_with("repair") || point.starts_with("recovery") {
            overwrite(&p.with_extension("pages"), offset, &[0; HEAD]);
        }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "transactions::scaled_gate::checks::dirty_kill_child",
                "--ignored",
                "--exact",
            ])
            .env("DIRTY_PATH", &p)
            .env("DIRTY_POINT", point)
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !p.with_extension("ready").exists() {
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
        let mut s = Dirty::open(&p, config(), &mut noop).unwrap();
        assert_eq!(s.rows, expected);
        s.switch(&mut noop).unwrap();
        drop(s);
        assert_eq!(Dirty::open(&p, config(), &mut noop).unwrap().rows, expected);
    }
}
