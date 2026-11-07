fn four_config() -> Config {
    Config {
        logs: 8,
        ..config()
    }
}
fn prototype(mode: &str, p: &Path) -> Backend {
    if mode == "dual" {
        Backend::Full(Store::create(p, four_config(), data(500), BUFFER_BUDGET, &mut noop).unwrap())
    } else {
        Backend::Pages(Dirty::create(p, four_config(), data(500)).unwrap())
    }
}
#[test]
fn concurrent_point_range_and_write_dependencies_survive_switch() {
    for mode in ["dual", "dirty"] {
        let p = dir().join("deps.db");
        let run = Four::new(prototype(mode, &p), 16);
        let mut point = Dependencies::default();
        let mut range = Dependencies::default();
        {
            let g = run.publication.lock().unwrap();
            assert!(g.point(0, &key(7), &mut point).unwrap().is_some());
            assert!(
                g.range(0, key(700), key(702), &mut range)
                    .unwrap()
                    .is_empty()
            );
        }
        let w: WriteSet = [(key(7), None), (key(701), Some(vec![1]))].into();
        std::thread::scope(|s| {
            s.spawn(|| {
                let mut g = run.publication.lock().unwrap();
                g.publish(&run.epoch, 0, &w, &Dependencies::default(), &mut |event| {
                    if event == "append.before-sync" {
                        assert_eq!(run.epoch.load(Ordering::Acquire), 0);
                    }
                    Ok(())
                })
                .unwrap()
                .unwrap();
                g.backend.checkpoint(&mut noop);
            })
            .join()
            .unwrap();
        });
        let mut g = run.publication.lock().unwrap();
        let other = [(key(900), Some(vec![2]))].into();
        for (writes, dep) in [
            (&other, &point),
            (&other, &range),
            (&w, &Dependencies::default()),
        ] {
            assert_eq!(
                g.publish(&run.epoch, 0, writes, dep, &mut noop).unwrap(),
                None
            );
        }
        assert!(g.point(0, &key(7), &mut Dependencies::default()).is_err());
        assert!(
            g.publish(&run.epoch, 0, &other, &Dependencies::default(), &mut noop)
                .unwrap()
                .is_some()
        );
    }
}

#[test]
#[ignore = "child process for four-writer durability"]
fn four_child() {
    let p = PathBuf::from(std::env::var_os("FOUR_PATH").unwrap());
    let mode = std::env::var("FOUR_MODE").unwrap();
    let event = std::env::var("FOUR_EVENT").unwrap();
    let run = Four::new(prototype(&mode, &p), 16);
    let barrier = Barrier::new(4);
    std::thread::scope(|s| {
        for lane in 0..4 {
            let run = &run;
            let barrier = &barrier;
            let p = &p;
            let event = &event;
            s.spawn(move || {
                let w = [(key(1000 + lane), Some(vec![lane as u8]))].into();
                let turn = run.turn.as_ref().map(TurnQueue::enter);
                run.publication
                    .lock()
                    .unwrap()
                    .publish(&run.epoch, 0, &w, &Dependencies::default(), &mut noop)
                    .unwrap()
                    .unwrap();
                drop(turn);
                fs::write(p.with_extension(format!("ack{lane}")), b"ok").unwrap();
                barrier.wait();
                if lane == 0 {
                    let _turn = run.turn.as_ref().map(TurnQueue::enter);
                    let mut g = run.publication.lock().unwrap();
                    let mut stop = |n: &str| {
                        if n == event {
                            fs::write(p.with_extension("ready"), n)?;
                            loop {
                                std::thread::park()
                            }
                        }
                        Ok(())
                    };
                    let pending = [
                        (key(2000), Some(vec![8])),
                        (key(2001), None),
                        (key(12), None),
                    ]
                    .into();
                    g.publish(&run.epoch, 4, &pending, &Dependencies::default(), &mut stop)
                        .unwrap()
                        .unwrap();
                    g.backend.checkpoint(&mut stop);
                } else {
                    loop {
                        std::thread::park()
                    }
                }
            });
        }
    })
}
#[test]
fn four_acked_commits_recover_across_sync_and_publication() {
    for mode in ["dual", "dirty"] {
        for event in [
            "append.before-sync",
            "append.synced",
            "selector-0.half",
            "selector.synced",
        ] {
            let p = dir().join("crash.db");
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "transactions::scaled_gate::checks::timing::four_child",
                    "--exact",
                    "--ignored",
                ])
                .env("FOUR_PATH", &p)
                .env("FOUR_MODE", mode)
                .env("FOUR_EVENT", event)
                .stdout(Stdio::null())
                .spawn()
                .unwrap();
            let until = Instant::now() + Duration::from_secs(10);
            while !p.with_extension("ready").exists() {
                assert!(child.try_wait().unwrap().is_none());
                if Instant::now() > until {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("timeout {event}");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            child.kill().unwrap();
            child.wait().unwrap();
            let rows = if mode == "dual" {
                Store::open(&p, four_config(), BUFFER_BUDGET, &mut noop)
                    .unwrap()
                    .rows
            } else {
                Dirty::open(&p, four_config(), &mut noop).unwrap().rows
            };
            let mut expected = data(500);
            for lane in 0..4 {
                assert!(p.with_extension(format!("ack{lane}")).exists());
                expected.insert(key(1000 + lane), vec![lane as u8].into());
            }
            if event != "append.before-sync" || rows.contains_key(&key(2000)) {
                expected.insert(key(2000), vec![8].into());
                expected.remove(&key(12));
            }
            assert_eq!(rows, expected);
        }
    }
}
