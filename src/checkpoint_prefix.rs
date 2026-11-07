use super::*;

#[cfg(all(test, target_os = "linux", target_pointer_width = "64"))]
#[path = "../benchmarks/range-writeback/range.rs"]
mod range_writeback;

pub(super) struct CheckpointCursor {
    pub(super) pinned: Transaction,
    #[cfg(all(test, target_os = "linux", target_pointer_width = "64"))]
    writeback: Option<range_writeback::Pending>,
    last_key: Option<Vec<u8>>,
    after_key: Option<Vec<u8>>,
    chunk_limit: usize,
    pub(super) chunks: u32,
    pub(super) records: usize,
}
pub(super) struct CheckpointDraft {
    pub(super) cursor: CheckpointCursor,
    pub(super) before_bytes: u64,
    pub(super) epoch: u64,
    pub(super) temporary: PathBuf,
    pub(super) file: Option<File>,
}
impl Drop for CheckpointDraft {
    fn drop(&mut self) {
        // Publication takes ownership of the name and handle together.
        if let Some(file) = self.file.take() {
            drop(file);
            let _ = std::fs::remove_file(&self.temporary);
        }
    }
}
/// Experimental checkpoint prefix. Dropping it abandons the unpublished work.
pub struct PreparedCheckpoint {
    db: Database,
    pub(super) draft: Option<CheckpointDraft>,
}
impl PreparedCheckpoint {
    /// Completes and durably publishes the snapshot and the WAL suffix. A newer checkpoint invalidates this prefix.
    pub fn finish(mut self) -> Result<CheckpointInfo> {
        self.db
            .finish_checkpoint(self.draft.take(), false, None, |_| Ok(()))
    }
}
impl Drop for PreparedCheckpoint {
    fn drop(&mut self) {
        drop(self.draft.take());
        self.db
            .inner
            .prefix_active
            .store(false, std::sync::atomic::Ordering::Release);
    }
}

impl Database {
    pub(super) fn start_checkpoint(&self, epoch: u64) -> Result<CheckpointDraft> {
        #[cfg(all(test, target_os = "linux", not(feature = "retirement-timing")))]
        let attempted = independent_trace::stamp();
        let writer = self.inner.writer.lock().map_err(|_| Error::Poisoned)?;
        #[cfg(all(test, target_os = "linux", not(feature = "retirement-timing")))]
        let writer = independent_trace::observe(attempted, writer);
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
        let last_key = state.rows.last_key_value().map(|(key, _)| key.to_vec());
        let path = state.path.clone();
        drop(state);
        drop(writer);
        let temporary = temporary_path(&path);
        let replacement = Some(
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&temporary)?,
        );
        let mut draft = CheckpointDraft {
            cursor: CheckpointCursor {
                #[cfg(all(test, target_os = "linux", target_pointer_width = "64"))]
                writeback: range_writeback::Pending::configured(),
                pinned,
                last_key,
                chunk_limit,
                after_key: None,
                chunks: 0,
                records: 0,
            },
            before_bytes,
            epoch,
            temporary,
            file: replacement,
        };
        draft
            .file
            .as_mut()
            .unwrap()
            .write_all(&wal_header(self.inner.id, sequence, 0))?;
        Ok(draft)
    }
    pub(super) fn write_checkpoint_chunk(
        &self,
        cursor: &mut CheckpointCursor,
        output: &mut File,
        buffer: &mut BufferPermit,
        hook: &mut impl FnMut(CheckpointStage) -> io::Result<()>,
    ) -> Result<bool> {
        buffer.clear();
        let mut pending = BTreeMap::new();
        let mut bytes = 12;
        let mut visited = 0;
        {
            let state = self.state()?;
            let lower = cursor.after_key.as_deref().map_or(Unbounded, Excluded);
            let mut next = cursor.after_key.clone();
            for (key, versions) in state
                .rows
                .range::<[u8], _>((lower, Unbounded))
                .take_while(|(key, _)| {
                    cursor
                        .last_key
                        .as_ref()
                        .is_some_and(|last| key.as_slice() <= last.as_slice())
                })
                .take(256)
            {
                if let Some(value) =
                    visible(versions, cursor.pinned.snapshot).and_then(|v| v.value.as_deref())
                {
                    let size = operation_size(key, Some(value));
                    if size + 12 > cursor.chunk_limit {
                        return Err(Error::ResourceExhausted("checkpoint record"));
                    }
                    if bytes + size > cursor.chunk_limit {
                        break;
                    }
                    if pending.is_empty() {
                        buffer.add(24 + PREFIX_BYTES + TRAILER_BYTES)?;
                    }
                    buffer.add(128 + key.len() + value.len() + 2 * size)?;
                    pending.insert(key.to_vec(), Some(value.to_vec()));
                    bytes += size;
                }
                next = Some(key.to_vec());
                visited += 1;
            }
            cursor.after_key = next;
        }
        if visited == 0 {
            return Ok(false);
        }
        hook(CheckpointStage::ChunkCopied)?;
        if !pending.is_empty() {
            let frame = encode_frame(cursor.pinned.snapshot, &pending);
            output.write_all(&frame)?;
            #[cfg(all(test, target_os = "linux", target_pointer_width = "64"))]
            if let Some(writeback) = &mut cursor.writeback {
                writeback.written(frame.len(), |offset, len| {
                    range_writeback::submit(output, offset, len)
                })?;
            }
            cursor.records += pending.len();
            cursor.chunks = cursor
                .chunks
                .checked_add(1)
                .filter(|count| *count <= 65_536)
                .ok_or(Error::Limit("checkpoint chunks"))?;
        }
        Ok(true)
    }
    /// Writes up to 2,048 snapshot chunks into an unpublished replacement, one prefix per database.
    /// Call on a maintenance thread. The returned handle pins its snapshot until finished or dropped.
    pub fn prepare_checkpoint_prefix(&self, max_chunks: usize) -> Result<PreparedCheckpoint> {
        if !(1..=2048).contains(&max_chunks) {
            return Err(Error::Invalid("checkpoint prefix chunk count"));
        }
        if self
            .inner
            .prefix_active
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return Err(Error::ResourceExhausted("checkpoint prefix active"));
        }
        let mut prepared = PreparedCheckpoint {
            db: self.clone(),
            draft: None,
        };
        let checkpoint = self.inner.checkpoint.lock().map_err(|_| Error::Poisoned)?;
        let mut draft = self.start_checkpoint(*checkpoint)?;
        drop(checkpoint);
        let mut buffer = self.inner.budget.buffer();
        for _ in 0..max_chunks {
            if !self.write_checkpoint_chunk(
                &mut draft.cursor,
                draft.file.as_mut().unwrap(),
                &mut buffer,
                &mut |_| Ok(()),
            )? {
                break;
            }
        }
        prepared.draft = Some(draft);
        Ok(prepared)
    }
}
