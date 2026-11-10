use super::raft::{Entry, HardState, MAX_LOG_BYTES, MAX_LOG_ENTRIES};
use super::snapshot::{CHUNK, MAX_IMAGE, Snapshot};
use crate::transactions::{
    Database, MAX_KEY_BYTES, MAX_TRANSACTION_WRITES, ResourceLimits, raft_path,
};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::sync::Mutex;

pub(super) const MAX_COMMAND_BYTES: usize = 1024 * 1024;
const MAX_VALUE: usize = 256 * 1024;
const CONFIG: &[u8] = b"configuration/v1";
const STATE: &[u8] = b"hard-state/v1";

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn log_key(index: u64) -> [u8; 9] {
    let mut key = [b'L'; 9];
    key[1..].copy_from_slice(&index.to_be_bytes());
    key
}

fn number(value: &Value, name: &str) -> io::Result<u64> {
    value[name]
        .as_u64()
        .ok_or_else(|| invalid("invalid Raft state field"))
}

fn hex(value: &Value, maximum: usize) -> io::Result<Vec<u8>> {
    let encoded = value
        .as_str()
        .ok_or_else(|| invalid("expected hex string"))?;
    crate::service::hex_decode(encoded, maximum).map_err(io::Error::other)
}

type WriteSet = Vec<(Vec<u8>, Option<Vec<u8>>)>;

fn decode_command(command: &Value) -> io::Result<(WriteSet, usize)> {
    if command.get("membership").is_some() {
        super::membership::Membership::parse(command)?;
        return Ok((Vec::new(), serde_json::to_vec(command)?.len()));
    }
    if command.is_null() {
        return Ok((Vec::new(), 4));
    }
    let object = command
        .as_object()
        .ok_or_else(|| invalid("invalid replicated command"))?;
    let writes = object
        .get("writes")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid("missing write set"))?;
    if object.len() != 1 || writes.len() > MAX_TRANSACTION_WRITES {
        return Err(invalid("replicated command exceeds its limits"));
    }
    let mut seen = HashSet::new();
    let mut decoded = Vec::with_capacity(writes.len());
    let mut bytes = 13usize; // {"writes":[]}
    for (i, pair) in writes.iter().enumerate() {
        let pair = pair
            .as_array()
            .filter(|p| p.len() == 2)
            .ok_or_else(|| invalid("invalid write pair"))?;
        let key_len = pair[0].as_str().map_or(0, str::len);
        let value_len = pair[1]
            .as_str()
            .map_or(9, |s| 7usize.saturating_add(s.len()));
        bytes = bytes
            .saturating_add(key_len)
            .saturating_add(value_len)
            .saturating_add(usize::from(i != 0));
        if bytes > MAX_COMMAND_BYTES {
            return Err(invalid("replicated command exceeds its byte limit"));
        }
        let key = hex(&pair[0], MAX_KEY_BYTES)?;
        if !seen.insert(key.clone()) {
            return Err(invalid("duplicate key in replicated write set"));
        }
        let value = if pair[1].is_null() {
            None
        } else {
            Some(hex(&pair[1], MAX_VALUE)?)
        };
        decoded.push((key, value));
    }
    Ok((decoded, bytes))
}

pub(super) fn validate_command(command: &Value) -> io::Result<()> {
    decode_command(command).map(|_| ())
}

pub(super) fn command_size(command: &Value) -> io::Result<usize> {
    decode_command(command).map(|(_, size)| size)
}

const IMAGE: &[u8] = b"snapshot/header";
const ORIGIN: &[u8] = b"snapshot/application";
fn chunk_key(bank: u64, chunk: usize) -> Vec<u8> {
    format!("snapshot/{bank}/{chunk:016x}").into_bytes()
}

struct Durable {
    dynamic: bool,
    base: u64,
    bank: u64,
    term: u64,
    voted_for: Option<String>,
    commit: u64,
    prefix_bytes: Vec<usize>,
}

pub(super) struct Storage {
    dynamic_signature: Vec<u8>,
    origin: Mutex<(u64, u64, bool)>,
    data: Database,
    journal: Database,

