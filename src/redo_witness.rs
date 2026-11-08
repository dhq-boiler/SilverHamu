use super::*;

pub(super) const SLOT_BYTES: u64 = 64 * 1024;
pub(super) const HEADER: usize = 80;
const MAX_REDO: usize = SLOT_BYTES as usize - HEADER;
const MAGIC: &[u8; 8] = b"NDRWIT01";

pub(in super::super) fn validate_redo_capacity(capacity: u64) -> Result<()> {
    validate_capacity(capacity)?;
    if capacity > MAX_REDO_CAPACITY {
        return Err(Error::Invalid("redo log capacity"));
    }
    Ok(())
}

#[derive(PartialEq, Eq)]
struct Witness {
    generation: u64,
    start: u64,
    end: u64,
    data: Vec<u8>,
}

impl Witness {
    fn write(
        &self,
        mut file: &File,
        fixed_header: &[u8; HEADER_BYTES],
        capacity: u64,
        index: u64,
    ) -> io::Result<()> {
        let mut h = [0; HEADER];
        h[..8].copy_from_slice(MAGIC);
        h[8..16].copy_from_slice(&self.generation.to_le_bytes());
        h[16..24].copy_from_slice(&self.start.to_le_bytes());
        h[24..32].copy_from_slice(&self.end.to_le_bytes());
        h[32..40].copy_from_slice(&capacity.to_le_bytes());
        h[40..56].copy_from_slice(&fixed_header[16..32]);
        h[56..60].copy_from_slice(&fixed_header[44..48]);
        h[60..64].copy_from_slice(&(self.data.len() as u32).to_le_bytes());
        h[64..68].copy_from_slice(&crate::crc32(&self.data).to_le_bytes());
        let crc = crate::crc32(&h[..76]);
        h[76..].copy_from_slice(&crc.to_le_bytes());
        file.seek(SeekFrom::Start(capacity + index * SLOT_BYTES))?;
        file.write_all(&h)?;
        file.write_all(&self.data)
    }

    fn read(
        file: &mut File,
        fixed_header: &[u8; HEADER_BYTES],
        capacity: u64,
        index: u64,
        budget: &Arc<Budget>,
    ) -> Result<Option<(Self, BufferPermit)>> {
        file.seek(SeekFrom::Start(capacity + index * SLOT_BYTES))?;
        let mut h = [0; HEADER];
        file.read_exact(&mut h)?;
        let u = |at| u64::from_le_bytes(h[at..at + 8].try_into().unwrap());
        let (generation, start, end, len) = (u(8), u(16), u(24), read_u32(&h, 60) as usize);
        if &h[..8] != MAGIC
            || read_u32(&h, 76) != crate::crc32(&h[..76])
            || u(32) != capacity
            || h[40..56] != fixed_header[16..32]
            || h[56..60] != fixed_header[44..48]
            || h[68..76] != [0; 8]
            || !(HEADER_BYTES as u64..=capacity - SENTINEL).contains(&start)
            || !(start..=capacity - SENTINEL).contains(&end)
            || (generation == 0 && start != end)
            || (generation != 0 && (generation % 2 != index % 2 || start == end))
        {
            return Ok(None);
        }
        let delta = end - start;
        let expected = if delta <= MAX_REDO as u64 {
            delta as usize
        } else {
            0
        };
        if len != expected {
            return Ok(None);
        }
        let mut permit = budget.buffer();
        permit.add(len)?;
        let mut data = vec![0; len];
        file.read_exact(&mut data)?;
        if crate::crc32(&data) != read_u32(&h, 64) {
            return Ok(None);
        }
        Ok(Some((
            Self {
                generation,
                start,
                end,
                data,
            },
            permit,
        )))
    }
}

fn scratch(
    budget: &Arc<Budget>,
    maximum: usize,
    following_bytes: usize,
) -> Result<(BufferPermit, Vec<u8>)> {
    let usage = budget.snapshot()?;
    let len = maximum.min(usage.limits.max_buffer_bytes - usage.current[crate::resources::BUFFER]);
    if len == 0 {
        return Err(Error::ResourceExhausted("database resource budget"));
    }
    let mut permit = budget.buffer();
    permit.add(len.max(following_bytes))?;
    Ok((permit, vec![0; len]))
}

pub(in super::super) fn initialize_redo(
    file: &mut File,
    end: u64,
    capacity: u64,
    budget: &Arc<Budget>,
) -> Result<()> {
    initialize(file, end, capacity, budget, false)
}

pub(in super::super) fn initialize_managed_redo(
    file: &mut File,
    end: u64,
    capacity: u64,
    budget: &Arc<Budget>,
) -> Result<()> {
    initialize(file, end, capacity, budget, true)
}

