use super::*;
use crate::transactions::{CheckpointStage, Database, ResourceLimits, ScanLimits, Transaction};
use std::os::unix::fs::MetadataExt;

fn memory() -> (u64, u64) {
    let text = fs::read_to_string("/proc/self/status").unwrap();
    let find = |name: &str| {
        text.lines()
            .find(|l| l.starts_with(name))
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap()
            .parse::<u64>()
            .unwrap()
            * 1024
    };
    (find("VmRSS:"), find("VmHWM:"))
}
fn changes(i: usize, n: usize) -> WriteSet {
    let k = (i * 7919) % n;
    let mut w = WriteSet::new();
    w.insert(key(k), Some(value(k, (i + 1) as u64)));
    if i % 3 == 0 {
        w.insert(key((i * 3571 + 7) % n), None);
    }
    w.insert(key(n + i), Some(value(n + i, (i + 1) as u64)));
    if i > 0 {
        w.insert(key(n + i - 1), None);
    }
    w
}
fn oracle(n: usize, ops: usize) -> BTreeMap<usize, Option<Vec<u8>>> {
    let mut o = BTreeMap::new();
    for i in 0..ops {
        for (k, v) in changes(i, n) {
            o.insert(u64::from_be_bytes(k[..8].try_into().unwrap()) as usize, v);
        }
    }
    o
}
fn wanted(n: usize, o: &BTreeMap<usize, Option<Vec<u8>>>) -> usize {
    n + o.iter().filter(|(k, v)| **k >= n && v.is_some()).count()
        - o.iter().filter(|(k, v)| **k < n && v.is_none()).count()
}
fn check_row(
    k: &[u8],
    v: &[u8],
    n: usize,
    o: &BTreeMap<usize, Option<Vec<u8>>>,
    previous: &mut Option<Vec<u8>>,
) {
    if let Some(p) = previous.as_ref() {
        assert!(p.as_slice() < k);
    }
    *previous = Some(k.to_vec());
    let i = u64::from_be_bytes(k[..8].try_into().unwrap()) as usize;
    assert_eq!(k, key(i));
    if let Some(expected) = o.get(&i) {
        assert_eq!(Some(v), expected.as_deref());
    } else {
        assert!(i < n);
        assert_eq!(v, value(i, 0));
    }
}
fn check_data(rows: &Data, n: usize, o: &BTreeMap<usize, Option<Vec<u8>>>) {
    assert_eq!(rows.len(), wanted(n, o));
    let mut previous = None;
    for (k, v) in rows {
        check_row(k, v, n, o, &mut previous);
    }
}
fn check_tx(tx: &mut Transaction, n: usize, o: &BTreeMap<usize, Option<Vec<u8>>>) {
    let mut after = None;
    let mut count = 0;
    let mut previous = None;
    loop {
        let page = tx
            .scan_page(
                b"",
                None,
                after.as_deref(),
                ScanLimits {
                    max_rows: 1024,
                    max_bytes: PAGE,
                    max_visits: 65536,
                },
            )
            .unwrap();
        for (k, v) in &page.rows {
            check_row(k, v, n, o, &mut previous);
            count += 1;
        }
        after = page.next_after.clone();
        if after.is_none() {
            break;
        }
    }
    assert_eq!(count, wanted(n, o));
}

