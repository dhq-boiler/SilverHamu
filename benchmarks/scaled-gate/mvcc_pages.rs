use crate::transactions::{Budget, CheckpointInfo, State, TestPersistence, legacy_wal_file};
type Fault = Option<Arc<dyn Fn(&str) -> io::Result<()> + Send + Sync>>;

struct MvccPages {
    disk: Mutex<Dirty>,
    turns: TurnQueue,
    fault: Fault,
}

impl MvccPages {
    fn signal(&self, stage: &str) -> io::Result<()> {
        match &self.fault {
            Some(f) => f(stage),
            None => Ok(()),
        }
    }
    fn attach(path: &Path, disk: Dirty, fault: Fault) -> crate::transactions::Result<Database> {
        let limits = ResourceLimits {
            max_transactions: 64,
            max_staged_bytes: 32 * PAGE,
            max_dependency_bytes: 32 * PAGE,
            max_retained_bytes: 2 * 1024 * PAGE,
            max_buffer_bytes: 64 * PAGE,
        };
        // A read-only inert descriptor makes an accidental fallback write fail.
        let mut state = State::new(
            legacy_wal_file(File::open("/dev/null")?),
            path.to_owned(),
            Budget::new(limits),
        );
        let generation = disk.control.seq;
        let mut chunk = WriteSet::new();
        for (k, v) in &disk.rows {
            chunk.insert(k.clone(), Some(v.to_vec()));
            if chunk.len() == 1024 {
                state.apply(generation, std::mem::take(&mut chunk))?;
            }
        }
        state.apply(generation, chunk)?;
        let lease = disk.pool.try_clone()?;
        let db = Database::from_state(state, [87; 16], lease);
        db.set_group_commit(false);
        let backend = Arc::new(Self {
            disk: Mutex::new(disk),
            turns: TurnQueue::default(),
            fault,
        });
        assert!(db.inner.test_persistence.set(backend).is_ok());
        Ok(db)
    }
    fn create(path: &Path, cfg: Config, rows: Data, fault: Fault) -> Database {
        Self::attach(path, Dirty::create(path, cfg, rows).unwrap(), fault).unwrap()
    }
    fn open(path: &Path, cfg: Config, fault: Fault) -> Database {
        Self::attach(path, Dirty::open(path, cfg, &mut noop).unwrap(), fault).unwrap()
    }
}
impl TestPersistence for MvccPages {
    fn commit(&self, tx: Transaction) -> crate::transactions::Result<u64> {
        let _turn = self.turns.enter();
        tx.commit_with(|_, frame| {
            let (seq, w) = decode_frame(&frame[PREFIX_BYTES..frame.len() - TRAILER_BYTES])
                .map_err(io::Error::other)?;
            let mut disk = self.disk.lock().map_err(|_| err("page store poisoned"))?;
            if disk.control.seq.checked_add(1) != Some(seq) {
                return Err(err("MVCC and disk generation differ"));
            }
            disk.append(&w, &mut |stage| self.signal(stage))
        })
    }
    fn checkpoint(&self, db: &Database) -> crate::transactions::Result<CheckpointInfo> {
        let _turn = self.turns.enter();
        let _writer = db.inner.writer.lock().map_err(|_| TxError::Poisoned)?;
        let mut disk = self.disk.lock().map_err(|_| TxError::Poisoned)?;
        let version = db.state()?.sequence;
        if version != disk.control.seq {
            return Err(TxError::Invalid("MVCC and checkpoint generation differ"));
        }
        let bytes = |s: &Dirty| -> io::Result<u64> {
            Ok(s.control.file.metadata()?.len() + s.pool.metadata()?.len())
        };
        let before_bytes = bytes(&disk)?;
        if let Err(e) = disk.switch(&mut |s| self.signal(s)) {
            db.state()?.poisoned = true;
            return Err(TxError::CommitUncertain(e));
        }
        db.state()?.wal_bytes = crate::transactions::HEADER_BYTES as u64;
        Ok(CheckpointInfo {
            version,
            records: disk.rows.len(),
            before_bytes,
            after_bytes: bytes(&disk)?,
        })
    }
}
