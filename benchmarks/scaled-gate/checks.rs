use super::*;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn dir() -> PathBuf {
    let root = std::env::var_os("SCALED_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let p = root.join(format!(
        "scaled-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&p).unwrap();
    p
}
fn key(i: usize) -> Vec<u8> {
    let mut k = (i as u64).to_be_bytes().to_vec();
    if i % 2 == 1 {
        k.extend(0..24);
    }
    k
}
fn value(i: usize, g: u64) -> Vec<u8> {
    let mut v = vec![17; 256];
    v[..8].copy_from_slice(&(i as u64).to_be_bytes());
    v[8..16].copy_from_slice(&g.to_be_bytes());
    v
}
fn data(n: usize) -> Data {
    (0..n).map(|i| (key(i), Arc::from(value(i, 0)))).collect()
}
fn change() -> WriteSet {
    [
        (key(0), Some(vec![7; 8193])),
        (key(1), None),
        (key(90000), Some(vec![])),
    ]
    .into()
}
fn short() -> WriteSet {
    [
        (key(0), Some(vec![8])),
        (key(1), Some(vec![9; 33])),
        (key(90000), None),
    ]
    .into()
}
fn config() -> Config {
    Config {
        checkpoints: 4,
        logs: 4,
    }
}
fn fixture() -> (PathBuf, Store) {
    let p = dir().join("store.bin");
    let mut s = Store::create(&p, config(), data(5000), BUFFER_BUDGET, &mut noop).unwrap();
    assert!(s.root.chunks >= 2);
    for _ in 0..2 {
        s.append(&change(), &mut noop).unwrap();
        s.switch(&mut noop).unwrap();
    }
    (p, s)
}
fn overwrite(p: &Path, offset: u64, b: &[u8]) {
    let mut f = OpenOptions::new().write(true).open(p).unwrap();
    f.seek(SeekFrom::Start(offset)).unwrap();
    f.write_all(b).unwrap();
    f.sync_data().unwrap();
}

#[test]
fn eq_st_segmented_checkpoint_updates_old_snapshots_and_reuse() {
    let (p, mut s) = fixture();
    let old = s.snapshot();
    let expected_old = old.clone();
    assert!(Store::create(&p, config(), Data::new(), BUFFER_BUDGET, &mut noop).is_err());
    assert!(Store::open(&p, config(), BUFFER_BUDGET, &mut noop).is_err());
    for round in 0..4 {
        for _ in 0..4 {
            s.append(&if round % 2 == 0 { short() } else { change() }, &mut noop)
                .unwrap();
        }
        let length = s.file.metadata().unwrap().len();
        let seq = s.seq;
        assert!(s.append(&change(), &mut noop).is_err());
        s.append(&WriteSet::new(), &mut noop).unwrap();
        assert_eq!(s.seq, seq);
        s.switch(&mut noop).unwrap();
        assert_eq!(s.file.metadata().unwrap().len(), length);
        let expected = s.snapshot();
        drop(s);
        s = Store::open(&p, config(), BUFFER_BUDGET, &mut noop).unwrap();
        assert_eq!(s.rows, expected);
        assert!(s.costs.writes.is_empty());
    }
    assert_eq!(old, expected_old);
}

#[test]
fn bv_frames_budget_capacity_empty_and_generation_bounds() {
    let p = dir().join("empty.bin");
    assert!(Store::create(&p, config(), Data::new(), BUFFER_BUDGET - 1, &mut noop).is_err());
    assert!(!p.exists());
    let mut s = Store::create(&p, config(), Data::new(), BUFFER_BUDGET, &mut noop).unwrap();
    assert_eq!(s.root.chunks, 0);
    let n = PAYLOAD - (PREFIX_BYTES + 12 + TRAILER_BYTES) - 10;
    let fit: WriteSet = [(vec![0], Some(vec![1; n]))].into();
    assert_eq!(frame_size(&fit).unwrap(), PAYLOAD);
    let over: WriteSet = [(vec![0], Some(vec![1; n + 1]))].into();
    assert!(s.append(&over, &mut noop).is_err());
    s.append(&fit, &mut noop).unwrap();
    s.switch(&mut noop).unwrap();
    let expected = s.snapshot();
    drop(s);
    let mut s = Store::open(&p, config(), BUFFER_BUDGET, &mut noop).unwrap();
    assert_eq!(s.rows, expected);
    let b = fs::read(&p).unwrap();
    s.root.epoch = u64::MAX;
    assert!(s.switch(&mut noop).is_err());
    s.seq = u64::MAX;
    assert!(s.append(&short(), &mut noop).is_err());
    assert_eq!(fs::read(&p).unwrap(), b);
    drop(s);
    assert!(
        Store::open(
            &p,
            Config {
                checkpoints: 3,
                logs: 5
            },
            BUFFER_BUDGET,
            &mut noop
        )
        .is_err()
    );
    assert_eq!(fs::read(&p).unwrap(), b);
    let q = dir().join("too-small.bin");
    assert!(
        Store::create(
            &q,
            Config {
                checkpoints: 1,
                logs: 1
            },
            data(5000),
            BUFFER_BUDGET,
            &mut noop
        )
        .is_err()
    );
    assert!(!q.exists());
    let mut s = Store::create(
        &q,
        Config {
            checkpoints: 1,
            logs: 1,
        },
        Data::new(),
        BUFFER_BUDGET,
        &mut noop,
    )
    .unwrap();
    let w: WriteSet = [(vec![1; MAX_KEY_BYTES + 1], Some(vec![]))].into();
    assert!(s.append(&w, &mut noop).is_err());
    let excessive: WriteSet = (0..MAX_OPS + 1).map(|i| (key(i), Some(vec![]))).collect();
    assert!(s.append(&excessive, &mut noop).is_err());
    let forged = Record {
        kind: 3,
        payload: encode_frame(s.seq, &excessive),
        ..s.root.clone()
    };
    assert!(forged.writes().is_err());
    for n in [0, 14, 15, 16, 17, MAX_KEY_BYTES] {
        let w: WriteSet = [(vec![255; n], Some(vec![0; 2]))].into();
        s.append(&w, &mut noop).unwrap();
        s.switch(&mut noop).unwrap();
    }
}

#[test]
fn er_manifest_missing_swapped_chunks_and_disagreeing_copies_reject() {
    let (p, s) = fixture();
    let root = s.root.clone();
    drop(s);
    let before = fs::read(&p).unwrap();
    for lane in 0..2 {
        overwrite(&p, config().at(root.bank, true, 1, lane), &[0; HEAD]);
    }
    assert!(Store::open(&p, config(), BUFFER_BUDGET, &mut noop).is_err());
    fs::write(&p, &before).unwrap();
    for lane in 0..2 {
        let at = config().at(root.bank, true, 0, lane) as usize;
        overwrite(
            &p,
            config().at(root.bank, true, 1, lane),
            &before[at..at + PAGE],
        );
    }
    assert!(Store::open(&p, config(), BUFFER_BUDGET, &mut noop).is_err());
    fs::write(&p, &before).unwrap();
    let mut f = OpenOptions::new().read(true).open(&p).unwrap();
    let mut r = read_record(
        &mut f,
        &mut Costs::default(),
        config().at(root.bank, true, 0, 0),
    )
    .unwrap()
    .unwrap();
    let mut w = r.writes().unwrap();
    w.insert(key(0), Some(b"disagreement".to_vec()));
    r.payload = encode_frame(r.seq, &w);
    overwrite(&p, config().at(root.bank, true, 0, 0), &r.encode());
    assert!(Store::open(&p, config(), BUFFER_BUDGET, &mut noop).is_err());
}

#[test]
fn er_partial_append_model_and_both_copy_loss_counterexample() {
    let (p, mut s) = fixture();
    s.append(&change(), &mut noop).unwrap();
    s.switch(&mut noop).unwrap();
    let old = s.snapshot();
    let root = s.root.clone();
    let seq = s.seq;
    let chain = s.chain;
    let r = Record {
        kind: 3,
        seq: seq + 1,
        index: 0,
        previous: chain,
        payload: encode_frame(seq + 1, &short()),
        ..root.clone()
    };
    let bytes = r.encode();
    let mut expected = old.clone();
    apply(&mut expected, short());
    drop(s);
    let before = fs::read(&p).unwrap();
    let cuts = [
        0,
        1,
        47,
        HEAD - 1,
        HEAD,
        HEAD + 1,
        bytes.len() - 1,
        bytes.len(),
    ];
    for x in cuts {
        for y in cuts {
            fs::write(&p, &before).unwrap();
            for (lane, n) in [(0, x), (1, y)] {
                overwrite(&p, config().at(root.bank, false, 0, lane), &bytes[..n]);
            }
            let s = Store::open(&p, config(), BUFFER_BUDGET, &mut noop).unwrap();
            assert!(s.rows == old || s.rows == expected);
            if x == bytes.len() || y == bytes.len() {
                assert_eq!(s.rows, expected);
            }
        }
    }
    fs::write(&p, &before).unwrap();
    let mut s = Store::open(&p, config(), BUFFER_BUDGET, &mut noop).unwrap();
    s.append(&short(), &mut noop).unwrap();
    drop(s);
    for lane in 0..2 {
        overwrite(&p, config().at(root.bank, false, 0, lane), &[0; HEAD]);
    }
    let s = Store::open(&p, config(), BUFFER_BUDGET, &mut noop).unwrap();
    assert_eq!(s.rows, old);
    assert_ne!(s.rows, expected);
    println!("SCALED_MODEL,64,both-copy-loss-outside-contract");
}

#[test]
fn er_one_copy_repair_is_bounded_idempotent_and_failure_keeps_peer() {
    let (p, mut s) = fixture();
    s.append(&short(), &mut noop).unwrap();
    let expected = s.snapshot();
    let root = s.root.clone();
    drop(s);
    for (checkpoint, index, lane) in [(true, 0, 0), (true, 1, 1), (false, 0, 0)] {
        overwrite(
            &p,
            config().at(root.bank, checkpoint, index, lane),
            &[0; HEAD],
        );
    }
    overwrite(&p, root_at(root.epoch, 1), &[0; HEAD]);
    let mut hit = false;
    assert!(
        Store::open(&p, config(), BUFFER_BUDGET, &mut |name| {
            if name == "repair-data.before-sync" {
                hit = true;
                Err(err("injected repair sync failure"))
            } else {
                Ok(())
            }
        })
        .is_err()
    );
    assert!(hit);
    let s = Store::open(&p, config(), BUFFER_BUDGET, &mut noop).unwrap();
    assert_eq!(s.rows, expected);
    drop(s);
    let s = Store::open(&p, config(), BUFFER_BUDGET, &mut noop).unwrap();
    assert!(s.costs.writes.is_empty());
    assert_eq!(s.rows, expected);
}

#[test]
fn er_failed_sync_does_not_publish_and_requires_reopen() {
    for target in ["append-0.half", "append.before-sync", "append.synced"] {
        let (p, mut s) = fixture();
        let old = s.snapshot();
        let mut expected = old.clone();
        apply(&mut expected, short());
        let mut hit = false;
        assert!(
            s.append(&short(), &mut |name| {
                if name == target {
                    hit = true;
                    Err(err("injected persistence failure"))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        assert!(hit);
        assert_eq!(s.rows, old);
        assert!(s.append(&short(), &mut noop).is_err());
        drop(s);
        let s = Store::open(&p, config(), BUFFER_BUDGET, &mut noop).unwrap();
        assert!(s.rows == old || s.rows == expected);
        if target == "append.synced" {
            assert_eq!(s.rows, expected);
        }
    }
}

#[test]
fn er_failed_switch_reopen_updates_and_retry_preserve_committed_state() {
    for target in [
        "chunk-0-0.half",
        "checkpoint.before-sync",
        "checkpoint.synced",
        "selector.before-sync",
        "selector.synced",
    ] {
        let (p, mut s) = fixture();
        s.append(&short(), &mut noop).unwrap();
        let expected = s.snapshot();
        let mut hit = false;
        assert!(
            s.switch(&mut |name| {
                if name == target {
                    hit = true;
                    Err(err("injected switch failure"))
                } else {
                    Ok(())
                }
            })
            .is_err()
        );
        assert!(hit);
        assert_eq!(s.rows, expected);
        assert!(s.append(&change(), &mut noop).is_err());
        drop(s);
        let mut s = Store::open(&p, config(), BUFFER_BUDGET, &mut noop).unwrap();
        assert_eq!(s.rows, expected);
        s.append(&change(), &mut noop).unwrap();
        s.switch(&mut noop).unwrap();
        let expected = s.snapshot();
        drop(s);
        assert_eq!(
            Store::open(&p, config(), BUFFER_BUDGET, &mut noop)
                .unwrap()
                .rows,
            expected
        );
    }
}

#[test]
fn bv_checkpoint_growth_refusal_preserves_values_and_can_recover() {
    let p = dir().join("growth.bin");
    let cfg = Config {
        checkpoints: 1,
        logs: 4,
    };
    let mut s = Store::create(&p, cfg, Data::new(), BUFFER_BUDGET, &mut noop).unwrap();
    for byte in [1, 2] {
        s.append(&[(vec![byte], Some(vec![byte; 700000]))].into(), &mut noop)
            .unwrap();
    }
    let old = s.snapshot();
    let before = fs::read(&p).unwrap();
    assert!(s.switch(&mut noop).is_err());
    assert!(!s.poisoned);
    assert_eq!(s.rows, old);
    assert_eq!(fs::read(&p).unwrap(), before);
    s.append(&[(vec![2], None)].into(), &mut noop).unwrap();
    s.switch(&mut noop).unwrap();
    let expected = s.snapshot();
    drop(s);
    assert_eq!(
        Store::open(&p, cfg, BUFFER_BUDGET, &mut noop).unwrap().rows,
        expected
    );
}

#[test]
fn er_selector_prefix_models_and_reordered_checkpoint_reject_partial_generation() {
    let (p, mut s) = fixture();
    s.append(&short(), &mut noop).unwrap();
    let expected = s.snapshot();
    let old = fs::read(&p).unwrap();
    s.switch(&mut noop).unwrap();
    let root = s.root.clone();
    let new = fs::read(&p).unwrap();
    drop(s);
    let cuts = [0, 1, 47, HEAD - 1, HEAD];
    for a in cuts {
        for b in cuts {
            let mut image = new.clone();
            for lane in 0..2 {
                let off = root_at(root.epoch, lane) as usize;
                image[off..off + HEAD].copy_from_slice(&old[off..off + HEAD]);
            }
            for (lane, n) in [(0, a), (1, b)] {
                let off = root_at(root.epoch, lane) as usize;
                image[off..off + n].copy_from_slice(&new[off..off + n]);
            }
            fs::write(&p, &image).unwrap();
            assert_eq!(
                Store::open(&p, config(), BUFFER_BUDGET, &mut noop)
                    .unwrap()
                    .rows,
                expected
            );
        }
    }
    let mut image = old.clone();
    for lane in 0..2 {
        let off = root_at(root.epoch, lane) as usize;
        image[off..off + HEAD].copy_from_slice(&new[off..off + HEAD]);
    }
    fs::write(&p, &image).unwrap();
    assert!(Store::open(&p, config(), BUFFER_BUDGET, &mut noop).is_err());
    println!("SCALED_SELECTOR_MODEL,25,unsynced-data-root-rejected");
}

#[test]
fn er_header_payload_reordering_and_bounded_streaming_recovery() {
    let (p, mut s) = fixture();
    let old = s.snapshot();
    let root = s.root.clone();
    let seq = s.seq + 1;
    let r = Record {
        kind: 3,
        seq,
        index: 0,
        previous: s.chain,
        payload: encode_frame(seq, &short()),
        ..root.clone()
    };
    let bytes = r.encode();
    let mut expected = old.clone();
    apply(&mut expected, short());
    s.append(&short(), &mut noop).unwrap();
    drop(s);
    let full = fs::read(&p).unwrap();
    for (at, len) in [
        (0, HEAD),
        (HEAD, bytes.len() - HEAD),
        (HEAD + 20, bytes.len() - HEAD - 20),
    ] {
        for complete_peer in [false, true] {
            let mut image = full.clone();
            for lane in 0..2 {
                let off = config().at(root.bank, false, 0, lane) as usize;
                image[off..off + bytes.len()].fill(0);
            }
            for lane in 0..2 {
                let off = config().at(root.bank, false, 0, lane) as usize;
                if lane == 1 && complete_peer {
                    image[off..off + bytes.len()].copy_from_slice(&bytes);
                } else {
                    image[off + at..off + at + len].copy_from_slice(&bytes[at..at + len]);
                }
            }
            fs::write(&p, &image).unwrap();
            let s = Store::open(&p, config(), BUFFER_BUDGET, &mut noop).unwrap();
            if complete_peer {
                assert_eq!(s.rows, expected);
            } else {
                assert_eq!(s.rows, old);
            }
            assert!(s.costs.read_bytes < s.file.metadata().unwrap().len() / 2);
        }
    }
}

#[test]
#[ignore = "external crash barrier helper"]
fn crash_child() {
    let p = PathBuf::from(std::env::var_os("SCALED_PATH").unwrap());
    let ready = p.with_extension("ready");
    let target = std::env::var("SCALED_POINT").unwrap();
    let mode = std::env::var("SCALED_MODE").unwrap();
    let mut hook = |n: &str| {
        if n == target {
            fs::write(&ready, n)?;
            loop {
                std::thread::park();
            }
        }
        Ok(())
    };
    let mut s = Store::open(&p, config(), BUFFER_BUDGET, &mut hook).unwrap();
    if mode == "append" {
        s.append(&short(), &mut hook).unwrap();
    } else if mode == "switch" {
        s.switch(&mut hook).unwrap();
    }
}

#[test]
fn er_external_exit_checkpoint_publication_append_and_repair() {
    for (mode, points) in [
        (
            "append",
            vec![
                "append-0.half",
                "append-0.written",
                "append-1.half",
                "append.before-sync",
                "append.synced",
                "append.published",
            ],
        ),
        (
            "switch",
            vec![
                "chunk-0-0.half",
                "chunk-1-1.written",
                "checkpoint.before-sync",
                "checkpoint.synced",
                "selector-0.half",
                "selector-0.written",
                "selector-1.half",
                "selector.before-sync",
                "selector.synced",
                "switch.published",
            ],
        ),
        (
            "repair",
            vec![
                "repair-chunk-0-0.half",
                "repair-data.before-sync",
                "repair-data.synced",
                "repair-selector-1.half",
                "repair-selector.before-sync",
                "repair-selector.synced",
                "recovery.before-sync",
                "recovery.synced",
                "open.ready",
            ],
        ),
    ] {
        for point in points {
            let (p, mut s) = fixture();
            if mode != "append" {
                s.append(&short(), &mut noop).unwrap();
            }
            let old = s.snapshot();
            let root = s.root.clone();
            let mut expected = old.clone();
            apply(&mut expected, short());
            drop(s);
            if mode == "repair" {
                overwrite(&p, config().at(root.bank, true, 0, 0), &[0; HEAD]);
                overwrite(&p, root_at(root.epoch, 1), &[0; HEAD]);
            }
            let ready = p.with_extension("ready");
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "transactions::scaled_gate::checks::crash_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("SCALED_PATH", &p)
                .env("SCALED_POINT", point)
                .env("SCALED_MODE", mode)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap();
            let until = Instant::now() + Duration::from_secs(20);
            loop {
                if fs::read_to_string(&ready).ok().as_deref() == Some(point) {
                    break;
                }
                if Instant::now() > until {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("barrier timeout {point}");
                }
                assert!(child.try_wait().unwrap().is_none(), "child exited {point}");
                std::thread::sleep(Duration::from_millis(5));
            }
            child.kill().unwrap();
            assert!(!child.wait().unwrap().success());
            let mut s = Store::open(&p, config(), BUFFER_BUDGET, &mut noop).unwrap();
            assert!(s.rows == old || (mode == "append" && s.rows == expected));
            if point == "append.synced" || point == "append.published" {
                assert_eq!(s.rows, expected);
            }
            s.switch(&mut noop).unwrap();
            s.append(&change(), &mut noop).unwrap();
            let after = s.snapshot();
            drop(s);
            assert_eq!(
                Store::open(&p, config(), BUFFER_BUDGET, &mut noop)
                    .unwrap()
                    .rows,
                after
            );
            println!("SCALED_KILL,{mode},{point}");
        }
    }
}

#[path = "timing.rs"]
mod timing;

#[test]
fn er_identifiable_misplacement_uses_the_intact_peer() {
    for case in ["chunk", "log", "selector"] {
        let (p, mut s) = fixture();
        s.append(&change(), &mut noop).unwrap();
        s.append(&short(), &mut noop).unwrap();
        let expected = s.snapshot();
        let root = s.root.clone();
        drop(s);
        let image = fs::read(&p).unwrap();
        let (from, to, len) = match case {
            "chunk" => (
                config().at(root.bank, true, 1, 0),
                config().at(root.bank, true, 0, 0),
                PAGE,
            ),
            "log" => (
                config().at(root.bank, false, 1, 0),
                config().at(root.bank, false, 0, 0),
                PAGE,
            ),
            _ => (root_at(root.epoch - 1, 0), root_at(root.epoch, 1), HEAD),
        };
        overwrite(&p, to, &image[from as usize..from as usize + len]);
        let s = Store::open(&p, config(), BUFFER_BUDGET, &mut noop).unwrap();
        assert_eq!(s.rows, expected);
        assert!(!s.costs.writes.is_empty());
        drop(s);
        assert!(
            Store::open(&p, config(), BUFFER_BUDGET, &mut noop)
                .unwrap()
                .costs
                .writes
                .is_empty()
        );
    }
}

include!("dirty.rs");
include!("dirty_checks.rs");

include!("linear.rs");
