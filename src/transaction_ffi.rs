use crate::transactions::{
    ConflictKind, Database, Error, MAX_KEY_BYTES, MAX_VALUE_BYTES, ResourceLimits, ScanLimits,
    Transaction,
};
use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, OnceLock};
use std::{ptr, slice, str};

type Cell = Arc<Mutex<Option<Transaction>>>;
#[derive(Default)]
struct Registry {
    next: u64,
    databases: BTreeMap<u64, Database>,
    transactions: BTreeMap<u64, Cell>,
}
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(|| {
        Mutex::new(Registry {
            next: 1,
            ..Registry::default()
        })
    })
}
fn code(error: Error) -> i32 {
    match error {
        Error::Io(_) => 2,
        Error::CommitUncertain(_) => 20,
        Error::CheckpointUncertain(_) => 25,
        Error::Conflict(ConflictKind::Write) => 10,
        Error::Conflict(ConflictKind::Read) => 11,
        Error::Conflict(ConflictKind::Range) => 12,
        Error::Poisoned => 21,
        Error::Limit(_) => 22,
        Error::ResourceExhausted(_) => 26,
        Error::Locked(_) => 23,
        Error::Invalid(_) => 24,
    }
}
fn guard(action: impl FnOnce() -> Result<i32, i32>) -> i32 {
    match catch_unwind(AssertUnwindSafe(action)) {
        Ok(Ok(code)) => code,
        Ok(Err(code)) => code,
        Err(_) => 4,
    }
}
fn database(id: u64) -> Result<Database, i32> {
    registry()
        .lock()
        .map_err(|_| 21)?
        .databases
        .get(&id)
        .cloned()
        .ok_or(1)
}
fn transaction(id: u64) -> Result<Cell, i32> {
    registry()
        .lock()
        .map_err(|_| 21)?
        .transactions
        .get(&id)
        .cloned()
        .ok_or(1)
}
fn with_transaction<R>(
    id: u64,
    action: impl FnOnce(&mut Transaction) -> crate::transactions::Result<R>,
) -> Result<R, i32> {
    let cell = transaction(id)?;
    let mut entry = cell.lock().map_err(|_| 21)?;
    action(entry.as_mut().ok_or(1)?).map_err(code)
}
fn next(registry: &mut Registry) -> Result<u64, i32> {
    let id = registry.next;
    registry.next = id.checked_add(1).ok_or(22)?;
    Ok(id)
}
unsafe fn bytes<'a>(pointer: *const u8, length: u64, maximum: usize) -> Result<&'a [u8], i32> {
    let length = usize::try_from(length).map_err(|_| 1)?;
    if length > maximum {
        return Err(1);
    }
    if length == 0 {
        return Ok(&[]);
    }
    if pointer.is_null() {
        return Err(1);
    }
    Ok(unsafe { slice::from_raw_parts(pointer, length) })
}
unsafe fn path<'a>(pointer: *const u8, length: u64) -> Result<&'a str, i32> {
    let value = str::from_utf8(unsafe { bytes(pointer, length, 32768)? }).map_err(|_| 1)?;
    if value.is_empty() {
        return Err(1);
    }
    Ok(value)
}
unsafe fn reset(pointer: *mut u64) -> Result<(), i32> {
    if pointer.is_null() {
        return Err(1);
    }
    unsafe {
        *pointer = 0;
    }
    Ok(())
}
unsafe fn output(
    data: &[u8],
    pointer: *mut u8,
    capacity: u64,
    required: *mut u64,
) -> Result<i32, i32> {
    unsafe {
        *required = data.len() as u64;
    }
    if capacity < data.len() as u64 {
        return Ok(3);
    }
    if !data.is_empty() {
        if pointer.is_null() {
            return Err(1);
        }
        unsafe {
            ptr::copy_nonoverlapping(data.as_ptr(), pointer, data.len());
        }
    }
    Ok(0)
}
unsafe fn end<'a>(pointer: *const u8, length: u64, has_end: u32) -> Result<Option<&'a [u8]>, i32> {
    match has_end {
        0 if length == 0 => Ok(None),
        1 => Ok(Some(unsafe { bytes(pointer, length, MAX_KEY_BYTES)? })),
        _ => Err(1),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn ndb_tx_abi_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn ndb_tx_resource_api_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn ndb_tx_set_checkpoint_range_writeback(id: u64, enabled: u32) -> i32 {
    guard(|| {
        if enabled > 1 {
            return Err(1);
        }
        database(id)?.set_checkpoint_range_writeback(enabled == 1).map_err(|error| {
            if matches!(&error, Error::Io(e) if e.kind() == std::io::ErrorKind::Unsupported) {
                27
            } else {
                code(error)
            }
        })?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn ndb_tx_set_checkpoint_retirement(id: u64, enabled: u32, max_bytes: u64) -> i32 {
    guard(|| {
        if enabled > 1 {
            return Err(1);
        }
        database(id)?
            .set_checkpoint_retirement(enabled == 1, max_bytes)
            .map_err(code)?;
        Ok(0)
    })
}

/// # Safety
/// `output` must reference eight writable u64 values.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_checkpoint_retirement_status(id: u64, output: *mut u64) -> i32 {
    guard(|| {
        if output.is_null() {
            return Err(1);
        }
        let output = unsafe { slice::from_raw_parts_mut(output, 8) };
        output.fill(0);
        let status = database(id)?.checkpoint_retirement_status().map_err(code)?;
        output.copy_from_slice(&[
            u64::from(status.enabled),
            status.max_bytes,
            status.retained,
            status.reclaimed,
            status.fallbacks,
            status.link_ns,
            status.reclaim_ns,
            u64::from(status.last_error.is_some()),
        ]);
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn ndb_tx_set_group_commit(id: u64, enabled: u32) -> i32 {
    guard(|| {
        if enabled > 1 {
            return Err(1);
        }
        database(id)?.set_group_commit(enabled == 1);
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_commit_sync_attempts(id: u64, output: *mut u64) -> i32 {
    guard(|| {
        unsafe { reset(output)? };
        let attempts = database(id)?.commit_sync_attempts();
        unsafe { *output = attempts };
        Ok(0)
    })
}

unsafe fn resource_limits(pointer: *const u64) -> Result<ResourceLimits, i32> {
    if pointer.is_null() {
        return Err(1);
    }
    let values = unsafe { slice::from_raw_parts(pointer, 5) };
    let mut limits = [0usize; 5];
    for (index, value) in values.iter().enumerate() {
        limits[index] = usize::try_from(*value).map_err(|_| 1)?;
        if limits[index] == 0 {
            return Err(1);
        }
    }
    Ok(ResourceLimits {
        max_transactions: limits[0],
        max_staged_bytes: limits[1],
        max_dependency_bytes: limits[2],
        max_retained_bytes: limits[3],
        max_buffer_bytes: limits[4],
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_db_create_limited(
    p: *const u8,
    n: u64,
    identity: *const u8,
    limits: *const u64,
    out: *mut u64,
) -> i32 {
    guard(|| {
        unsafe {
            reset(out)?;
        }
        let limits = unsafe { resource_limits(limits)? };
        let id = unsafe { bytes(identity, 16, 16)? }
            .try_into()
            .map_err(|_| 1)?;
        let db = Database::create_with_limits(unsafe { path(p, n)? }, id, limits).map_err(code)?;
        let mut registry = registry().lock().map_err(|_| 21)?;
        let handle = next(&mut registry)?;
        registry.databases.insert(handle, db);
        unsafe {
            *out = handle;
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_db_open_limited(
    p: *const u8,
    n: u64,
    limits: *const u64,
    out: *mut u64,
) -> i32 {
    guard(|| {
        unsafe {
            reset(out)?;
        }
        let db =
            Database::open_with_limits(unsafe { path(p, n)? }, unsafe { resource_limits(limits)? })
                .map_err(code)?;
        let mut registry = registry().lock().map_err(|_| 21)?;
        let handle = next(&mut registry)?;
        registry.databases.insert(handle, db);
        unsafe {
            *out = handle;
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_db_resources(id: u64, output: *mut u64) -> i32 {
    guard(|| {
        if output.is_null() {
            return Err(1);
        }
        let usage = database(id)?.resource_usage().map_err(code)?;
        let mut values = [0u64; 15];
        for index in 0..5 {
            values[index] = usage.current[index] as u64;
            values[index + 5] = usage.peak[index] as u64;
            values[index + 10] = usage.limits.values()[index] as u64;
        }
        unsafe {
            ptr::copy_nonoverlapping(values.as_ptr(), output, 15);
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_scan_page(
    id: u64,
    start: *const u8,
    start_len: u64,
    finish: *const u8,
    finish_len: u64,
    has_end: u32,
    after: *const u8,
    after_len: u64,
    has_after: u32,
    max_rows: u64,
    max_bytes: u64,
    max_visits: u64,
    buffer: *mut u8,
    capacity: u64,
    required: *mut u64,
) -> i32 {
    guard(|| {
        unsafe {
            reset(required)?;
        }
        if capacity > 0 && buffer.is_null() {
            return Err(1);
        }
        let start = unsafe { bytes(start, start_len, MAX_KEY_BYTES)? };
        let end = unsafe { end(finish, finish_len, has_end)? };
        let after = unsafe { self::end(after, after_len, has_after)? };
        let limits = ScanLimits {
            max_rows: usize::try_from(max_rows).map_err(|_| 1)?,
            max_bytes: usize::try_from(max_bytes).map_err(|_| 1)?,
            max_visits: usize::try_from(max_visits).map_err(|_| 1)?,
        };
        let cell = transaction(id)?;
        let mut entry = cell.lock().map_err(|_| 21)?;
        let tx = entry.as_mut().ok_or(1)?;
        let page = tx.scan_page(start, end, after, limits).map_err(code)?;
        let cursor = page.next_after.as_deref().unwrap_or_default();
        let length = 40 + cursor.len() + page.rows.len() * 8 + page.payload_bytes;
        unsafe {
            *required = length as u64;
        }
        if capacity < length as u64 {
            return Ok(3);
        }
        let target = unsafe { slice::from_raw_parts_mut(buffer, length) };
        for (index, value) in [
            page.rows.len() as u64,
            page.visited_entries as u64,
            page.payload_bytes as u64,
            u64::from(page.next_after.is_some()),
            cursor.len() as u64,
        ]
        .iter()
        .enumerate()
        {
            target[index * 8..index * 8 + 8].copy_from_slice(&value.to_le_bytes());
        }
        let mut position = 40;
        target[position..position + cursor.len()].copy_from_slice(cursor);
        position += cursor.len();
        for (key, value) in &page.rows {
            target[position..position + 4].copy_from_slice(&(key.len() as u32).to_le_bytes());
            target[position + 4..position + 8].copy_from_slice(&(value.len() as u32).to_le_bytes());
            position += 8;
            target[position..position + key.len()].copy_from_slice(key);
            position += key.len();
            target[position..position + value.len()].copy_from_slice(value);
            position += value.len();
        }
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_db_create(
    p: *const u8,
    n: u64,
    id: *const u8,
    out: *mut u64,
) -> i32 {
    unsafe { create_db(p, n, id, out, |path, id| Database::create(path, id)) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_db_create_append(
    p: *const u8,
    n: u64,
    id: *const u8,
    out: *mut u64,
) -> i32 {
    unsafe { create_db(p, n, id, out, |path, id| Database::create_append(path, id)) }
}

unsafe fn create_db(
    p: *const u8,
    n: u64,
    id: *const u8,
    out: *mut u64,
    create: impl FnOnce(&str, [u8; 16]) -> crate::transactions::Result<Database>,
) -> i32 {
    guard(|| {
        unsafe {
            reset(out)?;
        }
        let path = unsafe { path(p, n)? };
        let id = unsafe { bytes(id, 16, 16)? }.try_into().map_err(|_| 1)?;
        let db = create(path, id).map_err(code)?;
        let mut r = registry().lock().map_err(|_| 21)?;
        let handle = next(&mut r)?;
        r.databases.insert(handle, db);
        unsafe {
            *out = handle;
        }
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_db_open(p: *const u8, n: u64, out: *mut u64) -> i32 {
    guard(|| {
        unsafe {
            reset(out)?;
        }
        let db = Database::open(unsafe { path(p, n)? }).map_err(code)?;
        let mut r = registry().lock().map_err(|_| 21)?;
        let id = next(&mut r)?;
        r.databases.insert(id, db);
        unsafe {
            *out = id;
        }
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn ndb_tx_db_close(id: u64) -> i32 {
    guard(|| {
        let db = registry()
            .lock()
            .map_err(|_| 21)?
            .databases
            .remove(&id)
            .ok_or(1)?;
        drop(db);
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_db_info(id: u64, identity: *mut u8, stats: *mut u64) -> i32 {
    guard(|| {
        if identity.is_null() || stats.is_null() {
            return Err(1);
        }
        let db = database(id)?;
        let s = db.stats().map_err(code)?;
        let values = [
            s.version,
            s.visible_records as u64,
            s.retained_versions as u64,
            s.active_transactions as u64,
            s.wal_bytes,
        ];
        unsafe {
            ptr::copy_nonoverlapping(db.id().as_ptr(), identity, 16);
            ptr::copy_nonoverlapping(values.as_ptr(), stats, 5);
        }
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_begin(id: u64, out: *mut u64, version: *mut u64) -> i32 {
    guard(|| {
        unsafe {
            reset(out)?;
            reset(version)?;
        }
        let tx = database(id)?.begin().map_err(code)?;
        let snapshot = tx.version();
        let mut r = registry().lock().map_err(|_| 21)?;
        let handle = next(&mut r)?;
        r.transactions
            .insert(handle, Arc::new(Mutex::new(Some(tx))));
        unsafe {
            *out = handle;
            *version = snapshot;
        }
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_get(
    id: u64,
    key: *const u8,
    key_len: u64,
    buffer: *mut u8,
    capacity: u64,
    required: *mut u64,
    found: *mut u64,
) -> i32 {
    guard(|| {
        unsafe {
            reset(required)?;
            reset(found)?;
        }
        let key = unsafe { bytes(key, key_len, MAX_KEY_BYTES)? };
        let value = with_transaction(id, |tx| tx.get(key))?;
        match value {
            None => Ok(0),
            Some(value) => unsafe {
                *found = 1;
                output(&value, buffer, capacity, required)
            },
        }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_put(
    id: u64,
    key: *const u8,
    key_len: u64,
    value: *const u8,
    value_len: u64,
) -> i32 {
    guard(|| {
        let key = unsafe { bytes(key, key_len, MAX_KEY_BYTES)? };
        let value = unsafe { bytes(value, value_len, MAX_VALUE_BYTES)? };
        with_transaction(id, |tx| tx.put(key, value))?;
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_delete(id: u64, key: *const u8, key_len: u64) -> i32 {
    guard(|| {
        let key = unsafe { bytes(key, key_len, MAX_KEY_BYTES)? };
        with_transaction(id, |tx| tx.delete(key))?;
        Ok(0)
    })
}
/// Reserves keys encoded as repeated little-endian u16 lengths followed by key bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_lock_keys(id: u64, frame: *const u8, len: u64) -> i32 {
    guard(|| {
        let mut frame = unsafe { bytes(frame, len, 4096 * (MAX_KEY_BYTES + 2))? };
        let mut keys = Vec::new();
        while !frame.is_empty() {
            let (length, rest) = frame.split_at_checked(2).ok_or(1)?;
            let length = usize::from(u16::from_le_bytes([length[0], length[1]]));
            let (key, rest) = rest.split_at_checked(length).ok_or(1)?;
            if keys.len() == 4096 || key.len() > MAX_KEY_BYTES { return Err(1); }
            keys.push(key);
            frame = rest;
        }
        with_transaction(id, |tx| tx.lock_keys(&keys))?;
        Ok(0)
    })
}
/// Operation 0 creates a mark, 1 rolls back to it, and 2 releases it. Names are UTF-8 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_savepoint(id: u64, operation: u32, name: *const u8, len: u64) -> i32 {
    guard(|| {
        let name = str::from_utf8(unsafe { bytes(name, len, 512)? }).map_err(|_| 1)?;
        with_transaction(id, |tx| match operation {
            0 => tx.savepoint(name),
            1 => tx.rollback_to_savepoint(name),
            2 => tx.release_savepoint(name),
            _ => Err(Error::Invalid("unknown savepoint operation")),
        })?;
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_scan(
    id: u64,
    start: *const u8,
    start_len: u64,
    finish: *const u8,
    finish_len: u64,
    has_end: u32,
    buffer: *mut u8,
    capacity: u64,
    required: *mut u64,
    count: *mut u64,
) -> i32 {
    guard(|| {
        unsafe {
            reset(required)?;
            reset(count)?;
        }
        let start = unsafe { bytes(start, start_len, MAX_KEY_BYTES)? };
        let end = unsafe { end(finish, finish_len, has_end)? };
        let mut scan = with_transaction(id, |tx| tx.scan_shared(start, end, true))?;
        let length = scan.iter().try_fold(0_usize, |total, (k, v)| {
            total
                .checked_add(8 + k.len() + v.len())
                .filter(|n| *n <= 64 * 1024 * 1024)
                .ok_or(22)
        })?;
        unsafe {
            *required = length as u64;
            *count = scan.rows.len() as u64;
        }
        if capacity < length as u64 {
            return Ok(3);
        }
        if length > 0 && buffer.is_null() {
            return Err(1);
        }
        scan.permit.add(length).map_err(code)?;
        let mut framed = Vec::with_capacity(length);
        for (k, v) in scan.iter() {
            framed.extend_from_slice(&(k.len() as u32).to_le_bytes());
            framed.extend_from_slice(&(v.len() as u32).to_le_bytes());
            framed.extend_from_slice(k);
            framed.extend_from_slice(v);
        }
        unsafe { output(&framed, buffer, capacity, required) }
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_commit(id: u64, version: *mut u64) -> i32 {
    guard(|| {
        unsafe {
            reset(version)?;
        }
        let cell = registry()
            .lock()
            .map_err(|_| 21)?
            .transactions
            .remove(&id)
            .ok_or(1)?;
        let tx = cell.lock().map_err(|_| 21)?.take().ok_or(1)?;
        let v = tx.commit().map_err(code)?;
        unsafe {
            *version = v;
        }
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn ndb_tx_rollback(id: u64) -> i32 {
    guard(|| {
        let cell = registry()
            .lock()
            .map_err(|_| 21)?
            .transactions
            .remove(&id)
            .ok_or(1)?;
        let tx = cell.lock().map_err(|_| 21)?.take().ok_or(1)?;
        tx.rollback();
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn ndb_tx_reclaim_checkpoint(id: u64) -> i32 {
    guard(|| {
        database(id)?.reclaim_checkpoint().map_err(code)?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_checkpoint_deferred(
    id: u64,
    version: *mut u64,
    records: *mut u64,
    before: *mut u64,
    after: *mut u64,
) -> i32 {
    guard(|| {
        unsafe {
            reset(version)?;
            reset(records)?;
            reset(before)?;
            reset(after)?;
        }
        let info = database(id)?.checkpoint_deferred_reclaim().map_err(code)?;
        unsafe {
            *version = info.version;
            *records = info.records as u64;
            *before = info.before_bytes;
            *after = info.after_bytes;
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_checkpoint(
    id: u64,
    version: *mut u64,
    records: *mut u64,
    before: *mut u64,
    after: *mut u64,
) -> i32 {
    guard(|| {
        unsafe {
            reset(version)?;
            reset(records)?;
            reset(before)?;
            reset(after)?;
        }
        let info = database(id)?.checkpoint().map_err(code)?;
        unsafe {
            *version = info.version;
            *records = info.records as u64;
            *before = info.before_bytes;
            *after = info.after_bytes;
        }
        Ok(0)
    })
}
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_backup(
    id: u64,
    destination: *const u8,
    destination_len: u64,
    version: *mut u64,
    records: *mut u64,
) -> i32 {
    guard(|| {
        unsafe {
            reset(version)?;
            reset(records)?;
        }
        let destination = unsafe { path(destination, destination_len)? };
        let info = database(id)?.backup(destination).map_err(code)?;
        unsafe {
            *version = info.version;
            *records = info.records as u64;
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_restore(
    source: *const u8,
    source_len: u64,
    destination: *const u8,
    destination_len: u64,
    version: *mut u64,
    records: *mut u64,
) -> i32 {
    guard(|| {
        unsafe {
            reset(version)?;
            reset(records)?;
        }
        let source = unsafe { path(source, source_len)? };
        let destination = unsafe { path(destination, destination_len)? };
        let info = Database::restore(source, destination).map_err(code)?;
        unsafe {
            *version = info.version;
            *records = info.records as u64;
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_tx_export(
    id: u64,
    p: *const u8,
    n: u64,
    generation: *const u8,
    start: *const u8,
    start_len: u64,
    finish: *const u8,
    finish_len: u64,
    has_end: u32,
    version: *mut u64,
    count: *mut u64,
) -> i32 {
    guard(|| {
        unsafe {
            reset(version)?;
            reset(count)?;
        }
        let path = unsafe { path(p, n)? };
        let generation = unsafe { bytes(generation, 16, 16)? }
            .try_into()
            .map_err(|_| 1)?;
        let start = unsafe { bytes(start, start_len, MAX_KEY_BYTES)? };
        let end = unsafe { end(finish, finish_len, has_end)? };
        let info = database(id)?
            .export_values(path, generation, start, end)
            .map_err(code)?;
        unsafe {
            *version = info.version;
            *count = info.records as u64;
        }
        Ok(0)
    })
}
