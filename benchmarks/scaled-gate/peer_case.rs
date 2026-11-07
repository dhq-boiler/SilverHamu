#[test]
#[ignore = "native test backend on the peer update workload"]
fn peer_updates() {
    let keys: Vec<u64> = fs::read_to_string("/trace/keys.txt")
        .unwrap()
        .lines()
        .map(|l| l.parse().unwrap())
        .collect();
    let rep: u64 = std::env::var("PEER_REP").unwrap().parse().unwrap();
    let ops = keys.len();
    assert_eq!(ops, 1024);
    let val = |i: u64, stamp: u64| {
        let mut b = vec![b'x'; 256];
        b[..8].copy_from_slice(&i.to_be_bytes());
        b[8..16].copy_from_slice(&stamp.to_be_bytes());
        b
    };
    let path = dir().join("peer.db");
    let cfg = Config {
        checkpoints: 40,
        logs: 1024,
    };
    let db = MvccPages::create_direct(
        &path,
        cfg,
        (0u64..100000).map(|i| (i.to_be_bytes().to_vec(), val(i, 0))),
        None,
    );
    let warmed = Barrier::new(5);
    let go = Barrier::new(5);
    let (ns, samples) = std::thread::scope(|s| {
        let mut threads = Vec::new();
        for (lane, work) in keys.chunks(ops / 4).enumerate() {
            let db = &db;
            let warmed = &warmed;
            let go = &go;
            threads.push(s.spawn(move || {
                for i in &work[..50] {
                    let mut tx = db.begin().unwrap();
                    assert!(tx.get(&i.to_be_bytes()).unwrap().is_some());
                    tx.rollback();
                }
                let mut samples = Vec::with_capacity(work.len());
                warmed.wait();
                go.wait();
                for (j, i) in work.iter().enumerate() {
                    let start = Instant::now();
                    let mut tx = db.begin().unwrap();
                    let stamp = rep * 1000000000 + 40000000 + (lane * ops + j + 1) as u64;
                    tx.put(&i.to_be_bytes(), &val(*i, stamp)).unwrap();
                    tx.commit().unwrap();
                    samples.push(start.elapsed().as_nanos());
                }
                samples
            }));
        }
        warmed.wait();
        let start = Instant::now();
        go.wait();
        let samples = threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect::<Vec<_>>();
        (start.elapsed().as_nanos(), samples)
    });
    let mut stamps = vec![0u64; 100000];
    for (lane, w) in keys.chunks(ops / 4).enumerate() {
        for (j, i) in w.iter().enumerate() {
            stamps[*i as usize] = rep * 1000000000 + 40000000 + (lane * ops + j + 1) as u64;
        }
    }
    drop(db);
    let db = MvccPages::open_direct(&path, cfg, None);
    let mut r = db.begin_read().unwrap();
    assert_eq!(r.version(), ops as u64);
    let mut after = None;
    let mut count = 0;
    loop {
        let page = r
            .scan_page(
                b"",
                None,
                after.as_deref(),
                ScanLimits {
                    max_rows: 512,
                    max_bytes: PAGE,
                    max_visits: 4096,
                },
            )
            .unwrap();
        for (k, v) in page.rows {
            assert_eq!(k, (count as u64).to_be_bytes());
            assert_eq!(v, val(count as u64, stamps[count]));
            count += 1;
        }
        after = page.next_after;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(count, 100000);
    println!("PEER_SUMMARY,{rep},{ns},{count}");
    for v in samples {
        println!("PEER_NS,{v}");
    }
}