    // An uncertain write must not be followed by a vote or another apply in this process.
    durable: Mutex<Option<Durable>>,
}

impl Storage {
    pub(super) fn database(&self) -> Database {
        self.data.clone()
    }

    pub(super) fn open(
        path: &Path,
        cluster: &str,
        node: &str,
        members: &[String],
        initialize: bool,
        limits: ResourceLimits,
    ) -> io::Result<(Self, HardState)> {
        let mut membership = members.to_vec();
        membership.sort();
        let ids: HashSet<_> = membership.iter().cloned().collect();
        if cluster.is_empty()
            || ids.len() != membership.len()
            || ids.contains("")
            || !super::membership::valid_id(node)
        {
            return Err(invalid("invalid replica identity"));
        }
        if limits.max_staged_bytes < 10 << 20
            || limits.max_buffer_bytes < 12 << 20
            || limits.max_retained_bytes < 20 << 20
            || limits.max_dependency_bytes < 1024
        {
            return Err(invalid(
                "cluster apply requires >=10 MiB staged, 12 MiB buffer, 20 MiB retained, and 1 KiB dependency budgets",
            ));
        }
        let journal_limits = ResourceLimits {
            max_retained_bytes: 512 << 20,
            max_buffer_bytes: 64 << 20,
            ..ResourceLimits::default()
        };
        let signature = json!({"format":1,"cluster":cluster,"node":node,"members":membership});
        let sidecar = raft_path(path);
        let (journal, data) = match (path.try_exists()?, sidecar.try_exists()?) {
            (false, false) if initialize => {
                let mut id = [0; 16];
                getrandom::getrandom(&mut id).map_err(|e| io::Error::other(e.to_string()))?;
                // Creation never enrolls an existing database. Publishing the sidecar first also
                // prevents a standalone server from claiming the application file during bootstrap.
                let journal = Database::create_replica_file(&sidecar, id, journal_limits)
                    .map_err(io::Error::other)?;
                let data =
                    Database::create_replica_file(path, id, limits).map_err(io::Error::other)?;
                let mut tx = journal.begin().map_err(io::Error::other)?;
                tx.put(CONFIG, &serde_json::to_vec(&signature)?)
                    .map_err(io::Error::other)?;
                tx.put(STATE, &encode_state(&HardState::default(), 0)?)
                    .map_err(io::Error::other)?;
                tx.commit().map_err(io::Error::other)?;
                #[cfg(unix)]
                std::fs::File::open(
                    path.parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .unwrap_or(Path::new(".")),
                )?
                .sync_all()?;
                (journal, data)
            }
            (true, true) => (
                Database::open_replica_file(&sidecar, journal_limits).map_err(io::Error::other)?,
                Database::open_replica_file(path, limits).map_err(io::Error::other)?,
            ),
            (false, false) => {
                return Err(invalid(
                    "replica databases do not exist; initialize a new cluster member",
                ));
            }
            _ => return Err(invalid("incomplete replica pair or standalone database")),
        };
        if data.id() != journal.id() {
            return Err(invalid("replica database generation mismatch"));
        }
        let mut reader = journal.begin_read().map_err(io::Error::other)?;
        let config = reader
            .get(CONFIG)
            .map_err(io::Error::other)?
            .ok_or_else(|| invalid("missing replica configuration"))?;
        let config: Value = serde_json::from_slice(&config)?;
        let mut dynamic_signature = signature.clone();
        dynamic_signature["format"] = 2.into();
        let dynamic = config == dynamic_signature;
        if !dynamic && config != signature {
            return Err(invalid(
                "replica node, cluster or fixed membership mismatch",
            ));
        }
        let header = reader
            .get(STATE)
            .map_err(io::Error::other)?
            .ok_or_else(|| invalid("missing Raft hard state"))?;
        let header: Value = serde_json::from_slice(&header)?;
        let last = number(&header, "last")?;
        let high = number(&header, "high")?;
        let mut snapshot = Snapshot::default();
        let mut bank = 0;
        if let Some(meta) = reader.get(IMAGE).map_err(io::Error::other)? {
            let (idx, term, slot, size, hash): (u64, u64, u64, usize, String) =
                serde_json::from_slice(&meta)?;
            if size > MAX_IMAGE || slot > 1 || idx == 0 || idx > i64::MAX as u64 || term == 0 {
                return Err(invalid("snapshot metadata"));
            }
            bank = slot;
            let mut data = Vec::with_capacity(size);
            for i in 0..size.div_ceil(CHUNK) {
                let chunk = reader
                    .get(&chunk_key(slot, i))
                    .map_err(io::Error::other)?
                    .ok_or_else(|| invalid("snapshot chunk missing"))?;
                if chunk.len() != (size - data.len()).min(CHUNK) {
                    return Err(invalid("snapshot chunk length"));
                }
                data.extend_from_slice(&chunk);
            }
            snapshot = Snapshot::new(idx, term, data);
            if let Some(bytes) = reader
                .get(b"snapshot/membership")
                .map_err(io::Error::other)?
            {
                snapshot.membership = serde_json::from_slice(&bytes)?;
            }
            if snapshot.hash != hash {
                return Err(invalid("snapshot checksum"));
            }
        }
        let base = snapshot.index;
        let origin = reader
            .get(ORIGIN)
            .map_err(io::Error::other)?
            .map(|b| serde_json::from_slice(&b))
            .transpose()?
            .unwrap_or((0, 0, false));
        if last < base || last > high || high - base > MAX_LOG_ENTRIES as u64 {
            return Err(invalid("invalid Raft log extent"));
        }
        let vote = match header.get("vote") {
            Some(Value::Null) => None,
            Some(Value::String(id)) if super::membership::valid_id(id) => Some(id.clone()),
            _ => return Err(invalid("invalid persisted vote")),
        };
        let mut state = HardState {
            term: number(&header, "term")?,
            voted_for: vote,
            commit: number(&header, "commit")?,
            snapshot,
            log: Vec::new(),
        };
        if state.commit < base
            || state.commit > last
            || (state.term == 0 && state.voted_for.is_some())
        {
            return Err(invalid("invalid durable hard state"));
        }
        let mut prefix_bytes = vec![0usize];
        let mut previous_term = state.snapshot.term;
        for index in base + 1..=last {
            let bytes = reader
                .get(&log_key(index))
                .map_err(io::Error::other)?
                .ok_or_else(|| invalid("Raft log hole"))?;
            let (term, command): (u64, Value) = serde_json::from_slice(&bytes)?;
            if term == 0 || term < previous_term || term > state.term {
                return Err(invalid("invalid persisted log term"));
            }
            let total = prefix_bytes.last().copied().unwrap() + command_size(&command)? + 48;
            if total > MAX_LOG_BYTES {
                return Err(invalid("Raft log byte limit exceeded"));
            }
            prefix_bytes.push(total);
            previous_term = term;
            state.log.push(Entry { term, command });
        }
        drop(reader);
        let store = Self {
            dynamic_signature: serde_json::to_vec(&dynamic_signature)?,
            origin: Mutex::new(origin),
            data,
            journal,

            durable: Mutex::new(Some(Durable {
                dynamic,
                base,
                bank,
                term: state.term,
                voted_for: state.voted_for.clone(),
                commit: state.commit,
                prefix_bytes,
            })),
        };
        store.apply_through(&state)?;
        store.remove_unreachable(&state, high)?;
        if base > 0 {
            store.prune(base, bank, state.snapshot.bytes.len())?;
        }
        Ok((store, state))
    }

