//! Serializable optimistic transactions over an append-only durable key/value log.
//!
//! Transactions stage writes independently. Commit validates their read/write sets and
//! predicates, syncs one complete WAL frame, and then publishes its version atomically.
//! A database has one process owner; cloned handles share that owner across threads.

use crate::resources::{ACTIVE, Budget, BufferPermit, DEPENDENCIES, RETAINED, STAGED};
pub use crate::resources::{ResourceLimits, ResourceUsage};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

#[cfg(feature = "fixed-log")]
#[path = "fixed_log.rs"]
mod fixed_log;
#[path = "group_commit.rs"]
mod group_commit;
#[cfg(feature = "fixed-log")]
use fixed_log::LogFile as WalFile;
#[cfg(not(feature = "fixed-log"))]
type WalFile = File;
#[path = "reclamation.rs"]
mod reclamation;
#[path = "retirement.rs"]
mod retirement;
#[cfg(any(feature = "commit-window", feature = "backlog-batch"))]
pub use group_commit::CommitOutcome;
pub use reclamation::{ReclamationProfile, ReclamationStatus};
pub use retirement::RetirementStatus;

const MAGIC: &[u8; 8] = b"NDTXN001";
const FRAME: &[u8; 8] = b"NDTXFR01";
const SEAL: &[u8; 8] = b"COMMIT01";
const HEADER_BYTES: usize = 48;
const PREFIX_BYTES: usize = 20;
const TRAILER_BYTES: usize = 12;
pub const MAX_KEY_BYTES: usize = 1024;
pub const MAX_VALUE_BYTES: usize = crate::MAX_RECORD_BYTES - MAX_KEY_BYTES - 4;
pub const MAX_TRANSACTION_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_TRANSACTION_WRITES: usize = 65_536;
pub const MAX_WAL_BYTES: u64 = 1024 * 1024 * 1024;
pub type Rows = Vec<(Vec<u8>, Vec<u8>)>;
pub type Result<T> = std::result::Result<T, Error>;

pub const MAX_SCAN_PAGE_ROWS: usize = 4096;
pub const MAX_SCAN_PAGE_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_SCAN_PAGE_VISITS: usize = 65_536;

#[derive(Debug, Clone, Copy)]
pub struct ScanLimits {
    pub max_rows: usize,
    /// Sum of returned key and value lengths, excluding container overhead.
    pub max_bytes: usize,
    /// Unique keys examined across stored records and staged writes, including invisible keys.
    pub max_visits: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_rows: 256,
            max_bytes: 1024 * 1024,
            max_visits: 4096,
        }
    }
}

#[derive(Debug)]
pub struct ScanPage {
    pub rows: Rows,
    /// Exclusive continuation key. None means the requested range is exhausted.
    /// An empty page may still have a continuation key when invisible keys use the visit budget.
    pub next_after: Option<Vec<u8>>,
    pub visited_entries: usize,
    pub payload_bytes: usize,
    _permit: BufferPermit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictKind {
    Write,
    Read,
    Range,
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// A complete frame may be present. Reopen and inspect before deciding to retry.
    CommitUncertain(io::Error),
    CheckpointUncertain(io::Error),
    Conflict(ConflictKind),
    Invalid(&'static str),
    Limit(&'static str),
    ResourceExhausted(&'static str),
    Locked(String),
    Poisoned,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "transaction store I/O failed: {e}"),
            Self::CommitUncertain(e) => write!(f, "commit outcome uncertain; reopen required: {e}"),
            Self::CheckpointUncertain(e) => {
                write!(f, "checkpoint outcome uncertain; reopen required: {e}")
            }
            Self::Conflict(kind) => write!(f, "transaction conflict: {kind:?}"),
            Self::Invalid(reason) => write!(f, "invalid transaction data: {reason}"),
            Self::Limit(reason) => write!(f, "transaction limit: {reason}"),
            Self::ResourceExhausted(reason) => write!(f, "resource exhausted: {reason}"),
            Self::Locked(reason) => write!(f, "database ownership lock unavailable: {reason}"),
            Self::Poisoned => write!(
                f,
                "database requires reopen after a persistence or mutex failure"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) | Self::CommitUncertain(e) | Self::CheckpointUncertain(e) => Some(e),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

#[derive(Clone)]
pub struct Database {
    inner: Arc<Inner>,
}

struct Inner {
    group: group_commit::GroupCommit,
    reclaimer: reclamation::Reclaimer,
    state: Arc<Mutex<State>>,
    writer: Mutex<()>,
    checkpoint: Mutex<()>,
    retirement: Mutex<retirement::Retirement>,
    id: [u8; 16],
    _owner: File,
    budget: Arc<Budget>,
}

#[cfg(not(feature = "lock-timing"))]
type StateGuard<'a> = MutexGuard<'a, State>;
#[cfg(feature = "lock-timing")]
struct StateGuard<'a> {
    guard: MutexGuard<'a, State>,
    start: std::time::Instant,
}
#[cfg(feature = "lock-timing")]
impl std::ops::Deref for StateGuard<'_> {
    type Target = State;
    fn deref(&self) -> &State {
        &self.guard
    }
}
#[cfg(feature = "lock-timing")]
impl std::ops::DerefMut for StateGuard<'_> {
    fn deref_mut(&mut self) -> &mut State {
        &mut self.guard
    }
}
#[cfg(feature = "lock-timing")]
thread_local! { static THREAD_HOLD: std::cell::Cell<[u64; 3]> = const { std::cell::Cell::new([0;3]) }; }
#[cfg(feature = "lock-timing")]
impl Drop for StateGuard<'_> {
    fn drop(&mut self) {
        let ns = self.start.elapsed().as_nanos() as u64;
        THREAD_HOLD.with(|v| {
            let mut a = v.get();
            a[0] += 1;
            a[1] += ns;
            a[2] = a[2].max(ns);
            v.set(a);
        });
    }
}
#[cfg(feature = "lock-timing")]
pub fn take_state_hold_timing() -> [u64; 3] {
    THREAD_HOLD.with(|v| v.replace([0; 3]))
}
fn wrap_state(guard: MutexGuard<'_, State>) -> StateGuard<'_> {
    #[cfg(feature = "lock-timing")]
    {
        StateGuard {
            guard,
            start: std::time::Instant::now(),
        }
    }
    #[cfg(not(feature = "lock-timing"))]
    {
        guard
    }
}

struct State {
    file: Option<WalFile>,
    path: PathBuf,
    sequence: u64,
    wal_bytes: u64,
    reservation_enabled: bool,
    reserved_bytes: u64,
    rows: BTreeMap<Vec<u8>, VecDeque<Version>>,
    active: BTreeMap<u64, usize>,
    history_keys: BTreeSet<Vec<u8>>,
    gc_floor: u64,
    gc_target: u64,
    gc_cursor: Option<Vec<u8>>,
    poisoned: bool,
    budget: Arc<Budget>,
}

struct RetiredVersions {
    values: Vec<Version>,
    charge: usize,
    budget: Arc<Budget>,
}
impl Drop for RetiredVersions {
    fn drop(&mut self) {
        drop(std::mem::take(&mut self.values));
        self.budget.release(RETAINED, self.charge);
    }
}

#[cfg(test)]
thread_local! { static READ_COPY_GATE: std::cell::RefCell<Option<Arc<[std::sync::Barrier; 2]>>> = const { std::cell::RefCell::new(None) }; }
fn read_copy_hook() {
    #[cfg(test)]
    if let Some(gate) = READ_COPY_GATE.with(|g| g.borrow_mut().take()) {
        gate[0].wait();
        gate[1].wait();
    }
}

enum ReadValue<'a> {
    Borrowed(&'a [u8]),
    Shared(Arc<[u8]>),
}
impl AsRef<[u8]> for ReadValue<'_> {
    fn as_ref(&self) -> &[u8] {
        match self {
            Self::Borrowed(v) => v,
            Self::Shared(v) => v,
        }
    }
}

pub(crate) struct SharedScanRow {
    key_start: usize,
    key_end: usize,
    value: Arc<[u8]>,
}
impl AsRef<[u8]> for SharedScanRow {
    fn as_ref(&self) -> &[u8] {
        &self.value
    }
}
pub(crate) struct SharedScan {
    keys: Vec<u8>,
    pub(crate) rows: Vec<SharedScanRow>,
    pub(crate) permit: BufferPermit,
}
impl SharedScan {
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&[u8], &[u8])> {
        self.rows
            .iter()
            .map(|r| (&self.keys[r.key_start..r.key_end], r.value.as_ref()))
    }
}

fn grow_scan_vec<T>(values: &mut Vec<T>, additional: usize) -> Result<()> {
    let needed = values
        .len()
        .checked_add(additional)
        .ok_or(Error::Limit("scan capacity"))?;
    if needed > values.capacity() {
        let target = values
            .capacity()
            .checked_mul(2)
            .ok_or(Error::Limit("scan capacity"))?
            .max(needed);
        values.reserve_exact(target - values.len());
    }
    Ok(())
}

struct Version {
    sequence: u64,
    value: Option<Arc<[u8]>>,
}

#[derive(Clone, PartialEq, Eq)]
struct KeyRange {
    start: Vec<u8>,
    end: Option<Vec<u8>>,
}

impl KeyRange {
    fn new(start: &[u8], end: Option<&[u8]>) -> Result<Self> {
        validate_key(start)?;
        if let Some(end) = end {
            validate_key(end)?;
            if start > end {
                return Err(Error::Invalid("reversed key range"));
            }
        }
        Ok(Self {
            start: start.to_vec(),
            end: end.map(<[u8]>::to_vec),
        })
    }
    fn bounds(&self) -> (std::ops::Bound<&[u8]>, std::ops::Bound<&[u8]>) {
        (
            Included(self.start.as_slice()),
            self.end.as_deref().map_or(Unbounded, Excluded),
        )
    }
}

pub struct Transaction {
    #[cfg(feature = "wal-prepare")]
    prepared: Option<PreparedFrame>,
    database: Database,
    snapshot: u64,
    read_only: bool,
    reads: BTreeSet<Vec<u8>>,
    ranges: Vec<KeyRange>,
    writes: BTreeMap<Vec<u8>, Option<Vec<u8>>>,
    write_bytes: usize,
    staged_charge: usize,
    dependency_charge: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct Stats {
    pub version: u64,
    pub visible_records: usize,
    pub retained_versions: usize,
    pub active_transactions: usize,
    pub wal_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct ExportInfo {
    pub version: u64,
    pub records: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct CheckpointInfo {
    /// Base snapshot version; concurrent commits may follow it in the replacement WAL.
    pub version: u64,
    /// Live records at the base snapshot version.
    pub records: usize,
    /// Original WAL length when the base snapshot was pinned.
    pub before_bytes: u64,
    /// Published WAL length including commits copied after the base snapshot.
    pub after_bytes: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CheckpointStage {
    ChunkCopied,
    Written,
    Synced,
    TailCopied,
    TailSynced,
    Published,
    Retained,
    Released,
    Reclaimed,
}

impl Database {
    /// Creates a new WAL without overwriting an existing file. The ID is caller supplied.
    pub fn create(path: impl AsRef<Path>, id: [u8; 16]) -> Result<Self> {
        Self::create_with_limits(path, id, ResourceLimits::unlimited())
    }

    pub fn create_with_limits(
        path: impl AsRef<Path>,
        id: [u8; 16],
        limits: ResourceLimits,
    ) -> Result<Self> {
        let budget = Budget::new(limits.validate()?);
        let path = database_path(path.as_ref())?;
        if path.try_exists()? {
            return Err(
                io::Error::new(io::ErrorKind::AlreadyExists, "database already exists").into(),
            );
        }
        let owner = ownership(&path)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.try_lock().map_err(|e| Error::Locked(e.to_string()))?;
        file.write_all(&wal_header(id, 0, 0))?;
        file.sync_all()?;
        Ok(Self::from_state(
            State::new(legacy_wal_file(file), path, budget),
            id,
            owner,
        ))
    }

    /// Opens an existing WAL. Incomplete final frames are removed; complete corruption is rejected.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_limits(path, ResourceLimits::unlimited())
    }

    pub fn open_with_limits(path: impl AsRef<Path>, limits: ResourceLimits) -> Result<Self> {
        let budget = Budget::new(limits.validate()?);
        let path = database_path(path.as_ref())?;
        let owner = ownership(&path)?;
        let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
        file.try_lock().map_err(|e| Error::Locked(e.to_string()))?;
        let length = file.metadata()?.len();
        if !(HEADER_BYTES as u64..=MAX_WAL_BYTES).contains(&length) {
            return Err(Error::Invalid("WAL length"));
        }
        let mut header = [0; HEADER_BYTES];
        file.read_exact(&mut header)?;
        if &header[..8] != MAGIC
            || !(read_u32(&header, 8) == 1
                || read_u32(&header, 8) == 2
                || (cfg!(feature = "fixed-log") && read_u32(&header, 8) == 3))
            || read_u32(&header, 12) != HEADER_BYTES as u32
            || read_u32(&header, 44) != crate::crc32(&header[..44])
        {
            return Err(Error::Invalid("WAL header"));
        }
        let id = header[16..32].try_into().unwrap();
        let legacy = read_u32(&header, 8) == 1;
        let base = if legacy {
            0
        } else {
            u64::from_le_bytes(header[32..40].try_into().unwrap())
        };
        let chunks = if legacy { 0 } else { read_u32(&header, 40) };
        if (legacy && header[32..44] != [0; 12]) || chunks > 65536 || (base == 0 && chunks != 0) {
            return Err(Error::Invalid("WAL checkpoint header"));
        }
        #[cfg(feature = "fixed-log")]
        let file = WalFile::open(file, header)?;
        #[cfg(feature = "fixed-log")]
        let confirmed = file.confirmed_end()?;
        #[cfg(not(feature = "fixed-log"))]
        let confirmed: Option<u64> = None;
        #[cfg(feature = "fixed-log")]
        let length = file.capacity().unwrap_or(length);
        let mut state = State::new(file, path, budget);
        state.sequence = base;
        let mut checkpoint_remaining = chunks;
        let mut previous_key: Option<Vec<u8>> = None;
        let mut offset = HEADER_BYTES as u64;
        while offset < length {
            let remaining = length - offset;
            if remaining < PREFIX_BYTES as u64 {
                if checkpoint_remaining > 0 {
                    return Err(Error::Invalid("incomplete checkpoint prefix"));
                }
                let mut tail = vec![0; remaining as usize];
                state.file.as_mut().unwrap().read_exact(&mut tail)?;
                let magic_length = tail.len().min(FRAME.len());
                if tail[..magic_length] != FRAME[..magic_length] {
                    return Err(Error::Invalid("invalid partial frame prefix"));
                }
                break;
            }
            let mut prefix = [0; PREFIX_BYTES];
            state.file.as_mut().unwrap().read_exact(&mut prefix)?;
            if confirmed.is_some_and(|end| offset >= end) && prefix == [0; PREFIX_BYTES] {
                break;
            }
            let body_length = read_u32(&prefix, 8);
            if &prefix[..8] != FRAME
                || read_u32(&prefix, 12) != !body_length
                || read_u32(&prefix, 16) != crate::crc32(&prefix[..16])
                || !(12..=MAX_TRANSACTION_BYTES).contains(&(body_length as usize))
            {
                if confirmed.is_some_and(|end| offset >= end) && checkpoint_remaining == 0 {
                    break;
                }
                return Err(Error::Invalid("WAL frame prefix"));
            }
            let frame_length = PREFIX_BYTES as u64 + u64::from(body_length) + TRAILER_BYTES as u64;
            if confirmed.is_some_and(|end| offset < end && offset + frame_length > end) {
                return Err(Error::Invalid("fixed log end splits frame"));
            }
            if remaining < frame_length {
                if checkpoint_remaining > 0 {
                    return Err(Error::Invalid("incomplete checkpoint frame"));
                }
                break;
            }
            let mut buffer = state.budget.buffer();
            let mut body_header = [0u8; 12];
            state.file.as_mut().unwrap().read_exact(&mut body_header)?;
            let operations = read_u32(&body_header, 8) as usize;
            if operations == 0 || operations > MAX_TRANSACTION_WRITES {
                if confirmed.is_some_and(|end| offset >= end) && checkpoint_remaining == 0 {
                    break;
                }
                return Err(Error::Invalid("WAL operation count"));
            }
            buffer
                .add(body_length as usize * 4 + operations * 192 + PREFIX_BYTES + TRAILER_BYTES)?;
            let mut body = vec![0; body_length as usize];
            body[..12].copy_from_slice(&body_header);
            state.file.as_mut().unwrap().read_exact(&mut body[12..])?;
            let mut trailer = [0; TRAILER_BYTES];
            state.file.as_mut().unwrap().read_exact(&mut trailer)?;
            if read_u32(&trailer, 0) != crate::crc32(&body) || &trailer[4..] != SEAL {
                if confirmed.is_some_and(|end| offset >= end) && checkpoint_remaining == 0 {
                    break;
                }
                return Err(Error::Invalid("WAL frame checksum or commit seal"));
            }
            let (sequence, writes) = decode_frame(&body)?;
            if checkpoint_remaining > 0 {
                if sequence != base
                    || writes.values().any(Option::is_none)
                    || previous_key
                        .as_ref()
                        .is_some_and(|key| key >= writes.first_key_value().unwrap().0)
                {
                    return Err(Error::Invalid("checkpoint contents"));
                }
                previous_key = writes.last_key_value().map(|(key, _)| key.clone());
                checkpoint_remaining -= 1;
            } else if Some(sequence) != state.sequence.checked_add(1) {
                return Err(Error::Invalid("WAL commit sequence"));
            }
            state.apply(sequence, writes)?;
            loop {
                drop(state.collect());
                if state.gc_target == 0 {
                    break;
                }
            }
            offset += frame_length;
        }
        if checkpoint_remaining > 0 {
            return Err(Error::Invalid("missing checkpoint frames"));
        }
        if confirmed.is_none() && offset < length {
            state.file.as_mut().unwrap().set_len(offset)?;
            state.file.as_mut().unwrap().sync_all()?;
        }
        state.file.as_mut().unwrap().seek(SeekFrom::Start(offset))?;
        #[cfg(feature = "fixed-log")]
        if confirmed.is_some() {
            state.file.as_ref().unwrap().sync_all()?;
        }
        state.wal_bytes = offset;
        Ok(Self::from_state(state, id, owner))
    }

    fn from_state(state: State, id: [u8; 16], owner: File) -> Self {
        let budget = state.budget.clone();
        let mut retirement = retirement::Retirement::new(state.path.clone(), id);
        retirement.cleanup();
        Self {
            inner: Arc::new(Inner {
                group: group_commit::GroupCommit::default(),
                reclaimer: reclamation::Reclaimer::default(),
                state: Arc::new(Mutex::new(state)),
                writer: Mutex::new(()),
                checkpoint: Mutex::new(()),
                retirement: Mutex::new(retirement),
                id,
                _owner: owner,
                budget,
            }),
        }
    }

    fn state(&self) -> Result<StateGuard<'_>> {
        #[cfg(feature = "lock-timing")]
        let started = std::time::Instant::now();
        let state = self.inner.state.lock().map_err(|_| Error::Poisoned)?;
        #[cfg(feature = "lock-timing")]
        THREAD_LOCK_WAIT.with(|total| {
            total.set(
                total
                    .get()
                    .saturating_add(started.elapsed().as_nanos() as u64),
            )
        });
        if state.poisoned || self.inner.writer.is_poisoned() {
            return Err(Error::Poisoned);
        }
        Ok(wrap_state(state))
    }

    fn persist_wal<'a>(
        &'a self,
        mut state: StateGuard<'a>,
        persist: impl FnOnce(&mut WalFile) -> io::Result<()>,
    ) -> io::Result<StateGuard<'a>> {
        let mut file = state
            .file
            .take()
            .ok_or_else(|| io::Error::other("missing WAL"))?;
        drop(state);
        let outcome = persist(&mut file);
        let mut state = self.state().map_err(io::Error::other)?;
        #[cfg(feature = "lock-timing")]
        detail_mark(4);
        state.file = Some(file);
        if let Err(error) = outcome {
            state.poisoned = true;
            return Err(error);
        }
        Ok(state)
    }

