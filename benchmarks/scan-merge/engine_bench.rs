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
#[ignore = "explicit full-scan merge performance experiment"]
fn full_scan_merge_benchmark() {
    let dir = PathBuf::from(std::env::var_os("CATDB_SCAN_OUTPUT").unwrap());
    std::fs::create_dir(&dir).unwrap();
    let n: u64 = std::env::var("CATDB_SCAN_ROWS").unwrap().parse().unwrap();
    assert!([100000, 1000000].contains(&n));
    let shape = std::env::var("CATDB_SCAN_SHAPE").unwrap();
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
        "sample,ns,calls,bytes,buffer_current,buffer_peak,rows,counted"
    )
    .unwrap();
    for sample in 0..8 {
        assert_eq!(
            db.resource_usage().unwrap().current[crate::resources::BUFFER],
            0
        );
        meter::start();
        let start = Instant::now();
        let (rows, permit) = reader.scan_reserved(b"", None).unwrap();
        let ns = start.elapsed().as_nanos();
        let (calls, bytes) = meter::stop();
        let usage = db.resource_usage().unwrap();
        assert_eq!(rows, want);
        assert!(usage.current[crate::resources::BUFFER] <= limits.max_buffer_bytes);
        writeln!(
            out,
            "{sample},{ns},{calls},{bytes},{},{},{},{}",
            usage.current[crate::resources::BUFFER],
            usage.peak[crate::resources::BUFFER],
            rows.len(),
            meter::counted()
        )
        .unwrap();
        drop(rows);
        drop(permit);
        assert_eq!(
            db.resource_usage().unwrap().current[crate::resources::BUFFER],
            0
        );
    }
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
