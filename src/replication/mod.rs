//! Replicated membership and consensus for the token-authenticated service.
mod membership;
mod raft;
mod snapshot;
mod storage;
mod transport;

use crate::transactions::{Database, Error, ResourceLimits, Transaction};
use membership::{Ids, Phase, Routes, endpoints, valid_id, validate_routes};
use raft::{HardState, Raft, ReadRound, Role};
use ring::hmac;
use serde_json::{Value, json};

use std::io::{self, Read};
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};
use storage::Storage;

pub struct ClusterConfig {
    pub(crate) admin_token: Option<Vec<u8>>,
    id: String,
    identity: String,
    node: String,
    members: Routes,
    remote: bool,
    peer: SocketAddr,
    client: SocketAddr,
}

impl ClusterConfig {
    pub fn set_admin_token(&mut self, token: Vec<u8>) {
        self.admin_token = Some(token);
    }
    pub fn load(path: &Path, node: &str, allow_remote: bool) -> io::Result<Self> {
        let mut bytes = Vec::new();
        std::fs::File::open(path)?
            .take(65537)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 65536 {
            return Err(io::Error::other("cluster config too large"));
        }
        let config: Value = serde_json::from_slice(&bytes)?;
        let cluster = config["cluster_id"]
            .as_str()
            .filter(|id| valid_id(id))
            .ok_or_else(|| io::Error::other("invalid cluster id"))?;
        let members = endpoints(&config["members"], allow_remote)?;
        let mut directory = members.clone();
        if let Some(join) = config.get("joining") {
            let id = join["id"]
                .as_str()
                .filter(|id| valid_id(id))
                .ok_or_else(|| io::Error::other("invalid joining id"))?;
            let addr = |field: &str| {
                join[field]
                    .as_str()
                    .unwrap_or("")
                    .parse::<SocketAddr>()
                    .map_err(io::Error::other)
            };
            if id != node
                || directory
                    .insert(id.into(), [addr("peer")?, addr("client")?])
                    .is_some()
            {
                return Err(io::Error::other("joining must name this new node"));
            }
        }
        validate_routes(&directory, allow_remote)?;
        let [peer, client] = *directory
            .get(node)
            .ok_or_else(|| io::Error::other("unknown node id"))?;
        Ok(Self {
            admin_token: None,
            id: cluster.into(),
            identity: serde_json::to_string(&(cluster, &members))?,
            node: node.into(),
            members,
            remote: allow_remote,
            peer,
            client,
        })
    }

    pub fn client_address(&self) -> SocketAddr {
        self.client
    }
}

struct Core {
    raft: Raft,
    storage: Storage,
}

pub(crate) struct Cluster {
    config: ClusterConfig,
    key: hmac::Key,
    core: Mutex<Core>,
    operations: Mutex<()>,
    lock_term: AtomicU64,
    failed: AtomicBool,
    database: Database,
    listener: TcpListener,
}

type ClientResult = Result<Value, Error>;

impl Cluster {
    pub(crate) fn open(
        config: ClusterConfig,
        token: &[u8],
        path: &Path,
        init: bool,
        limits: ResourceLimits,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(config.peer)?;
        listener.set_nonblocking(true)?;
        let ids = config.members.keys().cloned().collect::<Vec<_>>();
        let disk_identity = if config.members.contains_key(&config.node) {
            config.identity.clone()
        } else {
            serde_json::to_string(&(&config.identity, &config.node, config.peer, config.client))?
        };
        let (storage, hard) =
            Storage::open(path, &disk_identity, &config.node, &ids, init, limits)?;
        let database = storage.database();
        let lock_term = AtomicU64::new(hard.term);
        let raft = Raft::new(config.node.clone(), ids, hard)?;
        Ok(Self {
            config,
            key: hmac::Key::new(hmac::HMAC_SHA256, token),
            core: Mutex::new(Core { raft, storage }),
            operations: Mutex::new(()),
            lock_term,
            failed: AtomicBool::new(false),
            database,
            listener,
        })
    }

    pub(crate) fn database(&self) -> Database {
        self.database.clone()
    }