    pub fn id(&self) -> [u8; 16] {
        self.inner.id
    }

    /// Enables best-effort WAL allocation without changing EOF or durability.
    /// Windows defaults to enabled; other platforms retain normal allocation.
    pub fn set_log_reservation(&self, enabled: bool) -> Result<()> {
        self.state()?.reservation_enabled = enabled;
        Ok(())
    }

    /// Replaces the WAL with a synced snapshot and commits made during its construction.
    /// Active transactions retain their in-memory history and may still commit.
    pub fn checkpoint(&self) -> Result<CheckpointInfo> {
        self.checkpoint_with(|_| Ok(()))
    }

    /// Experimental fixed-length WAL conversion/expansion through atomic checkpoint replacement.
    #[cfg(feature = "fixed-log")]
    pub fn checkpoint_fixed_log(&self, capacity: u64) -> Result<CheckpointInfo> {
        fixed_log::validate_capacity(capacity)?;
        self.checkpoint_impl(false, Some(capacity), |_| Ok(()))
    }
    #[cfg(feature = "fixed-log")]
    pub fn fixed_log_capacity(&self) -> Result<Option<u64>> {
        Ok(self.state()?.file.as_ref().and_then(|f| f.capacity()))
    }

    #[cfg(all(feature = "fixed-log", feature = "lock-timing"))]
    #[doc(hidden)]
    pub fn fixed_log_metrics(&self) -> Result<Option<[u64; 3]>> {
        self.state()?
            .file
            .as_ref()
            .ok_or(Error::ResourceExhausted("writer active"))?
            .metrics()
            .map_err(Error::Io)
    }
    pub fn checkpoint_deferred_reclaim(&self) -> Result<CheckpointInfo> {
        self.checkpoint_impl(true, None, |_| Ok(()))
    }

    pub fn reclaim_checkpoint(&self) -> Result<()> {
        let _checkpoint = self.inner.checkpoint.lock().map_err(|_| Error::Poisoned)?;
        self.inner
            .retirement
            .lock()
            .map_err(|_| Error::Poisoned)?
            .cleanup();
        Ok(())
    }

    /// Opt-in bounded old-WAL retention. The limit includes reserved WAL bytes.
    pub fn set_checkpoint_retirement(&self, enabled: bool, max_bytes: u64) -> Result<()> {
        if max_bytes > MAX_WAL_BYTES {
            return Err(Error::Limit("retirement bytes"));
        }
        let mut retirement = self.inner.retirement.lock().map_err(|_| Error::Poisoned)?;
        retirement.status.enabled = enabled;
        retirement.status.max_bytes = max_bytes;
        Ok(())
    }

    pub fn checkpoint_retirement_status(&self) -> Result<RetirementStatus> {
        Ok(self
            .inner
            .retirement
            .lock()
            .map_err(|_| Error::Poisoned)?
            .status
            .clone())
    }

    fn checkpoint_with(
        &self,
        hook: impl FnMut(CheckpointStage) -> io::Result<()>,
    ) -> Result<CheckpointInfo> {
        self.checkpoint_impl(false, None, hook)
    }

    fn checkpoint_impl(
        &self,
        defer_reclaim: bool,
        _fixed_capacity: Option<u64>,
        mut hook: impl FnMut(CheckpointStage) -> io::Result<()>,
    ) -> Result<CheckpointInfo> {
        let _checkpoint = self.inner.checkpoint.lock().map_err(|_| Error::Poisoned)?;
        let mut retirement = self.inner.retirement.lock().map_err(|_| Error::Poisoned)?;
        retirement.cleanup();
        let writer = self.inner.writer.lock().map_err(|_| Error::Poisoned)?;
        let pinned = self.begin()?;
        let state = self.state()?;
        let chunk_limit = MAX_TRANSACTION_BYTES.min(
            state
                .budget
                .buffer_limit()
                .saturating_sub(PREFIX_BYTES + TRAILER_BYTES)
                / 16,
        );
        if chunk_limit < 12 {
            return Err(Error::ResourceExhausted("checkpoint buffer"));
        }
        let sequence = state.sequence;
        let before_bytes = state.wal_bytes;
        #[cfg(feature = "fixed-log")]
        let fixed_capacity =
            _fixed_capacity.or_else(|| state.file.as_ref().and_then(|f| f.capacity()));
        let last_key = state.rows.last_key_value().map(|(key, _)| key.clone());
        let path = state.path.clone();
        let mut buffer = state.budget.buffer();
        drop(state);
        drop(writer);
        let temporary = temporary_path(&path);
        let mut replacement = Some(
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&temporary)?,
        );
        let mut attempted_publication = false;
        let mut publication_guard = None;
        let result = (|| -> Result<CheckpointInfo> {
            let output = replacement.as_mut().unwrap();
            output.write_all(&wal_header(self.inner.id, sequence, 0))?;
            let mut after_key: Option<Vec<u8>> = None;
            let mut chunks = 0_u32;
            let mut records = 0;
            loop {
                buffer.clear();
                let mut pending = BTreeMap::new();
                let mut bytes = 12;
                let mut visited = 0;
                {
                    let state = self.state()?;
                    let lower = after_key.as_deref().map_or(Unbounded, Excluded);
                    let mut next = after_key.clone();
                    for (key, versions) in state
                        .rows
                        .range::<[u8], _>((lower, Unbounded))
                        .take_while(|(key, _)| last_key.as_ref().is_some_and(|last| *key <= last))
                        .take(256)
                    {
                        if let Some(value) =
                            visible(versions, pinned.snapshot).and_then(|v| v.value.as_deref())
                        {
                            let size = operation_size(key, Some(value));
                            if size + 12 > chunk_limit {
                                return Err(Error::ResourceExhausted("checkpoint record"));
                            }
                            if bytes + size > chunk_limit {
                                break;
                            }
                            if pending.is_empty() {
                                buffer.add(24 + PREFIX_BYTES + TRAILER_BYTES)?;
                            }
                            buffer.add(128 + key.len() + value.len() + 2 * size)?;
                            pending.insert(key.clone(), Some(value.to_vec()));
                            bytes += size;
                        }
                        next = Some(key.clone());
                        visited += 1;
                    }
                    after_key = next;
                }
                if visited == 0 {
                    break;
                }
                hook(CheckpointStage::ChunkCopied)?;
                if !pending.is_empty() {
                    output.write_all(&encode_frame(sequence, &pending))?;
                    records += pending.len();
                    chunks = chunks
                        .checked_add(1)
                        .filter(|count| *count <= 65_536)
                        .ok_or(Error::Limit("checkpoint chunks"))?;
                }
            }
            let after = output.stream_position()?;
            if after > MAX_WAL_BYTES {
                return Err(Error::Limit("checkpoint bytes"));
            }
            // Counting frames first would require another complete locked traversal.
            output.seek(SeekFrom::Start(0))?;
            output.write_all(&wal_header(self.inner.id, sequence, chunks))?;
            hook(CheckpointStage::Written)?;
            output.sync_all()?;
            hook(CheckpointStage::Synced)?;
            buffer.clear();
            retirement.prepare();

            #[cfg(feature = "lock-timing")]
            let timing_start = std::time::Instant::now();
            publication_guard = Some(self.inner.writer.lock().map_err(|_| Error::Poisoned)?);
            #[cfg(feature = "lock-timing")]
            let timing_acquired = std::time::Instant::now();
            let end = self.state()?.wal_bytes;
            let tail_bytes = end
                .checked_sub(before_bytes)
                .ok_or(Error::Invalid("checkpoint WAL changed"))?;
            let after = after
                .checked_add(tail_bytes)
                .filter(|size| *size <= MAX_WAL_BYTES)
                .ok_or(Error::Limit("checkpoint bytes"))?;
            if tail_bytes > 0 {
                output.seek(SeekFrom::End(0))?;
                self.copy_checkpoint_tail(output, before_bytes, end, &mut buffer)?;
            }

            #[cfg(feature = "lock-timing")]
            let timing_copied = std::time::Instant::now();
            hook(CheckpointStage::TailCopied)?;
            if tail_bytes > 0 {
                output.sync_all()?;
            }

            #[cfg(feature = "lock-timing")]
            let timing_synced = std::time::Instant::now();
            #[cfg(feature = "fixed-log")]
            if let Some(capacity) = fixed_capacity {
                buffer.add(32768)?;
                fixed_log::initialize(output, after, capacity)?;
            }
            hook(CheckpointStage::TailSynced)?;
            let charged_bytes = {
                let state = self.state()?;
                let bytes = state.wal_bytes.max(state.reserved_bytes);
                #[cfg(feature = "fixed-log")]
                let bytes = bytes.max(
                    state
                        .file
                        .as_ref()
                        .and_then(|f| f.capacity())
                        .map_or(0, |n| n + fixed_log::METADATA_BYTES),
                );
                bytes
            };
            retirement.retain(charged_bytes);
            hook(CheckpointStage::Retained)?;
            #[cfg(feature = "checkpoint-handle-reuse")]
            {
                output
                    .try_lock()
                    .map_err(|e| Error::Locked(e.to_string()))?;
                output.seek(SeekFrom::Start(after))?;
            }
            #[cfg(not(feature = "checkpoint-handle-reuse"))]
            drop(replacement.take());
            let previous = { self.state()?.file.take() };
            drop(previous);

            #[cfg(feature = "lock-timing")]
            let timing_closed = std::time::Instant::now();
            #[cfg(feature = "lock-timing")]
            let replace_started = diagnostic_now_ns();
            attempted_publication = true;
            replace_file(&temporary, &path)?;

            #[cfg(feature = "lock-timing")]
            CHECKPOINT_REPLACE_TRACE
                .with(|trace| trace.set([replace_started, diagnostic_now_ns()]));

            #[cfg(feature = "lock-timing")]
            let timing_replaced = std::time::Instant::now();
            hook(CheckpointStage::Published)?;
            #[cfg(not(feature = "checkpoint-handle-reuse"))]
            let file = {
                let mut file = OpenOptions::new().read(true).write(true).open(&path)?;
                file.try_lock().map_err(|e| Error::Locked(e.to_string()))?;
                file.seek(SeekFrom::Start(after))?;
                file
            };
            let mut state = self.state()?;
            // Reuse reduced the switch pause but regressed mixed-load p99, so it remains opt-in.
            #[cfg(feature = "checkpoint-handle-reuse")]
            {
                let file = replacement.take().unwrap();
                state.file = Some(wrap_checkpoint_file(file, after)?);
            }
            #[cfg(not(feature = "checkpoint-handle-reuse"))]
            {
                state.file = Some(wrap_checkpoint_file(file, after)?);
            }
            state.reserved_bytes = 0;
            state.wal_bytes = after;
            #[cfg(feature = "lock-timing")]
            CHECKPOINT_TIMING.with(|value| {
                value.set([
                    timing_acquired.duration_since(timing_start).as_nanos() as u64,
                    timing_copied.duration_since(timing_acquired).as_nanos() as u64,
                    timing_synced.duration_since(timing_copied).as_nanos() as u64,
                    timing_closed.duration_since(timing_synced).as_nanos() as u64,
                    timing_replaced.duration_since(timing_closed).as_nanos() as u64,
                    timing_replaced.elapsed().as_nanos() as u64,
                    timing_acquired.elapsed().as_nanos() as u64,
                    tail_bytes,
                ])
            });
            Ok(CheckpointInfo {
                version: sequence,
                records,
                before_bytes,
                after_bytes: after,
            })
        })();
        let result = match result {
            Ok(info) => Ok(info),
            Err(error) => {
                drop(replacement);
                let _ = std::fs::remove_file(&temporary);
                if attempted_publication {
                    self.state()?.poisoned = true;
                    let cause = match error {
                        Error::Io(e) => e,
                        other => io::Error::other(other),
                    };
                    Err(Error::CheckpointUncertain(cause))
                } else {
                    Err(error)
                }
            }
        };
        drop(publication_guard);
        if result.is_ok() {
            hook(CheckpointStage::Released)?;
        }
        if !defer_reclaim || result.is_err() {
            retirement.cleanup();
            if result.is_ok() {
                hook(CheckpointStage::Reclaimed)?;
            }
        }
        result
    }

    fn copy_checkpoint_tail(
        &self,
        output: &mut File,
        start: u64,
        end: u64,
        permit: &mut BufferPermit,
    ) -> Result<()> {
        let size = (end - start)
            .min(64 * 1024)
            .min(self.inner.budget.buffer_limit() as u64) as usize;
        permit.add(size)?;
        let mut bytes = vec![0; size];
        let mut source = self
            .state()?
            .file
            .as_ref()
            .ok_or(Error::Poisoned)?
            .try_clone()?;
        // Cloned handles share the cursor; the writer gate stays held until it is restored.
        let result = (|| -> io::Result<()> {
            source.seek(SeekFrom::Start(start))?;
            let mut remaining = end - start;
            while remaining > 0 {
                let count = remaining.min(size as u64) as usize;
                source.read_exact(&mut bytes[..count])?;
                output.write_all(&bytes[..count])?;
                remaining -= count as u64;
            }
            Ok(())
        })();
        if let Err(error) = source.seek(SeekFrom::Start(end)) {
            self.state()?.poisoned = true;
            return Err(Error::CommitUncertain(error));
        }
        result.map_err(Error::Io)
    }

    pub fn begin(&self) -> Result<Transaction> {
        self.begin_mode(false)
    }

    pub fn begin_read(&self) -> Result<Transaction> {
        self.begin_mode(true)
    }

    /// Reclaims one bounded portion; true means another portion may remain.
    pub fn collect_garbage(&self) -> Result<bool> {
        let mut state = self.state()?;
        let retired = state.collect();
        let pending = state.gc_pending();
        drop(state);
        drop(retired);
        Ok(pending)
    }

    pub fn set_background_reclamation(&self, enabled: bool) -> Result<()> {
        self.inner.reclaimer.set(enabled, &self.inner.state)?;
        Ok(())
    }

    pub fn reclamation_profile(&self) -> ReclamationProfile {
        self.inner.reclaimer.profile()
    }

    pub fn reclamation_status(&self) -> ReclamationStatus {
        self.inner.reclaimer.status()
    }

    pub fn reclamation_pending(&self) -> Result<bool> {
        let state = self.state()?;
        Ok(state.gc_pending() || self.inner.reclaimer.status().busy)
    }

    pub fn background_reclamation_enabled(&self) -> bool {
        self.inner.reclaimer.enabled()
    }

    fn begin_mode(&self, read_only: bool) -> Result<Transaction> {
        let mut state = self.state()?;
        let retired = (!self.inner.reclaimer.enabled()).then(|| state.collect());
        let result = (|| {
            state.budget.replace(ACTIVE, 0, 1)?;
            let snapshot = state.sequence;
            *state.active.entry(snapshot).or_default() += 1;
            Ok(Transaction {
                #[cfg(feature = "wal-prepare")]
                prepared: None,
                database: self.clone(),
                snapshot,
                read_only,
                reads: BTreeSet::new(),
                ranges: Vec::new(),
                writes: BTreeMap::new(),
                write_bytes: 12,
                staged_charge: 0,
                dependency_charge: 0,
            })
        })();
        drop(state);
        drop(retired);
        result
    }

    pub fn current_version(&self) -> Result<u64> {
        Ok(self.state()?.sequence)
    }

    pub fn stats(&self) -> Result<Stats> {
        let state = self.state()?;
        Ok(Stats {
            version: state.sequence,
            visible_records: state
                .rows
                .values()
                .filter(|v| v.back().unwrap().value.is_some())
                .count(),
            retained_versions: state.rows.values().map(VecDeque::len).sum(),
            active_transactions: state.active.values().sum(),
            wal_bytes: state.wal_bytes,
        })
    }

    pub fn resource_usage(&self) -> Result<ResourceUsage> {
        self.inner.budget.snapshot()
    }

    /// Writes committed values in binary key order using the existing immutable NDB format.
    /// Values must carry their own IDs if a consumer needs them; keys are not exported.
    pub fn export_values(
        &self,
        path: impl AsRef<Path>,
        generation: [u8; 16],
        start: &[u8],
        end: Option<&[u8]>,
    ) -> Result<ExportInfo> {
        let mut transaction = self.begin_read()?;
        let mut scan = transaction.scan_shared(start, end, false)?;
        scan.permit.add(64 * 1024)?;
        crate::create(path, generation, &scan.rows)?;
        Ok(ExportInfo {
            version: transaction.version(),
            records: scan.rows.len(),
        })
    }
}

