fn seed_ranges(n: usize) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut bytes = 140;
    for i in 0..n {
        let row = 9 + key(i).len() + 256;
        if bytes + row > BLOCK {
            ranges.push((start, i));
            start = i;
            bytes = 140;
        }
        bytes += row;
    }
    ranges.push((start, n));
    ranges
}
#[test]
#[ignore = "matched checkpoint density sweep"]
fn density_compare() {
    let n = 1_000_000;
    let rep: usize = std::env::var("DENSITY_REP").unwrap().parse().unwrap();
    let rows = seed_ranges(n);
    let cfg = Config {
        checkpoints: 340,
        logs: 256,
    };
    assert_eq!(
        fs::read_to_string("/sys/fs/cgroup/memory.max")
            .unwrap()
            .trim(),
        "4294967296"
    );
    let quota = fs::read_to_string("/sys/fs/cgroup/cpu.max").unwrap();
    let cpus: Vec<u64> = quota
        .split_whitespace()
        .map(|s| s.parse().unwrap())
        .collect();
    assert_eq!(cpus[0], 2 * cpus[1]);
    let path = dir().join("density.db");
    let db = MvccPages::create_direct(&path, cfg, (0..n).map(|i| (key(i), value(i, 0))), None);
    let mut old = db.begin_read().unwrap();
    let mut expected = BTreeMap::new();
    for j in 0..6 {
        let density = (j + 2 * (rep - 1)) % 6;
        let mut updated = 0;
        let start = Instant::now();
        for bucket in 0..96 {
            let (lo, hi) = rows[1 + bucket * (rows.len() - 2) / 96];
            let len = hi - lo;
            let many = [1, 4, 16, len.div_ceil(4), len.div_ceil(2), len][density];
            let mut tx = db.begin().unwrap();
            for i in 0..many {
                let id = lo + (i * len / many + (j * 7) % len) % len;
                let v = value(id, (j + 1) as u64);
                tx.put(&key(id), &v).unwrap();
                expected.insert(id, Some(v));
                updated += 1;
            }
            tx.commit().unwrap();
        }
        let updates = start.elapsed().as_nanos();
        let t = Instant::now();
        let cp = db.checkpoint().unwrap();
        let ns = t.elapsed().as_nanos();
        assert_eq!(cp.records, n);
        assert_eq!(cp.version, (j as u64 + 1) * 96);
        println!(
            "DENSITY,{density},{updated},{updates},{ns},{},{}",
            memory().0,
            cp.version
        );
        check_tx(&mut db.begin_read().unwrap(), n, &expected);
    }
    check_tx(&mut old, n, &BTreeMap::new());
    drop(old);
    drop(db);
    let db = MvccPages::open_direct(&path, cfg, None);
    let mut r = db.begin_read().unwrap();
    assert_eq!(r.version(), 576);
    check_tx(&mut r, n, &expected);
}