    fn lock(&self) -> io::Result<MutexGuard<'_, Core>> {
        let core = self
            .core
            .lock()
            .map_err(|_| io::Error::other("consensus mutex poisoned"))?;
        if self.failed.load(Ordering::Acquire) {
            return Err(io::Error::other("cluster storage requires recovery"));
        }
        Ok(core)
    }

    fn persist<'a>(
        &'a self,
        storage: &'a Storage,
    ) -> impl FnMut(&HardState, usize) -> io::Result<()> + 'a {
        move |state, keep| {
            let release = if self.lock_term.swap(state.term, Ordering::Relaxed) != state.term {
                self.database
                    .release_replica_locks()
                    .map_err(io::Error::other)
            } else {
                Ok(())
            };
            self.storage_result(release.and_then(|_| storage.persist(state, keep)))
        }
    }

    fn storage_result(&self, result: io::Result<()>) -> io::Result<()> {
        result.inspect_err(|err| {
            self.failed.store(true, Ordering::Release);
            eprintln!("catdb: cluster storage failed: {err}");
        })
    }

    fn rpc(&self, request: Value) -> io::Result<Value> {
        let from = request["from"].as_str().unwrap_or("");
        if request["group"].as_str() != Some(&self.config.identity)
            || request["to"].as_str() != Some(&self.config.node)
            || from == self.config.node
            || !valid_id(from)
            || request
                .get("endpoint")
                .is_some_and(|v| *v != json!([self.config.peer, self.config.client]))
        {
            return Err(io::Error::other("cluster membership mismatch"));
        }
        if request["hello"] == true {
            return Ok(json!({"hello":true}));
        }
        let mut core = self.lock()?;
        let Core { raft, storage } = &mut *core;
        let mut reply = raft.handle_rpc(from, &request["rpc"], self.persist(storage))?;
        reply["applied"] = storage.applied_index().map_err(io::Error::other)?.into();
        Ok(reply)
    }

    fn exchange(&self, peer: &str, rpc: &Value) -> io::Result<Value> {
        let (route, dynamic) = {
            let core = self.lock()?;
            let route = core
                .raft
                .membership
                .routes
                .get(peer)
                .or_else(|| self.config.members.get(peer))
                .copied()
                .ok_or_else(|| io::Error::other("unknown peer endpoint"))?;
            (
                route,
                core.raft.config_index != 0
                    || !rpc["membership"].is_null()
                    || rpc["entries"].as_array().is_some_and(|es| {
                        es.iter().any(|e| e["command"].get("membership").is_some())
                    }),
            )
        };
        if !self.config.remote && !route[0].ip().is_loopback() {
            return Err(io::Error::other("remote peer disabled"));
        }
        transport::call(
            route[0],
            &self.key,
            &json!({"group":self.config.identity,"from":self.config.node,"to":peer,"endpoint":route,"rpc":rpc}),
            dynamic || self.config.members.get(peer) != Some(&route),
        )
    }

    fn broadcast(&self, requests: Vec<(String, Value)>) -> Vec<(String, Value, Value)> {
        thread::scope(|scope| {
            let workers = requests
                .into_iter()
                .map(|(peer, request)| {
                    scope.spawn(move || {
                        let reply = self.exchange(&peer, &request).ok()?;
                        Some((peer, request, reply))
                    })
                })
                .collect::<Vec<_>>();
            workers
                .into_iter()
                .filter_map(|w| w.join().ok().flatten())
                .collect()
        })
    }

    fn replicate(&self, mut round: Option<&mut ReadRound>) -> io::Result<()> {
        let requests = {
            let core = self.lock()?;
            core.raft
                .replication_ids()
                .iter()
                .map(|peer| {
                    let rpc = match &round {
                        Some(r) => core.raft.append_for_round(peer, r)?,
                        None => core.raft.append_for(peer)?,
                    };
                    Ok((peer.clone(), rpc))
                })
                .collect::<io::Result<Vec<_>>>()?
        };
        for (peer, request, reply) in self.broadcast(requests) {
            let mut core = self.lock()?;
            let Core { raft, storage } = &mut *core;
            if raft.append_reply(&peer, &request, &reply, self.persist(storage))?
                && let Some(round) = round.as_deref_mut()
            {
                round.ack(&peer, &request, &reply);
            }
        }
        Ok(())
    }

    fn tick(&self) -> io::Result<()> {
        let request = {
            let mut core = self.lock()?;
            let Core { raft, storage } = &mut *core;
            self.storage_result(storage.retry_apply(raft.hard_state()))?;
            if storage.applied_index().map_err(io::Error::other)? < raft.commit_index() {
                raft.defer_leadership();
                return Ok(());
            }
            raft.advance_membership(self.persist(storage))?;
            if raft.election_due() {
                Some(raft.start_election(self.persist(storage))?)
            } else {
                None
            }
        };
        if let Some(request) = request {
            let requests = self
                .lock()?
                .raft
                .replication_ids()
                .iter()
                .map(|id| (id.clone(), request.clone()))
                .collect();
            for (peer, _, reply) in self.broadcast(requests) {
                let mut core = self.lock()?;
                let Core { raft, storage } = &mut *core;
                raft.vote_reply(&peer, &reply, self.persist(storage))?;
            }
        }
        if self.lock()?.raft.role() == Role::Leader {
            self.replicate(None)?;
        }
        Ok(())
    }

    fn compact(&self) -> io::Result<()> {
        let (read, index, term) = {
            let core = self.lock()?;
            let raft = &core.raft;
            if !raft.needs_snapshot()
                || core.storage.applied_index().map_err(io::Error::other)? != raft.commit_index()
            {
                return Ok(());
            }
            let index = raft.commit_index();
            (
                self.database.begin_read().map_err(io::Error::other)?,
                index,
                raft.entry(index).unwrap().term,
            )
        };
        let mut image = snapshot::Snapshot::capture(read, index, term).map_err(io::Error::other)?;
        let mut core = self.lock()?;
        let Core { raft, storage } = &mut *core;
        if raft.entry(index).is_some_and(|entry| entry.term == term) {
            image.membership = raft.configuration_at(index);
            raft.compact(image, &mut self.persist(storage))?;
        }
        Ok(())
    }

    pub(crate) fn run(self: Arc<Self>, stop: Arc<AtomicBool>) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            thread::scope(|scope| {
                scope.spawn(|| {
                    while !stop.load(Ordering::Acquire) {
                        let _ = self.tick();
                        thread::sleep(Duration::from_millis(75));
                    }
                });
                scope.spawn(|| {
                    let mut retry = Instant::now();
                    while !stop.load(Ordering::Acquire) {
                        if Instant::now() >= retry
                            && let Err(e) = self.compact()
                        {
                            eprintln!("catdb: snapshot deferred: {e}");
                            retry = Instant::now() + Duration::from_secs(10);
                        }
                        thread::sleep(Duration::from_millis(100));
                    }
                });
                let mut workers: Vec<thread::ScopedJoinHandle<'_, ()>> = Vec::new();
                while !stop.load(Ordering::Acquire) {
                    workers.retain(|w| !w.is_finished());
                    match self.listener.accept() {
                        Ok((stream, _)) if workers.len() < 16 => {
                            workers.push(scope.spawn(|| {
                                let _ = transport::serve(stream, &self.key, |request| {
                                    self.rpc(request)
                                });
                            }));
                        }
                        Ok(_) => {}
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(4))
                        }
                        Err(_) => {
                            stop.store(true, Ordering::Release);
                        }
                    }
                    if self.failed.load(Ordering::Acquire) {
                        stop.store(true, Ordering::Release);
                    }
                }
            })
        })
    }

    pub(crate) fn healthy(&self) -> bool {
        !self.failed.load(Ordering::Acquire)
    }

    fn status(&self) -> ClientResult {
        let core = self.lock().map_err(Error::Io)?;
        let raft = &core.raft;
        let role = match raft.role() {
            Role::Leader => "leader",
            Role::Follower => "follower",
            Role::Candidate => "candidate",
        };
        Ok(
            json!({"ok":true,"cluster_id":self.config.id,"node_id":self.config.node,"membership_phase":raft.membership.phase_name(),"configuration_index":raft.config_index.to_string(),"old_members":raft.membership.old,"members":raft.membership.target,"is_voter":raft.membership.voter(&self.config.node),"role":role,"leader_id":raft.leader(),"term":raft.term().to_string(),"commit_index":raft.commit_index().to_string(),"applied_index":core.storage.applied_index()?.to_string(),"snapshot_index":raft.hard_state().snapshot.index.to_string(),"log_entries":raft.hard_state().log.len(),"installed_snapshot_index":core.storage.installed_snapshot_index().to_string()}),
        )
    }

    fn barrier(&self, deadline: Instant) -> Result<MutexGuard<'_, Core>, &'static str> {
        loop {
            let round = {
                let mut core = self.lock().map_err(|_| "reopen_required")?;
                if core.raft.role() != Role::Leader {
                    return Err("not_leader");
                }
                core.raft.read_round()
            };
            if let Ok(mut round) = round {
                let _ = self.replicate(Some(&mut round));
                let core = self.lock().map_err(|_| "reopen_required")?;
                if round.ready(&core.raft) && core.raft.commit_index() == core.raft.last_index() {
                    if core
                        .storage
                        .applied_index()
                        .map_err(|_| "reopen_required")?
                        != core.raft.commit_index()
                    {
                        return Err("resource_exhausted");
                    }
                    return Ok(core);
                }
            } else {
                let _ = self.replicate(None);
            }
            if Instant::now() >= deadline {
                return Err("quorum_unavailable");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    pub(crate) fn dispatch(
        &self,
        tx: &mut Option<Transaction>,
        pinned: &mut Option<u64>,
        request: &Value,
        deadline: Instant,
    ) -> ClientResult {
        let op = request["op"].as_str().unwrap_or("");
        if op == "cluster_status" {
            return self.status();
        }
        if op == "cluster_reconfigure"
            && !self.config.admin_token.as_deref().is_some_and(|admin| {
                request["admin_token"]
                    .as_str()
                    .is_some_and(|t| crate::service::same_token(admin, t.as_bytes()))
            })
        {
            return Ok(crate::service::error("forbidden"));
        }
        if op == "rollback" {
            *pinned = None;
            return crate::service::dispatch(&self.database, tx, request);
        }
        let _operation = loop {
            if Instant::now() >= deadline {
                return Ok(crate::service::error("request_timeout"));
            }
            match self.operations.try_lock() {
                Ok(guard) => break guard,
                Err(std::sync::TryLockError::Poisoned(_)) => return Err(Error::Poisoned),
                Err(std::sync::TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(5)),
            }
        };
        let deadline = deadline.min(Instant::now() + Duration::from_millis(1250));
        let mut core = match self.barrier(deadline) {
            Ok(t) => t,
            Err(code) => {
                tx.take();
                *pinned = None;
                return Ok(crate::service::error(code));
            }
        };
        let term = core.raft.term();
        if tx.is_some() && *pinned != Some(term) {
            tx.take();
            *pinned = None;
            return Ok(crate::service::error("not_leader"));
        }
        if op == "cluster_reconfigure" {
            return self.reconfigure(core, tx.is_some(), request, deadline);
        }
        let mutating = match op {
            "put_commit" | "delete_commit" if tx.is_none() => {
                let mut local = self.database.begin()?;
                crate::service::stage_request(Some(&mut local), request, op == "put_commit")?;
                local
            }
            "commit" => tx.take().ok_or(Error::Invalid("no transaction"))?,
            "ping" | "get_latest" | "scan_latest" | "begin" | "begin_read" | "get" | "scan"
            | "put" | "delete" | "savepoint" | "rollback_to" | "release_savepoint"
            | "lock_keys" => {
                let result = crate::service::dispatch(&self.database, tx, request);
                *pinned = tx.as_ref().map(|_| term);
                return result.and_then(|mut v| {
                    if let Some(n) = v["version"].as_str().and_then(|s| s.parse().ok()) {
                        v["version"] = core.storage.logical_version(n)?.to_string().into();
                    }
                    Ok(v)
                });
            }
            "put_commit" | "delete_commit" => {
                return Err(Error::Invalid("single request requires no transaction"));
            }
            _ => return Ok(crate::service::error("unsupported_cluster_operation")),
        };
        *pinned = None;
        let version = core.storage.logical_version(mutating.version())?;
        let writes = mutating.replication_writes()?;
        if writes.is_empty() {
            return Ok(json!({"ok":true,"version":version.to_string()}));
        }
        let raw_bytes: usize = writes
            .iter()
            .map(|(key, value)| key.len() + value.as_ref().map_or(0, Vec::len))
            .sum();
        if raw_bytes > storage::MAX_COMMAND_BYTES / 2 {
            return Err(Error::Limit("replicated transaction size"));
        }
        let pairs = writes
            .iter()
            .map(|(k, v)| {
                (
                    crate::service::hex(k),
                    v.as_deref().map(crate::service::hex),
                )
            })
            .collect::<Vec<_>>();
        let command = json!({"writes":pairs});
        storage::validate_command(&command)
            .map_err(|_| Error::Limit("replicated transaction size"))?;
        let Core { raft, storage } = &mut *core;
        if Instant::now() >= deadline {
            return Ok(crate::service::error("request_timeout"));
        }
        let index = raft.propose(command, self.persist(storage)).map_err(|e| {
            if self.healthy() && e.kind() == io::ErrorKind::StorageFull {
                Error::Limit("replicated log capacity")
            } else {
                Error::CommitUncertain(e)
            }
        })?;
        drop(core);
        self.await_commit(term, index, deadline)?;
        Ok(json!({"ok":true,"version":index.to_string()}))
    }
    fn reconfigure(
        &self,
        mut core: MutexGuard<'_, Core>,
        active: bool,
        request: &Value,
        deadline: Instant,
    ) -> ClientResult {
        let term = core.raft.term();
        if active {
            return Err(Error::Invalid("membership change inside transaction"));
        }
        let target = endpoints(&request["members"], self.config.remote)
            .map_err(|_| Error::Invalid("invalid target membership"))?;
        let ids: Ids = target.keys().cloned().collect();
        let Core { raft, storage } = &mut *core;
        if request["expected_configuration_index"]
            .as_str()
            .and_then(|v| v.parse::<u64>().ok())
            != Some(raft.config_index)
        {
            return Ok(crate::service::error("configuration_changed"));
        }
        let mut routes = target;
        let mut known = raft.membership.target.clone();
        if raft.membership.phase != Phase::Stable {
            known.extend(raft.membership.old.iter().cloned());
        }
        for id in &known {
            let old = *raft
                .membership
                .routes
                .get(id)
                .or_else(|| self.config.members.get(id))
                .ok_or(Error::Invalid("missing peer route"))?;
            if routes
                .insert(id.clone(), old)
                .is_some_and(|proposed| proposed != old)
            {
                return Err(Error::Invalid("member endpoints cannot change"));
            }
        }
        validate_routes(&routes, self.config.remote)
            .map_err(|_| Error::Invalid("duplicate or invalid endpoint"))?;
        if ids == raft.membership.target {
            return Ok(
                json!({"ok":true,"phase":raft.membership.phase_name(),"configuration_index":raft.config_index.to_string()}),
            );
        }
        let cancel = raft.membership.phase == Phase::CatchUp && ids == raft.membership.old;
        if (!cancel && raft.membership.phase != Phase::Stable)
            || raft.config_index > raft.commit_index()
        {
            return Ok(crate::service::error("membership_change_in_progress"));
        }
        let index = raft
            .reconfigure(ids, routes, self.persist(storage))
            .map_err(|e| {
                if self.healthy() && e.kind() == io::ErrorKind::StorageFull {
                    Error::Limit("replicated log capacity")
                } else {
                    Error::CommitUncertain(e)
                }
            })?;
        drop(core);
        self.await_commit(term, index, deadline)?;
        Ok(json!({"ok":true,"phase":"accepted","configuration_index":index.to_string()}))
    }

    fn await_commit(&self, term: u64, index: u64, deadline: Instant) -> Result<(), Error> {
        loop {
            let _ = self.replicate(None);
            let core = self.lock().map_err(Error::CommitUncertain)?;
            if core.raft.term() != term || core.raft.role() != Role::Leader {
                return Err(Error::CommitUncertain(io::Error::other(
                    "leadership changed",
                )));
            }
            if core.raft.commit_index() >= index
                && core
                    .storage
                    .applied_index()
                    .map_err(|e| Error::CommitUncertain(io::Error::other(e)))?
                    >= index
            {
                return Ok(());
            }
            drop(core);
            if Instant::now() >= deadline {
                return Err(Error::CommitUncertain(io::Error::other(
                    "majority not confirmed",
                )));
            }
            thread::sleep(Duration::from_millis(20));
        }
    }
}
