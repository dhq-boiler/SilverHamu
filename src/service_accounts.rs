use super::{Config, Database, ResourceLimits, error};
use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, BufReader, Read, Write};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

type Failure = Box<dyn std::error::Error>;
const ROOT: &[u8] = b"v1/bootstrap-principal";

pub struct AccountOptions {
    pub certificate: PathBuf,
    pub private_key: PathBuf,
    pub bootstrap: Option<(String, String)>,
    pub attachments: Vec<(String, PathBuf)>,
}

pub(super) fn catalog_path(db: &Path) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(".accounts");
    name.into()
}

pub(super) fn owner_path(db: &Path) -> PathBuf {
    let mut name = db.as_os_str().to_owned();
    name.push(".account-owner");
    name.into()
}

fn bind_owner(db: &Path, owner: &[u8]) -> Result<(), Failure> {
    let path = owner_path(db);
    match File::options().write(true).create_new(true).open(&path) {
        Ok(mut f) => {
            f.write_all(owner)?;
            f.sync_all()?;
            #[cfg(unix)]
            File::open(
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?
            .sync_all()?;
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            let mut stored = Vec::new();
            File::open(path)?.take(17).read_to_end(&mut stored)?;
            if stored != owner {
                return Err("database belongs to another account catalog".into());
            }
        }
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn valid_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

fn valid_password(s: &str) -> bool {
    s.len() <= 1024 && s.chars().count() >= 15 && !s.contains('\0')
}

fn random_id() -> io::Result<[u8; 16]> {
    let mut id = [0; 16];
    getrandom::getrandom(&mut id).map_err(|_| io::Error::other("secure randomness unavailable"))?;
    Ok(id)
}

fn hash_password(password: &str) -> Result<String, Failure> {
    let salt = SaltString::encode_b64(&random_id()?).map_err(|_| "salt encoding failed")?;
    Ok(Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|_| "password hashing failed")?
        .to_string())
}

fn username(name: &str) -> Vec<u8> {
    [b"name/".as_slice(), name.as_bytes()].concat()
}
fn credential(id: &[u8]) -> Vec<u8> {
    [b"credential/password/".as_slice(), id].concat()
}
fn grant(database: &Database, id: &[u8]) -> Vec<u8> {
    [b"grant/".as_slice(), &database.id(), id].concat()
}

#[derive(Copy, Clone, Eq, PartialEq, PartialOrd, Ord)]
pub(super) enum Role {
    Reader = 1,
    Writer,
    Admin,
}
impl Role {
    fn parse(s: &str) -> Option<Self> {
        match s {
            "reader" => Some(Self::Reader),
            "writer" => Some(Self::Writer),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
    pub(super) fn permits(self, op: &str) -> bool {
        match op {
            "ping" | "begin" | "begin_read" | "get" | "scan" | "get_latest" | "scan_latest"
            | "commit" | "rollback" | "savepoint" | "rollback_to" | "release_savepoint" => true,
            "put" | "delete" | "put_commit" | "delete_commit" | "put_batch_commit" | "lock_keys" => {
                self >= Self::Writer
            }
            "checkpoint" | "create_user" | "set_role" | "sql" => self == Self::Admin,
            _ => false,
        }
    }
}

pub(super) struct Session {
    pub(super) id: Vec<u8>,
    pub role: Role,
    stamp: Vec<u8>,
    epoch: u64,
}
pub(super) struct Accounts {
    pub tls: Arc<rustls::ServerConfig>,
    dbs: HashMap<String, Database>,
    catalog: Database,
    root: Vec<u8>,
    dummy_hash: String,
    hashing: AtomicUsize,
    epoch: AtomicU64,
    reopen_required: AtomicBool,
    failures: Mutex<HashMap<IpAddr, (Instant, u8)>>,
}

struct HashPermit<'a>(&'a AtomicUsize);
impl Drop for HashPermit<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Accounts {
    pub fn open(
        config: &Config,
        primary: &Database,
        limits: ResourceLimits,
        options: AccountOptions,
    ) -> Result<Self, Failure> {
        let certs = rustls_pemfile::certs(&mut BufReader::new(File::open(options.certificate)?))
            .collect::<Result<Vec<_>, _>>()?;
        let key =
            rustls_pemfile::private_key(&mut BufReader::new(File::open(options.private_key)?))?
                .ok_or("missing TLS private key")?;
        let tls = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certs, key)?;
        let mut dbs = HashMap::from([(String::from("main"), primary.clone())]);
        for (name, path) in &options.attachments {
            if !valid_name(name)
                || dbs.contains_key(name)
                || path
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("accounts"))
                || catalog_path(path).try_exists()?
            {
                return Err("invalid attachment or independent account catalog".into());
            }
            let db = Database::open_with_limits(path, limits)?;
            if dbs.values().any(|d| d.id() == db.id()) {
                return Err("duplicate database identity".into());
            }
            db.set_group_commit(true);
            db.set_group_commit_wake_on_four(true);
            dbs.insert(name.clone(), db);
        }
        let path = catalog_path(&config.database);
        let catalog = if path.try_exists()? {
            Database::open_with_limits(&path, ResourceLimits::default())?
        } else {
            if options.bootstrap.is_none() || owner_path(&config.database).try_exists()? {
                return Err("missing account catalog; bootstrap locally".into());
            }
            let db = Database::create_append_with_limits(
                &path,
                primary.id(),
                ResourceLimits::default(),
            )?;
            #[cfg(unix)]
            File::open(
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?
            .sync_all()?;
            db
        };
        if catalog.id() != primary.id() {
            return Err("account catalog belongs to a different database".into());
        }
        let mut tx = catalog.begin()?;
        let root = match (tx.get(ROOT)?, options.bootstrap) {
            (Some(id), None) if id.len() == 16 => id,
            (None, Some((name, password)))
                if tx.version() == 0 && valid_name(&name) && valid_password(&password) =>
            {
                let id = random_id()?;
                tx.put(&username(&name), &id)?;
                tx.put(&credential(&id), hash_password(&password)?.as_bytes())?;
                tx.put(ROOT, &id)?;
                tx.commit()?;
                id.to_vec()
            }
            _ => return Err("invalid bootstrap or an existing administrator".into()),
        };
        bind_owner(&config.database, &root)?;
        for (_, path) in &options.attachments {
            bind_owner(path, &root)?;
        }
        Ok(Self {
            tls: Arc::new(tls),
            catalog,
            root,
            dbs,
            dummy_hash: hash_password("unused verification input")?,
            hashing: AtomicUsize::new(0),
            epoch: AtomicU64::new(0),
            reopen_required: AtomicBool::new(false),
            failures: Mutex::new(HashMap::new()),
        })
    }
    fn permit(&self) -> Option<HashPermit<'_>> {
        if self.hashing.fetch_add(1, Ordering::AcqRel) < 2 {
            Some(HashPermit(&self.hashing))
        } else {
            self.hashing.fetch_sub(1, Ordering::AcqRel);
            None
        }
    }
    fn stamp(
        &self,
        tx: &mut crate::transactions::Transaction,
        db: &Database,
        id: &[u8],
    ) -> Result<Option<Vec<u8>>, crate::transactions::Error> {
        if id == self.root {
            Ok(Some(vec![Role::Admin as u8]))
        } else {
            tx.get(&grant(db, id))
        }
    }
    pub fn login(&self, auth: &Value, peer: IpAddr) -> Option<(Database, Session)> {
        if self.reopen_required.load(Ordering::Acquire) {
            return None;
        }
        let window = Duration::from_secs(60);
        {
            let mut failures = self.failures.lock().ok()?;
            if failures.len() >= 1024 {
                failures.retain(|_, (start, _)| start.elapsed() < window);
            }
            match failures.get(&peer) {
                Some((start, n)) if start.elapsed() < window && *n >= 8 => return None,
                None if failures.len() >= 1024 => return None,
                _ => {}
            }
        }
        // Capacity refusals must not spend the peer's failed-password allowance.
        let _permit = self.permit()?;
        let session = self.verify(auth);
        if session.is_none() {
            let mut failures = self.failures.lock().ok()?;
            if failures.len() < 1024 || failures.contains_key(&peer) {
                let entry = failures.entry(peer).or_insert((Instant::now(), 0));
                if entry.0.elapsed() >= window {
                    *entry = (Instant::now(), 0);
                }
                entry.1 = entry.1.saturating_add(1);
            }
        }
        session
    }
    fn verify(&self, auth: &Value) -> Option<(Database, Session)> {
        if auth.get("op")?.as_str()? != "auth"
            || auth.get("version")?.as_u64()? != 2
            || auth.get("token").is_some()
        {
            return None;
        }
        let name = auth.get("username")?.as_str()?;
        let password = auth.get("password")?.as_str()?;
        if !valid_name(name) || !valid_password(password) {
            return None;
        }
        let epoch = self.epoch.load(Ordering::Acquire);
        let mut tx = self.catalog.begin_read().ok()?;
        let id = tx.get(&username(name)).ok()?;
        let hash = match &id {
            Some(id) => String::from_utf8(tx.get(&credential(id)).ok()??).ok()?,
            None => self.dummy_hash.clone(),
        };
        Argon2::default()
            .verify_password(password.as_bytes(), &PasswordHash::new(&hash).ok()?)
            .ok()?;
        let id = id?;
        let name = match auth.get("database") {
            None => "main",
            Some(n) => n.as_str()?,
        };
        let db = self.dbs.get(name)?.clone();
        let stamp = self.stamp(&mut tx, &db, &id).ok()??;
        let role = match stamp.first()? {
            1 => Role::Reader,
            2 => Role::Writer,
            3 => Role::Admin,
            _ => return None,
        };
        Some((
            db,
            Session {
                id,
                role,
                stamp,
                epoch,
            },
        ))
    }
    pub fn current(&self, db: &Database, s: &mut Session) -> bool {
        if self.reopen_required.load(Ordering::Acquire) {
            return false;
        }
        let epoch = self.epoch.load(Ordering::Acquire);
        if s.epoch == epoch {
            return true;
        }
        let current = self
            .catalog
            .begin_read()
            .and_then(|mut tx| self.stamp(&mut tx, db, &s.id))
            .is_ok_and(|v| v.as_ref() == Some(&s.stamp));
        if current {
            s.epoch = epoch;
        }
        current
    }
    pub fn manage(&self, db: &Database, s: &Session, req: &Value) -> Value {
        use crate::transactions::Error;
        self.change(db, s, req)
            .unwrap_or_else(|e| match e.downcast_ref::<Error>() {
                Some(Error::Conflict(_)) => error("conflict"),
                Some(
                    Error::Poisoned | Error::CommitUncertain(_) | Error::CheckpointUncertain(_),
                ) => {
                    self.reopen_required.store(true, Ordering::Release);
                    error("account_store_reopen_required")
                }
                _ => error("account_store_error"),
            })
    }
    fn change(&self, db: &Database, s: &Session, req: &Value) -> Result<Value, Failure> {
        let name = req["username"].as_str().unwrap_or("");
        let role = Role::parse(req["role"].as_str().unwrap_or(""));
        let create = req["op"] == "create_user";
        if !valid_name(name)
            || (role.is_none() && (create || req["role"] != "none"))
            || req.get("database").is_some()
        {
            return Ok(error("invalid_request"));
        }
        let mut tx = self.catalog.begin()?;
        if s.role != Role::Admin || self.stamp(&mut tx, db, &s.id)?.as_ref() != Some(&s.stamp) {
            return Ok(error("forbidden"));
        }
        let existing = tx.get(&username(name))?;
        let id = if create {
            if existing.is_some() {
                return Ok(error("user_exists"));
            }
            let password = req["password"].as_str().unwrap_or("");
            if !valid_password(password) {
                return Ok(error("invalid_request"));
            }
            let Some(_permit) = self.permit() else {
                return Ok(error("busy"));
            };
            let id = random_id()?.to_vec();
            tx.put(&username(name), &id)?;
            tx.put(&credential(&id), hash_password(password)?.as_bytes())?;
            id
        } else {
            let Some(id) = existing else {
                return Ok(error("unknown_user"));
            };
            if id == self.root || id == s.id {
                return Ok(error("forbidden"));
            }
            id
        };
        let key = grant(db, &id);
        if let Some(role) = role {
            let mut stamp = vec![role as u8];
            stamp.extend_from_slice(&random_id()?);
            tx.put(&key, &stamp)?;
        } else {
            tx.delete(&key)?;
        }
        tx.commit()?;
        self.epoch.fetch_add(1, Ordering::Release);
        Ok(json!({"ok":true}))
    }
}
