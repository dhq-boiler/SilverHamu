use super::*;
use std::ops::Deref;

const SLOT: u64 = 64;
const PAGE: u64 = 4096;
pub(super) const METADATA_BYTES: u64 = 2 * PAGE;
const END_MAGIC: &[u8; 8] = b"NDFEND01";
const SENTINEL: u64 = PREFIX_BYTES as u64;

pub(super) struct LogFile {
    file: File,
    fixed: Option<Fixed>,
}
struct Fixed {
    capacity: u64,
    header: [u8; HEADER_BYTES],
    end: Mutex<(u64, u64)>,
    #[cfg(feature = "lock-timing")]
    metrics: Mutex<[u64; 3]>,
}
impl From<File> for LogFile {
    fn from(file: File) -> Self {
        Self { file, fixed: None }
    }
}
impl Deref for LogFile {
    type Target = File;
    fn deref(&self) -> &File {
        &self.file
    }
}
impl Read for LogFile {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.file.read(b)
    }
}
impl Seek for LogFile {
    fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
        self.file.seek(p)
    }
}
impl Write for LogFile {
    fn write(&mut self, b: &[u8]) -> io::Result<usize> {
        if let Some(fixed) = &self.fixed {
            let end = self
                .file
                .stream_position()?
                .checked_add(b.len() as u64)
                .ok_or_else(|| io::Error::other("fixed log offset overflow"))?;
            if end > fixed.capacity - SENTINEL {
                return Err(io::Error::other("fixed log capacity; checkpoint required"));
            }
        }
        self.file.write(b)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}
fn invalid() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, "fixed log end metadata")
}
fn slot(header: &[u8; HEADER_BYTES], generation: u64, end: u64, capacity: u64) -> [u8; 64] {
    let mut b = [0; 64];
    b[..8].copy_from_slice(END_MAGIC);
    b[8..16].copy_from_slice(&generation.to_le_bytes());
    b[16..24].copy_from_slice(&end.to_le_bytes());
    b[24..32].copy_from_slice(&capacity.to_le_bytes());
    b[32..48].copy_from_slice(&header[16..32]);
    b[48..52].copy_from_slice(&header[44..48]);
    let crc = crate::crc32(&b[..60]);
    b[60..].copy_from_slice(&crc.to_le_bytes());
    b
}
fn decode(b: &[u8; 64], header: &[u8; HEADER_BYTES], capacity: u64) -> Option<(u64, u64)> {
    let u = |at| u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
    let end = u(16);
    if &b[..8] != END_MAGIC
        || u(24) != capacity
        || b[32..48] != header[16..32]
        || b[48..52] != header[44..48]
        || b[52..60] != [0; 8]
        || read_u32(b, 60) != crate::crc32(&b[..60])
        || !(HEADER_BYTES as u64..=capacity - SENTINEL).contains(&end)
    {
        return None;
    }
    Some((u(8), end))
}
pub(super) fn validate_capacity(capacity: u64) -> Result<()> {
    if !(4096..=MAX_WAL_BYTES - METADATA_BYTES).contains(&capacity)
        || !capacity.is_multiple_of(PAGE)
    {
        return Err(Error::Invalid("fixed log capacity"));
    }
    Ok(())
}
impl LogFile {
    pub(super) fn open(mut file: File, header: [u8; HEADER_BYTES]) -> io::Result<Self> {
        if read_u32(&header, 8) != 3 {
            return Ok(file.into());
        }
        let capacity = file
            .metadata()?
            .len()
            .checked_sub(METADATA_BYTES)
            .ok_or_else(invalid)?;
        validate_capacity(capacity).map_err(|_| invalid())?;
        file.seek(SeekFrom::Start(capacity))?;
        let mut values = Vec::new();
        for index in 0..2 {
            file.seek(SeekFrom::Start(capacity + index * PAGE))?;
            let mut b = [0; 64];
            file.read_exact(&mut b)?;
            if let Some(v) = decode(&b, &header, capacity) {
                values.push(v);
            }
        }
        values.sort_unstable();
        let end = *values.last().ok_or_else(invalid)?;
        if values.len() == 2
            && (values[0].0.abs_diff(values[1].0) > 1
                || values[0].1 > values[1].1
                || (values[0].0 == values[1].0 && values[0].1 != values[1].1))
        {
            return Err(invalid());
        }
        file.seek(SeekFrom::Start(HEADER_BYTES as u64))?;
        Ok(Self {
            file,
            fixed: Some(Fixed {
                capacity,
                header,
                end: Mutex::new(end),
                #[cfg(feature = "lock-timing")]
                metrics: Mutex::new([0; 3]),
            }),
        })
    }
    pub(super) fn capacity(&self) -> Option<u64> {
        self.fixed.as_ref().map(|f| f.capacity)
    }
    pub(super) fn confirmed_end(&self) -> io::Result<Option<u64>> {
        self.fixed
            .as_ref()
            .map(|f| f.end.lock().map(|e| e.1).map_err(|_| invalid()))
            .transpose()
    }
    #[cfg(feature = "lock-timing")]
    pub(super) fn metrics(&self) -> io::Result<Option<[u64; 3]>> {
        self.fixed
            .as_ref()
            .map(|f| f.metrics.lock().map(|m| *m).map_err(|_| invalid()))
            .transpose()
    }
    pub(super) fn sync_all(&self) -> io::Result<()> {
        let Some(fixed) = &self.fixed else {
            return self.file.sync_all();
        };
        let mut version = fixed.end.lock().map_err(|_| invalid())?;
        let mut file = &self.file;
        let end = file.stream_position()?;
        if !(version.1..=fixed.capacity - SENTINEL).contains(&end) {
            return Err(invalid());
        }
        let generation = version.0.checked_add(1).ok_or_else(invalid)?;
        file.write_all(&[0; PREFIX_BYTES])?;
        file.seek(SeekFrom::Start(end))?;
        if end > version.1 {
            phase("before-data-sync")?;
        }
        #[cfg(feature = "lock-timing")]
        let data_start = std::time::Instant::now();
        file.sync_data()?;
        #[cfg(feature = "lock-timing")]
        let data_ns = data_start.elapsed().as_nanos() as u64;
        if end > version.1 {
            phase("after-data-sync")?;
        }
        let marker = slot(&fixed.header, generation, end, fixed.capacity);
        file.seek(SeekFrom::Start(fixed.capacity + (generation % 2) * PAGE))?;
        let write = file.write_all(&marker);
        file.seek(SeekFrom::Start(end))?;
        write?;
        if end > version.1 {
            phase("before-end-sync")?;
        }
        // A durable end must never point at payload that has not already been synced.
        #[cfg(feature = "lock-timing")]
        let end_start = std::time::Instant::now();
        file.sync_data()?;
        #[cfg(feature = "lock-timing")]
        {
            let mut m = fixed.metrics.lock().map_err(|_| invalid())?;
            m[0] += 1;
            m[1] += data_ns;
            m[2] += end_start.elapsed().as_nanos() as u64;
        }
        if end > version.1 {
            phase("after-end-sync")?;
        }
        *version = (generation, end);
        Ok(())
    }
}
pub(super) fn initialize(file: &mut File, end: u64, capacity: u64) -> Result<()> {
    validate_capacity(capacity)?;
    if end.checked_add(SENTINEL).is_none_or(|n| n > capacity) {
        return Err(Error::ResourceExhausted("fixed log capacity"));
    }
    phase("fixed-initialization")?;
    file.seek(SeekFrom::Start(0))?;
    let mut header = [0; HEADER_BYTES];
    file.read_exact(&mut header)?;
    header[8..12].copy_from_slice(&3u32.to_le_bytes());
    let crc = crate::crc32(&header[..44]);
    header[44..].copy_from_slice(&crc.to_le_bytes());
    file.seek(SeekFrom::Start(0))?;
    file.write_all(&header)?;
    file.seek(SeekFrom::Start(end))?;
    // Sparse/preallocated unwritten extents would still require metadata work on first write.
    let zeros = [0u8; 32768];
    let mut remaining = capacity - end;
    while remaining > 0 {
        let n = remaining.min(zeros.len() as u64) as usize;
        file.write_all(&zeros[..n])?;
        remaining -= n as u64;
    }
    let marker = slot(&header, 0, end, capacity);
    // Adjacent copies could be damaged by the same torn sector/page write.
    let mut page = [0u8; PAGE as usize];
    page[..SLOT as usize].copy_from_slice(&marker);
    file.write_all(&page)?;
    file.write_all(&page)?;
    file.sync_all()?;
    file.seek(SeekFrom::Start(end))?;
    Ok(())
}
#[cfg(test)]
thread_local! { static FAIL: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) }; }
#[cfg(test)]
thread_local! { static GATE: std::cell::RefCell<Option<(&'static str, Arc<[std::sync::Barrier; 2]>)>> = const { std::cell::RefCell::new(None) }; }
fn phase(_stage: &str) -> io::Result<()> {
    #[cfg(test)]
    {
        let gate = GATE.with(|gate| {
            let mut g = gate.borrow_mut();
            if g.as_ref().is_some_and(|(stage, _)| *stage == _stage) {
                g.take()
            } else {
                None
            }
        });
        if let Some((_, g)) = gate {
            g[0].wait();
            g[1].wait();
        }
        if std::env::var("CATDB_FIXED_CRASH").ok().as_deref() == Some(_stage) {
            std::process::exit(87);
        }
        if FAIL.with(|f| f.get() == Some(_stage)) {
            return Err(io::Error::other("injected fixed-log sync failure"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("artifacts");
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir.join(format!(
                    "fixed-{}-{}-{}.wal",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                )))
        }
        fn create(&self, capacity: u64) -> Database {
            let db = Database::create(&self.0, [29; 16]).unwrap();
            db.checkpoint_fixed_log(capacity).unwrap();
            db
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let mut p = self.0.as_os_str().to_owned();
            p.push(".owner");
            let _ = std::fs::remove_file(PathBuf::from(p));
        }
    }
    fn put(db: &Database, key: &[u8], value: &[u8]) {
        let mut t = db.begin().unwrap();
        t.put(key, value).unwrap();
        t.commit().unwrap();
    }
    fn rows(db: &Database) -> Rows {
        db.begin().unwrap().scan(b"", None).unwrap()
    }
    #[test]
    fn eq_conversion_crud_checkpoint_and_reopen_keep_old_snapshots() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [29; 16]).unwrap();
        put(&db, b"a", b"old");
        put(&db, b"b", b"delete");
        let mut old = db.begin().unwrap();
        let expected = old.scan(b"", None).unwrap();
        db.checkpoint_fixed_log(65536).unwrap();
        assert_eq!(
            std::fs::metadata(&f.0).unwrap().len(),
            65536 + METADATA_BYTES
        );
        let mut tx = db.begin().unwrap();
        tx.put(b"a", b"new").unwrap();
        tx.delete(b"b").unwrap();
        tx.put(b"c", b"added").unwrap();
        tx.commit().unwrap();
        let current = rows(&db);
        assert_eq!(old.scan(b"", None).unwrap(), expected);
        db.checkpoint().unwrap();
        db.checkpoint_fixed_log(131072).unwrap();
        assert_eq!(old.scan(b"", None).unwrap(), expected);
        assert_eq!(rows(&db), current);
        drop(old);
        drop(db);
        let db = Database::open(&f.0).unwrap();
        assert_eq!(rows(&db), current);
        assert_eq!(db.fixed_log_capacity().unwrap(), Some(131072));
    }
    #[test]
    fn bv_exact_capacity_rejects_next_write_and_expansion_restores_progress() {
        let f = Fixture::new();
        let db = f.create(4096);
        drop(db);
        let db = Database::open(&f.0).unwrap();
        assert!(rows(&db).is_empty());
        let empty = BTreeMap::from([(b"a".to_vec(), Some(Vec::new()))]);
        let n = 4096 - PREFIX_BYTES - HEADER_BYTES - encode_frame(1, &empty).len();
        put(&db, b"a", &vec![7; n]);
        assert_eq!(db.stats().unwrap().wal_bytes, 4096 - PREFIX_BYTES as u64);
        let before = rows(&db);
        let mut tx = db.begin().unwrap();
        tx.put(b"b", b"x").unwrap();
        assert!(matches!(tx.commit(), Err(Error::ResourceExhausted(_))));
        assert_eq!(rows(&db), before);
        db.checkpoint_fixed_log(8192).unwrap();
        put(&db, b"b", b"x");
        assert!(db.checkpoint_fixed_log(4095).is_err());
        assert!(db.checkpoint_fixed_log(4097).is_err());
        assert!(db.checkpoint_fixed_log(MAX_WAL_BYTES).is_err());
        let current = rows(&db);
        drop(db);
        assert_eq!(rows(&Database::open(&f.0).unwrap()), current);
    }
    #[test]
    fn st_group_conflicts_and_old_generation_survive_fixed_log() {
        let f = Fixture::new();
        let db = f.create(65536);
        put(&db, b"same", b"old");
        db.set_group_commit(true);
        db.set_group_commit_wake_on_four(true);
        let mut old = db.begin().unwrap();
        let mut range = db.begin().unwrap();
        range.scan(b"", None).unwrap();
        let mut first = db.begin().unwrap();
        let mut second = db.begin().unwrap();
        first.put(b"same", b"first").unwrap();
        second.put(b"same", b"second").unwrap();
        first.commit().unwrap();
        assert!(matches!(second.commit(), Err(Error::Conflict(_))));
        let barrier = Arc::new(std::sync::Barrier::new(4));
        std::thread::scope(|s| {
            for i in 0..4u8 {
                let d = db.clone();
                let b = barrier.clone();
                s.spawn(move || {
                    let mut t = d.begin().unwrap();
                    t.put(&[i], &[i]).unwrap();
                    b.wait();
                    t.commit().unwrap();
                });
            }
        });
        assert_eq!(old.get(b"same").unwrap().unwrap(), b"old");
        range.put(b"z", b"x").unwrap();
        assert!(matches!(
            range.commit(),
            Err(Error::Conflict(ConflictKind::Range))
        ));
        let expected = rows(&db);
        drop(old);
        drop(db);
        assert_eq!(rows(&Database::open(&f.0).unwrap()), expected);
    }
    #[test]
    fn er_partial_tail_does_not_remove_confirmed_records() {
        let base = Fixture::new();
        let db = base.create(16384);
        put(&db, b"ack", b"safe");
        let end = db.stats().unwrap().wal_bytes;
        drop(db);
        let frame = encode_frame(
            2,
            &BTreeMap::from([
                (b"next-a".to_vec(), Some(b"a".to_vec())),
                (b"next-b".to_vec(), Some(b"b".to_vec())),
            ]),
        );
        for cut in [
            0,
            1,
            7,
            19,
            20,
            21,
            frame.len() - 13,
            frame.len() - 1,
            frame.len(),
        ] {
            let f = Fixture::new();
            std::fs::copy(&base.0, &f.0).unwrap();
            let mut file = OpenOptions::new().write(true).open(&f.0).unwrap();
            file.seek(SeekFrom::Start(end)).unwrap();
            file.write_all(&frame[..cut]).unwrap();
            file.sync_all().unwrap();
            drop(file);
            let db = Database::open(&f.0).unwrap();
            assert_eq!(db.begin().unwrap().get(b"ack").unwrap().unwrap(), b"safe");
            assert_eq!(rows(&db).len(), if cut == frame.len() { 3 } else { 1 });
            put(&db, b"later", b"ok");
            let expected = rows(&db);
            drop(db);
            assert_eq!(rows(&Database::open(&f.0).unwrap()), expected);
        }
    }
    #[test]
    fn er_marker_damage_recovers_payload_but_confirmed_payload_corruption_is_rejected() {
        for kind in [0, 1, 2, 3, 4] {
            let f = Fixture::new();
            let db = f.create(16384);
            put(&db, b"ack", b"safe");
            let expected = rows(&db);
            drop(db);
            let mut bytes = std::fs::read(&f.0).unwrap();
            match kind {
                0 => bytes[16384 + PAGE as usize + 60] ^= 1,
                1 => {
                    bytes[16384 + 60] ^= 1;
                    bytes[16384 + PAGE as usize + 60] ^= 1;
                }
                2 => bytes[HEADER_BYTES + PREFIX_BYTES + 12] ^= 1,
                3 => bytes[16384 + PAGE as usize..16384 + 2 * PAGE as usize].fill(0),
                _ => bytes[16384..16384 + PAGE as usize].fill(0),
            }
            std::fs::write(&f.0, &bytes).unwrap();
            if [0, 3, 4].contains(&kind) {
                assert_eq!(rows(&Database::open(&f.0).unwrap()), expected);
            } else {
                assert!(Database::open(&f.0).is_err());
                assert_eq!(std::fs::read(&f.0).unwrap(), bytes);
            }
        }
    }
    #[test]
    fn er_sync_failure_poisoning_preserves_acknowledged_data() {
        for stage in [
            "before-data-sync",
            "after-data-sync",
            "before-end-sync",
            "after-end-sync",
        ] {
            let f = Fixture::new();
            let db = f.create(16384);
            put(&db, b"ack", b"safe");
            let mut tx = db.begin().unwrap();
            tx.put(b"next", b"maybe").unwrap();
            FAIL.with(|v| v.set(Some(stage)));
            let result = tx.commit();
            FAIL.with(|v| v.set(None));
            assert!(matches!(result, Err(Error::CommitUncertain(_))));
            assert!(matches!(db.begin(), Err(Error::Poisoned)));
            drop(db);
            let db = Database::open(&f.0).unwrap();
            assert_eq!(db.begin().unwrap().get(b"ack").unwrap().unwrap(), b"safe");
        }
    }
    #[test]
    fn st_payload_sync_alone_neither_acknowledges_nor_publishes() {
        for (stage, group) in ["after-data-sync", "before-end-sync"]
            .into_iter()
            .flat_map(|s| [false, true].map(|g| (s, g)))
        {
            let f = Fixture::new();
            let db = f.create(16384);
            put(&db, b"ack", b"safe");
            db.set_group_commit(group);
            let mut old = db.begin().unwrap();
            let gate = Arc::new([std::sync::Barrier::new(2), std::sync::Barrier::new(2)]);
            struct Release(Arc<[std::sync::Barrier; 2]>);
            impl Drop for Release {
                fn drop(&mut self) {
                    self.0[1].wait();
                }
            }
            std::thread::scope(|s| {
                let (send, recv) = std::sync::mpsc::channel();
                let d = db.clone();
                let worker_gate = gate.clone();
                s.spawn(move || {
                    GATE.with(|g| *g.borrow_mut() = Some((stage, worker_gate)));
                    let mut t = d.begin().unwrap();
                    t.put(b"next", b"new").unwrap();
                    send.send(t.commit()).unwrap();
                });
                gate[0].wait();
                let release = Release(gate.clone());
                assert!(recv.try_recv().is_err());
                assert!(db.begin().unwrap().get(b"next").unwrap().is_none());
                assert!(old.get(b"next").unwrap().is_none());
                drop(release);
                recv.recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap()
                    .unwrap();
            });
            assert!(old.get(b"next").unwrap().is_none());
            assert_eq!(db.begin().unwrap().get(b"next").unwrap().unwrap(), b"new");
        }
    }
    #[test]
    #[ignore]
    fn fixed_crash_worker() {
        let db = Database::open(std::env::var_os("CATDB_FIXED_PATH").unwrap()).unwrap();
        if std::env::var_os("CATDB_FIXED_GROUP").is_some() {
            db.set_group_commit(true);
        }
        let mut tx = db.begin().unwrap();
        tx.put(b"next-a", b"a").unwrap();
        tx.put(b"next-b", b"b").unwrap();
        tx.commit().unwrap();
        panic!("crash hook not reached");
    }
    #[test]
    fn er_process_exit_at_both_barriers_keeps_confirmed_values_and_atomic_transactions() {
        for group in [false, true] {
            for stage in [
                "before-data-sync",
                "after-data-sync",
                "before-end-sync",
                "after-end-sync",
            ] {
                let f = Fixture::new();
                let db = f.create(16384);
                put(&db, b"ack", b"safe");
                drop(db);
                let mut command = std::process::Command::new(std::env::current_exe().unwrap());
                command
                    .args([
                        "--exact",
                        "transactions::fixed_log::tests::fixed_crash_worker",
                        "--ignored",
                        "--nocapture",
                    ])
                    .env("CATDB_FIXED_PATH", &f.0)
                    .env("CATDB_FIXED_CRASH", stage);
                if group {
                    command.env("CATDB_FIXED_GROUP", "1");
                }
                assert_eq!(command.status().unwrap().code(), Some(87));
                let db = Database::open(&f.0).unwrap();
                let mut tx = db.begin().unwrap();
                assert_eq!(tx.get(b"ack").unwrap().unwrap(), b"safe");
                assert_eq!(
                    tx.get(b"next-a").unwrap().is_some(),
                    tx.get(b"next-b").unwrap().is_some()
                );
            }
        }
    }
    #[test]
    fn er_checkpoint_preparation_failure_preserves_old_store() {
        let f = Fixture::new();
        let db = f.create(16384);
        put(&db, b"ack", b"safe");
        FAIL.with(|v| v.set(Some("fixed-initialization")));
        let result = db.checkpoint_fixed_log(32768);
        FAIL.with(|v| v.set(None));
        assert!(result.is_err());
        assert_eq!(db.fixed_log_capacity().unwrap(), Some(16384));
        put(&db, b"after", b"ok");
        let expected = rows(&db);
        drop(db);
        assert_eq!(rows(&Database::open(&f.0).unwrap()), expected);
    }
    #[test]
    #[ignore]
    fn fixed_checkpoint_worker() {
        let db = Database::open(std::env::var_os("CATDB_FIXED_PATH").unwrap()).unwrap();
        let phase = std::env::var("CATDB_FIXED_CHECKPOINT").unwrap();
        let mut updated = false;
        let _ = db.checkpoint_impl(false, Some(32768), |stage| {
            if stage == CheckpointStage::ChunkCopied && !updated {
                put(&db, b"during", b"safe");
                updated = true;
            }
            let stop = match phase.as_str() {
                "written" => CheckpointStage::Written,
                "tail" => CheckpointStage::TailSynced,
                _ => CheckpointStage::Published,
            };
            if stage == stop {
                std::process::exit(88);
            }
            Ok(())
        });
        panic!("checkpoint hook not reached");
    }
    #[test]
    fn er_fixed_checkpoint_exit_preserves_old_and_concurrent_confirmed_values() {
        for stage in ["written", "tail", "published"] {
            let f = Fixture::new();
            let db = f.create(16384);
            put(&db, b"ack", b"safe");
            drop(db);
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "transactions::fixed_log::tests::fixed_checkpoint_worker",
                    "--ignored",
                    "--nocapture",
                ])
                .env("CATDB_FIXED_PATH", &f.0)
                .env("CATDB_FIXED_CHECKPOINT", stage)
                .status()
                .unwrap();
            assert_eq!(status.code(), Some(88));
            let db = Database::open(&f.0).unwrap();
            let mut t = db.begin().unwrap();
            assert_eq!(t.get(b"ack").unwrap().unwrap(), b"safe");
            assert_eq!(t.get(b"during").unwrap().unwrap(), b"safe");
        }
    }
    #[test]
    fn er_torn_end_slot_never_discards_already_synced_payload() {
        let base = Fixture::new();
        let db = base.create(16384);
        put(&db, b"ack", b"safe");
        let end = db.stats().unwrap().wal_bytes;
        drop(db);
        let frame = encode_frame(
            2,
            &BTreeMap::from([(b"next".to_vec(), Some(b"safe".to_vec()))]),
        );
        let original = std::fs::read(&base.0).unwrap();
        let header = original[..HEADER_BYTES].try_into().unwrap();
        let marker = slot(&header, 2, end + frame.len() as u64, 16384);
        for cut in [0, 1, 7, 15, 23, 31, 59, 63, 64] {
            let f = Fixture::new();
            std::fs::copy(&base.0, &f.0).unwrap();
            let mut file = OpenOptions::new().write(true).open(&f.0).unwrap();
            file.seek(SeekFrom::Start(end)).unwrap();
            file.write_all(&frame).unwrap();
            file.sync_data().unwrap();
            file.seek(SeekFrom::Start(16384)).unwrap();
            file.write_all(&marker[..cut]).unwrap();
            file.sync_data().unwrap();
            drop(file);
            let db = Database::open(&f.0).unwrap();
            let mut t = db.begin().unwrap();
            assert_eq!(t.get(b"ack").unwrap().unwrap(), b"safe");
            assert_eq!(t.get(b"next").unwrap().unwrap(), b"safe");
        }
    }
}