    // Raft has already verified the unchanged prefix when accepting AppendEntries.
    // Passing the splice point avoids keeping a second in-memory copy of every command.
    pub(super) fn persist(&self, state: &HardState, common: usize) -> io::Result<()> {
        let mut guard = self
            .durable
            .lock()
            .map_err(|_| invalid("replica storage poisoned"))?;
        let durable = guard
            .as_mut()
            .ok_or_else(|| invalid("replica storage must be reopened"))?;
        let old_last = durable.base + (durable.prefix_bytes.len() - 1) as u64;
        let base = state.snapshot.index;
        let last = base + state.log.len() as u64;
        let moved = base != durable.base;
        let result = (|| {
            if state.term < durable.term
                || state.commit < durable.commit
                || state.commit > last
                || state.log.len() > MAX_LOG_ENTRIES
                || base < durable.base
                || common > state.log.len()
                || (common > 0 && base + common as u64 > old_last)
                || (state.term == 0 && state.voted_for.is_some())
                || state
                    .voted_for
                    .as_ref()
                    .is_some_and(|v| !super::membership::valid_id(v))
                || (state.term == durable.term
                    && durable.voted_for.is_some()
                    && state.voted_for != durable.voted_for)
            {
                return Err(invalid("invalid durable Raft transition"));
            }
            if base + (common as u64) < durable.commit {
                return Err(invalid("cannot rewrite a committed Raft entry"));
            }
            let high = old_last.max(last);
            let mut pending = false;
            if moved {
                let slot = 1 - durable.bank;
                for (batch, data) in state.snapshot.bytes.chunks(16 * CHUNK).enumerate() {
                    let mut write = self.journal.begin().map_err(io::Error::other)?;
                    for (part, chunk) in data.chunks(CHUNK).enumerate() {
                        write
                            .put(&chunk_key(slot, batch * 16 + part), chunk)
                            .map_err(io::Error::other)?;
                    }
                    write.commit().map_err(io::Error::other)?;
                }
                pending = self.applied_index().map_err(io::Error::other)? < state.snapshot.index;
                durable.bank = slot;
                durable.prefix_bytes = vec![0];
                for e in &state.log[..common] {
                    durable.prefix_bytes.push(
                        durable.prefix_bytes.last().unwrap() + command_size(&e.command)? + 48,
                    );
                }
            }
            let mut tx = self.journal.begin().map_err(io::Error::other)?;
            // Older binaries must refuse this journal even after the configuration
            // entry has been compacted into a snapshot they cannot interpret.
            if !durable.dynamic
                && (state.snapshot.membership.is_some()
                    || state.log[common..]
                        .iter()
                        .any(|e| e.command.get("membership").is_some()))
            {
                tx.put(CONFIG, &self.dynamic_signature)
                    .map_err(io::Error::other)?;
                durable.dynamic = true;
            }
            if moved {
                tx.put(
                    b"snapshot/membership",
                    &serde_json::to_vec(&state.snapshot.membership)?,
                )
                .map_err(io::Error::other)?;
                tx.put(
                    IMAGE,
                    &serde_json::to_vec(&(
                        base,
                        state.snapshot.term,
                        durable.bank,
                        state.snapshot.bytes.len(),
                        &state.snapshot.hash,
                    ))?,
                )
                .map_err(io::Error::other)?;
                if pending {
                    tx.put(ORIGIN, b"[0,0,true]").map_err(io::Error::other)?;
                }
            }
            let mut prior_term = state.log[..common]
                .last()
                .map_or(state.snapshot.term, |entry| entry.term);
            durable.prefix_bytes.truncate(common + 1);
            let mut total = durable.prefix_bytes[common];
            for (offset, entry) in state.log[common..].iter().enumerate() {
                if entry.term == 0 || entry.term < prior_term || entry.term > state.term {
                    return Err(invalid("invalid Raft log term"));
                }
                total += command_size(&entry.command)? + 48;
                if total > MAX_LOG_BYTES {
                    return Err(invalid("Raft log byte limit exceeded"));
                }
                durable.prefix_bytes.push(total);
                tx.put(
                    &log_key(base + (common + offset + 1) as u64),
                    &serde_json::to_vec(&(entry.term, &entry.command))?,
                )
                .map_err(io::Error::other)?;
                prior_term = entry.term;
            }
            tx.put(STATE, &encode_state(state, high)?)
                .map_err(io::Error::other)?;
            tx.commit().map_err(io::Error::other)?;
            if pending {
                *self.origin.lock().unwrap() = (0, 0, true);
            }
            self.apply_through(state)?;
            self.remove_unreachable(state, high)?;
            if moved {
                self.prune(base, durable.bank, state.snapshot.bytes.len())?;
            }
            durable.base = base;
            durable.term = state.term;
            durable.voted_for.clone_from(&state.voted_for);
            durable.commit = state.commit;
            Ok(())
        })();
        if result.is_err() {
            *guard = None;
        }
        result
    }

