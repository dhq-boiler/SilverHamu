use crate::transactions::Error as TxError;
use std::sync::{
    Barrier, Mutex, RwLock,
    atomic::{AtomicU64, AtomicUsize, Ordering},
};

enum Backend {
    Wal(Database),
    Full(Store),
    Pages(Dirty),
}
impl Backend {
    fn checkpoint(&mut self, hook: Hook<'_>) {
        match self {
            Self::Wal(db) => {
                db.checkpoint().unwrap();
            }
            Self::Full(s) => s.switch(hook).unwrap(),
            Self::Pages(s) => s.switch(hook).unwrap(),
        }
    }
    fn rows(&self) -> &Data {
        match self {
            Self::Full(s) => &s.rows,
            Self::Pages(s) => &s.rows,
            _ => unreachable!(),
        }
    }
}

#[derive(Default)]
struct Dependencies {
    points: Vec<Vec<u8>>,
    ranges: Vec<(Vec<u8>, Vec<u8>)>,
}
struct Publication {
    backend: Backend,
    changed: BTreeMap<Vec<u8>, u64>,
}
impl Publication {
    fn changed_after(&self, key: &[u8], snapshot: u64) -> bool {
        self.changed.get(key).is_some_and(|v| *v > snapshot)
    }
    fn point(
        &self,
        snapshot: u64,
        k: &[u8],
        deps: &mut Dependencies,
    ) -> Result<Option<Arc<[u8]>>, ()> {
        if self.changed_after(k, snapshot) {
            return Err(());
        }
        deps.points.push(k.to_vec());
        Ok(self.backend.rows().get(k).cloned())
    }
    fn range(
        &self,
        snapshot: u64,
        start: Vec<u8>,
        end: Vec<u8>,
        deps: &mut Dependencies,
    ) -> Result<Data, ()> {
        if self
            .changed
            .range(start.clone()..end.clone())
            .any(|(_, v)| *v > snapshot)
        {
            return Err(());
        }
        let found = self
            .backend
            .rows()
            .range(start.clone()..end.clone())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        deps.ranges.push((start, end));
        Ok(found)
    }
    fn publish(
        &mut self,
        epoch: &AtomicU64,
        snapshot: u64,
        w: &WriteSet,
        deps: &Dependencies,
        hook: Hook<'_>,
    ) -> io::Result<Option<u64>> {
        if w.keys()
            .chain(deps.points.iter())
            .any(|k| self.changed_after(k, snapshot))
            || deps.ranges.iter().any(|(a, b)| {
                self.changed
                    .range(a.clone()..b.clone())
                    .any(|(_, v)| *v > snapshot)
            })
        {
            return Ok(None);
        }
        let next = epoch
            .load(Ordering::Acquire)
            .checked_add(1)
            .ok_or_else(|| err("generation exhausted"))?;
        match &mut self.backend {
            Backend::Full(s) => s.append(w, hook)?,
            Backend::Pages(s) => s.append(w, hook)?,
            _ => unreachable!(),
        }
        for k in w.keys() {
            self.changed.insert(k.clone(), next);
        }
        epoch.store(next, Ordering::Release);
        Ok(Some(next))
    }
}
struct Sample {
    id: usize,
    lane: usize,
    seq: u64,
    start: u128,
    end: u128,
    retries: usize,
}
struct Four {
    publication: Mutex<Publication>,
    wal: Option<Database>,
    epoch: AtomicU64,
    admission: RwLock<()>,
    remaining: AtomicUsize,
    cap: usize,
    clock: Instant,
    checkpoints: Mutex<Vec<(u128, u128)>>,
}
impl Four {
    fn new(backend: Backend, cap: usize) -> Self {
        let wal = match &backend {
            Backend::Wal(db) => Some(db.clone()),
            _ => None,
        };
        Self {
            publication: Mutex::new(Publication {
                backend,
                changed: BTreeMap::new(),
            }),
            wal,
            epoch: AtomicU64::new(0),
            admission: RwLock::new(()),
            remaining: AtomicUsize::new(cap),
            cap,
            clock: Instant::now(),
            checkpoints: Mutex::new(Vec::new()),
        }
    }
    fn run_one(&self, id: usize, lane: usize, n: usize, first: Option<&Barrier>) -> Sample {
        let start = self.clock.elapsed().as_nanos();
        let w = changes(id, n);
        let mut retries = 0;
        loop {
            let snapshot = self.epoch.load(Ordering::Acquire);
            let mut tx = self.wal.as_ref().map(|db| db.begin().unwrap());
            if let Some(t) = &mut tx {
                for (k, v) in &w {
                    match v {
                        Some(v) => t.put(k, v).unwrap(),
                        None => t.delete(k).unwrap(),
                    }
                }
            }
            if retries == 0 {
                if let Some(barrier) = first {
                    barrier.wait();
                }
            }
            let _permit = loop {
                let guard = self.admission.read().unwrap();
                if self
                    .remaining
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_sub(1))
                    .is_ok()
                {
                    break guard;
                }
                drop(guard);
                let _exclusive = self.admission.write().unwrap();
                if self.remaining.load(Ordering::Acquire) == 0 {
                    let before = self.clock.elapsed().as_nanos();
                    self.publication
                        .lock()
                        .unwrap()
                        .backend
                        .checkpoint(&mut noop);
                    let after = self.clock.elapsed().as_nanos();
                    self.checkpoints.lock().unwrap().push((before, after));
                    self.remaining.store(self.cap, Ordering::Release);
                }
            };
            let seq = if let Some(t) = tx {
                match t.commit() {
                    Ok(seq) => Some(seq),
                    Err(TxError::Conflict(_)) => None,
                    Err(e) => panic!("{e:?}"),
                }
            } else {
                self.publication
                    .lock()
                    .unwrap()
                    .publish(
                        &self.epoch,
                        snapshot,
                        &w,
                        &Dependencies::default(),
                        &mut noop,
                    )
                    .unwrap()
            };
            if let Some(seq) = seq {
                return Sample {
                    id,
                    lane,
                    seq,
                    start,
                    end: self.clock.elapsed().as_nanos(),
                    retries,
                };
            }
            self.remaining.fetch_add(1, Ordering::Release);
            retries += 1;
            assert!(retries < 10000);
        }
    }
}
fn four_run(
    backend: Backend,
    p: &Path,
    cfg: Config,
    mode: &str,
    n: usize,
    ops: usize,
    old_tx: Option<Transaction>,
    old_map: Option<Data>,
    pin: Option<Pin>,
) {
    let run = Four::new(backend, cfg.logs as usize);
    let first = Barrier::new(4);
    let samples = std::thread::scope(|scope| {
        let threads: Vec<_> = (0..4)
            .map(|lane| {
                let engine = &run;
                let first = &first;
                scope.spawn(move || {
                    (lane..ops)
                        .step_by(4)
                        .map(|id| engine.run_one(id, lane, n, (id < 4).then_some(first)))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect::<Vec<_>>()
    });
    let mem = memory().0;
    let mut ordered: Vec<_> = samples.iter().collect();
    ordered.sort_by_key(|s| s.seq);
    let mut expected = BTreeMap::new();
    for (idx, s) in ordered.iter().enumerate() {
        assert_eq!(s.seq, ordered[0].seq + idx as u64);
        for (k, v) in changes(s.id, n) {
            expected.insert(u64::from_be_bytes(k[..8].try_into().unwrap()) as usize, v);
        }
    }
    let cp = run.checkpoints.into_inner().unwrap();
    assert_eq!(cp.len(), 3);
    let begin = samples.iter().map(|s| s.start).min().unwrap();
    let end = samples.iter().map(|s| s.end).max().unwrap();
    let backend = run.publication.into_inner().unwrap().backend;
    match &backend {
        Backend::Wal(db) => {
            check_tx(&mut old_tx.unwrap(), n, &BTreeMap::new());
            check_tx(&mut db.begin_read().unwrap(), n, &expected);
        }
        _ => {
            check_data(
                &old_map.as_ref().or(pin.as_ref().map(|p| &p.rows)).unwrap(),
                n,
                &BTreeMap::new(),
            );
            check_data(backend.rows(), n, &expected);
        }
    }
    drop(old_map);
    drop(pin);
    drop(backend);
    drop(run.wal);
    match mode {
        "wal" => check_tx(
            &mut Database::open(p).unwrap().begin_read().unwrap(),
            n,
            &expected,
        ),
        "dual" => check_data(
            &Store::open(p, cfg, BUFFER_BUDGET, &mut noop).unwrap().rows,
            n,
            &expected,
        ),
        _ => check_data(&Dirty::open(p, cfg, &mut noop).unwrap().rows, n, &expected),
    }
    println!(
        "SCALED_SUMMARY,{mode},{n},{ops},{},{mem},verified",
        end - begin
    );
    for (i, (a, b)) in cp.iter().enumerate() {
        println!("FOUR_CHECKPOINT,{i},{a},{b}");
    }
    for s in samples {
        println!(
            "SCALED_REQUEST,{mode},{},{},{},{},{},{}",
            s.id, s.lane, s.seq, s.start, s.end, s.retries
        );
    }
}