impl Transaction {
    #[cfg(feature = "wal-prepare")]
    fn prepare_log(&mut self) -> Result<()> {
        if self.writes.is_empty() || self.prepared.is_some() {
            return Ok(());
        }
        let permit = self.reserve_buffer(
            self.write_bytes * 4 + PREFIX_BYTES + TRAILER_BYTES + 64 * self.writes.len(),
        )?;
        let frame = prepare_frame(&self.writes);
        self.prepared = Some(PreparedFrame {
            frame,
            permit: Some(permit),
        });
        Ok(())
    }

    fn commit_frame(&mut self, sequence: u64, extra: usize) -> Result<(Vec<u8>, BufferPermit)> {
        #[cfg(feature = "lock-timing")]
        let _cost = CommitCost(1, diagnostic_now_ns());
        #[cfg(feature = "wal-prepare")]
        if let Some(mut prepared) = self.prepared.take() {
            finish_frame(&mut prepared.frame, sequence);
            return Ok((
                std::mem::take(&mut prepared.frame),
                prepared.permit.take().unwrap(),
            ));
        }
        let permit =
            self.reserve_buffer(self.write_bytes * 4 + PREFIX_BYTES + TRAILER_BYTES + extra)?;
        Ok((encode_frame(sequence, &self.writes), permit))
    }

    pub(crate) fn reserve_buffer(&self, bytes: usize) -> Result<BufferPermit> {
        let mut permit = self.database.inner.budget.buffer();
        permit.add(bytes)?;
        Ok(permit)
    }

    fn register_range(&mut self, range: KeyRange) -> Result<()> {
        if self.read_only {
            return Ok(());
        }
        if !self.ranges.contains(&range) {
            let charge = 96 + range.start.len() + range.end.as_ref().map_or(0, Vec::len);
            self.database
                .inner
                .budget
                .replace(DEPENDENCIES, 0, charge)?;
            self.dependency_charge += charge;
            self.ranges.push(range);
        }
        Ok(())
    }
    pub fn version(&self) -> u64 {
        self.snapshot
    }

    pub fn get(&mut self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        validate_key(key)?;
        let state = self.database.state()?;
        if let Some(value) = self.writes.get(key) {
            let _permit = self.reserve_buffer(value.as_ref().map_or(0, Vec::len))?;
            drop(state);
            return Ok(value.clone());
        }
        if !self.read_only && !self.reads.contains(key) {
            let charge = 96 + key.len();
            self.database
                .inner
                .budget
                .replace(DEPENDENCIES, 0, charge)?;
            self.dependency_charge += charge;
            self.reads.insert(key.to_vec());
        }
        let value = state
            .rows
            .get(key)
            .and_then(|versions| visible(versions, self.snapshot))
            .and_then(|v| v.value.clone());
        let _permit = self.reserve_buffer(value.as_ref().map_or(0, |v| v.len()))?;
        drop(state);
        read_copy_hook();
        Ok(value.map(|v| v.to_vec()))
    }

    /// Reads [start, end), with no upper bound when end is None; includes staged writes.
    pub fn scan(&mut self, start: &[u8], end: Option<&[u8]>) -> Result<Rows> {
        Ok(self.scan_reserved(start, end)?.0)
    }

    pub(crate) fn scan_reserved(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
    ) -> Result<(Rows, BufferPermit)> {
        let range = KeyRange::new(start, end)?;
        self.register_range(range.clone())?;
        let state = self.database.state()?;
        let mut permit = state.budget.buffer();
        let mut result = Vec::new();
        let mut stored = state.rows.range::<[u8], _>(range.bounds()).peekable();
        let mut staged = self.writes.range::<[u8], _>(range.bounds()).peekable();
        loop {
            let (key, value) = match (stored.peek(), staged.peek()) {
                (Some((stored_key, _)), Some((staged_key, _))) if stored_key == staged_key => {
                    stored.next();
                    let (key, value) = staged.next().unwrap();
                    (key, value.as_deref().map(ReadValue::Borrowed))
                }
                (Some((stored_key, _)), Some((staged_key, _))) if staged_key < stored_key => {
                    let (key, value) = staged.next().unwrap();
                    (key, value.as_deref().map(ReadValue::Borrowed))
                }
                (None, Some(_)) => {
                    let (key, value) = staged.next().unwrap();
                    (key, value.as_deref().map(ReadValue::Borrowed))
                }
                (Some(_), _) => {
                    let (key, versions) = stored.next().unwrap();
                    let value = visible(versions, self.snapshot)
                        .and_then(|version| version.value.clone())
                        .map(ReadValue::Shared);
                    (key, value)
                }
                (None, None) => break,
            };
            if let Some(value) = value {
                permit.add(key.len() + value.as_ref().len() + 160)?;
                if result.len() == result.capacity() {
                    // A one-row initial capacity keeps growth within the per-row reservation.
                    result.reserve_exact(result.capacity().max(1));
                }
                result.push((key.clone(), value));
            }
        }
        permit.add(result.len() * 64)?;
        drop(state);
        read_copy_hook();
        Ok((
            result
                .into_iter()
                .map(|(key, value)| (key, value.as_ref().to_vec()))
                .collect(),
            permit,
        ))
    }

    pub(crate) fn scan_shared(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        include_keys: bool,
    ) -> Result<SharedScan> {
        let range = KeyRange::new(start, end)?;
        self.register_range(range.clone())?;
        let state = self.database.state()?;
        let mut permit = state.budget.buffer();
        let mut keys = Vec::new();
        let mut result = Vec::new();
        let mut stored = state.rows.range::<[u8], _>(range.bounds()).peekable();
        let mut staged = self.writes.range::<[u8], _>(range.bounds()).peekable();
        loop {
            let (key, value) = match (stored.peek(), staged.peek()) {
                (Some((stored_key, _)), Some((staged_key, _))) if stored_key == staged_key => {
                    stored.next();
                    let (key, value) = staged.next().unwrap();
                    (key, value.as_deref().map(ReadValue::Borrowed))
                }
                (Some((stored_key, _)), Some((staged_key, _))) if staged_key < stored_key => {
                    let (key, value) = staged.next().unwrap();
                    (key, value.as_deref().map(ReadValue::Borrowed))
                }
                (None, Some(_)) => {
                    let (key, value) = staged.next().unwrap();
                    (key, value.as_deref().map(ReadValue::Borrowed))
                }
                (Some(_), _) => {
                    let (key, versions) = stored.next().unwrap();
                    let value = visible(versions, self.snapshot)
                        .and_then(|version| version.value.clone())
                        .map(ReadValue::Shared);
                    (key, value)
                }
                (None, None) => break,
            };
            if let Some(value) = value {
                let key_len = if include_keys { key.len() } else { 0 };
                let charge = key_len
                    .checked_mul(3)
                    .and_then(|n| n.checked_add(value.as_ref().len()))
                    .and_then(|n| n.checked_add(160))
                    .ok_or(Error::Limit("scan capacity"))?;
                // Shared payloads remain charged here even if history reclamation releases its charge.
                permit.add(charge)?;
                grow_scan_vec(&mut result, 1)?;
                grow_scan_vec(&mut keys, key_len)?;
                let key_start = keys.len();
                if include_keys {
                    keys.extend_from_slice(key);
                }
                let value = match value {
                    ReadValue::Shared(value) => value,
                    ReadValue::Borrowed(value) => Arc::from(value),
                };
                result.push(SharedScanRow {
                    key_start,
                    key_end: keys.len(),
                    value,
                });
            }
        }
        drop(state);
        read_copy_hook();
        Ok(SharedScan {
            keys,
            rows: result,
            permit,
        })
    }

    /// Reads a bounded page of [start, end), optionally continuing strictly after a key.
    /// Continue on this transaction with unchanged bounds to retain the same snapshot.
    /// Staged writes are read at each call; changing them between pages changes that local view.
    /// A visible record larger than max_bytes is rejected rather than skipped.
    /// In writable transactions the entire range remains a commit dependency, including continuation pages.
    pub fn scan_page(
        &mut self,
        start: &[u8],
        end: Option<&[u8]>,
        after: Option<&[u8]>,
        limits: ScanLimits,
    ) -> Result<ScanPage> {
        if !(1..=MAX_SCAN_PAGE_ROWS).contains(&limits.max_rows)
            || !(1..=MAX_SCAN_PAGE_BYTES).contains(&limits.max_bytes)
            || !(1..=MAX_SCAN_PAGE_VISITS).contains(&limits.max_visits)
        {
            return Err(Error::Limit("scan page limits"));
        }
        let range = KeyRange::new(start, end)?;
        if let Some(key) = after {
            validate_key(key)?;
            if key < start || end.is_some_and(|end| key >= end) {
                return Err(Error::Invalid("scan continuation outside range"));
            }
        }
        self.register_range(range)?;
        let state = self.database.state()?;
        let mut permit = state.budget.buffer();
        let lower = after.map_or(Included(start), Excluded);
        let upper = end.map_or(Unbounded, Excluded);
        let mut stored = state.rows.range::<[u8], _>((lower, upper)).peekable();
        let mut staged = self.writes.range::<[u8], _>((lower, upper)).peekable();
        let mut rows = Vec::new();
        let mut sources = Vec::new();
        let mut payload_bytes = 0;
        let mut visited_entries = 0;
        let mut last_consumed: Option<&Vec<u8>> = None;
        let mut deferred = false;

        while rows.len() < limits.max_rows && visited_entries < limits.max_visits {
            let (key, value) = match (stored.peek(), staged.peek()) {
                (Some((stored_key, _)), Some((staged_key, _))) if stored_key == staged_key => {
                    stored.next();
                    let (key, value) = staged.next().unwrap();
                    (key, value.as_deref().map(ReadValue::Borrowed))
                }
                (Some((stored_key, _)), Some((staged_key, _))) if staged_key < stored_key => {
                    let (key, value) = staged.next().unwrap();
                    (key, value.as_deref().map(ReadValue::Borrowed))
                }
                (None, Some(_)) => {
                    let (key, value) = staged.next().unwrap();
                    (key, value.as_deref().map(ReadValue::Borrowed))
                }
                (Some(_), _) => {
                    let (key, versions) = stored.next().unwrap();
                    (
                        key,
                        visible(versions, self.snapshot)
                            .and_then(|version| version.value.clone())
                            .map(ReadValue::Shared),
                    )
                }
                (None, None) => break,
            };
            visited_entries += 1;
            if let Some(value) = value {
                let size = key.len() + value.as_ref().len();
                if size > limits.max_bytes {
                    return Err(Error::Limit("scan record exceeds page bytes"));
                }
                if size > limits.max_bytes - payload_bytes {
                    // Advancing past this key would silently omit a record on the next page.
                    deferred = true;
                    break;
                }
                permit.add(size + 96)?;
                rows.push((key.clone(), Vec::new()));
                sources.push(value);
                payload_bytes += size;
            }
            last_consumed = Some(key);
        }
        let more = deferred || stored.peek().is_some() || staged.peek().is_some();
        if more {
            permit.add(last_consumed.map_or(0, Vec::len))?;
        }
        let next_after = if more { last_consumed.cloned() } else { None };
        drop(state);
        read_copy_hook();
        for (row, value) in rows.iter_mut().zip(sources) {
            row.1 = value.as_ref().to_vec();
        }
        Ok(ScanPage {
            rows,
            next_after,
            visited_entries,
            payload_bytes,
            _permit: permit,
        })
    }

    pub fn put(&mut self, key: &[u8], value: &[u8]) -> Result<()> {
        if value.len() > MAX_VALUE_BYTES {
            return Err(Error::Limit("value bytes"));
        }
        self.stage(key, Some(value))
    }

    pub fn delete(&mut self, key: &[u8]) -> Result<()> {
        self.stage(key, None)
    }

    fn stage(&mut self, key: &[u8], value: Option<&[u8]>) -> Result<()> {
        if self.read_only {
            return Err(Error::Invalid("read-only transaction"));
        }
        validate_key(key)?;
        let previous = self
            .writes
            .get(key)
            .map_or(0, |v| operation_size(key, v.as_deref()));
        let size = self.write_bytes - previous + operation_size(key, value);
        if size > MAX_TRANSACTION_BYTES {
            return Err(Error::Limit("transaction bytes"));
        }
        if !self.writes.contains_key(key) && self.writes.len() == MAX_TRANSACTION_WRITES {
            return Err(Error::Limit("transaction writes"));
        }
        let previous_charge = self.writes.get(key).map_or(0, |value| {
            128 + key.len() + value.as_ref().map_or(0, Vec::len)
        });
        let charge = 128 + key.len() + value.map_or(0, <[u8]>::len);
        let _buffer = self.reserve_buffer(charge)?;
        self.database
            .inner
            .budget
            .replace(STAGED, previous_charge, charge)?;
        self.staged_charge = self.staged_charge - previous_charge + charge;
        self.writes.insert(key.to_vec(), value.map(<[u8]>::to_vec));
        self.write_bytes = size;
        Ok(())
    }

    /// Consumes the transaction. A conflict applies no writes. Read-only commits return the snapshot version.
    pub fn commit(self) -> Result<u64> {
        #[cfg(feature = "lock-timing")]
        {
            take_commit_trace();
            take_commit_detail();
            take_commit_costs();
            detail_mark(7);
            trace_mark(0);
        }
        let database = self.database.clone();
        if !self.writes.is_empty() && database.inner.group.enabled() {
            return database.inner.group.submit(self);
        }
        self.commit_single()
    }

    fn commit_single(self) -> Result<u64> {
        let database = self.database.clone();
        self.commit_with(|file, frame| {
            file.write_all(frame)?;
            #[cfg(feature = "lock-timing")]
            trace_mark(5);
            database.inner.group.record_sync();
            #[cfg(feature = "lock-timing")]
            sync_start_mark();
            let result = file.sync_all();
            #[cfg(feature = "lock-timing")]
            trace_mark(6);
            result
        })
    }

