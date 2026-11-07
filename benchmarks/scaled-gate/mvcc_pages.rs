use crate::transactions::{Budget, CheckpointInfo, State, TestPersistence, legacy_wal_file};
type Fault = Option<Arc<dyn Fn(&str) -> io::Result<()> + Send + Sync>>;

struct MvccPages {
    arrivals: Option<Arrivals>,
    direct: bool,
    packed: bool,
    linear: bool,
    combine: bool,
    commit_profile: bool,
    profile: bool,
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
    fn empty_state(path: &Path) -> crate::transactions::Result<State> {
        let limits = ResourceLimits {
            max_transactions: 64,
            max_staged_bytes: 32 * PAGE,
            max_dependency_bytes: 32 * PAGE,
            max_retained_bytes: 2 * 1024 * PAGE,
            max_buffer_bytes: 64 * PAGE,
        };
        // A read-only inert descriptor makes an accidental fallback write fail.
        Ok(State::new(
            legacy_wal_file(File::open("/dev/null")?),
            path.to_owned(),
            Budget::new(limits),
        ))
    }
    fn attach(path: &Path, disk: Dirty, fault: Fault) -> crate::transactions::Result<Database> {
        let mut state = Self::empty_state(path)?;
        let generation = disk.control.seq;
        let mut chunk = WriteSet::new();
        for (k, v) in &disk.rows {
            chunk.insert(k.clone(), Some(v.to_vec()));
            if chunk.len() == 1024 {
                state.apply(generation, std::mem::take(&mut chunk))?;
            }
        }
        state.apply(generation, chunk)?;
        Self::attach_state(state, disk, fault, false)
    }
    fn attach_state(
        state: State,
        disk: Dirty,
        fault: Fault,
        direct: bool,
    ) -> crate::transactions::Result<Database> {
        let mut disk = disk;
        let logreuse = direct
            && (std::env::var_os("CATDB_TEST_LOG_REUSE").is_some()
                || std::env::var("SCALED_VARIANT").as_deref() == Ok("logreuse"));
        let patch = direct
            && (std::env::var_os("CATDB_TEST_PATCH").is_some()
                || std::env::var("SCALED_VARIANT").as_deref() == Ok("patch"));
        disk.patch = patch;
        let grouped = patch
            || direct
                && (std::env::var_os("CATDB_TEST_ARRIVALS").is_some()
                    || std::env::var("SCALED_VARIANT")
                        .as_deref()
                        .is_ok_and(|v| matches!(v, "arrivals" | "prewrite" | "defer")));
        if disk.group_version.is_some() && !grouped {
            return Err(TxError::Invalid("group persistence required"));
        }
        if grouped {
            let seq = disk.logical_seq();
            disk.group_version = Some(seq);
        }
        let combine = grouped
            || logreuse
            || direct
                && (std::env::var_os("CATDB_TEST_COMBINE").is_some()
                    || std::env::var("SCALED_VARIANT").as_deref() == Ok("combine"));
        let linear = combine
            || direct
                && (std::env::var_os("CATDB_TEST_LINEAR").is_some()
                    || std::env::var("SCALED_VARIANT").as_deref() == Ok("linear"));
        let reuse = linear
            || direct
                && (std::env::var_os("CATDB_TEST_REUSE").is_some()
                    || std::env::var("SCALED_VARIANT").as_deref() == Ok("reuse"));
        disk.control.reuse_append = logreuse;
        let commit_profile = std::env::var_os("CATDB_COMMIT_PHASES").is_some();
        disk.control.profile_append = commit_profile;
        disk.reuse_encoded = reuse;
        let lease = disk.pool.try_clone()?;
        let db = Database::from_state(state, [87; 16], lease);
        db.set_group_commit(false);
        let backend = Arc::new(Self {
            direct,
            disk: Mutex::new(disk),
            turns: TurnQueue::default(),
            arrivals: grouped.then(Arrivals::default),
            linear,
            combine,
            commit_profile,
            packed: reuse
                || direct
                    && (std::env::var_os("CATDB_TEST_PACK").is_some()
                        || std::env::var("SCALED_VARIANT").as_deref() == Ok("packed")),
            profile: std::env::var_os("CATDB_CHECKPOINT_PHASES").is_some(),
            fault,
        });
        assert!(db.inner.test_persistence.set(backend).is_ok());
        Ok(db)
    }
    fn create(path: &Path, cfg: Config, rows: Data, fault: Fault) -> Database {
        if std::env::var_os("CATDB_TEST_DIRECT").is_some() {
            return Self::create_direct(
                path,
                cfg,
                rows.into_iter().map(|(k, v)| (k, v.to_vec())),
                fault,
            );
        }
        Self::attach(path, Dirty::create(path, cfg, rows).unwrap(), fault).unwrap()
    }
    fn open(path: &Path, cfg: Config, fault: Fault) -> Database {
        if std::env::var_os("CATDB_TEST_DIRECT").is_some() {
            return Self::open_direct(path, cfg, fault);
        }
        Self::attach(path, Dirty::open(path, cfg, &mut noop).unwrap(), fault).unwrap()
    }
}
include!("prewrite.rs");
impl TestPersistence for MvccPages {
    fn prewrite(
        &self,
        db: &Database,
        count: usize,
        bytes: u64,
    ) -> crate::transactions::Result<(usize, u64)> {
        self.prepare_dirty(db, count, bytes)
    }
    fn commit(&self, tx: Transaction) -> crate::transactions::Result<u64> {
        if let Some(queue) = &self.arrivals {
            return queue.submit(self, tx);
        }
        let db = tx.database.clone();
        if self.commit_profile {
            CommitSample::attempt();
        }
        let _turn = self.turns.enter();
        if self.commit_profile {
            CommitSample::set(1);
        }
        let result = tx.commit_with(|_, frame| {
            let (seq, w) = decode_frame(&frame[PREFIX_BYTES..frame.len() - TRAILER_BYTES])
                .map_err(io::Error::other)?;
            let mut disk = self.disk.lock().map_err(|_| err("page store poisoned"))?;
            if disk.control.seq.checked_add(1) != Some(seq) {
                return Err(err("MVCC and disk generation differ"));
            }
            if self.direct {
                disk.append_log(&w, &mut |stage| self.signal(stage))?;
                db.inner.group.record_sync();
                Ok(())
            } else {
                disk.append(&w, &mut |stage| self.signal(stage))
            }
        });
        if self.commit_profile {
            CommitSample::stop();
        }
        result
    }
    fn checkpoint(&self, db: &Database) -> crate::transactions::Result<CheckpointInfo> {
        let _turn = self.turns.enter();
        let _writer = db.inner.writer.lock().map_err(|_| TxError::Poisoned)?;
        let mut disk = self.disk.lock().map_err(|_| TxError::Poisoned)?;
        let version = db.state()?.sequence;
        if version != disk.logical_seq() {
            return Err(TxError::Invalid("MVCC and checkpoint generation differ"));
        }
        let bytes = |s: &Dirty| -> io::Result<u64> {
            Ok(s.control.file.metadata()?.len() + s.pool.metadata()?.len())
        };
        let before_bytes = bytes(&disk)?;
        let prepared_count = disk.prepared.len();
        let page_bytes = disk.control.costs.writes.get("page").copied().unwrap_or(0);
        let mut scratch = db.inner.budget.buffer();
        if self.packed {
            scratch.add(PACK_SCRATCH)?;
        }
        let switched = if self.direct {
            let mut reader = db.begin_read()?;
            if reader.version() != version {
                return Err(TxError::Invalid("checkpoint snapshot"));
            }
            Phase::begin(self.profile);
            if self.linear {
                let mut page = LinearPage::new();
                page.combine = self.combine;
                disk.switch_sources(&mut |s| self.signal(s), |lo, hi, emit| {
                    Self::collect_pages(&reader, lo, hi, &mut page, &mut |start, page| {
                        emit(start, PageSource::Linear(page))
                    })
                })
            } else if self.packed {
                disk.switch_chunks(&mut |s| self.signal(s), |lo, hi, emit| {
                    Self::pack_range(&reader, lo, hi, emit)
                })
            } else {
                disk.switch_from(&mut |s| self.signal(s), |lo, hi, emit| {
                    Self::visit_range(&mut reader, lo, hi, emit)
                })
            }
        } else {
            Phase::begin(self.profile);
            disk.switch(&mut |s| self.signal(s))
        };
        if std::env::var_os("PREWRITE_COUNTS").is_some() {
            println!(
                "PREWRITE_CHECKPOINT,{version},{prepared_count},{}",
                disk.control.costs.writes.get("page").copied().unwrap_or(0) - page_bytes
            );
        }
        if switched.is_ok() && std::env::var_os("CARRY_WINDOW").is_some() {
            let w = &disk.control.costs.writes;
            println!(
                "WINDOW_BYTES,{version},{},{},{}",
                w.get("page").copied().unwrap_or(0),
                w.values().sum::<u64>(),
                disk.stage_bytes.invalidated
            );
        }
        let phase_times = Phase::finish();
        if let Some(ns) = phase_times {
            println!(
                "PATCH_PHASE,{version},{},{},{},{},{},{}",
                ns[7],
                ns[8],
                ns[9],
                disk.patch_counts.used,
                disk.patch_counts.fallback,
                disk.patch_counts.bytes
            );
            println!("PAGE_CRC,{version},{},{}", ns[5], ns[6]);
            println!(
                "CHECKPOINT_PHASE,{version},{},{},{},{},{}",
                ns[0],
                ns[1],
                ns[2] + ns[5] + ns[6],
                ns[3],
                ns[4]
            );
        }
        if std::env::var_os("DENSITY_COUNTS").is_some() {
            println!(
                "PAGE_COUNTS,{version},{},{},{}",
                disk.patch_counts.used, disk.patch_counts.fallback, disk.patch_counts.bytes
            );
        }
        if let Err(e) = switched {
            db.state()?.poisoned = true;
            return Err(TxError::CommitUncertain(e));
        }
        db.state()?.wal_bytes = crate::transactions::HEADER_BYTES as u64;
        Ok(CheckpointInfo {
            version,
            records: if self.direct {
                disk.pages.values().map(|p| p.rows).sum()
            } else {
                disk.rows.len()
            },
            before_bytes,
            after_bytes: bytes(&disk)?,
        })
    }
}
