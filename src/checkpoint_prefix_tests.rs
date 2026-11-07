#[test]
fn prefix_replays_changes_on_both_sides_of_the_scan_cursor() {
    let path = Fixture::new();
    let db = Database::create(&path.0, [91; 16]).unwrap();
    let mut seed = db.begin().unwrap();
    for i in 0..600u32 {
        seed.put(&i.to_be_bytes(), &i.to_be_bytes()).unwrap();
    }
    seed.commit().unwrap();
    let mut old = db.begin_read().unwrap();
    let mut point = db.begin().unwrap();
    point.get(&7u32.to_be_bytes()).unwrap();
    let mut range = db.begin().unwrap();
    range
        .scan(&700u32.to_be_bytes(), Some(&800u32.to_be_bytes()))
        .unwrap();
    let prefix = db.prepare_checkpoint_prefix(1).unwrap();
    let mut update = db.begin().unwrap();
    update.put(&7u32.to_be_bytes(), b"new").unwrap();
    update.delete(&510u32.to_be_bytes()).unwrap();
    update.put(&720u32.to_be_bytes(), b"insert").unwrap();
    update.commit().unwrap();
    assert_eq!(prefix.finish().unwrap().version, 1);
    for mut tx in [point, range] {
        tx.put(b"unrelated", b"v").unwrap();
        assert!(matches!(tx.commit(), Err(Error::Conflict(_))));
    }
    assert_eq!(
        old.get(&7u32.to_be_bytes()).unwrap(),
        Some(7u32.to_be_bytes().to_vec())
    );
    assert_eq!(
        old.get(&510u32.to_be_bytes()).unwrap(),
        Some(510u32.to_be_bytes().to_vec())
    );
    assert!(old.get(&720u32.to_be_bytes()).unwrap().is_none());
    drop(old);
    drop(db);
    let db = Database::open(&path.0).unwrap();
    let mut r = db.begin_read().unwrap();
    assert_eq!(r.version(), 2);
    let rows = r.scan(b"", None).unwrap();
    assert_eq!(rows.len(), 600);
    for (k, v) in rows {
        let id = u32::from_be_bytes(k.try_into().unwrap());
        assert!(id < 600 || id == 720);
        assert_ne!(id, 510);
        assert_eq!(
            v,
            match id {
                7 => b"new".to_vec(),
                720 => b"insert".to_vec(),
                _ => id.to_be_bytes().to_vec(),
            }
        );
    }
}

#[test]
fn prefix_releases_resources_after_refusal_failure_or_stale_wal() {
    let path = Fixture::new();
    let limits = ResourceLimits {
        max_transactions: 1,
        ..ResourceLimits::default()
    };
    let db = Database::create_with_limits(&path.0, [92; 16], limits).unwrap();
    let mut tx = db.begin().unwrap();
    tx.put(b"k", b"v").unwrap();
    assert!(matches!(
        db.prepare_checkpoint_prefix(1),
        Err(Error::ResourceExhausted(_))
    ));
    tx.commit().unwrap();
    for count in [0, 2049] {
        assert!(matches!(
            db.prepare_checkpoint_prefix(count),
            Err(Error::Invalid(_))
        ));
    }
    let mut prefix = db.prepare_checkpoint_prefix(2048).unwrap();
    assert!(matches!(
        db.prepare_checkpoint_prefix(1),
        Err(Error::ResourceExhausted(_))
    ));
    let draft = prefix.draft.as_mut().unwrap();
    let temporary = draft.temporary.clone();
    draft.file = Some(File::open(&temporary).unwrap());
    assert!(matches!(prefix.finish(), Err(Error::Io(_))));
    assert!(!temporary.exists());
    let abandoned = db.prepare_checkpoint_prefix(1).unwrap();
    let tmp = abandoned.draft.as_ref().unwrap().temporary.clone();
    drop(abandoned);
    assert!(!tmp.exists());
    assert_eq!(db.resource_usage().unwrap().current[ACTIVE], 0);
    drop(db);
    let db = Database::open(&path.0).unwrap();
    let p = db.prepare_checkpoint_prefix(1).unwrap();
    db.checkpoint().unwrap();
    assert!(matches!(
        p.finish(),
        Err(Error::Invalid("checkpoint prefix superseded"))
    ));
    db.prepare_checkpoint_prefix(1).unwrap().finish().unwrap();
    assert_eq!(
        db.begin_read().unwrap().get(b"k").unwrap(),
        Some(b"v".to_vec())
    );
}

#[test]
fn prefix_buffer_refusal_preserves_the_original_file() {
    let path = Fixture::new();
    let db = Database::create(&path.0, [94; 16]).unwrap();
    let mut tx = db.begin().unwrap();
    tx.put(b"large", &vec![3; 8192]).unwrap();
    tx.commit().unwrap();
    drop(db);
    let limited = Database::open_with_limits(
        &path.0,
        ResourceLimits {
            max_buffer_bytes: 96 * 1024,
            ..ResourceLimits::default()
        },
    )
    .unwrap();
    assert!(matches!(
        limited.prepare_checkpoint_prefix(1),
        Err(Error::ResourceExhausted(_))
    ));
    assert_eq!(limited.resource_usage().unwrap().current[ACTIVE], 0);
    drop(limited);
    assert_eq!(
        Database::open(&path.0)
            .unwrap()
            .begin_read()
            .unwrap()
            .get(b"large")
            .unwrap()
            .unwrap(),
        vec![3; 8192]
    );
}
#[cfg(feature = "fixed-log")]
#[test]
fn prefix_preserves_fixed_capacity_and_commits_after_the_snapshot() {
    let path = Fixture::new();
    let db = Database::create(&path.0, [93; 16]).unwrap();
    db.checkpoint_fixed_log(65536).unwrap();
    let prefix = db.prepare_checkpoint_prefix(1).unwrap();
    let mut update = db.begin().unwrap();
    update.put(b"later", b"durable").unwrap();
    update.commit().unwrap();
    prefix.finish().unwrap();
    drop(db);
    let reopened = Database::open(&path.0).unwrap();
    assert_eq!(reopened.fixed_log_capacity().unwrap(), Some(65536));
    assert_eq!(
        reopened.begin_read().unwrap().get(b"later").unwrap(),
        Some(b"durable".to_vec())
    );
}