    fn validate_commit(&self, state: &State, earlier: &BTreeSet<&[u8]>) -> Result<()> {
        #[cfg(feature = "lock-timing")]
        let _cost = CommitCost(0, diagnostic_now_ns());
        for key in self.writes.keys() {
            if (state.sequence != self.snapshot && state.changed(key, self.snapshot))
                || earlier.contains(key.as_slice())
            {
                return Err(Error::Conflict(ConflictKind::Write));
            }
        }
        for key in &self.reads {
            if (state.sequence != self.snapshot && state.changed(key, self.snapshot))
                || earlier.contains(key.as_slice())
            {
                return Err(Error::Conflict(ConflictKind::Read));
            }
        }
        for range in &self.ranges {
            // Same-group writes are not reflected in state.sequence until publication.
            if (state.sequence != self.snapshot
                && state
                    .rows
                    .range::<[u8], _>(range.bounds())
                    .any(|(_, v)| v.back().unwrap().sequence > self.snapshot))
                || earlier.range::<[u8], _>(range.bounds()).next().is_some()
            {
                return Err(Error::Conflict(ConflictKind::Range));
            }
        }
        Ok(())
    }

    fn commit_with(
        mut self,
        persist: impl FnOnce(&mut WalFile, &[u8]) -> io::Result<()>,
    ) -> Result<u64> {
        let database = self.database.clone();
        if self.writes.is_empty() {
            drop(database.state()?);
            return Ok(self.snapshot);
        }
        #[cfg(feature = "lock-timing")]
        {
            detail_mark(1);
            trace_mark(1);
            trace_mark(2);
        }
        #[cfg(feature = "wal-prepare")]
        self.prepare_log()?;
        let _writer = database.inner.writer.lock().map_err(|_| Error::Poisoned)?;
        #[cfg(feature = "lock-timing")]
        trace_mark(3);
        let mut state = database.state()?;
        #[cfg(feature = "lock-timing")]
        detail_mark(2);
        let validation = self.validate_commit(&state, &BTreeSet::new());
        validation?;
        let sequence = state
            .sequence
            .checked_add(1)
            .ok_or(Error::Limit("commit sequence"))?;
        state.check_retained(&self.writes)?;
        let (frame, _buffer) = self.commit_frame(sequence, 0)?;
        let bytes = state
            .wal_bytes
            .checked_add(frame.len() as u64)
            .ok_or(Error::Limit("WAL bytes"))?;
        if bytes > MAX_WAL_BYTES {
            return Err(Error::Limit("WAL bytes; checkpoint support required"));
        }
        state.check_log_capacity(bytes)?;
        #[cfg(feature = "lock-timing")]
        detail_mark(3);
        state.reserve_log(bytes);
        #[cfg(feature = "lock-timing")]
        trace_mark(4);
        state = database
            .persist_wal(state, |file| persist(file, &frame))
            .map_err(Error::CommitUncertain)?;
        if let Err(error) = state.apply(sequence, std::mem::take(&mut self.writes)) {
            state.poisoned = true;
            return Err(Error::CommitUncertain(io::Error::other(error)));
        }
        state.wal_bytes = bytes;
        #[cfg(feature = "lock-timing")]
        trace_mark(7);
        Ok(sequence)
    }

    pub fn rollback(self) {}
}

impl Drop for Transaction {
    fn drop(&mut self) {
        self.database.inner.budget.release(ACTIVE, 1);
        self.database
            .inner
            .budget
            .release(STAGED, self.staged_charge);
        self.database
            .inner
            .budget
            .release(DEPENDENCIES, self.dependency_charge);
        if let Ok(state) = self.database.inner.state.lock() {
            let mut state = wrap_state(state);
            if let Some(count) = state.active.get_mut(&self.snapshot) {
                *count -= 1;
                if *count == 0 {
                    state.active.remove(&self.snapshot);
                }
            }
            let retired = (!self.database.inner.reclaimer.enabled()).then(|| state.collect());
            drop(state);
            drop(retired);
        }
    }
}

fn database_path(path: &Path) -> io::Result<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path);
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing database filename"))?;
    Ok(std::fs::canonicalize(parent)?.join(name))
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

fn ownership(path: &Path) -> Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(with_suffix(path, ".owner"))?;
    file.try_lock().map_err(|e| Error::Locked(e.to_string()))?;
    Ok(file)
}

fn temporary_path(path: &Path) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    with_suffix(
        path,
        &format!(".checkpoint-{}-{time}-{next}.tmp", std::process::id()),
    )
}

fn wal_header(id: [u8; 16], base: u64, chunks: u32) -> [u8; HEADER_BYTES] {
    let mut header = [0_u8; HEADER_BYTES];
    header[..8].copy_from_slice(MAGIC);
    header[8..12].copy_from_slice(&2_u32.to_le_bytes());
    header[12..16].copy_from_slice(&(HEADER_BYTES as u32).to_le_bytes());
    header[16..32].copy_from_slice(&id);
    header[32..40].copy_from_slice(&base.to_le_bytes());
    header[40..44].copy_from_slice(&chunks.to_le_bytes());
    let crc = crate::crc32(&header[..44]);
    header[44..].copy_from_slice(&crc.to_le_bytes());
    header
}

#[cfg(windows)]
fn replace_file(source: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn MoveFileExW(source: *const u16, target: *const u16, flags: u32) -> i32;
    }
    let source: Vec<_> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let target: Vec<_> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    // Cross-volume copy-and-delete cannot provide the same publication boundary.
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), 0x1 | 0x8) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(feature = "lock-timing")]
#[doc(hidden)]
pub fn diagnostic_replace_file(source: &Path, target: &Path) -> io::Result<()> {
    replace_file(source, target)
}

#[cfg(not(windows))]
fn replace_file(source: &Path, target: &Path) -> io::Result<()> {
    std::fs::rename(source, target)?;
    File::open(target.parent().unwrap_or_else(|| Path::new(".")))?.sync_all()
}

impl State {
    fn new(file: WalFile, path: PathBuf, budget: Arc<Budget>) -> Self {
        Self {
            file: Some(file),
            path,
            sequence: 0,
            wal_bytes: HEADER_BYTES as u64,
            reservation_enabled: cfg!(windows),
            reserved_bytes: 0,
            rows: BTreeMap::new(),
            active: BTreeMap::new(),
            history_keys: BTreeSet::new(),
            gc_floor: 0,
            gc_target: 0,
            gc_cursor: None,
            poisoned: false,
            budget,
        }
    }
    fn check_log_capacity(&self, _needed: u64) -> Result<()> {
        #[cfg(feature = "fixed-log")]
        if self
            .file
            .as_ref()
            .and_then(|f| f.capacity())
            .is_some_and(|n| _needed > n - PREFIX_BYTES as u64)
        {
            return Err(Error::ResourceExhausted(
                "fixed log capacity; checkpoint required",
            ));
        }
        Ok(())
    }
    fn reserve_log(&mut self, needed: u64) {
        #[cfg(feature = "fixed-log")]
        if self.file.as_ref().and_then(|f| f.capacity()).is_some() {
            return;
        }
        if !self.reservation_enabled || needed <= self.reserved_bytes {
            return;
        }
        let allocation = needed.div_ceil(32 * 1024) * (32 * 1024);
        let allocation = allocation.min(MAX_WAL_BYTES);
        // Speculative space must not reject a write that still fits on the device.
        if let Some(file) = &self.file {
            match reserve_wal_space(file, allocation) {
                Ok(()) => self.reserved_bytes = allocation,
                Err(_) => self.reservation_enabled = false,
            }
        }
    }

    fn changed(&self, key: &[u8], snapshot: u64) -> bool {
        self.rows
            .get(key)
            .is_some_and(|v| v.back().unwrap().sequence > snapshot)
    }
    fn retained_growth(&self, writes: &BTreeMap<Vec<u8>, Option<Vec<u8>>>) -> usize {
        writes
            .iter()
            .map(|(key, value)| {
                let key_charge = if self.rows.contains_key(key) {
                    0
                } else {
                    192 + 2 * key.len()
                };
                key_charge + 64 + value.as_ref().map_or(0, Vec::len)
            })
            .sum()
    }
    fn check_retained(&self, writes: &BTreeMap<Vec<u8>, Option<Vec<u8>>>) -> Result<()> {
        self.budget
            .check_add(RETAINED, self.retained_growth(writes))
    }
    fn apply(&mut self, sequence: u64, writes: BTreeMap<Vec<u8>, Option<Vec<u8>>>) -> Result<()> {
        self.budget
            .replace(RETAINED, 0, self.retained_growth(&writes))?;
        for (key, value) in writes {
            let versions = self.rows.entry(key.clone()).or_default();
            if !versions.is_empty() || value.is_none() {
                self.history_keys.insert(key);
            }
            versions.push_back(Version {
                sequence,
                value: value.map(Arc::from),
            });
        }
        self.sequence = sequence;
        Ok(())
    }
    fn gc_pending(&self) -> bool {
        self.gc_target != 0
            || (!self.history_keys.is_empty()
                && self.active.keys().next().copied().unwrap_or(self.sequence) > self.gc_floor)
    }
    fn collect(&mut self) -> RetiredVersions {
        let mut retired = RetiredVersions {
            values: Vec::new(),
            charge: 0,
            budget: self.budget.clone(),
        };
        // Persistence admission relies on tombstones and their retained charges staying stable.
        if self.file.is_none() {
            return retired;
        }
        let floor = self.active.keys().next().copied().unwrap_or(self.sequence);
        if self.gc_target == 0 {
            if floor <= self.gc_floor {
                return retired;
            }
            self.gc_target = floor;
            self.gc_cursor = None;
        }
        for _ in 0..64 {
            let lower = self.gc_cursor.as_deref().map_or(Unbounded, Excluded);
            let Some(key) = self
                .history_keys
                .range::<[u8], _>((lower, Unbounded))
                .next()
                .cloned()
            else {
                self.gc_floor = self.gc_target;
                self.gc_target = 0;
                self.gc_cursor = None;
                break;
            };
            let versions = self.rows.get_mut(&key).unwrap();
            while versions.len() > 1 && versions[1].sequence <= self.gc_target {
                let charge = 64 + versions[0].value.as_ref().map_or(0, |v| v.len());
                // One oversized value must make progress; it still counts until freed.
                if retired.values.len() >= 128
                    || (!retired.values.is_empty() && retired.charge + charge > 1024 * 1024)
                {
                    return retired;
                }
                retired.charge += charge;
                retired.values.push(versions.pop_front().unwrap());
            }
            let tombstone = versions.len() == 1
                && versions[0].sequence <= self.gc_target
                && versions[0].value.is_none();
            if tombstone {
                self.rows.remove(&key);
                self.budget.release(RETAINED, 192 + 2 * key.len() + 64);
                self.history_keys.remove(&key);
            } else if versions.len() == 1 && versions[0].value.is_some() {
                self.history_keys.remove(&key);
            }
            self.gc_cursor = Some(key);
        }
        retired
    }
}

fn validate_key(key: &[u8]) -> Result<()> {
    if key.len() > MAX_KEY_BYTES {
        return Err(Error::Limit("key bytes"));
    }
    Ok(())
}

fn visible(versions: &VecDeque<Version>, snapshot: u64) -> Option<&Version> {
    let position = versions.partition_point(|v| v.sequence <= snapshot);
    position.checked_sub(1).map(|i| &versions[i])
}

fn operation_size(key: &[u8], value: Option<&[u8]>) -> usize {
    9 + key.len() + value.map_or(0, <[u8]>::len)
}
fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

#[cfg(not(feature = "wal-direct"))]
fn encode_frame(sequence: u64, writes: &BTreeMap<Vec<u8>, Option<Vec<u8>>>) -> Vec<u8> {
    encode_frame_legacy(sequence, writes)
}

#[cfg(feature = "wal-prepare")]
struct PreparedFrame {
    frame: Vec<u8>,
    permit: Option<BufferPermit>,
}

#[cfg(feature = "wal-direct")]
fn prepare_frame(writes: &BTreeMap<Vec<u8>, Option<Vec<u8>>>) -> Vec<u8> {
    let length = 12
        + writes
            .iter()
            .map(|(k, v)| operation_size(k, v.as_deref()))
            .sum::<usize>();
    let mut frame = Vec::with_capacity(PREFIX_BYTES + length + TRAILER_BYTES);
    frame.extend_from_slice(FRAME);
    frame.extend_from_slice(&(length as u32).to_le_bytes());
    frame.extend_from_slice(&(!(length as u32)).to_le_bytes());
    frame.extend_from_slice(&crate::crc32(&frame).to_le_bytes());
    frame.extend_from_slice(&0u64.to_le_bytes());
    frame.extend_from_slice(&(writes.len() as u32).to_le_bytes());
    for (key, value) in writes {
        frame.push(u8::from(value.is_some()));
        frame.extend_from_slice(&(key.len() as u32).to_le_bytes());
        frame.extend_from_slice(&(value.as_ref().map_or(0, Vec::len) as u32).to_le_bytes());
        frame.extend_from_slice(key);
        if let Some(value) = value {
            frame.extend_from_slice(value);
        }
    }
    frame.extend_from_slice(&0u32.to_le_bytes());
    frame.extend_from_slice(SEAL);
    frame
}

#[cfg(feature = "wal-direct")]
fn finish_frame(frame: &mut [u8], sequence: u64) {
    let end = frame.len() - TRAILER_BYTES;
    frame[PREFIX_BYTES..PREFIX_BYTES + 8].copy_from_slice(&sequence.to_le_bytes());
    let crc = crate::crc32(&frame[PREFIX_BYTES..end]);
    frame[end..end + 4].copy_from_slice(&crc.to_le_bytes());
}

#[cfg(feature = "wal-direct")]
fn encode_frame(sequence: u64, writes: &BTreeMap<Vec<u8>, Option<Vec<u8>>>) -> Vec<u8> {
    let mut frame = prepare_frame(writes);
    finish_frame(&mut frame, sequence);
    frame
}

#[cfg(any(not(feature = "wal-direct"), test))]
fn encode_frame_legacy(sequence: u64, writes: &BTreeMap<Vec<u8>, Option<Vec<u8>>>) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&sequence.to_le_bytes());
    body.extend_from_slice(&(writes.len() as u32).to_le_bytes());
    for (key, value) in writes {
        body.push(u8::from(value.is_some()));
        body.extend_from_slice(&(key.len() as u32).to_le_bytes());
        body.extend_from_slice(&(value.as_ref().map_or(0, Vec::len) as u32).to_le_bytes());
        body.extend_from_slice(key);
        if let Some(value) = value {
            body.extend_from_slice(value);
        }
    }
    let length = body.len() as u32;
    let mut frame = Vec::with_capacity(PREFIX_BYTES + body.len() + TRAILER_BYTES);
    frame.extend_from_slice(FRAME);
    frame.extend_from_slice(&length.to_le_bytes());
    frame.extend_from_slice(&(!length).to_le_bytes());
    frame.extend_from_slice(&crate::crc32(&frame).to_le_bytes());
    frame.extend_from_slice(&body);
    frame.extend_from_slice(&crate::crc32(&body).to_le_bytes());
    frame.extend_from_slice(SEAL);
    frame
}

type WriteSet = BTreeMap<Vec<u8>, Option<Vec<u8>>>;
fn decode_frame(bytes: &[u8]) -> Result<(u64, WriteSet)> {
    let mut input = Decoder { bytes, position: 0 };
    let sequence = u64::from_le_bytes(input.take(8)?.try_into().unwrap());
    let count = read_u32(input.take(4)?, 0) as usize;
    if count == 0 || count > MAX_TRANSACTION_WRITES {
        return Err(Error::Invalid("WAL operation count"));
    }
    let mut writes = BTreeMap::new();
    let mut previous: Option<Vec<u8>> = None;
    for _ in 0..count {
        let tag = input.take(1)?[0];
        let key_length = read_u32(input.take(4)?, 0) as usize;
        let value_length = read_u32(input.take(4)?, 0) as usize;
        if key_length > MAX_KEY_BYTES
            || value_length > MAX_VALUE_BYTES
            || tag > 1
            || (tag == 0 && value_length != 0)
        {
            return Err(Error::Invalid("WAL operation"));
        }
        let key = input.take(key_length)?.to_vec();
        if previous.as_ref().is_some_and(|p| *p >= key) {
            return Err(Error::Invalid("WAL key order"));
        }
        previous = Some(key.clone());
        let value = if tag == 1 {
            Some(input.take(value_length)?.to_vec())
        } else {
            None
        };
        writes.insert(key, value);
    }
    if input.position != bytes.len() {
        return Err(Error::Invalid("WAL trailing bytes"));
    }
    Ok((sequence, writes))
}

