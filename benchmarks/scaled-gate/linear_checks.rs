#[test]
fn linear_pages_are_byte_identical_across_generations_boundaries_and_splits() {
    let path = dir().join("linear.db");
    let db = MvccPages::create_direct(
        &path,
        four_config(),
        (0..1300).map(|i| (key(i), value(i, 0))),
        None,
    );
    let old = db.begin_read().unwrap();
    let mut edit = db.begin().unwrap();
    for i in 0..1026 {
        edit.delete(&key(i)).unwrap();
    }
    edit.put(&key(1026), &vec![0xff; BLOCK - 140 - 9 - key(1026).len()])
        .unwrap();
    edit.put(&[0, 0xff, 0], &[]).unwrap();
    edit.commit().unwrap();
    let root = Record {
        kind: 1,
        bank: 1,
        epoch: 991,
        seq: 81,
        index: 47,
        previous: 0,
        config: four_config(),
        chunks: 1,
        records: 1300,
        chain: 0,
        payload: vec![],
    };
    let mut page = LinearPage::new();
    page.combine = true;
    let address = page.bytes.as_ptr();
    for mut reader in [old, db.begin_read().unwrap()] {
        for (lo, hi) in [
            (vec![], None),
            (key(1), Some(key(1))),
            (key(1026), Some(key(1028))),
        ] {
            let expected = reader.scan(&lo, hi.as_deref()).unwrap();
            let mut reference = Vec::new();
            MvccPages::pack_range(&reader, &lo, hi.as_deref(), &mut |fence, w| {
                let r = Record {
                    payload: encode_frame(root.seq, w),
                    ..root.clone()
                };
                reference.push((fence, r.encode()));
                Ok(())
            })
            .unwrap();
            let mut actual = Vec::new();
            let mut count = 0;
            MvccPages::collect_pages(&reader, &lo, hi.as_deref(), &mut page, &mut |fence, p| {
                count += p.count();
                let bytes = p.seal(&root)?;
                assert_eq!(bytes.as_ptr(), address);
                assert!(bytes.len() <= BLOCK);
                actual.push((fence, bytes.to_vec()));
                Ok(())
            })
            .unwrap();
            assert_eq!(actual, reference);
            assert_eq!(count, expected.len());
            assert_eq!(page.count(), 0);
        }
    }
}