    fn apply_through(&self, state: &HardState) -> io::Result<()> {
        if self.origin.lock().unwrap().2 {
            match state.snapshot.install(&self.data) {
                Ok(()) => (),
                Err(crate::transactions::Error::ResourceExhausted(_)) => return Ok(()),
                Err(e) => return Err(io::Error::other(e)),
            }
            let origin = (
                state.snapshot.index,
                self.data.current_version().map_err(io::Error::other)?,
                false,
            );
            let mut tx = self.journal.begin().map_err(io::Error::other)?;
            tx.put(ORIGIN, &serde_json::to_vec(&origin)?)
                .map_err(io::Error::other)?;
            tx.commit().map_err(io::Error::other)?;
            *self.origin.lock().unwrap() = origin;
        }
        let version = self.applied_index().map_err(io::Error::other)?;
        if version < state.snapshot.index
            || version > state.commit
            || state.commit - state.snapshot.index > state.log.len() as u64
        {
            return Err(invalid("invalid Raft application watermark"));
        }
        for idx in version + 1..=state.commit {
            let entry = &state.log[(idx - state.snapshot.index - 1) as usize];
            let (writes, _) = decode_command(&entry.command)?;
            match self.apply_entry(idx, writes) {
                Ok(()) => (),
                // A live transaction can pin memory after leadership changes. The journal
                // remains durable; replay starts at the native version once that memory is free.
                Err(crate::transactions::Error::ResourceExhausted(_)) => return Ok(()),
                Err(error) => return Err(io::Error::other(error)),
            }
        }
        Ok(())
    }

