#[test]
fn direct_dirty_ranges_split_delete_and_restore_empty_generation() {
    let path = dir().join("direct.bin");
    let cfg = four_config();
    let mut expected = data(1600);
    let db = MvccPages::create_direct(&path, cfg, (0..1600).map(|i| (key(i), value(i, 0))), None);
    let mut old = db.begin_read().unwrap();
    let mut tx = db.begin().unwrap();
    for i in 0..900 {
        tx.delete(&key(i)).unwrap();
        expected.remove(&key(i));
    }
    for i in [899, 900, 1000, 1599, 1600] {
        let v = vec![238; 16000];
        tx.put(&key(i), &v).unwrap();
        expected.insert(key(i), v.into());
    }
    tx.commit().unwrap();
    assert_eq!(db.checkpoint().unwrap().records, expected.len());
    assert_eq!(old.scan(b"", None).unwrap().len(), 1600);
    drop(old);
    drop(db);
    let db = MvccPages::open_direct(&path, cfg, None);
    let got: Data = db
        .begin_read()
        .unwrap()
        .scan(b"", None)
        .unwrap()
        .into_iter()
        .map(|(k, v)| (k, v.into()))
        .collect();
    assert_eq!(got, expected);
    let mut tx = db.begin().unwrap();
    for k in expected.keys() {
        tx.delete(k).unwrap();
    }
    let seq = tx.commit().unwrap();
    assert_eq!(db.checkpoint().unwrap().records, 0);
    drop(db);
    let db = MvccPages::open_direct(&path, cfg, None);
    assert_eq!(db.begin_read().unwrap().version(), seq);
    let mut tx = db.begin().unwrap();
    tx.put(b"", b"new").unwrap();
    tx.commit().unwrap();
    assert_eq!(db.checkpoint().unwrap().records, 1);
    drop(db);
    assert_eq!(
        MvccPages::open_direct(&path, cfg, None)
            .begin_read()
            .unwrap()
            .get(b"")
            .unwrap(),
        Some(b"new".to_vec())
    );
}
