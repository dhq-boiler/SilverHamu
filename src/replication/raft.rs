use super::snapshot::{CHUNK, MAX_IMAGE, Snapshot};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::time::{Duration, Instant};

use super::storage::command_size;

pub(super) const MAX_LOG_ENTRIES: usize = 65_536;
pub(super) const MAX_LOG_BYTES: usize = 128 * 1024 * 1024;
const APPEND_BYTES: usize = 3 * 1024 * 1024;
const APPEND_COUNT: usize = 64;
const SNAPSHOT_ENTRIES: usize = 128;
const SNAPSHOT_BYTES: usize = 8 * 1024 * 1024;
const ENTRY_OVERHEAD: usize = 48;

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Entry {
    pub term: u64,
    pub command: Value,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct HardState {
    pub term: u64,
    pub voted_for: Option<String>,
    pub log: Vec<Entry>,
    pub snapshot: Snapshot,
    pub commit: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Role {
    Follower,
    Candidate,
    Leader,
}

struct Progress {
    snapshot: Option<Snapshot>,
    offset: usize,
    next: u64,
    matched: u64,
}

impl Progress {
    fn advance_to(&mut self, next: u64, current_snapshot: u64) {
        self.next = self.next.max(next);
        let transferring = self.snapshot.as_ref().map_or(current_snapshot, |s| s.index);
        if self.next > transferring {
            self.snapshot = None;
            self.offset = 0;
        }
    }
}

pub(super) struct Raft {
    id: String,
    members: BTreeSet<String>,
    hard: HardState,
    role: Role,
    leader: Option<String>,
    votes: BTreeSet<String>,
    peers: BTreeMap<String, Progress>,
    entry_bytes: Vec<usize>,
    log_bytes: usize,
    deadline: Instant,
    random: u64,
    round_id: u64,
    failed: bool,
    incoming: Option<(u64, String, Vec<u8>)>,
}

pub(super) struct ReadRound {
    term: u64,
    id: u64,
    leader: String,
    acks: BTreeSet<String>,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn number(value: &Value, field: &str) -> io::Result<u64> {
    value[field]
        .as_u64()
        .ok_or_else(|| invalid("invalid Raft integer"))
}

pub(super) fn valid_members(members: &[String]) -> bool {
    members.len() >= 3
        && members.len() % 2 == 1
        && members.iter().all(|id| !id.is_empty())
        && members.iter().collect::<BTreeSet<_>>().len() == members.len()
}

fn entry_size(command: &Value) -> io::Result<usize> {
    Ok(command_size(command)? + ENTRY_OVERHEAD)
}

impl ReadRound {
    pub(super) fn ack(&mut self, peer: &str, sent: &Value, reply: &Value) {
        if sent["to"].as_str() == Some(peer)
            && sent["term"].as_u64() == Some(self.term)
            && reply["term"].as_u64() == Some(self.term)
            && reply["success"] == true
            && sent["round"].as_u64() == Some(self.id)
            && reply["round"].as_u64() == Some(self.id)
        {
            self.acks.insert(peer.to_owned());
        }
    }

    pub(super) fn ready(&self, raft: &Raft) -> bool {
        !raft.failed
            && raft.role == Role::Leader
            && raft.id == self.leader
            && raft.term() == self.term
            && self.acks.intersection(&raft.members).count() >= raft.majority()
    }
}

impl Raft {
    pub(super) fn new(id: String, members: Vec<String>, hard: HardState) -> io::Result<Self> {
        if !valid_members(&members)
            || !members.contains(&id)
            || hard
                .voted_for
                .as_ref()
                .is_some_and(|vote| !members.contains(vote))
            || hard.term == 0 && hard.voted_for.is_some()
            || hard.log.len() > MAX_LOG_ENTRIES
            || hard.snapshot.term > hard.term
            || hard.commit < hard.snapshot.index
            || hard.snapshot.index > i64::MAX as u64
            || hard.commit > hard.snapshot.index + hard.log.len() as u64
        {
            return Err(invalid("invalid Raft membership or hard state"));
        }
        let mut previous = hard.snapshot.term;
        let mut entry_bytes = Vec::with_capacity(hard.log.len());
        let mut log_bytes = 0;
        for entry in &hard.log {
            if entry.term == 0 || entry.term < previous || entry.term > hard.term {
                return Err(invalid("invalid persisted log terms"));
            }
            previous = entry.term;
            let size = entry_size(&entry.command)?;
            log_bytes += size;
            if log_bytes > MAX_LOG_BYTES {
                return Err(invalid("replicated log exceeds limit"));
            }
            entry_bytes.push(size);
        }
        let mut seed = [0; 8];
        getrandom::getrandom(&mut seed)
            .map_err(|_| io::Error::other("election randomness unavailable"))?;
        let mut raft = Self {
            id,
            members: members.into_iter().collect(),
            hard,
            role: Role::Follower,
            leader: None,
            votes: BTreeSet::new(),
            peers: BTreeMap::new(),
            entry_bytes,
            log_bytes,
            deadline: Instant::now(),
            random: u64::from_le_bytes(seed).max(1),
            round_id: 0,
            failed: false,
            incoming: None,
        };
        raft.reset_deadline();
        Ok(raft)
    }

    pub(super) fn term(&self) -> u64 {
        self.hard.term
    }
    pub(super) fn role(&self) -> Role {
        self.role
    }
    pub(super) fn leader(&self) -> Option<&str> {
        self.leader.as_deref()
    }
    pub(super) fn commit_index(&self) -> u64 {
        self.hard.commit
    }
    pub(super) fn hard_state(&self) -> &HardState {
        &self.hard
    }
    pub(super) fn last_index(&self) -> u64 {
        self.hard.snapshot.index + self.hard.log.len() as u64
    }
    pub(super) fn entry(&self, index: u64) -> Option<&Entry> {
        self.hard
            .log
            .get(usize::try_from(index.checked_sub(self.hard.snapshot.index + 1)?).ok()?)
    }
    fn term_at(&self, index: u64) -> Option<u64> {
        if index == self.hard.snapshot.index {
            Some(self.hard.snapshot.term)
        } else {
            self.entry(index).map(|entry| entry.term)
        }
    }
    fn majority(&self) -> usize {
        self.members.len() / 2 + 1
    }
    pub(super) fn election_due(&self) -> bool {
        !self.failed
            && self.role != Role::Leader
            && self.has_barrier_space()
            && Instant::now() >= self.deadline
    }

    pub(super) fn defer_leadership(&mut self) {
        if self.role != Role::Follower {
            self.follower();
        }
        self.reset_deadline();
    }

    fn has_barrier_space(&self) -> bool {
        self.hard.log.len() < MAX_LOG_ENTRIES
            && self.log_bytes + ENTRY_OVERHEAD + b"null".len() <= MAX_LOG_BYTES
    }

    fn reset_deadline(&mut self) {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        self.deadline = Instant::now() + Duration::from_millis(650 + self.random % 551);
    }
    fn healthy(&self) -> io::Result<()> {
        if self.failed {
            Err(io::Error::other("Raft storage failed; restart required"))
        } else {
            Ok(())
        }
    }
    fn check_peer(&self, peer: &str) -> io::Result<()> {
        self.healthy()?;
        if self.id == peer || !self.members.contains(peer) {
            return Err(invalid("unknown remote voter"));
        }
        Ok(())
    }
    fn require_leader(&self) -> io::Result<()> {
        self.healthy()?;
        if self.role != Role::Leader {
            return Err(io::Error::other("not leader"));
        }
        Ok(())
    }
    fn follower(&mut self) {
        self.role = Role::Follower;
        self.leader = None;
        self.votes.clear();
        self.peers.clear();
    }
    fn flush(
        &mut self,
        keep: usize,
        persist: &mut impl FnMut(&HardState, usize) -> io::Result<()>,
    ) -> io::Result<()> {
        if let Err(error) = persist(&self.hard, keep) {
            // The failure may have followed a durable write; in-memory rollback could allow a second vote.
            self.failed = true;
            self.follower();
            return Err(error);
        }
        Ok(())
    }
    fn observe_term(
        &mut self,
        term: u64,
        persist: &mut impl FnMut(&HardState, usize) -> io::Result<()>,
    ) -> io::Result<()> {
        if term > self.term() {
            self.incoming = None;
            self.follower();
            self.hard.term = term;
            self.hard.voted_for = None;
            self.flush(self.hard.log.len(), persist)?;
        }
        Ok(())
    }

    pub(super) fn start_election(
        &mut self,
        mut persist: impl FnMut(&HardState, usize) -> io::Result<()>,
    ) -> io::Result<Value> {
        self.healthy()?;
        if self.role == Role::Leader {
            return Err(invalid("leader cannot start an election"));
        }
        // A full node can still vote and accept a replacement uncommitted suffix.
        // Campaigning without space for its barrier could repeatedly depose a functioning leader.
        if !self.has_barrier_space() {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "no space for election barrier",
            ));
        }
        self.hard.term = self
            .term()
            .checked_add(1)
            .ok_or_else(|| invalid("Raft term exhausted"))?;
        self.incoming = None;
        self.hard.voted_for = Some(self.id.clone());
        self.flush(self.hard.log.len(), &mut persist)?;
        self.role = Role::Candidate;
        self.leader = None;
        self.votes.clear();
        self.votes.insert(self.id.clone());
        self.reset_deadline();
        Ok(
            json!({"kind":"vote","term":self.term(),"last_index":self.last_index(),"last_term":self.term_at(self.last_index())}),
        )
    }

    pub(super) fn handle_rpc(
        &mut self,
        from: &str,
        rpc: &Value,
        mut persist: impl FnMut(&HardState, usize) -> io::Result<()>,
    ) -> io::Result<Value> {
        self.check_peer(from)?;
        let term = number(rpc, "term")?;
        if term == 0 {
            return Err(invalid("zero RPC term"));
        }
        match rpc["kind"].as_str() {
            Some("snapshot") => self.receive_snapshot(from, rpc, term, &mut persist),
            Some("vote") => {
                let last_index = number(rpc, "last_index")?;
                let last_term = number(rpc, "last_term")?;
                if (last_index == 0) != (last_term == 0)
                    || last_term > term
                    || last_index > i64::MAX as u64
                {
                    return Err(invalid("invalid candidate log position"));
                }
                self.observe_term(term, &mut persist)?;
                let grant = term == self.term()
                    && self.hard.voted_for.as_deref().is_none_or(|v| v == from)
                    && (last_term, last_index)
                        >= (self.term_at(self.last_index()).unwrap(), self.last_index());
                if grant {
                    if self.hard.voted_for.as_deref() != Some(from) {
                        self.hard.voted_for = Some(from.to_owned());
                        self.flush(self.hard.log.len(), &mut persist)?;
                    }
                    self.reset_deadline();
                }
                Ok(json!({"kind":"vote","term":self.term(),"granted":grant}))
            }
            Some("append") => self.receive_append(from, rpc, term, &mut persist),
            _ => Err(invalid("unknown Raft RPC")),
        }
    }

    pub(super) fn vote_reply(
        &mut self,
        peer: &str,
        reply: &Value,
        mut persist: impl FnMut(&HardState, usize) -> io::Result<()>,
    ) -> io::Result<()> {
        self.check_peer(peer)?;
        let term = number(reply, "term")?;
        self.observe_term(term, &mut persist)?;
        if reply["kind"] != "vote" {
            return Err(invalid("unexpected vote reply"));
        }
        if term != self.term() || self.role != Role::Candidate || reply["granted"] != true {
            return Ok(());
        }
        self.votes.insert(peer.to_owned());
        if self.votes.len() < self.majority() {
            return Ok(());
        }
        let next = self.last_index() + 1;
        self.push(Value::Null, true, &mut persist)?;
        self.peers = self
            .members
            .iter()
            .filter(|id| **id != self.id)
            .map(|id| {
                (
                    id.clone(),
                    Progress {
                        next,
                        matched: 0,
                        snapshot: None,
                        offset: 0,
                    },
                )
            })
            .collect();
        self.role = Role::Leader;
        self.leader = Some(self.id.clone());
        Ok(())
    }

    fn push(
        &mut self,
        command: Value,
        noop: bool,
        persist: &mut impl FnMut(&HardState, usize) -> io::Result<()>,
    ) -> io::Result<u64> {
        let bytes = entry_size(&command)?;
        let max_entries = if noop {
            MAX_LOG_ENTRIES
        } else {
            MAX_LOG_ENTRIES - 1024
        };
        let max_bytes = if noop {
            MAX_LOG_BYTES
        } else {
            MAX_LOG_BYTES - 64 * 1024
        };
        if self.hard.log.len() >= max_entries || self.log_bytes + bytes > max_bytes {
            return Err(io::Error::new(
                io::ErrorKind::StorageFull,
                "replicated log capacity exhausted",
            ));
        }
        self.hard.log.push(Entry {
            term: self.term(),
            command,
        });
        self.entry_bytes.push(bytes);
        self.log_bytes += bytes;
        self.flush(self.hard.log.len() - 1, persist)?;
        Ok(self.last_index())
    }
    pub(super) fn propose(
        &mut self,
        command: Value,
        mut persist: impl FnMut(&HardState, usize) -> io::Result<()>,
    ) -> io::Result<u64> {
        self.require_leader()?;
        if command.is_null() {
            return Err(invalid("null is reserved for the election barrier"));
        }
        self.push(command, false, &mut persist)
    }

    pub(super) fn needs_snapshot(&self) -> bool {
        let committed = (self.commit_index() - self.hard.snapshot.index) as usize;
        committed >= SNAPSHOT_ENTRIES
            || (self.log_bytes >= SNAPSHOT_BYTES
                && self.entry_bytes[..committed].iter().sum::<usize>() >= SNAPSHOT_BYTES)
    }
    pub(super) fn compact(
        &mut self,
        snapshot: Snapshot,
        persist: &mut impl FnMut(&HardState, usize) -> io::Result<()>,
    ) -> io::Result<()> {
        let old = self.hard.snapshot.index;
        if snapshot.index <= old
            || (snapshot.index <= self.commit_index()
                && self.term_at(snapshot.index) != Some(snapshot.term))
        {
            return Err(invalid("snapshot would replace a committed boundary"));
        }
        for progress in self.peers.values_mut() {
            if progress.next <= old && progress.offset != 0 && progress.snapshot.is_none() {
                progress.snapshot = Some(self.hard.snapshot.clone());
            }
        }
        let keep = if self.term_at(snapshot.index) == Some(snapshot.term) {
            let n = (snapshot.index - old) as usize;
            self.hard.log.drain(..n);
            self.entry_bytes.drain(..n);
            self.hard.log.len()
        } else {
            self.hard.log.clear();
            self.entry_bytes.clear();
            0
        };
        self.log_bytes = self.entry_bytes.iter().sum();
        self.hard.commit = self.hard.commit.max(snapshot.index);
        self.hard.snapshot = snapshot;
        self.flush(keep, persist)
    }
    fn receive_snapshot(
        &mut self,
        from: &str,
        rpc: &Value,
        term: u64,
        persist: &mut impl FnMut(&HardState, usize) -> io::Result<()>,
    ) -> io::Result<Value> {
        let index = number(rpc, "index")?;
        let base_term = number(rpc, "base_term")?;
        let size = number(rpc, "size")?;
        let offset = number(rpc, "offset")?;
        let hash = rpc["hash"]
            .as_str()
            .filter(|h| h.len() == 64)
            .ok_or_else(|| invalid("snapshot digest"))?;
        if rpc["to"] != self.id
            || index == 0
            || index > i64::MAX as u64
            || base_term == 0
            || base_term > term
            || size > MAX_IMAGE as u64
            || offset > size
        {
            return Err(invalid("invalid snapshot header"));
        }
        self.observe_term(term, persist)?;
        let mut reply = json!({"kind":"snapshot","term":self.term(),"installed":false,"offset":0});
        if term < self.term() {
            return Ok(reply);
        }
        if self.role == Role::Leader || self.leader.as_deref().is_some_and(|l| l != from) {
            return Err(invalid("conflicting snapshot leader"));
        }
        self.follower();
        self.leader = Some(from.to_owned());
        self.reset_deadline();
        if index <= self.commit_index() {
            reply["installed"] = true.into();
            return Ok(reply);
        }
        if offset == 0
            && self
                .incoming
                .as_ref()
                .is_none_or(|(i, h, _)| *i != index || h != hash)
        {
            self.incoming = Some((index, hash.into(), Vec::new()));
        }
        let Some((i, h, buf)) = &mut self.incoming else {
            return Ok(reply);
        };
        if *i != index || h != hash {
            return Ok(reply);
        }
        if offset == buf.len() as u64 {
            let chunk = crate::service::hex_decode(
                rpc["data"]
                    .as_str()
                    .ok_or_else(|| invalid("snapshot chunk"))?,
                CHUNK,
            )
            .map_err(io::Error::other)?;
            if offset + chunk.len() as u64 > size {
                return Err(invalid("snapshot extent"));
            }
            buf.extend_from_slice(&chunk);
        }
        reply["offset"] = buf.len().into();
        if buf.len() as u64 == size {
            let (_, _, data) = self.incoming.take().unwrap();
            let snap = Snapshot::new(index, base_term, data);
            if snap.hash != hash {
                return Err(invalid("snapshot digest mismatch"));
            }
            snap.validate()?;
            self.compact(snap, persist)?;
            reply["installed"] = true.into();
        }
        Ok(reply)
    }

    pub(super) fn read_round(&mut self) -> io::Result<ReadRound> {
        self.require_leader()?;
        if self.term_at(self.commit_index()) != Some(self.term()) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "current term not committed",
            ));
        }
        self.round_id = self
            .round_id
            .checked_add(1)
            .ok_or_else(|| invalid("read round exhausted"))?;
        Ok(ReadRound {
            id: self.round_id,
            term: self.term(),
            leader: self.id.clone(),
            acks: BTreeSet::from([self.id.clone()]),
        })
    }
    pub(super) fn append_for_round(&self, peer: &str, round: &ReadRound) -> io::Result<Value> {
        if round.term != self.term() || round.leader != self.id {
            return Err(invalid("obsolete read round"));
        }
        let mut request = self.append_for(peer)?;
        request["round"] = json!(round.id);
        Ok(request)
    }
    pub(super) fn append_for(&self, peer: &str) -> io::Result<Value> {
        self.require_leader()?;
        let next = self
            .peers
            .get(peer)
            .ok_or_else(|| invalid("unknown remote voter"))?
            .next;
        if next <= self.hard.snapshot.index {
            let progress = &self.peers[peer];
            let snap = progress.snapshot.as_ref().unwrap_or(&self.hard.snapshot);
            let offset = progress.offset;
            let end = (offset + CHUNK).min(snap.bytes.len());
            return Ok(
                json!({"kind":"snapshot","to":peer,"term":self.term(),"index":snap.index,"base_term":snap.term,"hash":snap.hash,"size":snap.bytes.len(),"offset":offset,"data":crate::service::hex(&snap.bytes[offset..end])}),
            );
        }
        let prev = next - 1;
        let start = (prev - self.hard.snapshot.index) as usize;
        let mut bytes = 0;
        let mut entries = Vec::new();
        for (entry, size) in self.hard.log[start..]
            .iter()
            .zip(&self.entry_bytes[start..])
            .take(APPEND_COUNT)
        {
            if bytes + size > APPEND_BYTES {
                break;
            }
            bytes += size;
            entries.push(json!({"term":entry.term,"command":entry.command}));
        }
        Ok(
            json!({"kind":"append","to":peer,"term":self.term(),"prev_index":prev,"prev_term":self.term_at(prev),
            "commit":self.commit_index(),"entries":entries}),
        )
    }

    fn receive_append(
        &mut self,
        from: &str,
        rpc: &Value,
        term: u64,
        persist: &mut impl FnMut(&HardState, usize) -> io::Result<()>,
    ) -> io::Result<Value> {
        if rpc["to"] != self.id {
            return Err(invalid("Raft RPC addressed to a different voter"));
        }
        let prev = number(rpc, "prev_index")?;
        let prev_term = number(rpc, "prev_term")?;
        let leader_commit = number(rpc, "commit")?;
        let incoming = rpc["entries"]
            .as_array()
            .ok_or_else(|| invalid("missing entries"))?;
        if prev > i64::MAX as u64
            || (prev + incoming.len() as u64).saturating_sub(self.hard.snapshot.index)
                > MAX_LOG_ENTRIES as u64
                && prev <= self.last_index()
            || incoming.len() > APPEND_COUNT
            || (prev == 0) != (prev_term == 0)
            || prev_term > term
        {
            return Err(invalid("invalid log prefix or append length"));
        }
        self.observe_term(term, persist)?;
        let reject = |term: u64, hint: u64| json!({"kind":"append","term":term,"success":false,"hint":hint,"round":rpc["round"]});
        if term < self.term() {
            return Ok(reject(self.term(), self.last_index() + 1));
        }
        if self.role == Role::Leader || self.leader.as_ref().is_some_and(|leader| leader != from) {
            return Err(invalid("two leaders in the same term"));
        }
        self.follower();
        self.leader = Some(from.to_owned());
        self.reset_deadline();
        if prev < self.hard.snapshot.index {
            let mut response = reject(self.term(), self.hard.snapshot.index + 1);
            response["base_term"] = self.hard.snapshot.term.into();
            return Ok(response);
        }
        if self.term_at(prev) != Some(prev_term) {
            let hint = if prev > self.last_index() {
                self.last_index() + 1
            } else {
                let conflict_term = self.term_at(prev).unwrap();
                self.hard
                    .log
                    .partition_point(|entry| entry.term < conflict_term) as u64
                    + self.hard.snapshot.index
                    + 1
            };
            return Ok(reject(self.term(), hint));
        }
        let mut entry_term = prev_term;
        let mut batch_bytes = 0;
        let mut suffix = Vec::new();
        let mut truncate = None;
        for (offset, item) in incoming.iter().enumerate() {
            let index = prev + offset as u64 + 1;
            let next_term = number(item, "term")?;
            if next_term == 0 || next_term < entry_term || next_term > term {
                return Err(invalid("invalid log term"));
            }
            entry_term = next_term;
            let command = item
                .get("command")
                .ok_or_else(|| invalid("missing log command"))?;
            if truncate.is_none()
                && let Some(existing) = self.entry(index)
                && existing.term == next_term
            {
                if &existing.command != command {
                    return Err(invalid("disagreement at identical log term and index"));
                }
                batch_bytes += self.entry_bytes[(index - self.hard.snapshot.index - 1) as usize];
            } else {
                if index <= self.commit_index() {
                    return Err(invalid("cannot replace a committed log entry"));
                }
                truncate.get_or_insert((index - self.hard.snapshot.index - 1) as usize);
                let size = entry_size(command)?;
                batch_bytes += size;
                suffix.push((
                    Entry {
                        term: next_term,
                        command: command.clone(),
                    },
                    size,
                ));
            }
        }
        if batch_bytes > APPEND_BYTES {
            return Err(invalid("append batch exceeds size limit"));
        }
        let matched = prev + incoming.len() as u64;
        // A shorter or delayed RPC proves nothing about the remaining local suffix.
        let commit = self.commit_index().max(leader_commit.min(matched));
        let changed = truncate.is_some() || commit != self.commit_index();
        if let Some(keep) = truncate {
            let retained = self.log_bytes - self.entry_bytes[keep..].iter().sum::<usize>();
            let total = retained + suffix.iter().map(|(_, size)| size).sum::<usize>();
            if total > MAX_LOG_BYTES {
                return Err(invalid("replicated log exceeds size limit"));
            }
            self.hard.log.truncate(keep);
            self.entry_bytes.truncate(keep);
            for (entry, size) in suffix {
                self.hard.log.push(entry);
                self.entry_bytes.push(size);
            }
            self.log_bytes = total;
        }
        if changed {
            self.hard.commit = commit;
            if self
                .incoming
                .as_ref()
                .is_some_and(|(index, _, _)| *index <= commit)
            {
                self.incoming = None;
            }
            self.flush(truncate.unwrap_or(self.hard.log.len()), persist)?;
        }
        Ok(
            json!({"kind":"append","term":self.term(),"success":true,"matched":matched,"round":rpc["round"]}),
        )
    }

    pub(super) fn append_reply(
        &mut self,
        peer: &str,
        sent: &Value,
        reply: &Value,
        mut persist: impl FnMut(&HardState, usize) -> io::Result<()>,
    ) -> io::Result<bool> {
        self.check_peer(peer)?;
        let term = number(reply, "term")?;
        self.observe_term(term, &mut persist)?;
        if sent["kind"] == "snapshot" && reply["kind"] == "snapshot" && sent["to"] == peer {
            if self.role != Role::Leader || term != self.term() || sent["term"] != term {
                return Ok(false);
            }
            let p = self.peers.get_mut(peer).unwrap();
            let snap = p.snapshot.as_ref().unwrap_or(&self.hard.snapshot);
            if sent["index"] != snap.index || p.next > snap.index || sent["offset"] != p.offset {
                return Ok(false);
            }
            if reply["installed"] == true {
                p.matched = p.matched.max(snap.index);
                p.next = p.matched + 1;
                p.snapshot = None;
                p.offset = 0;
            } else if let Some(offset) = reply["offset"]
                .as_u64()
                .filter(|o| *o <= snap.bytes.len() as u64)
            {
                p.offset = offset as usize;
            }
            return Ok(false);
        }
        if reply["kind"] != "append" || sent["kind"] != "append" || sent["to"] != peer {
            return Err(invalid("unexpected append reply"));
        }
        if self.role != Role::Leader || self.term() != term || sent["term"].as_u64() != Some(term) {
            return Ok(false);
        }
        let prev = number(sent, "prev_index")?;
        let matched = prev
            + sent["entries"]
                .as_array()
                .ok_or_else(|| invalid("missing sent entries"))?
                .len() as u64;
        if matched > self.last_index() {
            return Err(invalid("reply refers to an unknown log position"));
        }
        let compacted_hint = reply["hint"]
            .as_u64()
            .and_then(|n| n.checked_sub(1))
            .filter(|i| {
                *i > prev
                    && reply["base_term"]
                        .as_u64()
                        .is_some_and(|t| self.term_at(*i) == Some(t))
            });
        let progress = self.peers.get_mut(peer).unwrap();
        if reply["success"] == false {
            let hint = number(reply, "hint")?;
            if let Some(base) = compacted_hint
                && progress.next == prev + 1
            {
                progress.advance_to(base + 1, self.hard.snapshot.index);
            } else if prev > progress.matched && progress.next == prev + 1 {
                progress.next = hint.clamp(progress.matched + 1, prev);
            }
            return Ok(false);
        }
        if reply["success"] != true
            || number(reply, "matched")? != matched
            || reply["round"] != sent["round"]
        {
            return Err(invalid("invalid append acknowledgment"));
        }
        progress.matched = progress.matched.max(matched);
        progress.advance_to(matched + 1, self.hard.snapshot.index);
        let mut matches = self.peers.values().map(|p| p.matched).collect::<Vec<_>>();
        matches.push(self.last_index());
        matches.sort_unstable();
        let majority_index = matches[matches.len() - self.majority()];
        if majority_index > self.commit_index() && self.term_at(majority_index) == Some(term) {
            self.hard.commit = majority_index;
            self.flush(self.hard.log.len(), &mut persist)?;
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members() -> Vec<String> {
        ["a", "b", "c"].map(str::to_owned).into()
    }

    fn node(id: &str, hard: HardState) -> Raft {
        Raft::new(id.to_owned(), members(), hard).unwrap()
    }

    fn durable(_: &HardState, _: usize) -> io::Result<()> {
        Ok(())
    }

    fn write(value: u8) -> Value {
        json!({"writes":[["6b", format!("{value:02X}")]]})
    }

    fn elect(leader: &mut Raft, voter: &mut Raft) {
        let request = leader.start_election(durable).unwrap();
        let reply = voter.handle_rpc(&leader.id, &request, durable).unwrap();
        leader.vote_reply(&voter.id, &reply, durable).unwrap();
        assert_eq!(leader.role(), Role::Leader);
    }

    fn exchange(leader: &mut Raft, voter: &mut Raft) -> (Value, Value, bool) {
        let sent = leader.append_for(&voter.id).unwrap();
        let reply = voter.handle_rpc(&leader.id, &sent, durable).unwrap();
        let accepted = leader
            .append_reply(&voter.id, &sent, &reply, durable)
            .unwrap();
        (sent, reply, accepted)
    }

    #[test]
    fn interrupted_snapshot_restarts_at_zero_and_only_a_verified_image_advances_commit() {
        let mut c = node("c", HardState::default());
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0u32.to_be_bytes());
        bytes.extend_from_slice(&(CHUNK as u32).to_be_bytes());
        bytes.resize(CHUNK + 8, 7);
        let snap = Snapshot::new(70_000, 2, bytes);
        let chunk = |offset: usize, data: &[u8]| {
            json!({"kind":"snapshot","to":"c","term":3,
            "index":snap.index,"base_term":snap.term,"size":snap.bytes.len(),"hash":snap.hash,
            "offset":offset,"data":crate::service::hex(data)})
        };
        let first = chunk(0, &snap.bytes[..CHUNK]);
        let last = chunk(CHUNK, &snap.bytes[CHUNK..]);
        assert_eq!(c.handle_rpc("a", &first, durable).unwrap()["offset"], CHUNK);
        assert_eq!(c.commit_index(), 0);
        c = node("c", c.hard);
        assert_eq!(c.handle_rpc("a", &last, durable).unwrap()["offset"], 0);
        c.handle_rpc("a", &first, durable).unwrap();
        assert!(c.handle_rpc("a", &chunk(CHUNK, &[42; 8]), durable).is_err());
        assert_eq!(c.commit_index(), 0);
        c.handle_rpc("a", &first, durable).unwrap();
        c.handle_rpc("a", &first, durable).unwrap();
        assert_eq!(
            c.handle_rpc("a", &last, durable).unwrap()["installed"],
            true
        );
        assert_eq!(c.term_at(70_000), Some(2));
        let tail = json!({"kind":"append","to":"c","term":3,"prev_index":70000,"prev_term":2,
            "commit":70001,"entries":[{"term":3,"command":write(1)}]});
        assert_eq!(c.handle_rpc("a", &tail, durable).unwrap()["success"], true);
        c.handle_rpc("a", &last, durable).unwrap();
        assert_eq!(c.commit_index(), 70_001);
        assert_eq!(c.hard.log.len(), 1);
    }

    #[test]
    fn installing_a_snapshot_keeps_only_a_suffix_with_a_matching_boundary_term() {
        for (base_term, expected_log_len) in [(1, 1), (2, 0)] {
            let mut c = node(
                "c",
                HardState {
                    term: 2,
                    log: vec![
                        Entry {
                            term: 1,
                            command: write(1),
                        },
                        Entry {
                            term: 2,
                            command: write(2),
                        },
                    ],
                    ..Default::default()
                },
            );
            let empty = Snapshot::new(1, base_term, Vec::new());
            let request = json!({"kind":"snapshot","to":"c","term":3,"index":1,"base_term":base_term,
                "hash":empty.hash,"size":0,"offset":0,"data":""});
            assert_eq!(
                c.handle_rpc("a", &request, durable).unwrap()["installed"],
                true
            );
            assert_eq!(c.hard.log.len(), expected_log_len);
            assert_eq!(c.term_at(1), Some(base_term));
            assert_eq!(c.commit_index(), 1);
        }
    }

    #[test]
    fn vote_survives_restart_and_rejects_a_longer_but_older_candidate_log() {
        let mut voter = node(
            "c",
            HardState {
                term: 3,
                log: vec![Entry {
                    term: 3,
                    command: write(7),
                }],
                ..Default::default()
            },
        );
        let mut disk = voter.hard.clone();
        let mut save = |hard: &HardState, _: usize| {
            disk = hard.clone();
            Ok(())
        };
        let stale = json!({"kind":"vote","term":4,"last_index":20,"last_term":2});
        assert_eq!(
            voter.handle_rpc("a", &stale, &mut save).unwrap()["granted"],
            false
        );
        let current = json!({"kind":"vote","term":4,"last_index":1,"last_term":3});
        assert_eq!(
            voter.handle_rpc("b", &current, save).unwrap()["granted"],
            true
        );
        assert_eq!(disk.voted_for.as_deref(), Some("b"));
        let mut restarted = node("c", disk);
        assert_eq!(
            restarted.handle_rpc("a", &current, durable).unwrap()["granted"],
            false
        );
        assert_eq!(
            restarted.handle_rpc("b", &current, durable).unwrap()["granted"],
            true
        );
    }

    #[test]
    fn leader_requires_a_majority_for_a_current_term_entry_before_committing_prior_terms() {
        let old = Entry {
            term: 1,
            command: write(11),
        };
        let mut a = node(
            "a",
            HardState {
                term: 1,
                log: vec![old; APPEND_COUNT + 5],
                ..Default::default()
            },
        );
        let mut b = node("b", HardState::default());
        elect(&mut a, &mut b);
        assert!(a.read_round().is_err());
        assert_eq!(a.commit_index(), 0);
        let (probe, reject, accepted) = exchange(&mut a, &mut b);
        assert!(!accepted);
        assert!(exchange(&mut a, &mut b).2);
        assert_eq!(b.last_index(), APPEND_COUNT as u64);
        assert_eq!(a.commit_index(), 0);
        assert!(exchange(&mut a, &mut b).2);
        assert_eq!(a.commit_index(), a.last_index());
        assert!(a.entry(a.last_index()).unwrap().command.is_null());
        assert!(!a.append_reply("b", &probe, &reject, durable).unwrap());
        let heartbeat = a.append_for("b").unwrap();
        assert_eq!(heartbeat["entries"], json!([]));
        let ack = b.handle_rpc("a", &heartbeat, durable).unwrap();
        assert_eq!(b.commit_index(), a.commit_index());
        let mut persisted = 0;
        a.append_reply("b", &heartbeat, &ack, |_, _| {
            persisted += 1;
            Ok(())
        })
        .unwrap();
        b.handle_rpc("a", &heartbeat, |_, _| {
            persisted += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(persisted, 0, "an unchanged heartbeat must not sync storage");
    }

    #[test]
    fn follower_only_commits_the_verified_prefix_and_repairs_an_uncommitted_suffix() {
        let mut c = node(
            "c",
            HardState {
                term: 3,
                commit: 1,
                voted_for: None,
                snapshot: Snapshot::default(),
                log: vec![
                    Entry {
                        term: 1,
                        command: write(1),
                    },
                    Entry {
                        term: 3,
                        command: write(3),
                    },
                ],
            },
        );
        let mut request = json!({"kind":"append","to":"c","term":4,"prev_index":1,"prev_term":1,"entries":[],"commit":2});
        assert_eq!(
            c.handle_rpc("a", &request, durable).unwrap()["success"],
            true
        );
        assert_eq!(c.commit_index(), 1);
        assert_eq!(c.entry(2).unwrap().command, write(3));
        request["entries"] = json!([{"term":4,"command":write(4)}]);
        let mut disk = HardState::default();
        assert_eq!(
            c.handle_rpc("a", &request, |state, keep| {
                assert_eq!(keep, 1, "only the committed prefix survives suffix repair");
                disk = state.clone();
                Ok(())
            })
            .unwrap()["success"],
            true
        );
        c = node("c", disk);
        assert_eq!(c.commit_index(), 2);
        assert_eq!(c.entry(2).unwrap().command, write(4));
        request["prev_index"] = json!(0);
        request["prev_term"] = json!(0);
        request["entries"] = json!([{"term":4,"command":write(8)}]);
        assert!(c.handle_rpc("a", &request, durable).is_err());
        assert_eq!(c.entry(1).unwrap().command, write(1));
    }

    #[test]
    fn cached_heartbeats_and_another_reads_acks_cannot_authorize_a_read() {
        let mut a = node("a", HardState::default());
        let mut b = node("b", HardState::default());
        elect(&mut a, &mut b);
        let (heartbeat, cached, _) = exchange(&mut a, &mut b);
        let mut first = a.read_round().unwrap();
        first.ack("b", &heartbeat, &cached);
        assert!(!first.ready(&a));
        let sent = a.append_for_round("b", &first).unwrap();
        let reply = b.handle_rpc("a", &sent, durable).unwrap();
        assert!(a.append_reply("b", &sent, &reply, durable).unwrap());
        first.ack("b", &sent, &reply);
        assert!(first.ready(&a));
        let mut second = a.read_round().unwrap();
        second.ack("b", &sent, &reply);
        assert!(!second.ready(&a));
        let superseded = json!({"kind":"append","term":a.term()+1,"success":false});
        assert!(!a.append_reply("b", &sent, &superseded, durable).unwrap());
        assert_eq!(a.role(), Role::Follower);
        assert!(!first.ready(&a));
        assert!(a.propose(write(1), durable).is_err());
    }

    #[test]
    fn an_ambiguous_disk_failure_stops_voting_proposals_and_read_fences() {
        let mut a = node("a", HardState::default());
        let mut b = node("b", HardState::default());
        elect(&mut a, &mut b);
        exchange(&mut a, &mut b);
        let mut round = a.read_round().unwrap();
        let sent = a.append_for_round("b", &round).unwrap();
        let reply = b.handle_rpc("a", &sent, durable).unwrap();
        round.ack("b", &sent, &reply);
        assert!(round.ready(&a));
        let mut disk = HardState::default();
        assert!(
            a.propose(write(1), |hard, _| {
                disk = hard.clone();
                Err(io::Error::other("sync outcome unknown"))
            })
            .is_err()
        );
        assert!(!round.ready(&a));
        assert!(a.start_election(durable).is_err());
        assert!(
            a.handle_rpc(
                "c",
                &json!({"kind":"vote","term":8,"last_term":1,"last_index":2}),
                durable
            )
            .is_err()
        );
        assert_eq!(node("a", disk).commit_index(), 1);
    }

    #[test]
    fn five_voters_require_three_distinct_votes_and_three_durable_copies() {
        let voters = ["a", "b", "c", "d", "e"].map(str::to_owned).to_vec();
        let mut a = Raft::new("a".into(), voters.clone(), HardState::default()).unwrap();
        let mut b = Raft::new("b".into(), voters.clone(), HardState::default()).unwrap();
        let mut c = Raft::new("c".into(), voters, HardState::default()).unwrap();
        let req = a.start_election(durable).unwrap();
        let vote = b.handle_rpc("a", &req, durable).unwrap();
        a.vote_reply("b", &vote, durable).unwrap();
        a.vote_reply("b", &vote, durable).unwrap();
        assert!(a.vote_reply("nonmember", &vote, durable).is_err());
        assert_eq!(a.role(), Role::Candidate);
        let vote = c.handle_rpc("a", &req, durable).unwrap();
        a.vote_reply("c", &vote, durable).unwrap();
        let index = a.propose(write(3), durable).unwrap();
        let (req, ack, _) = exchange(&mut a, &mut b);
        a.append_reply("b", &req, &ack, durable).unwrap();
        assert_eq!(a.commit_index(), 0);
        exchange(&mut a, &mut c);
        assert_eq!(a.commit_index(), index);
    }

    #[test]
    fn exhausted_nodes_can_vote_but_do_not_disrupt_other_leaders_by_campaigning() {
        let last = Entry {
            term: 1,
            command: Value::Null,
        };
        let mut a = node(
            "a",
            HardState {
                term: 1,
                log: vec![last; MAX_LOG_ENTRIES - 1],
                ..Default::default()
            },
        );
        let mut b = node("b", HardState::default());
        elect(&mut a, &mut b);
        assert_eq!(a.last_index(), MAX_LOG_ENTRIES as u64);
        assert_eq!(
            a.propose(write(0), durable).unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
        let term = a.term() + 1;
        let vote =
            json!({"kind":"vote","term":term,"last_term":a.term(),"last_index":a.last_index()});
        assert_eq!(a.handle_rpc("c", &vote, durable).unwrap()["granted"], true);
        a.deadline = Instant::now();
        assert!(!a.election_due());
        assert_eq!(
            a.start_election(durable).unwrap_err().kind(),
            io::ErrorKind::StorageFull
        );
        assert_eq!(a.term(), term);
    }

    #[test]
    fn an_apply_lagging_leader_relinquishes_reads_and_a_lagging_follower_keeps_its_hint() {
        let mut a = node("a", HardState::default());
        let mut b = node("b", HardState::default());
        elect(&mut a, &mut b);
        exchange(&mut a, &mut b);
        let mut round = a.read_round().unwrap();
        let sent = a.append_for_round("b", &round).unwrap();
        let reply = b.handle_rpc("a", &sent, durable).unwrap();
        round.ack("b", &sent, &reply);
        assert!(round.ready(&a));
        a.defer_leadership();
        assert_eq!(a.role(), Role::Follower);
        assert_eq!(a.leader(), None);
        assert!(!round.ready(&a));
        b.deadline = Instant::now();
        b.defer_leadership();
        assert_eq!(b.leader(), Some("a"));
        assert!(!b.election_due());
    }
}
