impl MvccPages {
    fn visit_range(
        reader: &mut Transaction,
        lower: &[u8],
        upper: Option<&[u8]>,
        emit: &mut dyn FnMut(&[u8], &[u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut after = None;
        loop {
            let scan = Phase::enter(1);
            let page = reader
                .scan_page(
                    lower,
                    upper,
                    after.as_deref(),
                    ScanLimits {
                        max_rows: 256,
                        max_bytes: BLOCK,
                        max_visits: 512,
                    },
                )
                .map_err(io::Error::other)?;
            drop(scan);
            for (k, v) in &page.rows {
                emit(k, v)?;
            }
            match page.next_after {
                None => return Ok(()),
                Some(key) => after = Some(key),
            }
        }
    }
    fn create_direct(
        path: &Path,
        cfg: Config,
        seed: impl IntoIterator<Item = (Vec<u8>, Vec<u8>)>,
        fault: Fault,
    ) -> Database {
        let mut disk = Dirty::create(path, cfg, Data::new()).unwrap();
        let mut state = Self::empty_state(path).unwrap();
        let mut pending = WriteSet::new();
        for (k, v) in seed {
            pending.insert(k, Some(v));
            if pending.len() == 256 {
                state.apply(0, std::mem::take(&mut pending)).unwrap();
            }
        }
        state.apply(0, pending).unwrap();
        disk.dirty.insert(vec![]);
        let db = Self::attach_state(state, disk, fault, true).unwrap();
        db.checkpoint().unwrap();
        db
    }
    fn open_direct(path: &Path, cfg: Config, fault: Fault) -> Database {
        let mut state = Self::empty_state(path).unwrap();
        let disk = Dirty::open_into(path, cfg, &mut noop, |generation, w| {
            state.apply(generation, w).map_err(io::Error::other)
        })
        .unwrap();
        // An empty checkpoint has no page callback to carry its commit sequence.
        state.sequence = disk.control.seq;
        while !state.history_keys.is_empty() {
            drop(state.collect());
            if state.gc_target == 0 {
                break;
            }
        }
        assert!(disk.rows.is_empty());
        Self::attach_state(state, disk, fault, true).unwrap()
    }
}
#[test]
#[ignore = "fresh-process warm reopen RSS"]
fn direct_reopen_memory() {
    let path = PathBuf::from(std::env::var_os("CATDB_RECOVER_FILE").unwrap());
    let n: usize = std::env::var("SCALED_ROWS").unwrap().parse().unwrap();
    let cfg = Config {
        logs: std::env::var("SCALED_CAP").unwrap().parse().unwrap(),
        checkpoints: (n / 3000 + 4) as u32,
    };
    let initial = memory().0;
    let db = match std::env::var("SCALED_VARIANT").unwrap().as_str() {
        "wal" => Database::open(path).unwrap(),
        "mvcc" => MvccPages::open(&path, cfg, None),
        "direct" | "packed" | "reuse" => MvccPages::open_direct(&path, cfg, None),
        _ => unreachable!(),
    };
    let read = db.begin_read().unwrap();
    assert_eq!(
        read.version(),
        std::env::var("CATDB_EXPECT_SEQUENCE")
            .unwrap()
            .parse::<u64>()
            .unwrap()
    );
    let (rss, peak) = memory();
    println!("REOPEN_MEMORY,{initial},{rss},{peak}");
}
