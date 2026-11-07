use std::collections::{BTreeSet, HashMap};
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::sync::Weak;

const BLOCK: usize = 65536;
const PAIR: u64 = (2 * BLOCK) as u64;
type Directory = BTreeMap<Vec<u8>, Arc<Address>>;

struct Address {
    rows: usize,
    slot: u32,
    generation: u64,
    digest: u32,
}
struct Pin {
    rows: Data,
    _pages: Directory,
    _lease: File,
}
#[derive(Default)]
struct StageBytes {
    slots: HashMap<u32, u64>,
    reused: u64,
    invalidated: u64,
    split: u64,
    failed: u64,
}
impl Drop for StageBytes {
    fn drop(&mut self) {
        if std::env::var_os("DEFER_COUNTS").is_some() {
            println!(
                "STAGE_BYTES,{},{},{},{},{}",
                self.reused,
                self.invalidated,
                self.split,
                self.failed,
                self.slots.values().sum::<u64>()
            );
        }
    }
}
struct Dirty {
    stage_bytes: StageBytes,
    deferred: BTreeSet<Vec<u8>>,
    delay_invalidated: bool,
    prepared: Directory,
    prepared_cursor: u32,
    patch: bool,
    structural: BTreeSet<Vec<u8>>,
    patch_counts: PatchCounts,
    group_version: Option<u64>,
    reuse_encoded: bool,
    control: Store,
    pool: File,
    rows: Data,
    pages: Directory,
    dirty: BTreeSet<Vec<u8>>,
    live: HashMap<u32, Weak<Address>>,
    failed: bool,
}
fn tagged(tag: u8, key: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(1 + key.len());
    v.push(tag);
    v.extend_from_slice(key);
    v
}
fn fence(dir: &Directory, key: &[u8]) -> Vec<u8> {
    dir.range::<[u8], _>((Unbounded, Included(key)))
        .next_back()
        .map(|(k, _)| k.clone())
        .unwrap_or_default()
}
impl Dirty {
    fn create(path: &Path, cfg: Config, rows: Data) -> io::Result<Self> {
        let pool = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path.with_extension("pages"))?;
        pool.try_lock().map_err(|e| err(&e.to_string()))?;
        let control = Store::create(path, cfg, Data::new(), BUFFER_BUDGET, &mut noop)?;
        let mut s = Self {
            stage_bytes: StageBytes::default(),
            deferred: BTreeSet::new(),
            delay_invalidated: std::env::var("SCALED_VARIANT").as_deref() == Ok("defer"),
            prepared: Directory::new(),
            prepared_cursor: 0,
            patch: false,
            structural: BTreeSet::new(),
            patch_counts: PatchCounts::default(),
            reuse_encoded: false,
            group_version: None,
            control,
            pool,
            rows,
            pages: Directory::new(),
            dirty: [vec![]].into(),
            live: HashMap::new(),
            failed: false,
        };
        s.switch(&mut noop)?;
        Ok(s)
    }
    fn pin(&self) -> io::Result<Pin> {
        Ok(Pin {
            rows: self.rows.clone(),
            _pages: self.pages.clone(),
            _lease: self.pool.try_clone()?,
        })
    }
    fn append(&mut self, w: &WriteSet, hook: Hook<'_>) -> io::Result<()> {
        self.append_log(w, hook)?;
        apply(&mut self.rows, w.clone());
        Ok(())
    }
    fn append_log(&mut self, w: &WriteSet, hook: Hook<'_>) -> io::Result<()> {
        self.append_changes(w, None, hook)
    }
    fn logical_seq(&self) -> u64 {
        self.group_version.unwrap_or(self.control.seq)
    }
    fn append_changes(
        &mut self,
        w: &WriteSet,
        group: Option<u64>,
        hook: Hook<'_>,
    ) -> io::Result<()> {
        if self.failed {
            return Err(err("reopen required"));
        }
        let mut overlay = WriteSet::new();
        for (k, v) in w {
            if 140 + 9 + k.len() + v.as_ref().map_or(0, Vec::len) > BLOCK {
                return Err(err("row exceeds page"));
            }
            overlay.insert(
                tagged(1, k),
                Some(match v {
                    Some(data) => tagged(1, data),
                    None => vec![0],
                }),
            );
        }
        if let Some(seq) = group {
            overlay.insert(vec![2], Some(seq.to_le_bytes().to_vec()));
        }
        self.control.append(&overlay, hook)?;
        self.group_version = group;
        for k in w.keys() {
            let lo = fence(&self.pages, k);
            if self.patch && w[k].is_none() {
                self.structural.insert(lo.clone());
            }
            if !self.prepared.is_empty() {
                if let Some(previous) = self.prepared.remove(&lo) {
                    self.prepared_cursor = self.prepared_cursor.min(previous.slot);
                    self.stage_bytes.invalidated +=
                        self.stage_bytes.slots.remove(&previous.slot).unwrap();
                    if self.delay_invalidated && self.deferred.len() < 96 {
                        self.deferred.insert(lo.clone());
                    }
                }
            }
            self.dirty.insert(lo);
        }

        Ok(())
    }
    fn switch(&mut self, hook: Hook<'_>) -> io::Result<()> {
        let rows = std::mem::take(&mut self.rows);
        let result = self.switch_from(hook, |lower, upper, emit| {
            for (k, v) in
                rows.range::<[u8], _>((Included(lower), upper.map_or(Unbounded, Excluded)))
            {
                emit(k, v)?;
            }
            Ok(())
        });
        self.rows = rows;
        result
    }
    fn switch_from(
        &mut self,
        hook: Hook<'_>,
        mut visit: impl FnMut(
            &[u8],
            Option<&[u8]>,
            &mut dyn FnMut(&[u8], &[u8]) -> io::Result<()>,
        ) -> io::Result<()>,
    ) -> io::Result<()> {
        self.switch_chunks(hook, |lower, upper, emit| {
            let _phase = Phase::enter(2);
            let mut entries = WriteSet::new();
            let mut bytes = 140;
            let mut start = lower.to_vec();
            visit(lower, upper, &mut |k, v| {
                let size = 9 + k.len() + v.len();
                if 140 + size > BLOCK {
                    return Err(err("row exceeds page"));
                }
                if bytes + size > BLOCK || entries.len() == MAX_OPS {
                    emit(std::mem::take(&mut start), &entries)?;
                    start = k.to_vec();
                    entries.clear();
                    bytes = 140;
                }
                entries.insert(k.to_vec(), Some(v.to_vec()));
                bytes += size;
                Ok(())
            })?;
            if !entries.is_empty() {
                emit(start, &entries)?;
            }
            Ok(())
        })
    }
    fn switch_chunks(
        &mut self,
        hook: Hook<'_>,
        mut chunks: impl FnMut(
            &[u8],
            Option<&[u8]>,
            &mut dyn FnMut(Vec<u8>, &WriteSet) -> io::Result<()>,
        ) -> io::Result<()>,
    ) -> io::Result<()> {
        self.switch_sources(hook, |lo, hi, emit| {
            chunks(lo, hi, &mut |start, w| emit(start, PageSource::Tree(w)))
        })
    }
    fn switch_sources(
        &mut self,
        hook: Hook<'_>,
        mut chunks: impl FnMut(
            &[u8],
            Option<&[u8]>,
            &mut dyn FnMut(Vec<u8>, PageSource<'_>) -> io::Result<()>,
        ) -> io::Result<()>,
    ) -> io::Result<()> {
        if self.failed || self.control.poisoned {
            return Err(err("reopen required"));
        }
        self.failed = true;
        let generation = self
            .control
            .root
            .epoch
            .checked_add(1)
            .ok_or_else(|| err("generation exhausted"))?;
        let mut next = self.pages.clone();
        let mut cursor = 0u32;
        let mut scratch = self.patch.then(LinearPage::new);
        self.patch_counts = PatchCounts::default();
        for lower in &self.dirty {
            let upper = self
                .pages
                .range::<[u8], _>((Excluded(lower.as_slice()), Unbounded))
                .next()
                .map(|(k, _)| k.as_slice());
            next.remove(lower);
            if let Some(page) = self.prepared.get(lower) {
                if page.generation != generation {
                    return Err(err("prepared generation mismatch"));
                }
                next.insert(lower.clone(), page.clone());
                continue;
            }
            if let (Some(page), Some(addr)) = (scratch.as_mut(), self.pages.get(lower)) {
                page.combine = true;
                if !self.structural.contains(lower)
                    && patch_copy(
                        &mut self.pool,
                        addr,
                        self.control.root.config,
                        &self.control.rows,
                        lower,
                        upper,
                        page,
                        &mut self.patch_counts,
                    )?
                {
                    let _phase = Phase::enter(2);
                    hook("patch.validated")?;
                    let new = save(
                        &mut self.pool,
                        &mut self.control.costs,
                        &mut self.live,
                        &mut cursor,
                        &self.control.root,
                        generation,
                        PageSource::Linear(page),
                        self.reuse_encoded,
                        hook,
                    )?;
                    next.insert(lower.clone(), new);
                    self.patch_counts.used += 1;
                    hook("patch.used")?;
                    continue;
                }
            }
            self.patch_counts.fallback += 1;
            if self.patch {
                hook("patch.fallback")?;
            }
            chunks(lower, upper, &mut |start, entries| {
                let _phase = Phase::enter(2);
                let page = save(
                    &mut self.pool,
                    &mut self.control.costs,
                    &mut self.live,
                    &mut cursor,
                    &self.control.root,
                    generation,
                    entries,
                    self.reuse_encoded,
                    hook,
                )?;
                next.insert(start, page);
                Ok(())
            })?;
        }
        if !self.dirty.is_empty() {
            sync(&self.pool, &mut self.control.costs, "pages", hook)?;
        }
        let mut manifest = Data::new();
        for (k, p) in &next {
            let mut b = p.slot.to_le_bytes().to_vec();
            b.extend(p.generation.to_le_bytes());
            b.extend(p.digest.to_le_bytes());
            manifest.insert(tagged(0, k), b.into());
        }
        if let Some(seq) = self.group_version {
            manifest.insert(vec![2], seq.to_le_bytes().to_vec().into());
        }
        self.control.rows = manifest;
        self.control.switch(hook)?;
        self.pages = next;
        hook("reclaim.before")?;
        self.live.retain(|_, w| w.strong_count() > 0);
        hook("reclaim.after")?;
        self.stage_bytes.reused += self.stage_bytes.slots.values().sum::<u64>();
        self.stage_bytes.slots.clear();
        self.deferred.clear();
        self.prepared.clear();
        self.prepared_cursor = 0;
        self.structural.clear();
        self.dirty.clear();
        self.failed = false;
        Ok(())
    }
    fn open(path: &Path, cfg: Config, hook: Hook<'_>) -> io::Result<Self> {
        let mut rows = Data::new();
        let mut disk = Self::open_into(path, cfg, hook, |_, w| {
            apply(&mut rows, w);
            Ok(())
        })?;
        disk.rows = rows;
        Ok(disk)
    }
    fn open_into(
        path: &Path,
        cfg: Config,
        hook: Hook<'_>,
        mut accept: impl FnMut(u64, WriteSet) -> io::Result<()>,
    ) -> io::Result<Self> {
        let mut pool = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.with_extension("pages"))?;
        pool.try_lock().map_err(|e| err(&e.to_string()))?;
        let mut control = Store::open(path, cfg, BUFFER_BUDGET, hook)?;
        let group_version = control
            .rows
            .get([2u8].as_slice())
            .map(|v| -> io::Result<u64> {
                Ok(u64::from_le_bytes(
                    v.as_ref()
                        .try_into()
                        .map_err(|_| err("group sequence length"))?,
                ))
            })
            .transpose()?;
        let generation = group_version.unwrap_or(control.seq);
        if generation < control.seq {
            return Err(err("group sequence precedes physical frame"));
        }
        let mut pages = Directory::new();
        let mut live = HashMap::new();
        let mut dirty = BTreeSet::new();
        let length = pool.metadata()?.len();
        let mut last = None;
        for (encoded, b) in &control.rows {
            let (&tag, k) = encoded
                .split_first()
                .ok_or_else(|| err("empty tagged key"))?;
            if tag == 2 && k.is_empty() {
                continue;
            }
            if tag == 1 {
                dirty.insert(fence(&pages, k));
                let value = match b.split_first() {
                    Some((0, [])) => None,
                    Some((1, v)) => Some(v.to_vec()),
                    _ => return Err(err("overlay")),
                };
                accept(generation, [(k.to_vec(), value)].into())?;
                continue;
            }
            if tag != 0
                || b.len() != 16
                || last.as_ref().is_some_and(|x: &Vec<u8>| x.as_slice() >= k)
            {
                return Err(err("manifest order"));
            }
            let mut page = Address {
                rows: 0,
                slot: u32::from_le_bytes(b[..4].try_into().unwrap()),
                generation: u64::from_le_bytes(b[4..12].try_into().unwrap()),
                digest: u32::from_le_bytes(b[12..].try_into().unwrap()),
            };
            if page.generation > control.root.epoch
                || (u64::from(page.slot) + 1) * PAIR > length
                || live.contains_key(&page.slot)
            {
                return Err(err("manifest address"));
            }
            let mut copies = Vec::new();
            for lane in 0..2 {
                let r = read_record(
                    &mut pool,
                    &mut control.costs,
                    u64::from(page.slot) * PAIR + lane * BLOCK as u64,
                )?;
                copies.push(r.filter(|r| {
                    r.kind == 1
                        && r.index == page.slot
                        && r.epoch == page.generation
                        && r.config == cfg
                        && r.payload.len() + HEAD <= BLOCK
                        && r.checksum() == page.digest
                }));
            }
            let good = copies
                .iter()
                .flatten()
                .next()
                .ok_or_else(|| err("both pages missing"))?;
            if copies.iter().flatten().any(|p| p != good) {
                return Err(err("pages disagree"));
            }
            let w = good.writes()?;
            if w.is_empty() || w.first_key_value().unwrap().0.as_slice() < k {
                return Err(err("page key bounds"));
            }
            last = w.last_key_value().map(|(key, _)| key.clone());
            page.rows = w.len();
            let page = Arc::new(page);
            live.insert(page.slot, Arc::downgrade(&page));
            accept(generation, w)?;
            for (lane, r) in copies.iter().enumerate() {
                if r.is_none() {
                    write_record(
                        &mut pool,
                        &mut control.costs,
                        u64::from(page.slot) * PAIR + lane as u64 * BLOCK as u64,
                        good,
                        "repair-page",
                        "repair-page",
                        hook,
                    )?;
                }
            }
            pages.insert(k.to_vec(), page);
        }
        sync(&pool, &mut control.costs, "recovery-pages", hook)?;
        Ok(Self {
            group_version,
            stage_bytes: StageBytes::default(),
            deferred: BTreeSet::new(),
            delay_invalidated: std::env::var("SCALED_VARIANT").as_deref() == Ok("defer"),
            prepared: Directory::new(),
            prepared_cursor: 0,
            patch: false,
            structural: BTreeSet::new(),
            patch_counts: PatchCounts::default(),
            reuse_encoded: false,
            control,
            pool,
            rows: Data::new(),
            pages,
            live,
            dirty,
            failed: false,
        })
    }
}
fn save(
    pool: &mut File,
    costs: &mut Costs,
    live: &mut HashMap<u32, Weak<Address>>,
    cursor: &mut u32,
    root: &Record,
    generation: u64,
    source: PageSource<'_>,
    reuse_encoded: bool,
    hook: Hook<'_>,
) -> io::Result<Arc<Address>> {
    while live.get(cursor).is_some_and(|p| p.strong_count() > 0) {
        *cursor = cursor.checked_add(1).ok_or_else(|| err("slot exhausted"))?;
    }
    let slot = *cursor;
    *cursor = cursor.checked_add(1).ok_or_else(|| err("slot exhausted"))?;
    let end = (u64::from(slot) + 1) * PAIR;
    if pool.metadata()?.len() < end {
        pool.set_len(end)?;
    }
    let mut r = Record {
        kind: 1,
        index: slot,
        epoch: generation,
        seq: generation,
        payload: vec![],
        ..root.clone()
    };
    let (count, digest) = match source {
        PageSource::Linear(page) => {
            let count = page.count;
            let encoded = page.seal(&r)?;
            (count, write_pair(pool, costs, slot, encoded, hook)?)
        }
        PageSource::Tree(w) => {
            r.payload = encode_frame(generation, w);
            if r.payload.len() + HEAD > BLOCK {
                return Err(err("block overflow"));
            }
            let digest = if reuse_encoded {
                let encoded = r.encode();
                write_pair(pool, costs, slot, &encoded, hook)?
            } else {
                for lane in 0..2 {
                    write_record(
                        pool,
                        costs,
                        u64::from(slot) * PAIR + lane * BLOCK as u64,
                        &r,
                        "page",
                        "page",
                        hook,
                    )?;
                }
                r.checksum()
            };
            (w.len(), digest)
        }
    };
    let address = Arc::new(Address {
        rows: count,
        slot,
        generation,
        digest,
    });
    live.insert(slot, Arc::downgrade(&address));
    Ok(address)
}

fn write_pair(
    pool: &mut File,
    costs: &mut Costs,
    slot: u32,
    encoded: &[u8],
    hook: Hook<'_>,
) -> io::Result<u32> {
    let checksum = u32::from_le_bytes(encoded[80..84].try_into().unwrap());
    for lane in 0..2 {
        write_encoded_record(
            pool,
            costs,
            u64::from(slot) * PAIR + lane * BLOCK as u64,
            encoded,
            "page",
            "page",
            hook,
        )?;
    }
    Ok(checksum)
}
