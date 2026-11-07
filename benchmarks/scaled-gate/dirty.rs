use std::collections::{BTreeSet, HashMap};
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::sync::Weak;

const BLOCK: usize = 65536;
const PAIR: u64 = (2 * BLOCK) as u64;
type Directory = BTreeMap<Vec<u8>, Arc<Address>>;

struct Address {
    slot: u32,
    generation: u64,
    digest: u32,
}
struct Pin {
    rows: Data,
    _pages: Directory,
    _lease: File,
}
struct Dirty {
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
        self.control.append(&overlay, hook)?;
        for k in w.keys() {
            self.dirty.insert(fence(&self.pages, k));
        }
        apply(&mut self.rows, w.clone());
        Ok(())
    }
    fn switch(&mut self, hook: Hook<'_>) -> io::Result<()> {
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
        for lower in &self.dirty {
            let upper = self
                .pages
                .range::<[u8], _>((Excluded(lower.as_slice()), Unbounded))
                .next()
                .map(|(k, _)| Excluded(k.as_slice()))
                .unwrap_or(Unbounded);
            next.remove(lower);
            let mut entries = WriteSet::new();
            let mut bytes = 140;
            let mut start = lower.clone();
            for (k, v) in self
                .rows
                .range::<[u8], _>((Included(lower.as_slice()), upper))
            {
                let size = 9 + k.len() + v.len();
                if 140 + size > BLOCK {
                    return Err(err("row exceeds page"));
                }
                if bytes + size > BLOCK || entries.len() == MAX_OPS {
                    let page = save(
                        &mut self.pool,
                        &mut self.control.costs,
                        &mut self.live,
                        &mut cursor,
                        &self.control.root,
                        generation,
                        &entries,
                        hook,
                    )?;
                    next.insert(start, page);
                    start = k.clone();
                    entries.clear();
                    bytes = 140;
                }
                entries.insert(k.clone(), Some(v.to_vec()));
                bytes += size;
            }
            if !entries.is_empty() {
                let page = save(
                    &mut self.pool,
                    &mut self.control.costs,
                    &mut self.live,
                    &mut cursor,
                    &self.control.root,
                    generation,
                    &entries,
                    hook,
                )?;
                next.insert(start, page);
            }
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
        self.control.rows = manifest;
        self.control.switch(hook)?;
        self.pages = next;
        hook("reclaim.before")?;
        self.live.retain(|_, w| w.strong_count() > 0);
        hook("reclaim.after")?;
        self.dirty.clear();
        self.failed = false;
        Ok(())
    }
    fn open(path: &Path, cfg: Config, hook: Hook<'_>) -> io::Result<Self> {
        let mut pool = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path.with_extension("pages"))?;
        pool.try_lock().map_err(|e| err(&e.to_string()))?;
        let mut control = Store::open(path, cfg, BUFFER_BUDGET, hook)?;
        let mut rows = Data::new();
        let mut pages = Directory::new();
        let mut live = HashMap::new();
        let mut dirty = BTreeSet::new();
        let length = pool.metadata()?.len();
        let mut last = None;
        for (encoded, b) in &control.rows {
            let (&tag, k) = encoded
                .split_first()
                .ok_or_else(|| err("empty tagged key"))?;
            if tag == 1 {
                dirty.insert(fence(&pages, k));
                match b.split_first() {
                    Some((0, [])) => {
                        rows.remove(k);
                    }
                    Some((1, v)) => {
                        rows.insert(k.to_vec(), Arc::from(v));
                    }
                    _ => return Err(err("overlay")),
                }
                continue;
            }
            if tag != 0
                || b.len() != 16
                || last.as_ref().is_some_and(|x: &Vec<u8>| x.as_slice() >= k)
            {
                return Err(err("manifest order"));
            }
            let page = Arc::new(Address {
                slot: u32::from_le_bytes(b[..4].try_into().unwrap()),
                generation: u64::from_le_bytes(b[4..12].try_into().unwrap()),
                digest: u32::from_le_bytes(b[12..].try_into().unwrap()),
            });
            if page.generation > control.root.epoch
                || (u64::from(page.slot) + 1) * PAIR > length
                || live.insert(page.slot, Arc::downgrade(&page)).is_some()
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
            apply(&mut rows, w);
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
            control,
            pool,
            rows,
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
    w: &WriteSet,
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
    let r = Record {
        kind: 1,
        index: slot,
        epoch: generation,
        seq: generation,
        payload: encode_frame(generation, w),
        ..root.clone()
    };
    if r.payload.len() + HEAD > BLOCK {
        return Err(err("block overflow"));
    }
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
    let address = Arc::new(Address {
        slot,
        generation,
        digest: r.checksum(),
    });
    live.insert(slot, Arc::downgrade(&address));
    Ok(address)
}
