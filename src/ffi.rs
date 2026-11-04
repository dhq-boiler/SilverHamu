use crate::{MAX_PAGE_BYTES, MAX_PAGE_RECORDS, Snapshot, create};
use std::collections::BTreeMap;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, OnceLock};
use std::{ptr, slice, str};

struct Handles {
    next: u64,
    snapshots: BTreeMap<u64, Snapshot>,
}

static HANDLES: OnceLock<Mutex<Handles>> = OnceLock::new();

fn handles() -> &'static Mutex<Handles> {
    HANDLES.get_or_init(|| {
        Mutex::new(Handles {
            next: 1,
            snapshots: BTreeMap::new(),
        })
    })
}

fn guarded(action: impl FnOnce() -> io::Result<i32>) -> i32 {
    match catch_unwind(AssertUnwindSafe(action)) {
        Ok(Ok(code)) => code,
        Ok(Err(_)) => 2,
        Err(_) => 4,
    }
}

fn bad() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, "invalid ABI argument")
}

unsafe fn input_bytes<'a>(data: *const u8, len: u64, maximum: usize) -> io::Result<&'a [u8]> {
    let len = usize::try_from(len).map_err(|_| bad())?;
    if len == 0 {
        return Ok(&[]);
    }
    if len > maximum || data.is_null() {
        return Err(bad());
    }
    Ok(unsafe { slice::from_raw_parts(data, len) })
}

#[unsafe(no_mangle)]
pub extern "C" fn ndb_abi_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_open(
    path: *const u8,
    path_len: u64,
    generation: *const u8,
    handle_out: *mut u64,
    count_out: *mut u64,
) -> i32 {
    guarded(|| {
        if handle_out.is_null() || count_out.is_null() {
            return Err(bad());
        }
        unsafe {
            *handle_out = 0;
            *count_out = 0;
        }
        let path =
            str::from_utf8(unsafe { input_bytes(path, path_len, 32768)? }).map_err(|_| bad())?;
        let generation: [u8; 16] = unsafe { input_bytes(generation, 16, 16)? }
            .try_into()
            .map_err(|_| bad())?;
        let snapshot = Snapshot::open(path, generation)?;
        let mut state = handles().lock().map_err(|_| bad())?;
        let id = state.next;
        state.next = id.checked_add(1).ok_or_else(bad)?;
        unsafe {
            *count_out = snapshot.len();
        }
        state.snapshots.insert(id, snapshot);
        unsafe {
            *handle_out = id;
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_read_page(
    handle: u64,
    start: u64,
    limit: u32,
    output: *mut u8,
    capacity: u64,
    bytes_out: *mut u64,
    count_out: *mut u64,
) -> i32 {
    guarded(|| {
        if bytes_out.is_null() || count_out.is_null() || limit as usize > MAX_PAGE_RECORDS {
            return Err(bad());
        }
        unsafe {
            *bytes_out = 0;
            *count_out = 0;
        }
        let mut state = handles().lock().map_err(|_| bad())?;
        let snapshot = state.snapshots.get_mut(&handle).ok_or_else(bad)?;
        let records = snapshot.read_page(start, limit as usize)?;
        let required: usize = records.iter().map(|r| r.len() + 4).sum();
        unsafe {
            *bytes_out = required as u64;
            *count_out = records.len() as u64;
        }
        if capacity < required as u64 {
            return Ok(3);
        }
        if required == 0 {
            return Ok(0);
        }
        if output.is_null() || capacity > (MAX_PAGE_BYTES + MAX_PAGE_RECORDS * 4) as u64 {
            return Err(bad());
        }
        let mut offset = 0;
        for record in records {
            let len = u32::try_from(record.len())
                .map_err(|_| bad())?
                .to_le_bytes();
            unsafe {
                ptr::copy_nonoverlapping(len.as_ptr(), output.add(offset), 4);
                ptr::copy_nonoverlapping(record.as_ptr(), output.add(offset + 4), record.len());
            }
            offset += record.len() + 4;
        }
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn ndb_close(handle: u64) -> i32 {
    guarded(|| {
        handles()
            .lock()
            .map_err(|_| bad())?
            .snapshots
            .remove(&handle)
            .ok_or_else(bad)?;
        Ok(0)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ndb_create(
    path: *const u8,
    path_len: u64,
    generation: *const u8,
    framed: *const u8,
    framed_len: u64,
    count: u64,
) -> i32 {
    guarded(|| {
        if count > 1_000_000 {
            return Err(bad());
        }
        let path =
            str::from_utf8(unsafe { input_bytes(path, path_len, 32768)? }).map_err(|_| bad())?;
        let generation: [u8; 16] = unsafe { input_bytes(generation, 16, 16)? }
            .try_into()
            .map_err(|_| bad())?;
        let bytes = unsafe { input_bytes(framed, framed_len, 256 * 1024 * 1024)? };
        let mut records = Vec::new();
        let mut offset = 0_usize;
        for _ in 0..count {
            let len_bytes = bytes.get(offset..offset + 4).ok_or_else(bad)?;
            let len = u32::from_le_bytes(len_bytes.try_into().map_err(|_| bad())?) as usize;
            offset += 4;
            let end = offset.checked_add(len).ok_or_else(bad)?;
            records.push(bytes.get(offset..end).ok_or_else(bad)?);
            offset = end;
        }
        if offset != bytes.len() {
            return Err(bad());
        }
        create(path, generation, &records)?;
        Ok(0)
    })
}