fn initialize(
    file: &mut File,
    end: u64,
    capacity: u64,
    budget: &Arc<Budget>,
    managed: bool,
) -> Result<()> {
    validate_redo_capacity(capacity)?;
    if managed {
        file.seek(SeekFrom::Start(40))?;
        let mut chunks = [0; 4];
        file.read_exact(&mut chunks)?;
        file.seek(SeekFrom::Start(40))?;
        file.write_all(&(u32::from_le_bytes(chunks) | MANAGED_REDO_FLAG).to_le_bytes())?;
    }
    let (_permit, zeros) = scratch(budget, 32768, 0)?;
    let header = initialize_data(file, end, capacity, 4, &zeros)?;
    write_zeros(file, 4 * SLOT_BYTES, &zeros)?;
    let initial = Witness {
        generation: 0,
        start: end,
        end,
        data: Vec::new(),
    };
    for index in 0..4 {
        initial.write(file, &header, capacity, index)?;
    }
    file.sync_all()?;
    file.seek(SeekFrom::Start(end))?;
    Ok(())
}

pub(super) fn open(
    mut file: File,
    header: [u8; HEADER_BYTES],
    budget: &Arc<Budget>,
) -> Result<LogFile> {
    let capacity = file
        .metadata()?
        .len()
        .checked_sub(4 * SLOT_BYTES)
        .ok_or_else(invalid)?;
    validate_redo_capacity(capacity).map_err(|_| invalid())?;
    let mut records: [Option<(u64, Witness, BufferPermit)>; 4] = std::array::from_fn(|_| None);
    for (index, entry) in records.iter_mut().enumerate() {
        *entry = Witness::read(&mut file, &header, capacity, index as u64, budget)?
            .map(|(record, permit)| (index as u64, record, permit));
    }
    records.sort_unstable_by_key(|entry| entry.as_ref().map(|(_, w, _)| w.generation));
    let (newest_index, newest, witness_permit) = records[3].take().ok_or_else(invalid)?;
    let generation = newest.generation;
    let end = newest.end;
    let mut missing = [
        generation % 2 != newest_index,
        2 + generation % 2 != newest_index,
    ];
    let mut newer = &newest;
    for (index, older, _) in records.iter().rev().flatten() {
        match newer.generation - older.generation {
            0 if older == newer => {}
            1 if older.end == newer.start => {}
            2 if older.end < newer.start => {}
            _ => return Err(invalid().into()),
        }
        if older == &newest && index % 2 == generation % 2 {
            missing[*index as usize / 2] = false;
        }
        newer = older;
    }
    drop(records);
    let mut primary_permit = budget.buffer();
    primary_permit.add(newest.data.len())?;
    let mut primary = vec![0; newest.data.len()];
    file.seek(SeekFrom::Start(newest.start))?;
    file.read_exact(&mut primary)?;
    let damaged_primary = primary != newest.data;
    if damaged_primary && complete_frames(&primary) {
        return Err(invalid().into());
    }
    let repair = (damaged_primary || missing.contains(&true)).then_some(Repair {
        record: newest,
        _permit: witness_permit,
        damaged_primary,
        missing,
    });
    file.seek(SeekFrom::Start(HEADER_BYTES as u64))?;
    Ok(LogFile {
        file,
        repair,
        fixed: Some(Fixed {
            capacity,
            header,
            end: Mutex::new((generation, end)),
            #[cfg(feature = "lock-timing")]
            metrics: Mutex::new([0; 3]),
        }),
    })
}

fn complete_frames(mut bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    while bytes.len() >= PREFIX_BYTES {
        let len = read_u32(bytes, 8);

        if &bytes[..8] != FRAME
            || !(12..=MAX_TRANSACTION_BYTES).contains(&(len as usize))
            || read_u32(bytes, 12) != !len
            || read_u32(bytes, 16) != crate::crc32(&bytes[..16])
            || len as usize > bytes.len().saturating_sub(PREFIX_BYTES + TRAILER_BYTES)
        {
            return false;
        }
        let (frame, rest) = bytes.split_at(PREFIX_BYTES + len as usize + TRAILER_BYTES);
        let seal = PREFIX_BYTES + len as usize;
        if read_u32(frame, seal) != crate::crc32(&frame[PREFIX_BYTES..seal])
            || &frame[seal + 4..] != SEAL
        {
            return false;
        }
        bytes = rest;
    }
    bytes.is_empty()
}

pub(super) struct Repair {
    record: Witness,
    _permit: BufferPermit,
    damaged_primary: bool,
    missing: [bool; 2],
}

