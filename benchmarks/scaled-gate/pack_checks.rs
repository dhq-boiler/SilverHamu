#[test]
fn packed_ranges_match_snapshot_across_split_and_tombstone_continuations() {
    let p = dir().join("pack.db");
    let db = MvccPages::create_direct(
        &p,
        four_config(),
        (0..1400).map(|i| (key(i), value(i, 0))),
        None,
    );
    let old = db.begin_read().unwrap();
    let mut t = db.begin().unwrap();
    for i in 0..1100 {
        t.delete(&key(i)).unwrap();
    }
    t.put(&key(1101), &vec![9; BLOCK - 140 - 9 - key(1101).len()])
        .unwrap();
    t.commit().unwrap();
    for mut reader in [old, db.begin_read().unwrap()] {
        for (lo, hi) in [
            (key(0), key(1300)),
            (key(12), key(12)),
            (key(1101), key(1102)),
        ] {
            let expected = reader.scan(&lo, Some(&hi)).unwrap();
            let mut emitted = Vec::new();
            MvccPages::pack_range(&reader, &lo, Some(&hi), &mut |start, w| {
                assert!(start <= *w.keys().next().unwrap());
                assert!(frame_size(w)? + HEAD <= BLOCK);
                emitted.extend(w.iter().map(|(k, v)| (k.clone(), v.clone().unwrap())));
                Ok(())
            })
            .unwrap();
            assert_eq!(emitted, expected);
        }
    }
}
#[test]
#[ignore = "explicit packed-mode budget boundary"]
fn packed_budget_refusal_preserves_updates_and_permit_reuse() {
    assert!(std::env::var_os("CATDB_TEST_PACK").is_some());
    let p = dir().join("budget.db");
    let db = MvccPages::create_direct(
        &p,
        four_config(),
        (0..16).map(|i| (key(i), value(i, 0))),
        None,
    );
    let mut t = db.begin().unwrap();
    t.put(b"k", b"v").unwrap();
    let version = t.commit().unwrap();
    let mut hold = db.inner.budget.buffer();
    hold.add(db.inner.budget.buffer_limit() - PACK_SCRATCH + 1)
        .unwrap();
    assert!(matches!(
        db.checkpoint(),
        Err(TxError::ResourceExhausted(_))
    ));
    assert_eq!(
        db.begin_read().unwrap().get(b"k").unwrap(),
        Some(b"v".to_vec())
    );
    hold.clear();
    hold.add(db.inner.budget.buffer_limit() - PACK_SCRATCH)
        .unwrap();
    assert_eq!(db.checkpoint().unwrap().version, version);
    drop(hold);
    drop(db);
    assert_eq!(
        MvccPages::open_direct(&p, four_config(), None)
            .begin_read()
            .unwrap()
            .get(b"k")
            .unwrap(),
        Some(b"v".to_vec())
    );
}
