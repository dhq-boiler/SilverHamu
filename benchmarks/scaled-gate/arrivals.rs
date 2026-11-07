use std::collections::VecDeque;
use std::sync::mpsc::{Sender, TryRecvError, channel};
type Outcome = crate::transactions::Result<u64>;
const GROUP_COUNT: usize = 4;
const GROUP_BYTES: usize = 512 * 1024;
const QUEUE_BYTES: usize = 4 * 1024 * 1024;
const GROUP_OVERHEAD: usize = 140 + 9 + 1 + 8;
struct Job {
    id: u64,
    bytes: usize,
    tx: Transaction,
    answer: Sender<Outcome>,
}
#[derive(Default)]
struct QueueState {
    next: u64,
    busy: bool,
    pending: VecDeque<Job>,
    bytes: usize,
}
#[derive(Default)]
struct Arrivals {
    state: Mutex<QueueState>,
    ready: Condvar,
}
impl Arrivals {
    fn submit(&self, backend: &MvccPages, tx: Transaction) -> Outcome {
        if tx.writes.is_empty() {
            return tx.commit_single();
        }
        if tx.writes.keys().any(|k| k.len() >= MAX_KEY_BYTES) {
            return Err(TxError::Limit("delta tagged key"));
        }
        let entries = tx.writes.len();
        let bytes: usize = tx
            .writes
            .iter()
            .map(|(k, v)| 10 + k.len() + v.as_ref().map_or(1, |v| 1 + v.len()))
            .sum();
        if bytes + GROUP_OVERHEAD > GROUP_BYTES || entries >= MAX_OPS {
            return Err(TxError::ResourceExhausted("delta batch capacity"));
        }
        if tx
            .writes
            .iter()
            .any(|(k, v)| 140 + 9 + k.len() + v.as_ref().map_or(0, Vec::len) > BLOCK)
        {
            return Err(TxError::Limit("delta page row"));
        }
        let (answer, receiver) = channel();
        let mut state = self.state.lock().map_err(|_| TxError::Poisoned)?;
        if state.pending.len() >= 32 || state.bytes + bytes > QUEUE_BYTES {
            return Err(TxError::ResourceExhausted("delta arrival queue"));
        }
        backend.signal("arrival.admitted").map_err(TxError::Io)?;
        let id = state.next;
        state.next = id.checked_add(1).ok_or(TxError::Limit("delta ticket"))?;
        state.bytes += bytes;
        state.pending.push_back(Job {
            id,
            bytes,
            tx,
            answer,
        });
        loop {
            match receiver.try_recv() {
                Ok(result) => return result,
                Err(TryRecvError::Disconnected) => return Err(TxError::Poisoned),
                Err(TryRecvError::Empty) => {}
            }
            if state.busy || state.pending.front().map(|j| j.id) != Some(id) {
                state = self.ready.wait(state).map_err(|_| TxError::Poisoned)?;
                continue;
            }
            state.busy = true;
            let mut txs = Vec::new();
            let mut replies = Vec::new();
            let mut size = GROUP_OVERHEAD;
            let mut ops = 1;
            while let Some(job) = state.pending.front() {
                if txs.len() == GROUP_COUNT
                    || size + job.bytes > GROUP_BYTES
                    || ops + job.tx.writes.len() > MAX_OPS
                {
                    break;
                }
                let job = state.pending.pop_front().unwrap();
                size += job.bytes;
                ops += job.tx.writes.len();
                state.bytes -= job.bytes;
                txs.push(job.tx);
                replies.push(job.answer);
            }
            drop(state);
            let db = txs[0].database.clone();
            let outcomes = crate::transactions::group_commit::persist_entries(
                &db,
                &mut txs,
                |tx| tx,
                |_, frames| {
                    let mut disk = backend
                        .disk
                        .lock()
                        .map_err(|_| err("delta lock poisoned"))?;
                    let mut seq = disk.logical_seq();
                    let mut joined = WriteSet::new();
                    for frame in frames {
                        let (version, w) =
                            decode_frame(&frame[PREFIX_BYTES..frame.len() - TRAILER_BYTES])
                                .map_err(io::Error::other)?;
                        if seq.checked_add(1) != Some(version) {
                            return Err(err("delta group sequence"));
                        }
                        seq = version;
                        for (k, v) in w {
                            if joined.insert(k, v).is_some() {
                                return Err(err("delta group duplicate write"));
                            }
                        }
                    }
                    disk.append_changes(&joined, Some(seq), &mut |s| backend.signal(s))?;
                    db.inner.group.record_sync();
                    Ok(())
                },
            );
            drop(txs);
            state = self.state.lock().map_err(|_| TxError::Poisoned)?;
            for (answer, result) in replies.into_iter().zip(outcomes) {
                let _ = answer.send(result);
            }
            state.busy = false;
            self.ready.notify_all();
        }
    }
}
