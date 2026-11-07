#[cfg(all(target_os = "linux", target_pointer_width = "64"))]
#[test]
#[ignore = "requires CATDB_TEST_RANGE_WRITEBACK=1"]
fn range_writeback_recovers_after_failure_and_exit_around_final_sync() {
    assert_eq!(
        std::env::var("CATDB_TEST_RANGE_WRITEBACK").as_deref(),
        Ok("1")
    );
    let path = Fixture::new();
    let db = Database::create(&path.0, [72; 16]).unwrap();
    for page in 0..9_u8 {
        let mut tx = db.begin().unwrap();
        for slot in 0..=255_u8 {
            tx.put(&[page, slot], &vec![slot; 4096]).unwrap();
        }
        tx.commit().unwrap();
    }
    let mut old = db.begin_read().unwrap();
    let result = db.checkpoint_with(|phase| {
        if phase == CheckpointStage::Written {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "failed before final sync",
            ));
        }
        Ok(())
    });
    assert!(matches!(result, Err(Error::Io(_))));
    assert_eq!(old.get(&[8, 255]).unwrap(), Some(vec![255; 4096]));
    drop(old);
    drop(db);

    for phase in ["written", "prefix-synced", "prefix-published"] {
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "transactions::tests::checkpoint_crash_worker",
            ])
            .env("NEWDBMS_CHECKPOINT_PATH", &path.0)
            .env("NEWDBMS_CHECKPOINT_PHASE", phase)
            .output()
            .unwrap();
        assert_eq!(child.status.code(), Some(76), "{child:?}");
        let db = Database::open(&path.0).unwrap();
        let mut read = db.begin_read().unwrap();
        assert_eq!(read.get(b"key").unwrap(), Some(vec![20]));
        for page in 0..9_u8 {
            for slot in 0..=255_u8 {
                assert_eq!(read.get(&[page, slot]).unwrap(), Some(vec![slot; 4096]));
            }
        }
    }
}
