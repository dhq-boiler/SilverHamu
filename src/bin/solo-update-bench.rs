use silverhamu::transactions::{Database, Transaction};
use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Instant};

fn value(key: u64, stamp: u64) -> [u8; 256] {
    let mut v = [b'x'; 256];
    v[..8].copy_from_slice(&key.to_be_bytes());
    v[8..16].copy_from_slice(&stamp.to_be_bytes());
    v
}
fn verify(tx: &mut Transaction, rows: u64, changed: &BTreeMap<u64, u64>) {
    for k in 0..rows {
        assert_eq!(
            tx.get(&k.to_be_bytes()).unwrap().unwrap(),
            value(k, *changed.get(&k).unwrap_or(&0))
        );
    }
    assert_eq!(tx.get(&rows.to_be_bytes()).unwrap(), None);
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let plan: Value = serde_json::from_slice(&std::fs::read(&args[1]).unwrap()).unwrap();
    let grouped = args[2] == "on";
    let rows = plan["rows"].as_u64().unwrap();
    let operations: Vec<(u64, u64)> = serde_json::from_value(plan["operations"].clone()).unwrap();
    assert!(rows > 0 && !operations.is_empty() && operations.len() <= 8000);
    assert!(operations.iter().all(|(k, s)| *k < rows && *s > 0));
    let path = "/data/solo.wal";
    let db = Database::create(path, *b"catdb-solo-test1").unwrap();
    assert_eq!(db.begin().unwrap().get(&0_u64.to_be_bytes()).unwrap(), None);
    for start in (0..rows).step_by(1000) {
        let mut tx = db.begin().unwrap();
        for k in start..(start + 1000).min(rows) {
            tx.put(&k.to_be_bytes(), &value(k, 0)).unwrap();
        }
        tx.commit().unwrap();
    }
    db.checkpoint().unwrap();
    let mut initial = db.begin().unwrap();
    verify(&mut initial, rows, &BTreeMap::new());
    drop(initial);
    db.set_group_commit(grouped);
    let mut samples = Vec::with_capacity(operations.len());
    #[allow(unused_mut)]
    let mut diagnostics: Vec<Value> = Vec::with_capacity(if cfg!(feature = "lock-timing") {
        operations.len()
    } else {
        0
    });
    let sync_before = db.commit_sync_attempts();
    let wall = Instant::now();
    for &(k, stamp) in &operations {
        let start = Instant::now();
        #[cfg(feature = "lock-timing")]
        let entered = silverhamu::transactions::diagnostic_now_ns();
        let mut tx = db.begin().unwrap();
        tx.put(&k.to_be_bytes(), &value(k, stamp)).unwrap();
        #[cfg(feature = "lock-timing")]
        let commit = silverhamu::transactions::diagnostic_now_ns();
        tx.commit().unwrap();
        #[cfg(feature = "lock-timing")]
        let ended = silverhamu::transactions::diagnostic_now_ns();
        samples.push(start.elapsed().as_nanos() as u64);
        #[cfg(feature="lock-timing")]
        diagnostics.push(json!({"entered":entered,"commit":commit,"ended":ended,"trace":silverhamu::transactions::take_commit_trace(),"detail":silverhamu::transactions::take_commit_detail(),"sync_start":silverhamu::transactions::take_commit_sync_start()}));
    }
    let elapsed = wall.elapsed().as_secs_f64();
    let syncs = db.commit_sync_attempts() - sync_before;
    assert_eq!(syncs, operations.len() as u64);
    let mut changed: BTreeMap<_, _> = operations.into_iter().collect();
    let mut old = db.begin().unwrap();
    assert_eq!(
        old.get(&0_u64.to_be_bytes()).unwrap().unwrap(),
        value(0, *changed.get(&0).unwrap_or(&0))
    );
    let mut safety = db.begin().unwrap();
    safety
        .put(&0_u64.to_be_bytes(), &value(0, u64::MAX))
        .unwrap();
    safety.commit().unwrap();
    verify(&mut old, rows, &changed);
    changed.insert(0, u64::MAX);
    drop(old);
    let mut latest = db.begin().unwrap();
    verify(&mut latest, rows, &changed);
    drop(latest);
    drop(db);
    let reopened = Database::open(path).unwrap();
    verify(&mut reopened.begin().unwrap(), rows, &changed);
    println!(
        "{}",
        json!({"api":"rust","group":grouped,"diagnostic":cfg!(feature="lock-timing"),"rows":rows,"samples_ns":samples,"elapsed_s":elapsed,"sync_attempts":syncs,"checks":["initial","old","latest","reopen"],"diagnostics":diagnostics})
    );
}