    fn apply_entry(&self, index: u64, writes: WriteSet) -> crate::transactions::Result<()> {
        let mut tx = self.data.begin()?;
        if self.logical_version(tx.version())? != index - 1 {
            return Err(crate::transactions::Error::Invalid(
                "application database changed outside Raft",
            ));
        }
        if writes.is_empty() {
            tx.stage_replica_noop()?;
        } else {
            for (key, value) in writes {
                match value {
                    Some(value) => tx.put(&key, &value)?,
                    None => tx.delete(&key)?,
                }
            }
        }
        if self.logical_version(tx.commit()?)? != index {
            return Err(crate::transactions::Error::Invalid(
                "applied log index mismatch",
            ));
        }
        Ok(())
    }

    pub(super) fn retry_apply(&self, state: &HardState) -> io::Result<()> {
        let mut guard = self
            .durable
            .lock()
            .map_err(|_| invalid("replica storage poisoned"))?;
        let durable = guard
            .as_ref()
            .ok_or_else(|| invalid("replica storage must be reopened"))?;
        let result = if state.commit != durable.commit {
            Err(invalid("apply retry requires the durable commit index"))
        } else {
            self.apply_through(state)
        };
        if result.is_err() {
            *guard = None;
        }
        result
    }

    pub(super) fn logical_version(&self, native: u64) -> crate::transactions::Result<u64> {
        let (absolute, origin, pending) = *self.origin.lock().unwrap();
        if pending {
            return Ok(0);
        }
        native
            .checked_sub(origin)
            .and_then(|n| absolute.checked_add(n))
            .ok_or(crate::transactions::Error::Invalid(
                "snapshot version mapping",
            ))
    }
    pub(super) fn applied_index(&self) -> crate::transactions::Result<u64> {
        self.logical_version(self.data.current_version()?)
    }
    pub(super) fn installed_snapshot_index(&self) -> u64 {
        self.origin.lock().unwrap().0
    }
    fn prune(&self, base: u64, bank: u64, size: usize) -> io::Result<()> {
        self.delete_range(&log_key(0), &log_key(base + 1))?;
        self.delete_range(&chunk_key(1 - bank, 0), &chunk_key(1 - bank, usize::MAX))?;
        self.delete_range(
            &chunk_key(bank, size.div_ceil(CHUNK)),
            &chunk_key(bank, usize::MAX),
        )
    }
    fn delete_range(&self, start: &[u8], end: &[u8]) -> io::Result<()> {
        let mut after = None;
        loop {
            let page = self
                .journal
                .begin_read()
                .map_err(io::Error::other)?
                .scan_page(
                    start,
                    Some(end),
                    after.as_deref(),
                    crate::transactions::ScanLimits {
                        max_bytes: 2 * 1024 * 1024,
                        ..Default::default()
                    },
                )
                .map_err(io::Error::other)?;
            if !page.rows.is_empty() {
                let mut tx = self.journal.begin().map_err(io::Error::other)?;
                for (key, _) in page.rows {
                    tx.delete(&key).map_err(io::Error::other)?;
                }
                tx.commit().map_err(io::Error::other)?;
            }
            after = page.next_after;
            if after.is_none() {
                return Ok(());
            }
        }
    }

