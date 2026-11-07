#![allow(dead_code)]
use super::{
    FRAME, MAX_KEY_BYTES, MAX_TRANSACTION_BYTES, MAX_TRANSACTION_WRITES, MAX_VALUE_BYTES,
    PREFIX_BYTES, SEAL, TRAILER_BYTES, WriteSet, decode_frame, encode_frame,
};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

include!("phase.rs");
const PAGE: usize = 1024 * 1024;
const HEAD: usize = 96;
const PAYLOAD: usize = PAGE - HEAD;
const MAX_OPS: usize = 4096;
const BUFFER_BUDGET: usize = 8 * PAGE;
type Data = BTreeMap<Vec<u8>, Arc<[u8]>>;
type Hook<'a> = &'a mut dyn FnMut(&str) -> io::Result<()>;
fn err(s: &str) -> io::Error {
    io::Error::other(s.to_owned())
}
fn noop(_: &str) -> io::Result<()> {
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Config {
    checkpoints: u32,
    logs: u32,
}
impl Config {
    fn length(self) -> io::Result<u64> {
        if self.checkpoints == 0 || self.checkpoints > 4096 || self.logs == 0 || self.logs > 4096 {
            return Err(err("configuration limit"));
        }
        Ok((4 + 4 * u64::from(self.checkpoints + self.logs)) * PAGE as u64)
    }
    fn at(self, bank: u32, checkpoint: bool, index: u32, lane: u32) -> u64 {
        let slot = if checkpoint {
            index
        } else {
            self.checkpoints + index
        };
        (4 + u64::from(bank) * u64::from(self.checkpoints + self.logs) * 2
            + u64::from(slot) * 2
            + u64::from(lane))
            * PAGE as u64
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Record {
    kind: u32,
    bank: u32,
    epoch: u64,
    seq: u64,
    index: u32,
    previous: u32,
    config: Config,
    chunks: u32,
    records: u64,
    chain: u32,
    payload: Vec<u8>,
}
impl Record {
    fn encode(&self) -> Vec<u8> {
        let mut b = vec![0; HEAD + self.payload.len()];
        b[..8].copy_from_slice(b"CATBIG01");
        for (at, value) in [
            (8, self.kind),
            (12, self.bank),
            (32, self.index),
            (36, self.payload.len() as u32),
            (40, self.previous),
            (44, self.config.checkpoints),
            (48, self.config.logs),
            (52, self.chunks),
            (64, self.chain),
            (76, !(self.payload.len() as u32)),
        ] {
            b[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (at, value) in [(16, self.epoch), (24, self.seq), (56, self.records)] {
            b[at..at + 8].copy_from_slice(&value.to_le_bytes());
        }
        // A CRC over a complete inner CRC frame can have a constant residue.
        let digest = if self.kind == 2 {
            crate::crc32(&[])
        } else {
            crate::crc32(&self.payload[..self.payload.len() - TRAILER_BYTES])
        };
        b[72..76].copy_from_slice(&digest.to_le_bytes());
        let checksum = crate::crc32(&b[..80]);
        b[80..84].copy_from_slice(&checksum.to_le_bytes());
        b[84..88].copy_from_slice(&(!checksum).to_le_bytes());
        b[88..96].copy_from_slice(b"BIGSEAL1");
        b[HEAD..].copy_from_slice(&self.payload);
        b
    }
    fn checksum(&self) -> u32 {
        u32::from_le_bytes(self.encode()[80..84].try_into().unwrap())
    }
    fn writes(&self) -> io::Result<WriteSet> {
        if self.kind == 2 {
            return Ok(WriteSet::new());
        }
        let b = &self.payload;
        if b.len() < PREFIX_BYTES + 12 + TRAILER_BYTES {
            return Err(err("short inner frame"));
        }
        let n = u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
        if &b[..8] != FRAME
            || n > MAX_TRANSACTION_BYTES
            || n + PREFIX_BYTES + TRAILER_BYTES != b.len()
            || u32::from_le_bytes(b[12..16].try_into().unwrap()) != !(n as u32)
            || u32::from_le_bytes(b[16..20].try_into().unwrap()) != crate::crc32(&b[..16])
        {
            return Err(err("inner prefix"));
        }
        let body = &b[PREFIX_BYTES..PREFIX_BYTES + n];
        let tail = &b[PREFIX_BYTES + n..];
        if body.len() < 12 || u32::from_le_bytes(body[8..12].try_into().unwrap()) as usize > MAX_OPS
        {
            return Err(err("inner operation budget"));
        }
        if &tail[4..] != SEAL
            || u32::from_le_bytes(tail[..4].try_into().unwrap()) != crate::crc32(body)
        {
            return Err(err("inner checksum"));
        }
        let (seq, writes) = decode_frame(body).map_err(|_| err("inner decode"))?;
        if seq != self.seq
            || writes.len() > MAX_OPS
            || (self.kind == 1 && writes.values().any(Option::is_none))
        {
            return Err(err("inner identity"));
        }
        Ok(writes)
    }
}

#[derive(Default, Debug)]
struct Costs {
    writes: BTreeMap<&'static str, u64>,
    syncs: BTreeMap<&'static str, u64>,
    read_bytes: u64,
}
fn write_record(
    file: &mut File,
    cost: &mut Costs,
    offset: u64,
    r: &Record,
    category: &'static str,
    label: &str,
    hook: Hook<'_>,
) -> io::Result<()> {
    let b = r.encode();
    write_encoded_record(file, cost, offset, &b, category, label, hook)
}
fn write_encoded_record(
    file: &mut File,
    cost: &mut Costs,
    offset: u64,
    b: &[u8],
    category: &'static str,
    label: &str,
    hook: Hook<'_>,
) -> io::Result<()> {
    let _phase = Phase::enter(3);
    file.seek(SeekFrom::Start(offset))?;
    let mid = b.len() / 2;
    file.write_all(&b[..mid])?;
    *cost.writes.entry(category).or_default() += mid as u64;
    hook(&format!("{label}.half"))?;
    file.write_all(&b[mid..])?;
    *cost.writes.entry(category).or_default() += (b.len() - mid) as u64;
    hook(&format!("{label}.written"))
}
fn sync(file: &File, cost: &mut Costs, category: &'static str, hook: Hook<'_>) -> io::Result<()> {
    hook(&format!("{category}.before-sync"))?;
    *cost.syncs.entry(category).or_default() += 1;
    {
        let _phase = Phase::enter(4);
        file.sync_data()?;
    }
    hook(&format!("{category}.synced"))
}
fn read_record(file: &mut File, cost: &mut Costs, offset: u64) -> io::Result<Option<Record>> {
    let mut b = [0; HEAD];
    file.seek(SeekFrom::Start(offset))?;
    file.read_exact(&mut b)?;
    cost.read_bytes += HEAD as u64;
    let u = |at| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let q = |at| u64::from_le_bytes(b[at..at + 8].try_into().unwrap());
    let n = u(36) as usize;
    if &b[..8] != b"CATBIG01"
        || &b[88..] != b"BIGSEAL1"
        || ![1, 2, 3].contains(&u(8))
        || u(12) > 1
        || q(16) == 0
        || n > PAYLOAD
        || u(76) != !(n as u32)
        || u(80) != crate::crc32(&b[..80])
        || u(84) != !u(80)
        || b[68..72] != [0; 4]
    {
        return Ok(None);
    }
    let mut payload = vec![0; n];
    file.read_exact(&mut payload)?;
    cost.read_bytes += n as u64;
    let r = Record {
        kind: u(8),
        bank: u(12),
        epoch: q(16),
        seq: q(24),
        index: u(32),
        previous: u(40),
        config: Config {
            checkpoints: u(44),
            logs: u(48),
        },
        chunks: u(52),
        records: q(56),
        chain: u(64),
        payload,
    };
    if r.kind == 2 {
        if n != 0
            || r.index != 0
            || r.previous != 0
            || r.chunks > r.config.checkpoints
            || u(72) != crate::crc32(&[])
        {
            return Ok(None);
        }
    } else {
        if n < PREFIX_BYTES + 12 + TRAILER_BYTES
            || u(72) != crate::crc32(&r.payload[..n - TRAILER_BYTES])
            || r.writes().is_err()
        {
            return Ok(None);
        }
    }
    Ok(Some(r))
}
fn frame_size(w: &WriteSet) -> io::Result<usize> {
    if w.len() > MAX_OPS.min(MAX_TRANSACTION_WRITES) {
        return Err(err("operation limit"));
    }
    let mut n = PREFIX_BYTES + 12 + TRAILER_BYTES;
    for (k, v) in w {
        if k.len() > MAX_KEY_BYTES || v.as_ref().is_some_and(|v| v.len() > MAX_VALUE_BYTES) {
            return Err(err("key/value limit"));
        }
        n = n
            .checked_add(9 + k.len() + v.as_ref().map_or(0, Vec::len))
            .ok_or_else(|| err("size overflow"))?;
    }
    if n > PAYLOAD {
        return Err(err("prototype frame capacity"));
    }
    Ok(n)
}
fn apply(rows: &mut Data, writes: WriteSet) {
    for (k, v) in writes {
        if let Some(v) = v {
            rows.insert(k, Arc::from(v));
        } else {
            rows.remove(&k);
        }
    }
}
fn root_at(epoch: u64, lane: u32) -> u64 {
    ((epoch % 2) * 2 + u64::from(lane)) * PAGE as u64
}
fn needed_chunks(rows: &Data) -> io::Result<u32> {
    let mut count = 0;
    let mut n = PREFIX_BYTES + 12 + TRAILER_BYTES;
    let mut ops = 0;
    for (k, v) in rows {
        if k.len() > MAX_KEY_BYTES || v.len() > MAX_VALUE_BYTES {
            return Err(err("key/value limit"));
        }
        let size = 9 + k.len() + v.len();
        if size + PREFIX_BYTES + 12 + TRAILER_BYTES > PAYLOAD {
            return Err(err("checkpoint record limit"));
        }
        if ops == MAX_OPS || n + size > PAYLOAD {
            count += 1;
            n = PREFIX_BYTES + 12 + TRAILER_BYTES;
            ops = 0;
        }
        ops += 1;
        n += size;
    }
    Ok(count + u32::from(ops > 0))
}
fn emit_chunk(
    file: &mut File,
    cost: &mut Costs,
    root: &Record,
    index: u32,
    previous: u32,
    w: &WriteSet,
    hook: Hook<'_>,
) -> io::Result<u32> {
    let payload = encode_frame(root.seq, w);
    assert_eq!(payload.len(), frame_size(w)?);
    let r = Record {
        kind: 1,
        index,
        previous,
        payload,
        ..root.clone()
    };
    for lane in 0..2 {
        write_record(
            file,
            cost,
            root.config.at(root.bank, true, index, lane),
            &r,
            "checkpoint",
            &format!("chunk-{index}-{lane}"),
            hook,
        )?;
    }
    Ok(r.checksum())
}
fn save_checkpoint(
    file: &mut File,
    cost: &mut Costs,
    rows: &Data,
    mut root: Record,
    hook: Hook<'_>,
) -> io::Result<Record> {
    let mut pending = WriteSet::new();
    let mut n = PREFIX_BYTES + 12 + TRAILER_BYTES;
    let mut index = 0;
    let mut chain = 0;
    for (k, v) in rows {
        let size = 9 + k.len() + v.len();
        if pending.len() == MAX_OPS || n + size > PAYLOAD {
            chain = emit_chunk(file, cost, &root, index, chain, &pending, hook)?;
            index += 1;
            pending.clear();
            n = PREFIX_BYTES + 12 + TRAILER_BYTES;
        }
        pending.insert(k.clone(), Some(v.to_vec()));
        n += size;
    }
    if !pending.is_empty() {
        chain = emit_chunk(file, cost, &root, index, chain, &pending, hook)?;
        index += 1;
    }
    assert_eq!(index, root.chunks);
    root.chain = chain;
    sync(file, cost, "checkpoint", hook)?;
    for lane in 0..2 {
        write_record(
            file,
            cost,
            root_at(root.epoch, lane),
            &root,
            "selector",
            &format!("selector-{lane}"),
            hook,
        )?;
    }
    sync(file, cost, "selector", hook)?;
    hook("switch.published")?;
    Ok(root)
}

struct Store {
    file: File,
    root: Record,
    rows: Data,
    used: u32,
    seq: u64,
    chain: u32,
    poisoned: bool,
    costs: Costs,
}
impl Store {
    fn create(
        path: &Path,
        config: Config,
        rows: Data,
        budget: usize,
        hook: Hook<'_>,
    ) -> io::Result<Self> {
        let length = config.length()?;
        let chunks = needed_chunks(&rows)?;
        if budget < BUFFER_BUDGET || chunks > config.checkpoints {
            return Err(err("buffer or checkpoint capacity"));
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)?;
        file.try_lock().map_err(|e| err(&e.to_string()))?;
        file.set_len(length)?;
        let root = Record {
            kind: 2,
            bank: 0,
            epoch: 1,
            seq: 0,
            index: 0,
            previous: 0,
            config,
            chunks,
            records: rows.len() as u64,
            chain: 0,
            payload: vec![],
        };
        let mut costs = Costs::default();
        let root = save_checkpoint(&mut file, &mut costs, &rows, root, hook)?;
        File::open(path.parent().unwrap())?.sync_all()?;
        *costs.syncs.entry("directory").or_default() += 1;
        Ok(Self {
            file,
            chain: root.chain,
            root,
            rows,
            used: 0,
            seq: 0,
            poisoned: false,
            costs,
        })
    }
    fn open(path: &Path, config: Config, budget: usize, hook: Hook<'_>) -> io::Result<Self> {
        if budget < BUFFER_BUDGET {
            return Err(err("buffer capacity"));
        }
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        file.try_lock().map_err(|e| err(&e.to_string()))?;
        if file.metadata()?.len() != config.length()? {
            return Err(err("file size/config mismatch"));
        }
        let mut costs = Costs::default();
        let mut roots = Vec::with_capacity(4);
        for i in 0..4 {
            if let Some(r) = read_record(&mut file, &mut costs, i * PAGE as u64)? {
                if r.kind == 2 && r.config == config && i / 2 == r.epoch % 2 {
                    roots.push(r);
                }
            }
        }
        let root = roots
            .iter()
            .max_by_key(|r| r.epoch)
            .cloned()
            .ok_or_else(|| err("no selector"))?;
        if roots.iter().any(|r| r.epoch == root.epoch && r != &root) {
            return Err(err("selector disagreement"));
        }
        let (rows, used, seq, chain, repair) =
            scan(&mut file, &mut costs, &root, true, false, hook)?;
        if repair {
            let (_, again, again_seq, again_chain, _) =
                scan(&mut file, &mut costs, &root, false, true, hook)?;
            if (again, again_seq, again_chain) != (used, seq, chain) {
                return Err(err("repair identity changed"));
            }
            sync(&file, &mut costs, "repair-data", hook)?;
        }
        let mut repaired = false;
        for lane in 0..2 {
            if read_record(&mut file, &mut costs, root_at(root.epoch, lane))?.as_ref()
                != Some(&root)
            {
                write_record(
                    &mut file,
                    &mut costs,
                    root_at(root.epoch, lane),
                    &root,
                    "repair-selector",
                    &format!("repair-selector-{lane}"),
                    hook,
                )?;
                repaired = true;
            }
        }
        if repaired {
            sync(&file, &mut costs, "repair-selector", hook)?;
        }
        // A previous repair may be visible in cache despite an interrupted sync.
        sync(&file, &mut costs, "recovery", hook)?;
        hook("open.ready")?;
        Ok(Self {
            file,
            root,
            rows,
            used,
            seq,
            chain,
            poisoned: false,
            costs,
        })
    }
    fn snapshot(&self) -> Data {
        self.rows.clone()
    }
    fn append(&mut self, w: &WriteSet, hook: Hook<'_>) -> io::Result<()> {
        if self.poisoned {
            return Err(err("poisoned"));
        }
        if w.is_empty() {
            return Ok(());
        }
        if self.used == self.root.config.logs {
            return Err(err("log full"));
        }
        frame_size(w)?;
        let seq = self
            .seq
            .checked_add(1)
            .ok_or_else(|| err("sequence exhausted"))?;
        let r = Record {
            kind: 3,
            seq,
            index: self.used,
            previous: self.chain,
            payload: encode_frame(seq, w),
            ..self.root.clone()
        };
        let pending = w.clone();
        self.poisoned = true;
        hook("append.prepared")?;
        for lane in 0..2 {
            write_record(
                &mut self.file,
                &mut self.costs,
                self.root.config.at(r.bank, false, self.used, lane),
                &r,
                "append",
                &format!("append-{lane}"),
                hook,
            )?;
        }
        sync(&self.file, &mut self.costs, "append", hook)?;
        apply(&mut self.rows, pending);
        self.seq = seq;
        self.chain = r.checksum();
        self.used += 1;
        hook("append.published")?;
        self.poisoned = false;
        Ok(())
    }
    fn switch(&mut self, hook: Hook<'_>) -> io::Result<()> {
        if self.poisoned {
            return Err(err("poisoned"));
        }
        let epoch = self
            .root
            .epoch
            .checked_add(1)
            .ok_or_else(|| err("epoch exhausted"))?;
        let chunks = needed_chunks(&self.rows)?;
        if chunks > self.root.config.checkpoints {
            return Err(err("checkpoint capacity"));
        }
        let root = Record {
            bank: 1 - self.root.bank,
            epoch,
            seq: self.seq,
            chunks,
            records: self.rows.len() as u64,
            chain: 0,
            ..self.root.clone()
        };
        self.poisoned = true;
        hook("switch.prepared")?;
        let root = save_checkpoint(&mut self.file, &mut self.costs, &self.rows, root, hook)?;
        self.chain = root.chain;
        self.root = root;
        self.used = 0;
        self.poisoned = false;
        Ok(())
    }
}

fn pair(
    file: &mut File,
    cost: &mut Costs,
    root: &Record,
    checkpoint: bool,
    index: u32,
    previous: u32,
) -> io::Result<(Option<Record>, [bool; 2])> {
    let a = read_record(file, cost, root.config.at(root.bank, checkpoint, index, 0))?;
    let b = read_record(file, cost, root.config.at(root.bank, checkpoint, index, 1))?;
    let expected_seq = if checkpoint {
        Some(root.seq)
    } else {
        root.seq.checked_add(u64::from(index) + 1)
    };
    let matches = |r: &&Record| {
        r.config == root.config
            && r.epoch == root.epoch
            && r.bank == root.bank
            && r.index == index
            && r.kind == if checkpoint { 1 } else { 3 }
            && Some(r.seq) == expected_seq
            && r.previous == previous
    };
    let x = a.as_ref().filter(matches);
    let y = b.as_ref().filter(matches);
    // A valid current/future envelope in the wrong position is not an unused tail.
    if x.is_none()
        && y.is_none()
        && [a.as_ref(), b.as_ref()]
            .into_iter()
            .flatten()
            .any(|r| r.epoch >= root.epoch)
    {
        return Err(err("no peer with expected identity"));
    }
    if x.is_some() && y.is_some() && x != y {
        return Err(err("copies disagree"));
    }
    let chosen = x.or(y).cloned();
    let repairs = [a.as_ref() != chosen.as_ref(), b.as_ref() != chosen.as_ref()];
    Ok((chosen, repairs))
}
fn scan(
    file: &mut File,
    cost: &mut Costs,
    root: &Record,
    collect: bool,
    repair: bool,
    hook: Hook<'_>,
) -> io::Result<(Data, u32, u64, u32, bool)> {
    let mut rows = Data::new();
    let mut previous = 0;
    let mut count = 0;
    let mut last: Option<Vec<u8>> = None;
    let mut needs = false;
    for index in 0..root.chunks {
        let (r, fix) = pair(file, cost, root, true, index, previous)?;
        let r = r.ok_or_else(|| err("selected chunk missing"))?;
        if r.kind != 1
            || r.seq != root.seq
            || r.index != index
            || r.config != root.config
            || r.previous != previous
        {
            return Err(err("checkpoint chunk identity/chain"));
        }
        let w = r.writes()?;
        if w.is_empty() {
            return Err(err("empty checkpoint chunk"));
        }
        if last
            .as_ref()
            .is_some_and(|k| w.first_key_value().unwrap().0 <= k)
        {
            return Err(err("checkpoint key order"));
        }
        last = w.last_key_value().map(|(k, _)| k.clone());
        count += w.len() as u64;
        previous = r.checksum();
        if collect {
            apply(&mut rows, w);
        }
        for lane in 0..2 {
            if fix[lane as usize] {
                needs = true;
                if repair {
                    write_record(
                        file,
                        cost,
                        root.config.at(root.bank, true, index, lane),
                        &r,
                        "repair-data",
                        &format!("repair-chunk-{index}-{lane}"),
                        hook,
                    )?;
                }
            }
        }
    }
    if count != root.records || previous != root.chain {
        return Err(err("checkpoint manifest"));
    }
    let mut seq = root.seq;
    let mut used = 0;
    let mut gap = false;
    for index in 0..root.config.logs {
        let (r, fix) = pair(file, cost, root, false, index, previous)?;
        let Some(r) = r else {
            gap = true;
            continue;
        };
        if gap
            || r.kind != 3
            || r.index != index
            || r.config != root.config
            || r.previous != previous
            || seq.checked_add(1) != Some(r.seq)
        {
            return Err(err("log order/chain"));
        }
        let w = r.writes()?;
        if w.is_empty() {
            return Err(err("empty log"));
        }
        if collect {
            apply(&mut rows, w);
        }
        seq = r.seq;
        previous = r.checksum();
        used = index + 1;
        for lane in 0..2 {
            if fix[lane as usize] {
                needs = true;
                if repair {
                    write_record(
                        file,
                        cost,
                        root.config.at(root.bank, false, index, lane),
                        &r,
                        "repair-data",
                        &format!("repair-log-{index}-{lane}"),
                        hook,
                    )?;
                }
            }
        }
    }
    Ok((rows, used, seq, previous, needs))
}

#[cfg(test)]
mod checks;