impl Repair {
    pub(super) fn read(&self, file: &mut File, bytes: &mut [u8]) -> io::Result<usize> {
        if !self.damaged_primary {
            return file.read(bytes);
        }
        let pos = file.stream_position()?;
        if pos < self.record.start {
            let available = self.record.start - pos;
            let n = available.min(bytes.len() as u64) as usize;
            return file.read(&mut bytes[..n]);
        }
        if pos < self.record.end {
            let offset = (pos - self.record.start) as usize;
            let n = bytes.len().min(self.record.data.len() - offset);
            bytes[..n].copy_from_slice(&self.record.data[offset..offset + n]);
            file.seek(SeekFrom::Start(pos + n as u64))?;
            return Ok(n);
        }
        file.read(bytes)
    }

    fn apply(&self, mut file: &File, fixed: &Fixed) -> io::Result<()> {
        if self.damaged_primary {
            file.seek(SeekFrom::Start(self.record.start))?;
            file.write_all(&self.record.data)?;
        }
        for (lane, missing) in self.missing.into_iter().enumerate() {
            if missing {
                let index = lane as u64 * 2 + self.record.generation % 2;
                self.record
                    .write(file, &fixed.header, fixed.capacity, index)?;
            }
        }
        phase("redo-before-repair-sync")?;
        file.sync_data()?;
        phase("redo-after-repair-sync")
    }
}

pub(super) fn finish_recovery(
    mut file: &File,
    fixed: &Fixed,
    repair: Option<Repair>,
    budget: &Arc<Budget>,
) -> Result<()> {
    let end = file.stream_position()?;
    let previous = fixed.end.lock().map_err(|_| invalid())?.1;
    if end > fixed.capacity - SENTINEL || end < previous {
        return Err(invalid().into());
    }
    let delta = end - previous;
    let seal_bytes = if delta <= MAX_REDO as u64 {
        delta as usize
    } else {
        0
    };
    let (_permit, mut bytes) = scratch(
        budget,
        (fixed.capacity - end).min(8192) as usize,
        seal_bytes,
    )?;
    if let Some(repair) = repair {
        repair.apply(file, fixed)?;
        file.seek(SeekFrom::Start(end))?;
    }
    // Old, abandoned frames beyond a torn tail could otherwise reappear after a subsequent
    // foreground append overwrites its prefix and crashes before its new sentinel reaches disk.
    let mut pos = end;
    while pos < fixed.capacity {
        let n = (fixed.capacity - pos).min(bytes.len() as u64) as usize;
        file.read_exact(&mut bytes[..n])?;
        if bytes[..n].iter().any(|byte| *byte != 0) {
            file.seek(SeekFrom::Start(pos))?;
            bytes[..n].fill(0);
            file.write_all(&bytes[..n])?;
        }
        pos += n as u64;
    }
    file.seek(SeekFrom::Start(end))?;
    drop(bytes);
    Ok(sync(file, fixed)?)
}

pub(super) fn sync(mut file: &File, fixed: &Fixed) -> io::Result<()> {
    let mut confirmed = fixed.end.lock().map_err(|_| invalid())?;
    let (generation, start) = *confirmed;
    let end = file.stream_position()?;
    if !(start..=fixed.capacity - SENTINEL).contains(&end) {
        return Err(invalid());
    }
    if start == end {
        // Reopen can see prior repair writes in cache even after an interrupted repair sync.
        return file.sync_data();
    }
    let generation = generation.checked_add(1).ok_or_else(invalid)?;
    file.write_all(&[0; PREFIX_BYTES])?;
    let delta = end - start;
    #[cfg(feature = "lock-timing")]
    let mut payload_ns = 0;
    let data = if delta > MAX_REDO as u64 {
        phase("redo-before-payload-sync")?;
        #[cfg(feature = "lock-timing")]
        let started = std::time::Instant::now();
        file.sync_data()?;
        #[cfg(feature = "lock-timing")]
        {
            payload_ns = started.elapsed().as_nanos() as u64;
        }
        phase("redo-after-payload-sync")?;
        Vec::new()
    } else {
        let mut bytes = vec![0; delta as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut bytes)?;
        bytes
    };
    phase("redo-before-witness")?;
    // Without an inline redo the witness could reach disk before all its primary bytes.
    let witness = Witness {
        generation,
        start,
        end,
        data,
    };
    for index in [generation % 2, 2 + generation % 2] {
        witness.write(file, &fixed.header, fixed.capacity, index)?;
    }
    file.seek(SeekFrom::Start(end))?;
    phase("redo-before-sync")?;
    #[cfg(feature = "lock-timing")]
    let started = std::time::Instant::now();
    file.sync_data()?;
    #[cfg(feature = "lock-timing")]
    {
        let mut metrics = fixed.metrics.lock().map_err(|_| invalid())?;
        metrics[0] += 1;
        metrics[1] += payload_ns;
        metrics[2] += started.elapsed().as_nanos() as u64;
    }
    phase("redo-after-sync")?;
    *confirmed = (generation, end);
    Ok(())
}