    fn remove_unreachable(&self, state: &HardState, mut high: u64) -> io::Result<()> {
        // The published logical length already excludes these keys; cleanup is restartable.
        let last = state.snapshot.index + state.log.len() as u64;
        while high > last {
            let next = high.saturating_sub(256).max(last);
            let mut tx = self.journal.begin().map_err(io::Error::other)?;
            for idx in next + 1..=high {
                tx.delete(&log_key(idx)).map_err(io::Error::other)?;
            }
            tx.put(STATE, &encode_state(state, next)?)
                .map_err(io::Error::other)?;
            tx.commit().map_err(io::Error::other)?;
            high = next;
        }
        Ok(())
    }
}

fn encode_state(state: &HardState, high: u64) -> io::Result<Vec<u8>> {
    Ok(serde_json::to_vec(
        &json!({"term":state.term, "vote":state.voted_for, "commit":state.commit,
        "last":state.snapshot.index+state.log.len() as u64, "high":high}),
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Files(std::path::PathBuf);
    impl Files {
        fn new() -> Self {
            let mut id = [0; 16];
            getrandom::getrandom(&mut id).unwrap();
            let dir =
                std::env::temp_dir().join(format!("catdb-raft-{:x}", u128::from_be_bytes(id)));
            std::fs::create_dir(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> PathBuf {
            self.0.join("app.ndb")
        }
        fn open(&self, init: bool) -> io::Result<(Storage, HardState)> {
            Storage::open(
                &self.path(),
                "topology",
                "a",
                &["a".into(), "b".into(), "c".into()],
                init,
                ResourceLimits::default(),
            )
        }
    }
    impl Drop for Files {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn write(term: u64, key: &str, value: &str) -> Entry {
        Entry {
            term,
            command: json!({"writes":[[key,value]]}),
        }
    }
    fn get(store: &Storage, key: &[u8]) -> Option<Vec<u8>> {
        store.database().begin_read().unwrap().get(key).unwrap()
    }

    #[test]
    fn durable_vote_and_conflicting_uncommitted_suffix_survive_restart() {
        let files = Files::new();
        let (store, mut hard) = files.open(true).unwrap();
        hard.term = 1;
        hard.voted_for = Some("a".into());
        hard.log = vec![
            write(1, "aa", "01"),
            write(1, "bb", "02"),
            write(1, "cc", "03"),
        ];
        hard.commit = 1;
        store.persist(&hard, 0).unwrap();
        assert_eq!(get(&store, &[0xbb]), None);
        drop(store);
        let (store, recovered) = files.open(false).unwrap();
        assert_eq!(hard, recovered);
        hard.log.truncate(1);
        hard.term = 2;
        hard.voted_for = Some("b".into());
        hard.log.push(write(2, "dd", "04"));
        hard.commit = 2;
        store.persist(&hard, 1).unwrap();
        drop(store);
        let (store, recovered) = files.open(false).unwrap();
        assert_eq!(recovered, hard);
        assert_eq!(get(&store, &[0xaa]), Some(vec![1]));
        assert_eq!(get(&store, &[0xdd]), Some(vec![4]));
        assert_eq!(get(&store, &[0xcc]), None);
        assert!(
            store
                .journal
                .begin_read()
                .unwrap()
                .get(&log_key(3))
                .unwrap()
                .is_none()
        );
        hard.log[0] = write(1, "aa", "ff");
        assert!(store.persist(&hard, 0).is_err());
        drop(store);
        assert_eq!(files.open(false).unwrap().1, recovered);
    }

    #[test]
    fn restart_replays_only_missing_applies_including_noop_with_occupied_key() {
        let files = Files::new();
        let (store, mut hard) = files.open(true).unwrap();
        hard.term = 2;
        hard.log = vec![
            write(1, "63617464622d726166742d6e6f6f70", "abcd"),
            Entry {
                term: 1,
                command: json!({"writes":[["",""],["abcd","11"]]}),
            },
            Entry {
                term: 2,
                command: Value::Null,
            },
            Entry {
                term: 2,
                command: json!({"writes":[["abcd",null]]}),
            },
        ];
        store.persist(&hard, 0).unwrap();
        hard.commit = 4;
        let mut tx = store.journal.begin().unwrap();
        tx.put(STATE, &encode_state(&hard, 4).unwrap()).unwrap();
        tx.commit().unwrap();
        let mut partial = hard.clone();
        partial.commit = 2;
        store.apply_through(&partial).unwrap();
        drop(store);
        let (store, recovered) = files.open(false).unwrap();
        assert_eq!(hard, recovered);
        assert_eq!(store.database().current_version().unwrap(), 4);
        assert_eq!(get(&store, b"catdb-raft-noop"), Some(vec![0xab, 0xcd]));
        assert_eq!(get(&store, b""), Some(vec![]));
        assert_eq!(get(&store, &[0xab, 0xcd]), None);
        store.persist(&hard, hard.log.len()).unwrap();
        assert_eq!(store.database().current_version().unwrap(), 4);
    }

    #[test]
    fn standalone_files_swapped_generations_and_foreign_topologies_are_rejected() {
        let files = Files::new();
        let db = Database::create(&files.path(), [4; 16]).unwrap();
        let mut tx = db.begin().unwrap();
        tx.put(b"existing", b"value").unwrap();
        tx.commit().unwrap();
        drop(db);
        assert!(files.open(true).is_err());
        assert!(!raft_path(&files.path()).exists());
        let pair = Files::new();
        drop(pair.open(true).unwrap());
        assert!(
            Storage::open(
                &pair.path(),
                "different",
                "a",
                &["a".into(), "b".into(), "c".into()],
                true,
                ResourceLimits::default()
            )
            .is_err()
        );
        std::fs::copy(files.path(), pair.path()).unwrap();
        assert!(pair.open(false).is_err());
    }

    #[test]
    fn validates_binary_limits_and_rejects_ambiguous_write_sets() {
        for command in [
            Value::Null,
            json!({"writes":[]}),
            json!({"writes":[["",""],["AA",null]]}),
            json!({"writes":[["ab".repeat(MAX_KEY_BYTES),"cd".repeat(MAX_VALUE)]]}),
        ] {
            assert_eq!(
                command_size(&command).unwrap(),
                serde_json::to_vec(&command).unwrap().len()
            );
        }
        for command in [
            json!({}),
            json!({"writes":[], "term":4}),
            json!({"writes":[["00",false]]}),
            json!({"writes":[["?",null]]}),
            json!({"writes":[["AA","ab"],["aa",null]]}),
            json!({"writes":[["aa".repeat(MAX_KEY_BYTES+1),null]]}),
            json!({"writes":[["ab","00".repeat(MAX_VALUE+1)]]}),
            json!({"writes":[["ab","00".repeat(MAX_VALUE)],["aa","00".repeat(MAX_VALUE)]]}),
        ] {
            assert!(validate_command(&command).is_err());
        }
    }

    #[test]
    fn restart_finishes_snapshot_install_before_applying_the_retained_suffix() {
        let files = Files::new();
        let (store, mut hard) = files.open(true).unwrap();
        hard.term = 1;
        hard.log = vec![write(1, "00", "ff")];
        hard.commit = 1;
        store.persist(&hard, 0).unwrap();
        let mut row = Vec::new();
        for field in [b"k".as_slice(), b"value"] {
            row.extend_from_slice(&(field.len() as u32).to_be_bytes());
            row.extend_from_slice(field);
        }
        hard.snapshot = Snapshot::new(70_000, 1, row);
        hard.log.clear();
        hard.commit = 70_000;
        store.persist(&hard, 0).unwrap();
        assert!(
            store
                .journal
                .begin_read()
                .unwrap()
                .get(&log_key(1))
                .unwrap()
                .is_none()
        );
        hard.log.push(write(1, "ff", "ef"));
        hard.commit += 1;
        // Model a power loss after journal publication and a partial application batch.
        let mut journal = store.journal.begin().unwrap();
        journal.put(ORIGIN, b"[0,0,true]").unwrap();
        journal
            .put(
                &log_key(70001),
                &serde_json::to_vec(&(1, &hard.log[0].command)).unwrap(),
            )
            .unwrap();
        journal
            .put(STATE, &encode_state(&hard, 70001).unwrap())
            .unwrap();
        journal.commit().unwrap();
        let mut partial = store.data.begin().unwrap();
        partial.put(b"k", b"old").unwrap();
        partial.put(b"obsolete", b"x").unwrap();
        partial.commit().unwrap();
        drop(store);
        let (store, recovered) = files.open(false).unwrap();
        assert_eq!(recovered, hard);
        assert_eq!(store.applied_index().unwrap(), 70001);
        assert_eq!(get(&store, b"k").unwrap(), b"value");
        assert_eq!(get(&store, b"obsolete"), None);
        assert_eq!(get(&store, &[255]), Some(vec![0xef]));
    }

    #[test]
    fn missing_half_or_application_ahead_of_commit_fails_closed_on_boot() {
        let files = Files::new();
        let (store, _) = files.open(true).unwrap();
        let mut foreign = store.data.begin().unwrap();
        foreign.put(b"bypassed-consensus", b"broken").unwrap();
        foreign.commit().unwrap();
        drop(store);
        assert!(files.open(false).is_err());
        std::fs::remove_file(files.path()).unwrap();
        assert!(files.open(true).is_err());
        assert!(!files.path().exists());
    }

    #[test]
    fn a_pinned_staging_budget_delays_apply_but_not_durable_votes_or_commits() {
        let files = Files::new();
        let staged_limit = 10 << 20;
        let limits = ResourceLimits {
            max_staged_bytes: staged_limit,
            ..ResourceLimits::default()
        };
        let members = ["a".into(), "b".into(), "c".into()];
        let (store, mut hard) =
            Storage::open(&files.path(), "topology", "a", &members, true, limits).unwrap();
        let mut busy = store.database().begin().unwrap();
        busy.put(b"uncommitted", &vec![9; staged_limit - 1024])
            .unwrap();
        hard.term = 1;
        hard.log = vec![write(1, "01", "ff"), write(1, "02", &"bb".repeat(2048))];
        hard.commit = 2;
        store.persist(&hard, 0).unwrap();
        assert_eq!(store.database().current_version().unwrap(), 1);
        assert_eq!(get(&store, &[2]), None);
        hard.term = 2;
        hard.voted_for = Some("b".into());
        store.persist(&hard, 2).unwrap();
        store.retry_apply(&hard).unwrap();
        let journal_bytes = store
            .journal
            .begin_read()
            .unwrap()
            .get(STATE)
            .unwrap()
            .unwrap();
        let durable: Value = serde_json::from_slice(&journal_bytes).unwrap();
        assert_eq!(
            (durable["commit"].as_u64(), durable["vote"].as_str()),
            (Some(2), Some("b"))
        );
        drop(busy);
        store.retry_apply(&hard).unwrap();
        store.retry_apply(&hard).unwrap();
        assert_eq!(store.database().current_version().unwrap(), 2);
        assert_eq!(get(&store, &[2]), Some(vec![0xbb; 2048]));
        assert_eq!(get(&store, b"uncommitted"), None);
    }
}
