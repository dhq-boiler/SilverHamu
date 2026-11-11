use super::*;

#[test]
fn tiny_budget_rollover_releases_the_user_slot_and_preserves_the_last_acknowledged_value() {
    let path = std::env::temp_dir().join(format!(
        "silverhamu-managed-tiny-{}-{}.ndb",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let limits = ResourceLimits {
        max_transactions: 1,
        max_buffer_bytes: 1024,
        ..ResourceLimits::default()
    };
    for grouped in [false, true] {
        let db = Database::create_with_log(
            &path,
            [19; 16],
            limits,
            Some(CheckpointLog::ManagedRedo(4096)),
        )
        .unwrap();
        db.set_group_commit(grouped);
        let mut rollovers = 0;
        let mut refused = false;
        let mut last = 0_u8;
        for stamp in 1..=100_u8 {
            let before = db.stats().unwrap();
            let mut tx = db.begin().unwrap();
            tx.put(b"hot", &[stamp; 64]).unwrap();
            let incoming = (tx.write_bytes + PREFIX_BYTES + TRAILER_BYTES) as u64;
            if !refused && db.state().unwrap().needs_managed_checkpoint(incoming) {
                let mut held = db.inner.budget.buffer();
                held.add(900).unwrap();
                assert!(matches!(tx.commit(), Err(Error::ResourceExhausted(_))));
                drop(held);
                assert_eq!(db.stats().unwrap().version, before.version);
                assert_eq!(
                    db.begin_read().unwrap().get(b"hot").unwrap().unwrap(),
                    vec![last; 64]
                );
                tx = db.begin().unwrap();
                tx.put(b"hot", &[stamp; 64]).unwrap();
                refused = true;
            }
            assert_eq!(tx.commit().unwrap(), u64::from(stamp));
            last = stamp;
            if db.stats().unwrap().wal_bytes < before.wal_bytes {
                rollovers += 1;
            }
        }
        assert!(refused && rollovers >= 2);
        assert_eq!(db.fixed_log_capacity().unwrap(), Some(4096));
        assert_eq!(db.stats().unwrap().active_transactions, 0);
        drop(db);
        let again = Database::open_with_limits(&path, limits).unwrap();
        assert_eq!(
            again.begin_read().unwrap().get(b"hot").unwrap().unwrap(),
            vec![100; 64]
        );
        drop(again);
        std::fs::remove_file(&path).unwrap();
    }
    let mut owner = path.into_os_string();
    owner.push(".owner");
    let _ = std::fs::remove_file(owner);
}