struct Decoder<'a> {
    bytes: &'a [u8],
    position: usize,
}
impl<'a> Decoder<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or(Error::Invalid("truncated WAL operation"))?;
        let result = &self.bytes[self.position..end];
        self.position = end;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn wal_preparation_eq_encoding_matches_legacy_bytes_and_decoding() {
        for n in [0u64, 1, 2, 17, 100] {
            let mut writes = BTreeMap::new();
            for i in 0..n {
                writes.insert(
                    i.to_be_bytes().to_vec(),
                    if i % 3 == 0 {
                        None
                    } else {
                        Some(vec![i as u8; if i % 5 == 0 { 65536 } else { i as usize }])
                    },
                );
            }
            writes.insert(vec![], Some(vec![]));
            writes.insert(vec![255; MAX_KEY_BYTES], Some(vec![7; 256]));
            for seq in [0, 1, u64::MAX] {
                let expected = encode_frame_legacy(seq, &writes);
                assert_eq!(encode_frame(seq, &writes), expected);
                let body = &expected[PREFIX_BYTES..expected.len() - TRAILER_BYTES];
                let (got_seq, got_writes) = decode_frame(body).unwrap();
                assert_eq!(got_seq, seq);
                assert_eq!(got_writes, writes);
            }
        }
    }

    #[cfg(feature = "wal-prepare")]
    #[test]
    fn wal_preparation_bv_er_reservation_rejection_and_drop_restore_capacity() {
        let key = b"key";
        let value = [3; 256];
        let body = 12 + operation_size(key, Some(&value));
        let needed = 4 * body + PREFIX_BYTES + TRAILER_BYTES + 64;
        for limit in [needed - 1, needed, needed + 1] {
            let f = Fixture::new();
            let db = Database::create_with_limits(
                &f.0,
                [110; 16],
                ResourceLimits {
                    max_buffer_bytes: limit,
                    ..ResourceLimits::default()
                },
            )
            .unwrap();
            let mut tx = db.begin().unwrap();
            tx.put(key, &value).unwrap();
            let result = tx.prepare_log();
            if limit < needed {
                assert!(matches!(result, Err(Error::ResourceExhausted(_))));
                assert!(tx.prepared.is_none());
                assert_eq!(
                    tx.writes.get(key.as_slice()).unwrap().as_deref(),
                    Some(value.as_slice())
                );
            } else {
                result.unwrap();
                assert_eq!(
                    db.resource_usage().unwrap().current[crate::resources::BUFFER],
                    needed
                );
                tx.prepare_log().unwrap();
                assert_eq!(
                    db.resource_usage().unwrap().current[crate::resources::BUFFER],
                    needed
                );
            }
            drop(tx);
            assert_eq!(
                db.resource_usage().unwrap().current[crate::resources::BUFFER],
                0
            );
            assert_eq!(db.resource_usage().unwrap().current[STAGED], 0);
            let mut next = db.begin().unwrap();
            next.put(b"x", b"y").unwrap();
            next.commit().unwrap();
        }
    }

    #[cfg(feature = "wal-prepare")]
    #[test]
    fn wal_preparation_st_prepares_before_writer_lock_without_early_publication() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [111; 16]).unwrap();
        let mut old = db.begin_read().unwrap();
        let guard = db.inner.writer.lock().unwrap();
        let other = db.clone();
        let (send, recv) = std::sync::mpsc::channel();
        let child = std::thread::spawn(move || {
            let mut tx = other.begin().unwrap();
            tx.put(b"k", b"v").unwrap();
            tx.prepare_log().unwrap();
            send.send(()).unwrap();
            tx.commit().unwrap()
        });
        recv.recv_timeout(std::time::Duration::from_secs(2))
            .unwrap();
        assert_eq!(old.get(b"k").unwrap(), None);
        assert_eq!(db.current_version().unwrap(), 0);
        drop(guard);
        assert_eq!(child.join().unwrap(), 1);
        assert_eq!(old.get(b"k").unwrap(), None);
        drop(old);
        assert_eq!(
            db.begin_read().unwrap().get(b"k").unwrap(),
            Some(b"v".to_vec())
        );
        assert_eq!(
            db.resource_usage().unwrap().current[crate::resources::BUFFER],
            0
        );
        drop(db);
        assert_eq!(
            Database::open(&f.0)
                .unwrap()
                .begin_read()
                .unwrap()
                .get(b"k")
                .unwrap(),
            Some(b"v".to_vec())
        );
    }

    #[cfg(feature = "wal-prepare")]
    #[test]
    fn wal_preparation_st_conflict_discards_only_uncommitted_preparation() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [112; 16]).unwrap();
        let mut loser = db.begin().unwrap();
        loser.put(b"k", b"loser").unwrap();
        loser.prepare_log().unwrap();
        let mut winner = db.begin().unwrap();
        winner.put(b"k", b"winner").unwrap();
        winner.commit().unwrap();
        assert!(matches!(
            loser.commit(),
            Err(Error::Conflict(ConflictKind::Write))
        ));
        assert_eq!(
            db.resource_usage().unwrap().current[crate::resources::BUFFER],
            0
        );
        drop(db);
        assert_eq!(
            Database::open(&f.0)
                .unwrap()
                .begin_read()
                .unwrap()
                .get(b"k")
                .unwrap(),
            Some(b"winner".to_vec())
        );
    }

    #[test]
    fn copy_scan_eq_st_shared_rows_match_owned_ranges_and_survive_mutation() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [101; 16]).unwrap();
        let mut tx = db.begin().unwrap();
        for i in 0..100u64 {
            tx.put(&i.to_be_bytes(), &vec![i as u8; i as usize])
                .unwrap();
        }
        tx.commit().unwrap();
        let mut reader = db.begin().unwrap();
        reader.put(&[255; 1024], &[3; 65536]).unwrap();
        reader.delete(&10u64.to_be_bytes()).unwrap();
        reader.put(&20u64.to_be_bytes(), b"staged").unwrap();
        for start in [vec![], 10u64.to_be_bytes().to_vec(), vec![255; 1024]] {
            for end in [None, Some(vec![255; 1024])] {
                let owned = reader.scan(&start, end.as_deref()).unwrap();
                let shared = reader.scan_shared(&start, end.as_deref(), true).unwrap();
                assert_eq!(
                    shared
                        .iter()
                        .map(|(k, v)| (k.to_vec(), v.to_vec()))
                        .collect::<Rows>(),
                    owned
                );
            }
        }
        let held = reader.scan_shared(b"", None, true).unwrap();
        let want = held
            .iter()
            .map(|(k, v)| (k.to_vec(), v.to_vec()))
            .collect::<Rows>();
        reader.put(&20u64.to_be_bytes(), b"replaced").unwrap();
        let mut writer = db.begin().unwrap();
        writer.delete(&0u64.to_be_bytes()).unwrap();
        writer.commit().unwrap();
        assert!(matches!(reader.commit(), Err(Error::Conflict(_))));
        db.checkpoint().unwrap();
        assert_eq!(
            held.iter()
                .map(|(k, v)| (k.to_vec(), v.to_vec()))
                .collect::<Rows>(),
            want
        );
        assert!(db.resource_usage().unwrap().current[crate::resources::BUFFER] >= 65536);
        drop(held);
        assert_eq!(
            db.resource_usage().unwrap().current[crate::resources::BUFFER],
            0
        );
    }

    #[test]
    fn copy_scan_bv_er_budget_edges_release_and_preserve_staged_writes() {
        assert!(3 * std::mem::size_of::<SharedScanRow>() + 16 <= 160);
        for count in [0usize, 1, 2, 3, 4, 5, 8, 9, 16, 17] {
            for keys in [false, true] {
                let charge = count * (160 + 1 + if keys { 3 } else { 0 });
                for limit in [charge.saturating_sub(1).max(1), charge.max(1), charge + 1] {
                    let f = Fixture::new();
                    let db = Database::create_with_limits(
                        &f.0,
                        [102; 16],
                        ResourceLimits {
                            max_buffer_bytes: limit,
                            ..ResourceLimits::default()
                        },
                    )
                    .unwrap();
                    let mut tx = db.begin().unwrap();
                    for i in 0..count {
                        tx.put(&[i as u8], &[i as u8]).unwrap();
                    }
                    let result = tx.scan_shared(b"", None, keys);
                    if limit < charge {
                        assert!(matches!(result, Err(Error::ResourceExhausted(_))));
                    } else {
                        let scan = result.unwrap();
                        assert_eq!(scan.rows.len(), count);
                        assert_eq!(scan.keys.len(), if keys { count } else { 0 });
                        assert_eq!(
                            db.resource_usage().unwrap().current[crate::resources::BUFFER],
                            charge
                        );
                        drop(scan);
                    }
                    assert_eq!(
                        db.resource_usage().unwrap().current[crate::resources::BUFFER],
                        0
                    );
                    for i in 0..count {
                        assert_eq!(tx.get(&[i as u8]).unwrap(), Some(vec![i as u8]));
                    }
                }
            }
        }
    }

    #[test]
    fn copy_scan_st_shared_payloads_are_stable_after_unlock_and_owner_drop() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [103; 16]).unwrap();
        let mut seed = db.begin().unwrap();
        seed.put(b"k", &[1; 65536]).unwrap();
        seed.commit().unwrap();
        let gate = Arc::new([std::sync::Barrier::new(2), std::sync::Barrier::new(2)]);
        let child_db = db.clone();
        let child_gate = gate.clone();
        let child = std::thread::spawn(move || {
            let mut tx = child_db.begin_read().unwrap();
            READ_COPY_GATE.with(|g| *g.borrow_mut() = Some(child_gate));
            tx.scan_shared(b"", None, true).unwrap()
        });
        gate[0].wait();
        let mut writer = db.begin().unwrap();
        writer.put(b"k", &[2; 65536]).unwrap();
        writer.commit().unwrap();
        gate[1].wait();
        let held = child.join().unwrap();
        drop(db);
        assert_eq!(
            held.iter().next().unwrap(),
            (b"k".as_slice(), [1; 65536].as_slice())
        );
    }

    #[test]
    fn copy_scan_eq_export_avoids_keys_and_preserves_empty_and_large_values() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [104; 16]).unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"", b"").unwrap();
        tx.put(&[255; 1024], &[7; 65536]).unwrap();
        tx.commit().unwrap();
        let mut reader = db.begin_read().unwrap();
        let scan = reader.scan_shared(b"", None, false).unwrap();
        assert!(scan.keys.is_empty());
        assert_eq!(
            scan.rows.iter().map(AsRef::as_ref).collect::<Vec<_>>(),
            vec![b"".as_slice(), [7; 65536].as_slice()]
        );
        drop(scan);
        let out = Fixture::new();
        db.export_values(&out.0, [105; 16], b"", None).unwrap();
        let mut s = crate::Snapshot::open(&out.0, [105; 16]).unwrap();
        assert_eq!(s.read_page(0, 10).unwrap(), vec![vec![], vec![7; 65536]]);
        assert!(db.export_values(&out.0, [105; 16], b"", None).is_err());
        assert_eq!(
            db.resource_usage().unwrap().current[crate::resources::BUFFER],
            0
        );
    }

    #[test]
    fn scan_merge_matches_oracle_for_staged_changes_boundaries_and_old_versions() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [96; 16]).unwrap();
        let mut initial = BTreeMap::new();
        let mut seed = db.begin().unwrap();
        for i in 0..1000u64 {
            let k = (i * 2).to_be_bytes();
            let v = i.to_be_bytes();
            seed.put(&k, &v).unwrap();
            initial.insert(k.to_vec(), v.to_vec());
        }
        seed.commit().unwrap();
        let mut old = db.begin_read().unwrap();
        let mut tx = db.begin().unwrap();
        let mut expected = initial.clone();
        for i in 0..1500u64 {
            let k = (i * 739 % 2200).to_be_bytes();
            if i % 3 == 0 {
                tx.delete(&k).unwrap();
                expected.remove(k.as_slice());
            } else {
                let v = (i + 1).to_be_bytes();
                tx.put(&k, &v).unwrap();
                expected.insert(k.to_vec(), v.to_vec());
            }
        }
        let mut latest = db.begin().unwrap();
        latest.put(&0u64.to_be_bytes(), b"new").unwrap();
        latest.delete(&2u64.to_be_bytes()).unwrap();
        latest.commit().unwrap();
        let ends = [
            None,
            Some(0u64.to_be_bytes()),
            Some(13u64.to_be_bytes()),
            Some(2000u64.to_be_bytes()),
            Some(u64::MAX.to_be_bytes()),
        ];
        for start in [0u64, 1, 12, 1000, 2200, u64::MAX] {
            for end in ends {
                let k = start.to_be_bytes();
                if end.as_ref().is_some_and(|e| e < &k) {
                    continue;
                }
                let want: Rows = expected
                    .range::<[u8], _>((
                        Included(k.as_slice()),
                        end.as_ref().map_or(Unbounded, |e| Excluded(e.as_slice())),
                    ))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                assert_eq!(
                    tx.scan(&k, end.as_ref().map(|e| e.as_slice())).unwrap(),
                    want
                );
            }
        }
        assert_eq!(
            old.scan(b"", None).unwrap(),
            initial.into_iter().collect::<Rows>()
        );
        assert!(matches!(tx.commit(), Err(Error::Conflict(_))));
        let mut empty = db.begin_read().unwrap();
        assert!(empty.scan(b"z", Some(b"z")).unwrap().is_empty());
    }

    #[test]
    fn scan_merge_budget_boundary_releases_reservations_and_preserves_staged_values() {
        for count in [0usize, 1, 2, 3, 4, 5, 8, 9, 16, 17] {
            let bytes = count * (1 + 1 + 224);
            for limit in [bytes.saturating_sub(1).max(1), bytes.max(1), bytes + 1] {
                let f = Fixture::new();
                let db = Database::create_with_limits(
                    &f.0,
                    [97; 16],
                    ResourceLimits {
                        max_buffer_bytes: limit,
                        ..ResourceLimits::default()
                    },
                )
                .unwrap();
                let mut tx = db.begin().unwrap();
                for i in 0..count {
                    tx.put(&[i as u8], &[i as u8]).unwrap();
                }
                let result = tx.scan_reserved(b"", None);
                if limit < bytes {
                    assert!(matches!(result, Err(Error::ResourceExhausted(_))));
                } else {
                    let (rows, permit) = result.unwrap();
                    assert_eq!(rows.len(), count);
                    assert_eq!(
                        db.resource_usage().unwrap().current[crate::resources::BUFFER],
                        bytes
                    );
                    drop(rows);
                    drop(permit);
                }
                assert_eq!(
                    db.resource_usage().unwrap().current[crate::resources::BUFFER],
                    0
                );
                assert_eq!(tx.writes.len(), count);
                if count > 0 {
                    assert_eq!(tx.get(&[0]).unwrap(), Some(vec![0]));
                }
            }
        }
        assert!(3 * std::mem::size_of::<(Vec<u8>, ReadValue<'_>)>() <= 160);
        assert!(std::mem::size_of::<(Vec<u8>, Vec<u8>)>() <= 64);
    }

    #[test]
    fn scan_merge_shadowed_large_values_do_not_consume_output_budget() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [98; 16]).unwrap();
        let mut seed = db.begin().unwrap();
        seed.put(b"k", &vec![1; 65536]).unwrap();
        seed.commit().unwrap();
        let reserved = ResourceLimits::default().max_buffer_bytes - 512;
        let mut pressure = db.inner.budget.buffer();
        pressure.add(reserved).unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"k", b"small").unwrap();
        assert_eq!(
            tx.scan(b"", None).unwrap(),
            vec![(b"k".to_vec(), b"small".to_vec())]
        );
        tx.delete(b"k").unwrap();
        assert!(tx.scan(b"", None).unwrap().is_empty());
        assert_eq!(
            db.resource_usage().unwrap().current[crate::resources::BUFFER],
            reserved
        );
        drop(pressure);
        assert_eq!(
            db.resource_usage().unwrap().current[crate::resources::BUFFER],
            0
        );
    }

    #[test]
    fn scan_merge_full_copy_runs_unlocked_and_keeps_original_value() {
        let f = Fixture::new();
        let db = Database::create(&f.0, [99; 16]).unwrap();
        let mut seed = db.begin().unwrap();
        seed.put(b"k", &vec![1; 65536]).unwrap();
        seed.commit().unwrap();
        let gate = Arc::new([std::sync::Barrier::new(2), std::sync::Barrier::new(2)]);
        std::thread::scope(|scope| {
            let signal = gate.clone();
            let reader_db = db.clone();
            let reader = scope.spawn(move || {
                let mut tx = reader_db.begin_read().unwrap();
                READ_COPY_GATE.with(|v| *v.borrow_mut() = Some(signal));
                tx.scan(b"", None).unwrap()
            });
            gate[0].wait();
            let mut writer = db.begin().unwrap();
            writer.put(b"k", &vec![2; 65536]).unwrap();
            let committed = writer.commit();
            gate[1].wait();
            committed.unwrap();
            assert_eq!(
                reader.join().unwrap(),
                vec![(b"k".to_vec(), vec![1; 65536])]
            );
        });
    }

    #[test]
    fn elision_export_uses_no_write_dependencies_and_preserves_file_content() {
        let f = Fixture::new();
        let db = Database::create_with_limits(
            &f.0,
            [90; 16],
            ResourceLimits {
                max_dependency_bytes: 1,
                ..ResourceLimits::default()
            },
        )
        .unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"a", b"value").unwrap();
        tx.commit().unwrap();
        let out = Fixture::new();
        let info = db.export_values(&out.0, [91; 16], b"", None).unwrap();
        assert_eq!(info.records, 1);
        assert_eq!(info.version, 1);
        let mut snapshot = crate::Snapshot::open(&out.0, [91; 16]).unwrap();
        assert_eq!(snapshot.read_page(0, 10).unwrap(), vec![b"value".to_vec()]);
        assert_eq!(db.resource_usage().unwrap().current[DEPENDENCIES], 0);
        assert!(db.export_values(&out.0, [91; 16], b"", None).is_err());
    }

    #[test]
    fn elision_point_validation_keeps_advanced_generation_and_earlier_group_conflicts() {
        for read in [false, true] {
            for deletion in [false, true] {
                let f = Fixture::new();
                let db = Database::create(&f.0, [92; 16]).unwrap();
                let mut seed = db.begin().unwrap();
                seed.put(b"key", b"old").unwrap();
                seed.commit().unwrap();
                let mut tx = db.begin().unwrap();
                if read {
                    tx.get(b"key").unwrap();
                    tx.put(b"side", b"v").unwrap();
                } else {
                    tx.put(b"key", b"mine").unwrap();
                }
                let kind = if read {
                    ConflictKind::Read
                } else {
                    ConflictKind::Write
                };
                {
                    let state = db.state().unwrap();
                    assert!(tx.validate_commit(&state, &BTreeSet::new()).is_ok());
                    assert!(
                        matches!(tx.validate_commit(&state,&BTreeSet::from([b"key".as_slice()])),Err(Error::Conflict(k)) if k==kind)
                    );
                }
                let mut other = db.begin().unwrap();
                if deletion {
                    other.delete(b"key").unwrap();
                } else {
                    other.put(b"key", b"new").unwrap();
                }
                other.commit().unwrap();
                assert!(matches!(tx.commit(),Err(Error::Conflict(k)) if k==kind));
            }
        }
    }

    use super::*;

    #[test]
    #[cfg(feature = "lock-timing")]
    fn diagnostic_commit_timestamps_follow_each_callers_successful_commit() {
        for grouped in [false, true] {
            let path = Fixture::new();
            let db = Database::create(&path.0, [80; 16]).unwrap();
            db.set_group_commit(grouped);
            db.set_group_commit_wake_on_four(true);
            std::thread::scope(|scope| {
                for id in 0_u8..8 {
                    let db = &db;
                    scope.spawn(move || {
                        for n in 0..8 {
                            let start = diagnostic_now_ns();
                            let mut tx = db.begin().unwrap();
                            tx.put(&[id], &[n]).unwrap();
                            tx.commit().unwrap();
                            let trace = take_commit_trace();
                            assert!(trace[0] >= start);
                            assert!(trace.windows(2).all(|p| p[0] <= p[1]), "{trace:?}");
                            assert!(trace[7] <= diagnostic_now_ns());
                            assert_eq!(take_commit_trace(), [0; 8]);
                        }
                    });
                }
            });
        }
    }

    #[test]
    fn er_acknowledged_data_survives_storage_full_partial_write_and_sync_failure() {
        for failure in 0..4 {
            let path = Fixture::new();
            let db = Database::create(&path.0, [81; 16]).unwrap();
            let mut seed = db.begin().unwrap();
            seed.put(b"ack", b"must survive").unwrap();
            seed.commit().unwrap();
            db.checkpoint().unwrap();
            let mut tx = db.begin().unwrap();
            tx.put(b"a", b"uncertain").unwrap();
            tx.put(b"b", b"uncertain").unwrap();
            let result = tx.commit_with(|file, frame| {
                match failure {
                    0 => (),
                    1 => file.write_all(&frame[..frame.len() / 2])?,
                    2 => file.write_all(frame)?,
                    _ => {
                        file.write_all(frame)?;
                        file.sync_all()?;
                    }
                }
                Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "injected failure",
                ))
            });
            assert!(matches!(result, Err(Error::CommitUncertain(_))));
            assert!(matches!(db.begin(), Err(Error::Poisoned)));
            drop(db);
            let db = Database::open(&path.0).unwrap();
            let mut tx = db.begin().unwrap();
            assert_eq!(tx.get(b"ack").unwrap(), Some(b"must survive".to_vec()));
            let a = tx.get(b"a").unwrap();
            let b = tx.get(b"b").unwrap();
            assert_eq!(a, b);
            if failure < 2 {
                assert!(a.is_none());
            }
            if failure == 3 {
                assert_eq!(a, Some(b"uncertain".to_vec()));
            }
            drop(tx);
            let mut next = db.begin().unwrap();
            next.put(b"next", b"ok").unwrap();
            next.commit().unwrap();
            db.checkpoint().unwrap();
            drop(db);
            assert_eq!(
                Database::open(&path.0)
                    .unwrap()
                    .begin()
                    .unwrap()
                    .get(b"ack")
                    .unwrap(),
                Some(b"must survive".to_vec())
            );
        }
    }

    #[test]
    fn st_limited_failed_persistence_releases_reservations_and_requires_reopen() {
        let path = Fixture::new();
        let db = Database::create_with_limits(&path.0, [3; 16], ResourceLimits::default()).unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"a", b"pending").unwrap();
        let result =
            tx.commit_with(|_, _| Err(io::Error::new(io::ErrorKind::StorageFull, "injected")));
        assert!(matches!(result, Err(Error::CommitUncertain(_))));
        let usage = db.resource_usage().unwrap();
        assert_eq!(usage.current, [0; 5]);
        assert!(matches!(db.begin(), Err(Error::Poisoned)));
        drop(db);
        let db = Database::open_with_limits(&path.0, ResourceLimits::default()).unwrap();
        assert_eq!(db.begin().unwrap().get(b"a").unwrap(), None);
    }

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            std::fs::create_dir_all(Path::new(env!("CARGO_MANIFEST_DIR")).join("artifacts"))
                .unwrap();
            let time = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("artifacts")
                    .join(format!(
                        "newdbms-wal-fault-{}-{time}-{id}",
                        std::process::id()
                    )),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(with_suffix(&self.0, ".owner"));
            let _ = std::fs::remove_file(with_suffix(&self.0, ".retired"));
            let _ = std::fs::remove_file(with_suffix(&self.0, ".retired.meta"));
        }
    }

    #[test]
    fn deferred_reclaim_preserves_old_reads_updates_and_reopen() {
        let path = Fixture::new();
        let db = Database::create(&path.0, [59; 16]).unwrap();
        db.set_checkpoint_retirement(true, MAX_WAL_BYTES).unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"key", b"old").unwrap();
        tx.commit().unwrap();
        let mut old = db.begin().unwrap();
        assert_eq!(old.get(b"key").unwrap(), Some(b"old".to_vec()));
        db.checkpoint_deferred_reclaim().unwrap();
        assert!(with_suffix(&path.0, ".retired").exists());
        let mut tx = db.begin().unwrap();
        tx.put(b"key", b"new").unwrap();
        tx.commit().unwrap();
        db.reclaim_checkpoint().unwrap();
        db.reclaim_checkpoint().unwrap();
        assert!(!with_suffix(&path.0, ".retired").exists());
        assert_eq!(old.get(b"key").unwrap(), Some(b"old".to_vec()));
        old.put(b"other", b"value").unwrap();
        assert!(matches!(old.commit(), Err(Error::Conflict(_))));
        db.checkpoint_deferred_reclaim().unwrap();
        db.checkpoint_deferred_reclaim().unwrap();
        drop(db);
        let db = Database::open(&path.0).unwrap();
        assert_eq!(
            db.begin().unwrap().get(b"key").unwrap(),
            Some(b"new".to_vec())
        );
        assert!(!with_suffix(&path.0, ".retired").exists());
    }

    #[test]
    fn retirement_releases_writer_before_reclaim_and_preserves_old_generation_and_conflicts() {
        let path = Fixture::new();
        let db = Database::create(&path.0, [57; 16]).unwrap();
        db.set_checkpoint_retirement(true, MAX_WAL_BYTES).unwrap();
        let mut initial = db.begin().unwrap();
        initial.put(b"key", b"old").unwrap();
        initial.commit().unwrap();
        let mut old = db.begin().unwrap();
        assert_eq!(old.get(b"key").unwrap(), Some(b"old".to_vec()));
        db.checkpoint_with(|stage| {
            if stage == CheckpointStage::Released {
                assert!(with_suffix(&path.0, ".retired").exists());
                let db = db.clone();
                let (send, recv) = std::sync::mpsc::channel();
                let worker = std::thread::spawn(move || {
                    let mut tx = db.begin().unwrap();
                    tx.put(b"key", b"new").unwrap();
                    tx.put(b"added", b"new").unwrap();
                    tx.commit().unwrap();
                    send.send(()).unwrap();
                });
                recv.recv_timeout(std::time::Duration::from_secs(3))
                    .map_err(io::Error::other)?;
                worker.join().unwrap();
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(old.get(b"key").unwrap(), Some(b"old".to_vec()));
        old.put(b"other", b"value").unwrap();
        assert!(matches!(old.commit(), Err(Error::Conflict(_))));
        for _ in 0..3 {
            db.checkpoint().unwrap();
        }
        let status = db.checkpoint_retirement_status().unwrap();
        assert_eq!(
            (status.retained, status.reclaimed, status.fallbacks),
            (4, 4, 0)
        );
        drop(db);
        let db = Database::open(&path.0).unwrap();
        assert_eq!(
            db.begin().unwrap().get(b"key").unwrap(),
            Some(b"new".to_vec())
        );
        assert!(!with_suffix(&path.0, ".retired").exists());
    }

    #[test]
    fn retirement_byte_boundary_and_disabled_mode_fall_back_without_losing_data() {
        for (enabled, cap, retained) in
            [(true, 47, 0), (true, 48, 1), (true, 49, 1), (false, 49, 0)]
        {
            let path = Fixture::new();
            let db = Database::create(&path.0, [57; 16]).unwrap();
            db.set_log_reservation(false).unwrap();
            db.set_checkpoint_retirement(enabled, cap).unwrap();
            db.checkpoint().unwrap();
            assert_eq!(
                db.checkpoint_retirement_status().unwrap().retained,
                retained
            );
            assert_eq!(db.stats().unwrap().visible_records, 0);
        }
    }

    #[test]
    fn retirement_preserves_unrecognized_sidecars_and_uses_normal_replacement() {
        for suffix in [".retired", ".retired.meta"] {
            let path = Fixture::new();
            let db = Database::create(&path.0, [57; 16]).unwrap();
            let other = with_suffix(&path.0, suffix);
            std::fs::write(&other, b"unrelated").unwrap();
            db.set_checkpoint_retirement(true, MAX_WAL_BYTES).unwrap();
            db.checkpoint().unwrap();
            assert_eq!(db.checkpoint_retirement_status().unwrap().fallbacks, 1);
            drop(db);
            let _db = Database::open(&path.0).unwrap();
            assert_eq!(std::fs::read(other).unwrap(), b"unrelated");
        }
    }

    #[cfg(windows)]
    #[test]
    fn retirement_failed_cleanup_keeps_one_slot_and_retries_on_reopen() {
        use std::os::windows::fs::OpenOptionsExt;
        let path = Fixture::new();
        let db = Database::create(&path.0, [57; 16]).unwrap();
        db.set_checkpoint_retirement(true, MAX_WAL_BYTES).unwrap();
        let mut held = None;
        db.checkpoint_with(|stage| {
            if stage == CheckpointStage::Released {
                held = Some(
                    OpenOptions::new()
                        .read(true)
                        .share_mode(1)
                        .open(with_suffix(&path.0, ".retired"))?,
                );
            }
            Ok(())
        })
        .unwrap();
        for _ in 0..2 {
            db.checkpoint().unwrap();
        }
        let status = db.checkpoint_retirement_status().unwrap();
        assert_eq!(
            (status.retained, status.reclaimed, status.fallbacks),
            (1, 0, 2)
        );
        drop(held);
        drop(db);
        let db = Database::open(&path.0).unwrap();
        assert_eq!(db.checkpoint_retirement_status().unwrap().reclaimed, 1);
        assert!(!with_suffix(&path.0, ".retired").exists());
    }

    #[test]
    fn reads_during_persistence_keep_old_values_and_conflict_after_publication() {
        let path = Fixture::new();
        let db = Database::create(&path.0, [54; 16]).unwrap();
        let mut initial = db.begin().unwrap();
        initial.put(b"key", b"old").unwrap();
        initial.commit().unwrap();
        let mut writer = db.begin().unwrap();
        writer.put(b"key", b"new").unwrap();
        writer.put(b"insert", b"new").unwrap();
        let mut during = None;
        writer
            .commit_with(|file, frame| {
                file.write_all(frame)?;
                let (send, recv) = std::sync::mpsc::channel();
                let reader_db = db.clone();
                let reader = std::thread::spawn(move || -> Result<()> {
                    let mut tx = reader_db.begin()?;
                    assert_eq!(tx.get(b"key")?, Some(b"old".to_vec()));
                    assert_eq!(tx.get(b"insert")?, None);
                    assert!(tx.scan(b"insert", Some(b"j"))?.is_empty());
                    tx.put(b"dependent", b"value")?;
                    send.send(tx).unwrap();
                    Ok(())
                });
                let received = recv.recv_timeout(std::time::Duration::from_secs(2));
                if let Ok(tx) = received {
                    during = Some(tx);
                    reader.join().unwrap().unwrap();
                } else {
                    return Err(io::Error::other("reader blocked by persistence"));
                }
                file.sync_all()
            })
            .unwrap();
        let mut during = during.unwrap();
        assert_eq!(during.get(b"key").unwrap(), Some(b"old".to_vec()));
        assert!(matches!(during.commit(), Err(Error::Conflict(_))));
        assert_eq!(
            db.begin().unwrap().get(b"key").unwrap(),
            Some(b"new".to_vec())
        );
        drop(db);
        assert_eq!(
            Database::open(&path.0)
                .unwrap()
                .begin()
                .unwrap()
                .get(b"key")
                .unwrap(),
            Some(b"new".to_vec())
        );
    }

    #[test]
    fn panic_during_unlocked_persistence_requires_reopen() {
        let path = Fixture::new();
        let db = Database::create(&path.0, [55; 16]).unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"key", b"value").unwrap();
        let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            tx.commit_with(|_, _| panic!("injected persistence panic"))
        }));
        assert!(failure.is_err());
        assert!(matches!(db.begin(), Err(Error::Poisoned)));
        drop(db);
        assert_eq!(
            Database::open(&path.0)
                .unwrap()
                .begin()
                .unwrap()
                .get(b"key")
                .unwrap(),
            None
        );
    }

    #[test]
    fn er_persistence_failure_poisoning_requires_reopen_and_reports_uncertain_outcome() {
        for mode in ["none", "partial", "complete"] {
            let path = Fixture::new();
            let db = Database::create(&path.0, [3; 16]).unwrap();
            let mut transaction = db.begin().unwrap();
            transaction
                .put(b"key", b"durable only if complete")
                .unwrap();
            let result = transaction.commit_with(|file, frame| {
                match mode {
                    "partial" => file.write_all(&frame[..PREFIX_BYTES + 5])?,
                    "complete" => file.write_all(frame)?,
                    _ => (),
                }
                file.sync_all()?;
                Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "injected storage failure",
                ))
            });
            assert!(matches!(result, Err(Error::CommitUncertain(_))));
            assert!(matches!(db.begin(), Err(Error::Poisoned)));
            drop(db);
            let db = Database::open(&path.0).unwrap();
            assert_eq!(
                db.begin().unwrap().get(b"key").unwrap().is_some(),
                mode == "complete"
            );
            let mut transaction = db.begin().unwrap();
            transaction.put(b"after recovery", b"ok").unwrap();
            transaction.commit().unwrap();
        }
    }

    #[test]
    fn bv_wal_and_sequence_limits_reject_before_any_persistence() {
        let path = Fixture::new();
        let db = Database::create(&path.0, [3; 16]).unwrap();
        for sequence_limit in [false, true] {
            {
                let mut state = db.state().unwrap();
                state.wal_bytes = if sequence_limit {
                    HEADER_BYTES as u64
                } else {
                    MAX_WAL_BYTES
                };
                state.sequence = if sequence_limit { u64::MAX } else { 0 };
            }
            let mut tx = db.begin().unwrap();
            tx.put(b"a", b"b").unwrap();
            let result = tx.commit_with(|_, _| panic!("limit must reject before persistence"));
            assert!(matches!(result, Err(Error::Limit(_))));
            assert!(!db.state().unwrap().poisoned);
        }
        drop(db);
        assert_eq!(
            std::fs::metadata(&path.0).unwrap().len(),
            HEADER_BYTES as u64
        );
    }

    #[test]
    fn er_wal_decoder_rejects_invalid_counts_tags_lengths_order_and_trailing_bytes() {
        let writes = BTreeMap::from([
            (b"a".to_vec(), Some(b"one".to_vec())),
            (b"b".to_vec(), None),
        ]);
        let frame = encode_frame(1, &writes);
        let body = &frame[PREFIX_BYTES..frame.len() - TRAILER_BYTES];
        for (at, value) in [(8, 0), (12, 2), (13, 255), (17, 255), (34, b'a')] {
            let mut invalid = body.to_vec();
            if at < invalid.len() {
                invalid[at] = value;
            }
            assert!(decode_frame(&invalid).is_err(), "offset {at}");
        }
        let mut invalid = body.to_vec();
        invalid.push(0);
        assert!(decode_frame(&invalid).is_err());
        for length in 0..body.len() {
            assert!(decode_frame(&body[..length]).is_err());
        }
    }

    #[test]
    #[ignore = "subprocess helper"]
    fn crash_before_publication() {
        let path = std::env::var_os("NEWDBMS_WAL_CRASH_PATH").unwrap();
        let partial = std::env::var("NEWDBMS_WAL_CRASH_PARTIAL").unwrap() == "yes";
        let db = Database::open(path).unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"a", b"atomic").unwrap();
        tx.put(b"b", b"atomic").unwrap();
        let _ = tx.commit_with(|file, frame| {
            file.write_all(if partial {
                &frame[..PREFIX_BYTES + 5]
            } else {
                frame
            })?;
            file.sync_all()?;
            std::process::exit(74);
        });
        panic!("crash point was not reached");
    }

    #[test]
    fn er_process_exit_during_append_or_after_sync_before_publication_recovers_atomically() {
        for partial in [true, false] {
            let path = Fixture::new();
            let db = Database::create(&path.0, [2; 16]).unwrap();
            drop(db);
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "transactions::tests::crash_before_publication",
                    "--nocapture",
                ])
                .env("NEWDBMS_WAL_CRASH_PATH", &path.0)
                .env(
                    "NEWDBMS_WAL_CRASH_PARTIAL",
                    if partial { "yes" } else { "no" },
                )
                .output()
                .unwrap();
            assert_eq!(result.status.code(), Some(74), "{result:?}");
            let db = Database::open(&path.0).unwrap();
            let mut tx = db.begin().unwrap();
            assert_eq!(tx.get(b"a").unwrap().is_some(), !partial);
            assert_eq!(tx.get(b"b").unwrap().is_some(), !partial);
        }
    }
    #[test]
    fn eq_checkpoint_captures_concurrent_changes_and_preserves_old_generation_and_conflicts() {
        for count in [0_u32, 1, 255, 256, 257, 600] {
            let path = Fixture::new();
            let db = Database::create(&path.0, [62; 16]).unwrap();
            if count > 0 {
                let mut tx = db.begin().unwrap();
                for n in 0..count {
                    tx.put(&n.to_be_bytes(), b"old").unwrap();
                }
                tx.commit().unwrap();
            }
            let mut old = db.begin().unwrap();
            let mut dependent = db.begin().unwrap();
            dependent.scan(b"", None).unwrap();
            dependent.put(b"conflict", b"not committed").unwrap();
            let base = old.version();
            let mut updated = false;
            let info = db
                .checkpoint_with(|stage| {
                    if !updated
                        && matches!(
                            stage,
                            CheckpointStage::ChunkCopied | CheckpointStage::Written
                        )
                    {
                        let writer_db = db.clone();
                        let (send, recv) = std::sync::mpsc::channel();
                        let worker = std::thread::spawn(move || {
                            let mut tx = writer_db.begin().unwrap();
                            tx.put(b"added", b"new").unwrap();
                            if count > 0 {
                                tx.put(&0_u32.to_be_bytes(), b"edited").unwrap();
                            }
                            if count > 1 {
                                tx.delete(&1_u32.to_be_bytes()).unwrap();
                            }
                            tx.commit().unwrap();
                            send.send(()).unwrap();
                        });
                        recv.recv_timeout(std::time::Duration::from_secs(2))
                            .map_err(io::Error::other)?;
                        worker.join().unwrap();
                        updated = true;
                    }
                    if stage == CheckpointStage::Synced {
                        let mut tx = db.begin().unwrap();
                        tx.put(b"added", b"latest").unwrap();
                        tx.put(b"after-sync", b"durable").unwrap();
                        tx.commit().unwrap();
                    }
                    Ok(())
                })
                .unwrap();
            assert_eq!(info.version, base);
            assert_eq!(info.records, count as usize);
            assert!(matches!(
                dependent.commit(),
                Err(Error::Conflict(ConflictKind::Range))
            ));
            assert_eq!(old.scan(b"", None).unwrap().len(), count as usize);
            for n in 0..count {
                assert_eq!(old.get(&n.to_be_bytes()).unwrap(), Some(b"old".to_vec()));
            }
            assert_eq!(old.get(b"added").unwrap(), None);
            let expected = db.begin().unwrap().scan(b"", None).unwrap();
            assert_eq!(expected.len(), count as usize + 2 - usize::from(count > 1));
            assert_eq!(
                db.begin().unwrap().get(b"added").unwrap(),
                Some(b"latest".to_vec())
            );
            if count > 0 {
                assert_eq!(
                    db.begin().unwrap().get(&0_u32.to_be_bytes()).unwrap(),
                    Some(b"edited".to_vec())
                );
            }
            if count > 1 {
                assert_eq!(db.begin().unwrap().get(&1_u32.to_be_bytes()).unwrap(), None);
            }
            db.checkpoint().unwrap();
            assert_eq!(old.scan(b"", None).unwrap().len(), count as usize);
            drop(old);
            drop(db);
            let reopened = Database::open(&path.0).unwrap();
            assert_eq!(reopened.stats().unwrap().version, base + 2);
            assert_eq!(reopened.begin().unwrap().scan(b"", None).unwrap(), expected);
        }
    }

    #[test]
    fn er_checkpoint_tail_failure_keeps_acknowledged_writes_and_cursor_usable() {
        for phase in [CheckpointStage::TailCopied, CheckpointStage::TailSynced] {
            let path = Fixture::new();
            let db = Database::create(&path.0, [63; 16]).unwrap();
            let result = db.checkpoint_with(|stage| {
                if stage == CheckpointStage::Written {
                    let mut tx = db.begin().unwrap();
                    tx.put(b"ack", b"durable").unwrap();
                    tx.commit().unwrap();
                }
                if stage == phase {
                    return Err(io::Error::other("injected tail failure"));
                }
                Ok(())
            });
            assert!(matches!(result, Err(Error::Io(_))));
            let mut next = db.begin().unwrap();
            next.put(b"later", b"durable").unwrap();
            assert_eq!(next.commit().unwrap(), 2);
            drop(db);
            let reopened = Database::open(&path.0).unwrap();
            assert_eq!(
                reopened.begin().unwrap().get(b"ack").unwrap(),
                Some(b"durable".to_vec())
            );
            assert_eq!(
                reopened.begin().unwrap().get(b"later").unwrap(),
                Some(b"durable".to_vec())
            );
        }
    }

    #[test]
    fn bv_checkpoint_snapshot_uses_transaction_budget_and_releases_it_after_failure() {
        let path = Fixture::new();
        let db = Database::create_with_limits(
            &path.0,
            [64; 16],
            ResourceLimits {
                max_transactions: 1,
                ..ResourceLimits::default()
            },
        )
        .unwrap();
        let held = db.begin().unwrap();
        assert!(matches!(db.checkpoint(), Err(Error::ResourceExhausted(_))));
        drop(held);
        assert!(matches!(
            db.checkpoint_with(|_| Err(io::Error::other("failure"))),
            Err(Error::Io(_))
        ));
        assert_eq!(db.resource_usage().unwrap().current[ACTIVE], 0);
        db.checkpoint().unwrap();
        assert_eq!(db.resource_usage().unwrap().current[ACTIVE], 0);
    }

    #[test]
    fn bv_checkpoint_retained_limit_rejects_excess_update_then_reuses_capacity() {
        let path = Fixture::new();
        let db = Database::create_with_limits(
            &path.0,
            [65; 16],
            ResourceLimits {
                max_retained_bytes: 324,
                ..ResourceLimits::default()
            },
        )
        .unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"k", b"a").unwrap();
        tx.commit().unwrap();
        db.checkpoint_with(|stage| {
            if stage == CheckpointStage::Written {
                let mut tx = db.begin().unwrap();
                tx.put(b"k", b"b").unwrap();
                tx.commit().unwrap();
                assert_eq!(db.resource_usage().unwrap().current[RETAINED], 324);
                let mut excess = db.begin().unwrap();
                excess.put(b"k", b"c").unwrap();
                assert!(matches!(excess.commit(), Err(Error::ResourceExhausted(_))));
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(db.resource_usage().unwrap().current[RETAINED], 259);
        let mut next = db.begin().unwrap();
        next.put(b"k", b"c").unwrap();
        next.commit().unwrap();
        drop(db);
        assert_eq!(
            Database::open(&path.0)
                .unwrap()
                .begin()
                .unwrap()
                .get(b"k")
                .unwrap(),
            Some(b"c".to_vec())
        );
    }

    #[test]
    fn bv_checkpoint_rejects_snapshot_plus_tail_over_limit_before_replacing_original() {
        let path = Fixture::new();
        let db = Database::create(&path.0, [67; 16]).unwrap();
        let mut tx = db.begin().unwrap();
        for n in 0_u32..600 {
            tx.put(&n.to_be_bytes(), b"v").unwrap();
        }
        tx.commit().unwrap();
        drop(db);
        let original = std::fs::read(&path.0).unwrap();
        let db = Database::open(&path.0).unwrap();
        let result = db.checkpoint_with(|stage| {
            if stage == CheckpointStage::Synced {
                // A synthetic committed length avoids allocating a 1 GiB file just to test admission.
                db.state().unwrap().wal_bytes = MAX_WAL_BYTES;
            }
            Ok(())
        });
        db.state().unwrap().wal_bytes = original.len() as u64;
        assert!(matches!(result, Err(Error::Limit("checkpoint bytes"))));
        assert_eq!(db.resource_usage().unwrap().current[ACTIVE], 0);
        drop(db);
        assert_eq!(std::fs::read(&path.0).unwrap(), original);
        let db = Database::open(&path.0).unwrap();
        let mut next = db.begin().unwrap();
        next.put(b"next", b"ok").unwrap();
        next.commit().unwrap();
        drop(db);
        assert_eq!(
            Database::open(&path.0)
                .unwrap()
                .stats()
                .unwrap()
                .visible_records,
            601
        );
    }

    #[test]
    fn er_tail_output_write_failure_restores_original_wal_append_position() {
        let path = Fixture::new();
        let db = Database::create(&path.0, [66; 16]).unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"ack", &vec![7; 70_000]).unwrap();
        tx.commit().unwrap();
        let writer = db.inner.writer.lock().unwrap();
        let end = db.state().unwrap().wal_bytes;
        let mut readonly = File::open(&path.0).unwrap();
        let mut permit = db.inner.budget.buffer();
        assert!(matches!(
            db.copy_checkpoint_tail(&mut readonly, HEADER_BYTES as u64, end, &mut permit),
            Err(Error::Io(_))
        ));
        drop(readonly);
        drop(writer);
        let mut next = db.begin().unwrap();
        next.put(b"later", b"ok").unwrap();
        next.commit().unwrap();
        drop(db);
        let reopened = Database::open(&path.0).unwrap();
        assert_eq!(
            reopened.begin().unwrap().get(b"ack").unwrap(),
            Some(vec![7; 70_000])
        );
        assert_eq!(
            reopened.begin().unwrap().get(b"later").unwrap(),
            Some(b"ok".to_vec())
        );
    }

    #[test]
    fn bv_checkpoint_tail_copy_boundaries_preserve_old_reads_and_subsequent_appends() {
        for tail_bytes in [0_usize, 65_535, 65_536, 65_537, 131_072] {
            let path = Fixture::new();
            let db = Database::create(&path.0, [68; 16]).unwrap();
            let mut initial = db.begin().unwrap();
            initial.put(b"seed", b"stable").unwrap();
            initial.commit().unwrap();
            let mut old = db.begin().unwrap();
            let value = vec![17; tail_bytes.saturating_sub(54)];
            let info = db
                .checkpoint_with(|stage| {
                    if stage == CheckpointStage::Synced && tail_bytes > 0 {
                        let mut update = db.begin().unwrap();
                        update.put(b"k", &value).unwrap();
                        update.commit().unwrap();
                    }
                    Ok(())
                })
                .unwrap();
            assert_eq!(info.after_bytes - info.before_bytes, tail_bytes as u64);
            assert_eq!(old.get(b"k").unwrap(), None);
            assert_eq!(old.get(b"seed").unwrap(), Some(b"stable".to_vec()));
            let mut next = db.begin().unwrap();
            next.put(b"next", b"acknowledged").unwrap();
            next.commit().unwrap();
            db.checkpoint().unwrap();
            assert_eq!(old.get(b"k").unwrap(), None);
            drop(old);
            drop(db);
            let reopened = Database::open(&path.0).unwrap();
            let mut tx = reopened.begin().unwrap();
            assert_eq!(tx.get(b"seed").unwrap(), Some(b"stable".to_vec()));
            assert_eq!(tx.get(b"next").unwrap(), Some(b"acknowledged".to_vec()));
            assert_eq!(tx.get(b"k").unwrap(), (tail_bytes > 0).then_some(value));
        }
    }

    #[test]
    fn checkpoint_readers_progress_at_copy_sync_and_publication_stages() {
        let path = Fixture::new();
        let db = Database::create(&path.0, [61; 16]).unwrap();
        let mut first = db.begin().unwrap();
        first.put(b"key", b"old").unwrap();
        first.commit().unwrap();
        let mut old = db.begin().unwrap();
        let mut next = db.begin().unwrap();
        next.put(b"key", b"new").unwrap();
        for n in 0_u32..600 {
            next.put(&n.to_be_bytes(), b"v").unwrap();
        }
        next.commit().unwrap();
        let mut seen = [false; 9];
        db.checkpoint_with(|stage| {
            let index = match stage {
                CheckpointStage::ChunkCopied => 0,
                CheckpointStage::Written => 1,
                CheckpointStage::Synced => 2,
                CheckpointStage::Published => 3,
                CheckpointStage::TailCopied => 4,
                CheckpointStage::TailSynced => 5,
                CheckpointStage::Retained => 6,
                CheckpointStage::Released => 7,
                CheckpointStage::Reclaimed => 8,
            };
            seen[index] = true;
            let reader_db = db.clone();
            let (send, recv) = std::sync::mpsc::channel();
            let reader = std::thread::spawn(move || -> Result<()> {
                let mut tx = reader_db.begin()?;
                assert_eq!(tx.get(b"key")?, Some(b"new".to_vec()));
                tx.commit()?;
                send.send(()).unwrap();
                Ok(())
            });
            recv.recv_timeout(std::time::Duration::from_secs(2))
                .map_err(io::Error::other)?;
            reader.join().unwrap().unwrap();
            assert_eq!(
                old.get(b"key").map_err(io::Error::other)?,
                Some(b"old".to_vec())
            );
            Ok(())
        })
        .unwrap();
        assert!(seen.into_iter().all(|v| v));
        assert_eq!(old.get(b"key").unwrap(), Some(b"old".to_vec()));
        drop(old);
        drop(db);
        let reopened = Database::open(&path.0).unwrap();
        assert_eq!(reopened.stats().unwrap().visible_records, 601);
        assert_eq!(
            reopened.begin().unwrap().get(b"key").unwrap(),
            Some(b"new".to_vec())
        );
    }

    #[test]
    fn st_version_one_log_can_be_opened_and_upgraded_by_checkpoint() {
        let path = Fixture::new();
        let db = Database::create(&path.0, [2; 16]).unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"old", b"v1").unwrap();
        tx.commit().unwrap();
        drop(db);
        let mut bytes = std::fs::read(&path.0).unwrap();
        bytes[8..12].copy_from_slice(&1_u32.to_le_bytes());
        let crc = crate::crc32(&bytes[..44]);
        bytes[44..48].copy_from_slice(&crc.to_le_bytes());
        std::fs::write(&path.0, bytes).unwrap();
        let db = Database::open(&path.0).unwrap();
        assert_eq!(
            db.begin().unwrap().get(b"old").unwrap(),
            Some(b"v1".to_vec())
        );
        db.checkpoint().unwrap();
        drop(db);
        assert_eq!(read_u32(&std::fs::read(&path.0).unwrap(), 8), 2);
    }

    #[test]
    fn er_checkpoint_failure_before_publication_preserves_store_and_after_publication_requires_reopen()
     {
        for phase in [
            CheckpointStage::ChunkCopied,
            CheckpointStage::Written,
            CheckpointStage::Synced,
            CheckpointStage::TailCopied,
            CheckpointStage::TailSynced,
            CheckpointStage::Retained,
            CheckpointStage::Published,
        ] {
            let path = Fixture::new();
            let db = Database::create(&path.0, [2; 16]).unwrap();
            db.set_checkpoint_retirement(true, MAX_WAL_BYTES).unwrap();
            let mut tx = db.begin().unwrap();
            tx.put(b"a", b"stable").unwrap();
            tx.commit().unwrap();
            let result = db.checkpoint_with(|stage| {
                if stage == phase {
                    Err(io::Error::new(
                        io::ErrorKind::StorageFull,
                        "injected checkpoint failure",
                    ))
                } else {
                    Ok(())
                }
            });
            if phase == CheckpointStage::Published {
                assert!(matches!(result, Err(Error::CheckpointUncertain(_))));
                assert!(matches!(db.begin(), Err(Error::Poisoned)));
            } else {
                assert!(matches!(result, Err(Error::Io(_))));
                assert_eq!(
                    db.begin().unwrap().get(b"a").unwrap(),
                    Some(b"stable".to_vec())
                );
            }
            drop(db);
            let db = Database::open(&path.0).unwrap();
            assert_eq!(
                db.begin().unwrap().get(b"a").unwrap(),
                Some(b"stable".to_vec())
            );
        }
    }

    #[test]
    #[ignore = "subprocess helper"]
    fn checkpoint_crash_worker() {
        let path = std::env::var_os("NEWDBMS_CHECKPOINT_PATH").unwrap();
        let phase = std::env::var("NEWDBMS_CHECKPOINT_PHASE").unwrap();
        let db = Database::open(path).unwrap();
        db.set_checkpoint_retirement(true, MAX_WAL_BYTES).unwrap();
        if phase == "deferred-returned" {
            db.checkpoint_deferred_reclaim().unwrap();
            let mut tx = db.begin().unwrap();
            tx.put(b"key", &[20]).unwrap();
            tx.commit().unwrap();
            std::process::exit(76);
        }
        let mut updated = false;
        let _ = db.checkpoint_with(|stage| {
            if stage == CheckpointStage::Retained {
                let path = std::env::var_os("NEWDBMS_CHECKPOINT_PATH").unwrap();
                assert!(with_suffix(Path::new(&path), ".retired").exists());
            }
            if stage == CheckpointStage::ChunkCopied && !updated {
                let mut tx = db.begin().unwrap();
                tx.put(b"key", &[20]).unwrap();
                tx.commit().unwrap();
                updated = true;
            }
            let target = match phase.as_str() {
                "copied" => CheckpointStage::ChunkCopied,
                "written" => CheckpointStage::Written,
                "synced" => CheckpointStage::Synced,
                "tail" => CheckpointStage::TailCopied,
                "tail-synced" => CheckpointStage::TailSynced,
                "retained" => CheckpointStage::Retained,
                "released" => CheckpointStage::Released,
                "reclaimed" => CheckpointStage::Reclaimed,
                _ => CheckpointStage::Published,
            };
            if stage == target {
                std::process::exit(76);
            }
            Ok(())
        });
        panic!("checkpoint crash point not reached");
    }

    #[test]
    fn er_checkpoint_process_exit_before_and_after_replacement_preserves_committed_state() {
        for phase in [
            "copied",
            "written",
            "synced",
            "tail",
            "tail-synced",
            "published",
            "retained",
            "released",
            "reclaimed",
            "deferred-returned",
        ] {
            let path = Fixture::new();
            let db = Database::create(&path.0, [2; 16]).unwrap();
            for n in 0_u8..20 {
                let mut tx = db.begin().unwrap();
                tx.put(b"key", &[n]).unwrap();
                tx.commit().unwrap();
            }
            drop(db);
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--ignored",
                    "--exact",
                    "transactions::tests::checkpoint_crash_worker",
                    "--nocapture",
                ])
                .env("NEWDBMS_CHECKPOINT_PATH", &path.0)
                .env("NEWDBMS_CHECKPOINT_PHASE", phase)
                .output()
                .unwrap();
            assert_eq!(result.status.code(), Some(76), "{result:?}");
            let db = Database::open(&path.0).unwrap();
            assert_eq!(db.stats().unwrap().version, 21);
            assert_eq!(db.begin().unwrap().get(b"key").unwrap(), Some(vec![20]));
            let mut tx = db.begin().unwrap();
            tx.put(b"next", b"ok").unwrap();
            assert_eq!(tx.commit().unwrap(), 22);
            drop(db);
            let parent = path.0.parent().unwrap();
            let prefix = format!(
                "{}.checkpoint-",
                path.0.file_name().unwrap().to_string_lossy()
            );
            for entry in std::fs::read_dir(parent).unwrap().flatten() {
                if entry.file_name().to_string_lossy().starts_with(&prefix) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    }
}