#[test]
#[ignore = "explicit staged large-data comparison"]
fn compare() {
    assert_eq!(
        fs::read_to_string("/sys/fs/cgroup/memory.max")
            .unwrap()
            .trim(),
        "4294967296"
    );
    assert_eq!(
        fs::read_to_string("/sys/fs/cgroup/memory.swap.max")
            .unwrap()
            .trim(),
        "0"
    );
    let cpu = fs::read_to_string("/sys/fs/cgroup/cpu.max").unwrap();
    let cpu: Vec<u64> = cpu.split_whitespace().map(|x| x.parse().unwrap()).collect();
    assert_eq!(cpu[0], 2 * cpu[1]);
    let n: usize = std::env::var("SCALED_ROWS").unwrap().parse().unwrap();
    let ops: usize = std::env::var("SCALED_OPS").unwrap().parse().unwrap();
    let cap: u32 = std::env::var("SCALED_CAP").unwrap().parse().unwrap();
    let mode = std::env::var("SCALED_VARIANT").unwrap();
    assert!(matches!(
        mode.as_str(),
        "wal"
            | "prefix-on" | "prefix-off"
            | "dual"
            | "dirty"
            | "mvcc"
            | "direct"
            | "packed"
            | "reuse"
            | "linear"
            | "combine"
            | "logreuse"
            | "arrivals"
            | "patch"
            | "prewrite"
            | "defer"
    ));
    assert!(n >= 1000 && ops >= cap as usize * 3 && ops <= 16384);
    let cfg = Config {
        checkpoints: (n / 3000 + 4) as u32,
        logs: cap,
    };
    let p = dir().join("timed.db");
    let mut lat = Vec::with_capacity(ops);
    let start = Instant::now();
    let mut store = None;
    let mut db = None;
    let mut dirty = None;
    if matches!(
        mode.as_str(),
        "direct"
            | "packed"
            | "reuse"
            | "linear"
            | "combine"
            | "logreuse"
            | "arrivals"
            | "patch"
            | "prewrite"
            | "defer"
    ) {
        db = Some(MvccPages::create_direct(
            &p,
            cfg,
            (0..n).map(|i| (key(i), value(i, 0))),
            None,
        ));
    } else if mode == "mvcc" {
        db = Some(MvccPages::create(&p, cfg, data(n), None));
    } else if mode == "dirty" {
        dirty = Some(Dirty::create(&p, cfg, data(n)).unwrap());
    } else if mode == "dual" {
        store = Some(Store::create(&p, cfg, data(n), BUFFER_BUDGET, &mut noop).unwrap());
    } else {
        let limits = ResourceLimits {
            max_transactions: 64,
            max_staged_bytes: 32 * PAGE,
            max_dependency_bytes: 32 * PAGE,
            max_retained_bytes: 2 * 1024 * PAGE,
            max_buffer_bytes: 64 * PAGE,
        };
        let d = Database::create_with_limits(&p, [87; 16], limits).unwrap();
        if !mode.starts_with("prefix-") {d.set_group_commit(false);}
        for begin in (0..n).step_by(1000) {
            let mut tx = d.begin().unwrap();
            for i in begin..(begin + 1000).min(n) {
                tx.put(&key(i), &value(i, 0)).unwrap();
            }
            tx.commit().unwrap();
        }
        d.checkpoint().unwrap();
        db = Some(d);
    }
    let init_ns = start.elapsed().as_nanos();
    let pin = Instant::now();
    let pinned = dirty.as_ref().map(|s| s.pin().unwrap());
    let old_map = store.as_ref().map(Store::snapshot);
    let mut old_tx = db.as_ref().map(|d| d.begin_read().unwrap());
    let pin_ns = pin.elapsed().as_nanos();
    if std::env::var_os("SCALED_FOUR").is_some() {
        let backend = if let Some(d) = db {
            Backend::Wal(d)
        } else if let Some(s) = store {
            Backend::Full(s)
        } else {
            Backend::Pages(dirty.unwrap())
        };
        return four_run(backend, &p, cfg, &mode, n, ops, old_tx, old_map, pinned);
    }
    let mem_before = memory();
    let mut wal_written = 0_u64;
    let mut expected_wal = db.as_ref().map_or(0, |d| d.state().unwrap().wal_bytes);
    let before_sync = db.as_ref().map_or(0, Database::commit_sync_attempts);
    let mut checkpoint_syncs = 0;
    let mut transitions = 0;
    let control = store
        .as_ref()
        .or_else(|| dirty.as_ref().map(|s| &s.control));
    let dual_bytes_before = control
        .as_ref()
        .map_or(0, |s| s.costs.writes.values().sum::<u64>());
    let dual_sync_before = control
        .as_ref()
        .map_or(0, |s| s.costs.syncs.values().sum::<u64>());
    let begin = Instant::now();
    for i in 0..ops {
        let t = Instant::now();
        let mut switch_ns = 0;
        if i > 0 && i % (cap as usize) == 0 {
            let sw = Instant::now();
            if let Some(s) = dirty.as_mut() {
                s.switch(&mut noop).unwrap();
            } else if let Some(s) = store.as_mut() {
                s.switch(&mut noop).unwrap();
            } else {
                let cp = db
                    .as_ref()
                    .unwrap()
                    .checkpoint_impl(false, None, |stage| {
                        if matches!(stage, CheckpointStage::Synced | CheckpointStage::Published) {
                            checkpoint_syncs += 1;
                        }
                        Ok(())
                    })
                    .unwrap();
                expected_wal = cp.after_bytes;
                wal_written += cp.after_bytes + super::super::super::HEADER_BYTES as u64;
            }
            switch_ns = sw.elapsed().as_nanos();
            transitions += 1;
        }
        let w = changes(i, n);
        let ap = Instant::now();
        if let Some(s) = dirty.as_mut() {
            s.append(&w, &mut noop).unwrap();
        } else if let Some(s) = store.as_mut() {
            s.append(&w, &mut noop).unwrap();
        } else {
            let mut tx = db.as_ref().unwrap().begin().unwrap();
            for (k, v) in &w {
                if let Some(v) = v {
                    tx.put(k, v).unwrap();
                } else {
                    tx.delete(k).unwrap();
                }
            }
            tx.commit().unwrap();
            let bytes = frame_size(&w).unwrap() as u64;
            wal_written += bytes;
            expected_wal += bytes;
        }
        lat.push((
            i,
            t.elapsed().as_nanos(),
            ap.elapsed().as_nanos(),
            switch_ns,
        ));
    }
    let elapsed = begin.elapsed().as_nanos();
    let mem_after = memory();
    let control = store
        .as_ref()
        .or_else(|| dirty.as_ref().map(|s| &s.control));
    let io_bytes = control.map_or(wal_written, |s| {
        s.costs.writes.values().sum::<u64>() - dual_bytes_before
    });
    let syncs = if let Some(s) = control {
        s.costs.syncs.values().sum::<u64>() - dual_sync_before
    } else {
        db.as_ref().unwrap().commit_sync_attempts() - before_sync + checkpoint_syncs
    };
    assert_eq!(
        syncs,
        ops as u64 + (if mode == "dirty" { 3 } else { 2 }) * transitions
    );
    if let Some(d) = db.as_ref() {
        assert_eq!(d.state().unwrap().wal_bytes, expected_wal);
    }
    let final_count;
    let expected = oracle(n, ops);
    if let Some(s) = dirty.as_ref() {
        check_data(&s.rows, n, &expected);
        check_data(&pinned.as_ref().unwrap().rows, n, &BTreeMap::new());
        final_count = s.rows.len();
    } else if let Some(s) = store.as_ref() {
        check_data(&s.rows, n, &expected);
        check_data(old_map.as_ref().unwrap(), n, &BTreeMap::new());
        final_count = s.rows.len();
    } else {
        check_tx(
            &mut db.as_ref().unwrap().begin_read().unwrap(),
            n,
            &expected,
        );
        check_tx(old_tx.as_mut().unwrap(), n, &BTreeMap::new());
        final_count = wanted(n, &expected);
    }
    let meta = fs::metadata(&p).unwrap();
    let pool = dirty.as_ref().map(|s| s.pool.metadata().unwrap());
    let file_size = meta.len() + pool.as_ref().map_or(0, |m| m.len());
    let allocated = (meta.blocks() + pool.as_ref().map_or(0, |m| m.blocks())) * 512;
    drop(pinned);
    drop(dirty);
    drop(old_map);
    drop(old_tx);
    drop(store);
    drop(db);
    let t = Instant::now();
    let reopen_ns;
    let recovery_sync;
    if mode == "dirty" {
        let s = Dirty::open(&p, cfg, &mut noop).unwrap();
        reopen_ns = t.elapsed().as_nanos();
        recovery_sync = s.control.costs.syncs.values().sum();
        check_data(&s.rows, n, &expected);
    } else if mode == "dual" {
        let s = Store::open(&p, cfg, BUFFER_BUDGET, &mut noop).unwrap();
        reopen_ns = t.elapsed().as_nanos();
        recovery_sync = s.costs.syncs.values().sum::<u64>();
        check_data(&s.rows, n, &expected);
    } else {
        let d = Database::open(&p).unwrap();
        reopen_ns = t.elapsed().as_nanos();
        recovery_sync = 0;
        check_tx(&mut d.begin_read().unwrap(), n, &expected);
    }
    println!(
        "SCALED_SUMMARY,{mode},{n},{ops},{cap},{elapsed},{init_ns},{pin_ns},{io_bytes},{syncs},{transitions},{file_size},{allocated},{},{},{},{},{reopen_ns},{recovery_sync},{final_count},true",
        mem_before.0, mem_before.1, mem_after.0, mem_after.1
    );
    for (i, total, append, switch) in lat {
        println!("SCALED_REQUEST,{mode},{n},{i},{total},{append},{switch}");
    }
}

include!("four.rs");
include!("four_checks.rs");

include!("mvcc_pages.rs");
include!("mvcc_checks.rs");

include!("direct_pages.rs");
include!("direct_checks.rs");

const PACK_SCRATCH: usize = 8 * BLOCK + MAX_OPS * 192;
include!("pack.rs");
include!("pack_checks.rs");

include!("linear_checks.rs");

include!("combine_checks.rs");

include!("peer_case.rs");

include!("arrivals.rs");
include!("arrival_checks.rs");

include!("density.rs");
include!("patch_checks.rs");
include!("prewrite_checks.rs");
