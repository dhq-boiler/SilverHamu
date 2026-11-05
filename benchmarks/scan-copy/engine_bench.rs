use super::*;
use crate::work_elision_metrics as meter;
use std::time::Instant;
fn key(i: u64) -> [u8; 8] {
    i.to_be_bytes()
}
fn expected(db: &Database, map: &BTreeMap<Vec<u8>, Vec<u8>>) {
    let mut tx = db.begin_read().unwrap();
    let mut cursor = None;
    let mut got = BTreeMap::new();
    loop {
        let p = tx
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
        got.extend(p.rows.iter().cloned());
        cursor = p.next_after.clone();
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(&got, map);
}
#[test]
#[ignore = "explicit full-scan copy performance experiment"]
fn full_scan_copy_benchmark() {
    let dir = PathBuf::from(std::env::var_os("CATDB_COPY_OUTPUT").unwrap());
    std::fs::create_dir(&dir).unwrap();
    let n: u64 = std::env::var("CATDB_COPY_ROWS").unwrap().parse().unwrap();
    assert!([100000, 1000000].contains(&n));
    let shape = std::env::var("CATDB_COPY_SHAPE").unwrap();
    assert!(["plain", "mixed"].contains(&shape.as_str()));
    let limits = ResourceLimits {
        max_retained_bytes: 512 * 1024 * 1024,
        max_buffer_bytes: 512 * 1024 * 1024,
        ..ResourceLimits::default()
    };
    let path = dir.join("db.wal");
    let db = Database::create_with_limits(&path, [100; 16], limits).unwrap();
    let mut original = BTreeMap::new();
    for start in (0..n).step_by(4096) {
        let mut tx = db.begin().unwrap();
        for i in start..(start + 4096).min(n) {
            tx.put(&key(i * 2), &key(i)).unwrap();
            original.insert(key(i * 2).to_vec(), key(i).to_vec());
        }
        tx.commit().unwrap();
    }
    let mut old = db.begin_read().unwrap();
    let mut reader = if shape == "mixed" {
        db.begin().unwrap()
    } else {
        db.begin_read().unwrap()
    };
    let mut visible = original.clone();
    let mut current = original.clone();
    let mut other = db.begin().unwrap();
    for i in 0..1000 {
        let k = key(i * 2);
        if i % 3 == 0 {
            other.delete(&k).unwrap();
            current.remove(k.as_slice());
        } else {
            other.put(&k, &key(i + n)).unwrap();
            current.insert(k.to_vec(), key(i + n).to_vec());
        }
    }
    other.commit().unwrap();
    if shape == "mixed" {
        for i in 0..10000 {
            let k = key((i * 7919 % n) * 2 + u64::from(i % 3 == 0));
            if i % 3 == 1 {
                reader.delete(&k).unwrap();
                visible.remove(k.as_slice());
            } else {
                reader.put(&k, &key(n + i)).unwrap();
                visible.insert(k.to_vec(), key(n + i).to_vec());
            }
        }
    }
    let want: Rows = visible.into_iter().collect();
    let mut out = std::io::BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("samples.csv"))
            .unwrap(),
    );
    writeln!(
        out,
        "operation,sample,ns,calls,bytes,buffer_current,buffer_peak,counted"
    )
    .unwrap();
    measure(
        &db,
        &mut out,
        "owned",
        || reader.scan_reserved(b"", None).unwrap(),
        |r| assert_eq!(r.0, want),
    );
    measure(
        &db,
        &mut out,
        "framed",
        || frames(&mut reader),
        |r| {
            let mut at = 0;
            for (k, v) in &want {
                let kl = u32::from_le_bytes(r.0[at..at + 4].try_into().unwrap()) as usize;
                let vl = u32::from_le_bytes(r.0[at + 4..at + 8].try_into().unwrap()) as usize;
                at += 8;
                assert_eq!(&r.0[at..at + kl], k);
                at += kl;
                assert_eq!(&r.0[at..at + vl], v);
                at += vl;
            }
            assert_eq!(at, r.0.len());
        },
    );
    measure(
        &db,
        &mut out,
        "shared",
        || shared(&mut reader),
        |r| check_shared(r, &want),
    );
    let mut index = 0;
    measure(
        &db,
        &mut out,
        "export",
        || {
            let file = dir.join(format!("export-{index}.ndb"));
            index += 1;
            db.export_values(&file, [106; 16], b"", None).unwrap();
            file
        },
        |file| {
            let mut snapshot = crate::Snapshot::open(file, [106; 16]).unwrap();
            let want: Vec<_> = current.values().collect();
            for start in (0..want.len()).step_by(4096) {
                let got = snapshot.read_page(start as u64, 4096).unwrap();
                assert_eq!(
                    got.iter().collect::<Vec<_>>(),
                    want[start..(start + 4096).min(want.len())]
                );
            }
        },
    );
    assert_eq!(
        old.scan(b"", None).unwrap(),
        original.into_iter().collect::<Rows>()
    );
    expected(&db, &current);
    drop(reader);
    drop(old);
    drop(db);
    let db = Database::open_with_limits(&path, limits).unwrap();
    expected(&db, &current);
    out.flush().unwrap();
    println!("verified {n} {shape}: all pages, old values, committed state and reopen");
}

fn measure<T>(
    db: &Database,
    out: &mut impl Write,
    op: &str,
    mut f: impl FnMut() -> T,
    check: impl Fn(&T),
) {
    for sample in 0..8 {
        assert_eq!(
            db.resource_usage().unwrap().current[crate::resources::BUFFER],
            0
        );
        meter::start();
        let start = Instant::now();
        let result = f();
        let ns = start.elapsed().as_nanos();
        let (calls, bytes) = meter::stop();
        let usage = db.resource_usage().unwrap();
        check(&result);
        writeln!(
            out,
            "{op},{sample},{ns},{calls},{bytes},{},{},{}",
            usage.current[crate::resources::BUFFER],
            usage.peak[crate::resources::BUFFER],
            meter::counted()
        )
        .unwrap();
        drop(result);
        assert_eq!(
            db.resource_usage().unwrap().current[crate::resources::BUFFER],
            0
        );
    }
}
// BEGIN ADAPTER
type Shared = SharedScan;
fn shared(tx: &mut Transaction) -> Shared {
    tx.scan_shared(b"", None, true).unwrap()
}
fn check_shared(r: &Shared, want: &Rows) {
    assert!(
        r.iter()
            .eq(want.iter().map(|(k, v)| (k.as_slice(), v.as_slice())))
    );
}
fn frames(tx: &mut Transaction) -> (Vec<u8>, BufferPermit) {
    let mut scan = tx.scan_shared(b"", None, true).unwrap();
    let length = scan.iter().map(|(k, v)| 8 + k.len() + v.len()).sum();
    scan.permit.add(length).unwrap();
    let mut bytes = Vec::with_capacity(length);
    for (k, v) in scan.iter() {
        bytes.extend_from_slice(&(k.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(v.len() as u32).to_le_bytes());
        bytes.extend_from_slice(k);
        bytes.extend_from_slice(v);
    }
    (bytes, scan.permit)
}
// END ADAPTER
