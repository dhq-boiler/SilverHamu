use super::*;
use std::hint::black_box;
use std::time::Instant;

fn key(n: u64) -> [u8; 8] {
    n.to_be_bytes()
}
fn hash(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}
fn read_range(tx: &mut Transaction, start: u64, end: Option<u64>) {
    tx.scan_page(
        &key(start),
        end.as_ref()
            .map(|n| n.to_be_bytes())
            .as_ref()
            .map(|b| b.as_slice()),
        None,
        ScanLimits {
            max_rows: 100,
            ..ScanLimits::default()
        },
    )
    .unwrap();
}
fn verify(tx: &mut Transaction, expected: &BTreeMap<Vec<u8>, Vec<u8>>) {
    let mut got = BTreeMap::new();
    let mut cursor = None;
    loop {
        let page = tx
            .scan_page(
                b"",
                None,
                cursor.as_deref(),
                ScanLimits {
                    max_rows: 4096,
                    ..ScanLimits::default()
                },
            )
            .unwrap();
        got.extend(page.rows.iter().cloned());
        cursor = page.next_after.clone();
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(&got, expected);
}

#[test]
#[ignore = "explicit synthetic production-engine performance experiment"]
fn production_global_guard_benchmark() {
    let dir = PathBuf::from(std::env::var_os("CATDB_GUARD_OUTPUT").unwrap());
    std::fs::create_dir(&dir).unwrap();
    let n: u64 = std::env::var("CATDB_GUARD_ROWS").unwrap().parse().unwrap();
    let shape = std::env::var("CATDB_GUARD_SHAPE").unwrap();
    assert!([100000, 1000000].contains(&n));
    assert!(["local", "spread"].contains(&shape.as_str()));
    let limits = ResourceLimits {
        max_retained_bytes: 512 * 1024 * 1024,
        ..ResourceLimits::default()
    };
    let path = dir.join("db.wal");
    let db = Database::create_with_limits(&path, [86; 16], limits).unwrap();
    let mut expected = BTreeMap::new();
    for start in (0..n).step_by(4096) {
        let mut tx = db.begin().unwrap();
        for i in start..(start + 4096).min(n) {
            tx.put(&key(i * 2), &key(i)).unwrap();
            expected.insert(key(i * 2).to_vec(), key(i).to_vec());
        }
        tx.commit().unwrap();
    }
    let original = expected.clone();
    let mut old = db.begin_read().unwrap();
    let mut stale_wide = db.begin().unwrap();
    read_range(&mut stale_wide, n, None);
    let mut stale_narrow = db.begin().unwrap();
    read_range(&mut stale_narrow, n, Some(n + 200));
    let mut edit = db.begin().unwrap();
    for i in 0..2400 {
        let region = if shape == "local" { n / 100 } else { n };
        let k = key((hash(i) % region) * 2 + u64::from(i % 3 == 0));
        if i % 3 == 1 {
            edit.delete(&k).unwrap();
            expected.remove(k.as_slice());
        } else {
            edit.put(&k, &key(i)).unwrap();
            expected.insert(k.to_vec(), key(i).to_vec());
        }
    }
    edit.commit().unwrap();
    let mut stable_wide = db.begin().unwrap();
    read_range(&mut stable_wide, 0, None);
    let mut stable_narrow = db.begin().unwrap();
    read_range(&mut stable_narrow, n, Some(n + 200));
    let mut out = std::io::BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("samples.csv"))
            .unwrap(),
    );
    writeln!(out, "case,sample,ns,conflict").unwrap();
    {
        let state = db.state().unwrap();
        let earlier = BTreeSet::new();
        for (name, tx) in [
            ("stable_wide", &stable_wide),
            ("stable_narrow", &stable_narrow),
            ("stale_upper_half", &stale_wide),
            ("stale_narrow", &stale_narrow),
        ] {
            let expected_conflict = tx.ranges.iter().any(|range| {
                state
                    .rows
                    .range::<[u8], _>(range.bounds())
                    .any(|(_, v)| v.back().unwrap().sequence > tx.snapshot)
            });
            for i in 0..96 {
                let t = Instant::now();
                let result = black_box(tx.validate_commit(black_box(&state), black_box(&earlier)));
                let ns = t.elapsed().as_nanos();
                let conflict = matches!(result, Err(Error::Conflict(ConflictKind::Range)));
                assert_eq!(conflict, expected_conflict);
                assert!(result.is_ok() || conflict);
                writeln!(out, "{name},{i},{ns},{conflict}").unwrap();
            }
        }
    }
    drop(stable_wide);
    drop(stable_narrow);
    drop(stale_wide);
    drop(stale_narrow);
    for i in 0..192 {
        let mut tx = db.begin().unwrap();
        let prepare = Instant::now();
        read_range(&mut tx, 0, None);
        tx.put(b"~commit", &key(i)).unwrap();
        writeln!(out, "prepare,{i},{},false", prepare.elapsed().as_nanos()).unwrap();
        let t = Instant::now();
        tx.commit().unwrap();
        let ns = t.elapsed().as_nanos();
        writeln!(out, "commit,{i},{ns},false").unwrap();
        expected.insert(b"~commit".to_vec(), key(i).to_vec());
    }
    verify(&mut old, &original);
    verify(&mut db.begin_read().unwrap(), &expected);
    drop(old);
    drop(db);
    let db = Database::open_with_limits(&path, limits).unwrap();
    verify(&mut db.begin_read().unwrap(), &expected);
    out.flush().unwrap();
    println!("verified current, old snapshot, reopened, {n} rows {shape}");
}