#[cfg(windows)]
fn reserve_wal_space(file: &File, allocation: u64) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    unsafe extern "system" {
        fn SetFileInformationByHandle(
            handle: *mut std::ffi::c_void,
            class: i32,
            info: *const i64,
            size: u32,
        ) -> i32;
    }
    let bytes = allocation as i64;
    // Setting EOF would expose unwritten bytes as recovery records.
    if unsafe { SetFileInformationByHandle(file.as_raw_handle(), 5, &bytes, 8) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(not(windows))]
fn reserve_wal_space(_file: &File, _allocation: u64) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "WAL allocation is Windows-only",
    ))
}

#[cfg(all(test, windows))]
mod reservation_tests {
    use super::*;

    #[test]
    fn reservation_keeps_eof_old_reads_and_recovers_across_checkpoint() {
        let path = temporary_path(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("artifacts/reservation.wal"),
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = Database::create(&path, [17; 16]).unwrap();
        let mut old = db.begin().unwrap();
        for size in [32000, 32768, 33000] {
            let mut tx = db.begin().unwrap();
            tx.put(b"key", &vec![7; size]).unwrap();
            tx.commit().unwrap();
            let state = db.state().unwrap();
            assert_eq!(
                state.file.as_ref().unwrap().metadata().unwrap().len(),
                state.wal_bytes
            );
            assert!(state.reserved_bytes >= state.wal_bytes);
            assert!(state.reserved_bytes - state.wal_bytes < 32768);
        }
        assert_eq!(old.get(b"key").unwrap(), None);
        db.checkpoint().unwrap();
        assert_eq!(db.state().unwrap().reserved_bytes, 0);
        let mut tx = db.begin().unwrap();
        tx.put(b"after", b"checkpoint").unwrap();
        tx.commit().unwrap();
        assert!(db.state().unwrap().reserved_bytes > 0);
        drop(old);
        drop(db);
        let reopened = Database::open(&path).unwrap();
        assert_eq!(
            reopened.begin().unwrap().get(b"key").unwrap(),
            Some(vec![7; 33000])
        );
        let mut tx = reopened.begin().unwrap();
        tx.put(b"reopened", b"value").unwrap();
        tx.commit().unwrap();
        assert!(reopened.state().unwrap().reserved_bytes > 0);
        reopened.set_log_reservation(false).unwrap();
        let mut tx = reopened.begin().unwrap();
        tx.put(b"disabled", b"value").unwrap();
        tx.commit().unwrap();
        drop(reopened);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(with_suffix(&path, ".owner")).unwrap();
    }

    #[test]
    fn allocation_failure_preserves_eof_and_falls_back_without_poisoning() {
        let path = temporary_path(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("artifacts/reservation-failure.wal"),
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let db = Database::create(&path, [18; 16]).unwrap();
        {
            let mut state = db.state().unwrap();
            let writable = state.file.take();
            state.file = Some(legacy_wal_file(File::open(&path).unwrap()));
            state.reserve_log(32768);
            assert!(!state.reservation_enabled);
            assert!(!state.poisoned);
            assert_eq!(
                state.file.as_ref().unwrap().metadata().unwrap().len(),
                HEADER_BYTES as u64
            );
            state.file = writable;
        }
        let mut tx = db.begin().unwrap();
        tx.put(b"fallback", b"durable").unwrap();
        tx.commit().unwrap();
        drop(db);
        let db = Database::open(&path).unwrap();
        assert_eq!(
            db.begin().unwrap().get(b"fallback").unwrap(),
            Some(b"durable".to_vec())
        );
        drop(db);
        std::fs::remove_file(&path).unwrap();
        std::fs::remove_file(with_suffix(&path, ".owner")).unwrap();
    }
}

#[cfg(feature = "lock-timing")]
thread_local! {
    static THREAD_LOCK_WAIT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Diagnostic sum of this thread's database state-lock acquisition waits, reset on read.
/// Excludes transaction Drop cleanup and non-state locks; not enabled in normal builds.
#[cfg(feature = "lock-timing")]
pub fn take_thread_lock_wait_ns() -> u64 {
    THREAD_LOCK_WAIT.with(|total| total.replace(0))
}

#[cfg(feature = "lock-timing")]
thread_local! {
    static CHECKPOINT_TIMING: std::cell::Cell<[u64; 8]> = const { std::cell::Cell::new([0; 8]) };
    static CHECKPOINT_REPLACE_TRACE: std::cell::Cell<[u64; 2]> = const { std::cell::Cell::new([0; 2]) };
}

#[cfg(feature = "lock-timing")]
pub fn take_checkpoint_replace_trace() -> [u64; 2] {
    CHECKPOINT_REPLACE_TRACE.with(|trace| trace.replace([0; 2]))
}

/// Successful checkpoint: wait, copy, sync, close, replace, reopen, held nanoseconds, tail bytes.
#[cfg(feature = "lock-timing")]
pub fn take_checkpoint_timing() -> [u64; 8] {
    CHECKPOINT_TIMING.with(|value| value.replace([0; 8]))
}

#[cfg(feature = "lock-timing")]
thread_local! {
    static COMMIT_SYNC_START: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static COMMIT_TRACE: std::cell::Cell<[u64; 8]> = const { std::cell::Cell::new([0; 8]) };
}
#[cfg(feature = "lock-timing")]
thread_local! {
    static COMMIT_DETAIL: std::cell::Cell<[u64; 8]> = const { std::cell::Cell::new([0; 8]) };
}
#[cfg(feature = "lock-timing")]
thread_local! {
    static COMMIT_COSTS: std::cell::Cell<[u64; 2]> = const { std::cell::Cell::new([0; 2]) };
}
#[cfg(feature = "lock-timing")]
struct CommitCost(usize, u64);
#[cfg(feature = "lock-timing")]
impl Drop for CommitCost {
    fn drop(&mut self) {
        COMMIT_COSTS.with(|cell| {
            let mut value = cell.get();
            value[self.0] += diagnostic_now_ns() - self.1;
            cell.set(value);
        });
    }
}
#[cfg(feature = "lock-timing")]
pub fn take_commit_sync_start() -> u64 {
    COMMIT_SYNC_START.with(|value| value.replace(0))
}
#[cfg(feature = "lock-timing")]
fn sync_start_mark() {
    COMMIT_SYNC_START.with(|value| value.set(diagnostic_now_ns()));
}
/// Cumulative validation and frame-construction nanoseconds for this batch.
#[cfg(feature = "lock-timing")]
pub fn take_commit_costs() -> [u64; 2] {
    COMMIT_COSTS.with(|value| value.replace([0; 2]))
}
#[cfg(feature = "lock-timing")]
fn detail_mark(index: usize) {
    COMMIT_DETAIL.with(|trace| {
        let mut value = trace.get();
        value[index] = diagnostic_now_ns();
        trace.set(value);
    });
}
/// Pending wait, collection start, state acquired, reserve start, publication lock,
/// reply ready, reply received, commit entry. Zero means that phase was not reached.
#[cfg(feature = "lock-timing")]
pub fn take_commit_detail() -> [u64; 8] {
    COMMIT_DETAIL.with(|trace| trace.replace([0; 8]))
}

#[cfg(feature = "lock-timing")]
pub fn diagnostic_now_ns() -> u64 {
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    ORIGIN
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_nanos() as u64
        + 1
}
#[cfg(feature = "lock-timing")]
fn trace_mark(index: usize) {
    COMMIT_TRACE.with(|trace| {
        let mut v = trace.get();
        v[index] = diagnostic_now_ns();
        trace.set(v);
    });
}
/// Submit, batch selection, writer wait start, writer acquired, write start, write end, sync end, publication end.
#[cfg(feature = "lock-timing")]
pub fn take_commit_trace() -> [u64; 8] {
    COMMIT_TRACE.with(|trace| trace.replace([0; 8]))
}

fn wrap_checkpoint_file(mut file: File, end: u64) -> io::Result<WalFile> {
    #[cfg(feature = "fixed-log")]
    {
        file.seek(SeekFrom::Start(0))?;
        let mut header = [0; HEADER_BYTES];
        file.read_exact(&mut header)?;
        let mut file = WalFile::open(file, header)?;
        file.seek(SeekFrom::Start(end))?;
        Ok(file)
    }
    #[cfg(not(feature = "fixed-log"))]
    {
        file.seek(SeekFrom::Start(end))?;
        Ok(file)
    }
}

fn legacy_wal_file(file: File) -> WalFile {
    #[cfg(feature = "fixed-log")]
    {
        file.into()
    }
    #[cfg(not(feature = "fixed-log"))]
    {
        file
    }
}

#[cfg(test)]
mod read_copy_tests {
    use super::*;
    #[test]
    fn er_worker_start_failure_leaves_cooperative_reclamation_available() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("artifacts")
            .join(format!("gc-spawn-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::create(&path, [31; 16]).unwrap();
        reclamation::FAIL_START.with(|f| f.set(true));
        assert!(db.set_background_reclamation(true).is_err());
        assert!(!db.background_reclamation_enabled());
        let mut w = db.begin().unwrap();
        w.put(b"k", b"v").unwrap();
        w.commit().unwrap();
        db.set_background_reclamation(true).unwrap();
        assert_eq!(
            db.begin_read().unwrap().get(b"k").unwrap(),
            Some(b"v".to_vec())
        );
        db.set_background_reclamation(false).unwrap();
    }

    #[test]
    fn er_detached_versions_stay_charged_until_their_values_are_freed() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("artifacts")
            .join(format!("gc-charge-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Database::create(&path, [27; 16]).unwrap();
        let mut w = db.begin().unwrap();
        w.put(b"k", &[1; 1024]).unwrap();
        w.commit().unwrap();
        let old = db.begin_read().unwrap();
        let mut w = db.begin().unwrap();
        w.put(b"k", &[2; 1024]).unwrap();
        w.commit().unwrap();
        let before = db.resource_usage().unwrap().current[RETAINED];
        let mut state = db.state().unwrap();
        state.active.remove(&old.snapshot);
        let retired = state.collect();
        assert_eq!(retired.charge, 1088);
        assert_eq!(state.budget.snapshot().unwrap().current[RETAINED], before);
        drop(state);
        drop(retired);
        assert_eq!(
            db.resource_usage().unwrap().current[RETAINED],
            before - 1088
        );
        drop(old);
    }

    #[test]
    fn st_copy_is_unlocked_while_old_value_stays_pinned() {
        for page in [false, true] {
            let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("artifacts")
                .join(format!(
                    "copy-gate-{}-{}",
                    std::process::id(),
                    u8::from(page)
                ));
            let _ = std::fs::remove_file(&path);
            let db = Database::create(&path, [29; 16]).unwrap();
            let mut w = db.begin().unwrap();
            w.put(b"k", &vec![1; 65536]).unwrap();
            w.commit().unwrap();
            let gate = Arc::new([std::sync::Barrier::new(2), std::sync::Barrier::new(2)]);
            std::thread::scope(|scope| {
                let gate2 = gate.clone();
                let db2 = db.clone();
                let reader = scope.spawn(move || {
                    let mut r = db2.begin_read().unwrap();
                    READ_COPY_GATE.with(|g| *g.borrow_mut() = Some(gate2));
                    if page {
                        r.scan_page(b"", None, None, ScanLimits::default())
                            .unwrap()
                            .rows
                            .remove(0)
                            .1
                    } else {
                        r.get(b"k").unwrap().unwrap()
                    }
                });
                gate[0].wait();
                let mut w = db.begin().unwrap();
                w.put(b"k", &vec![2; 65536]).unwrap();
                let updated = w.commit();
                let retained = db.resource_usage().unwrap().current[3];
                gate[1].wait();
                assert!(updated.is_ok());
                assert!(retained >= 2 * 65536);
                assert_eq!(reader.join().unwrap(), vec![1; 65536]);
            });
            assert_eq!(
                db.begin_read().unwrap().get(b"k").unwrap().unwrap(),
                vec![2; 65536]
            );
        }
    }
}

#[cfg(test)]
#[path = "../benchmarks/global-generation/engine_bench.rs"]
mod generation_guard_bench;

#[cfg(test)]
#[path = "../benchmarks/work-elision/tx_bench.rs"]
mod work_elision_bench;

#[cfg(test)]
#[path = "../benchmarks/scan-merge/engine_bench.rs"]
mod scan_merge_bench;

#[cfg(test)]
#[path = "../benchmarks/scan-copy/engine_bench.rs"]
mod scan_copy_bench;

#[cfg(all(test, feature = "lock-timing"))]
#[path = "../benchmarks/wal-preparation/engine_bench.rs"]
mod wal_preparation_bench;
