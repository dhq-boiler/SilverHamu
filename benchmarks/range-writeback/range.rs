use super::HEADER_BYTES;
use std::{fs::File, io, os::fd::AsRawFd};

const INTERVAL: i64 = 8 * 1024 * 1024;
const SYNC_FILE_RANGE_WRITE: u32 = 2;
pub(super) struct Pending {
    written: i64,
    submitted: i64,
}

impl Pending {
    pub(super) fn configured() -> Option<Self> {
        (std::env::var_os("CATDB_TEST_RANGE_WRITEBACK").as_deref()
            == Some(std::ffi::OsStr::new("1")))
        .then_some(Self {
            written: HEADER_BYTES as i64,
            submitted: 0,
        })
    }

    pub(super) fn written(
        &mut self,
        size: usize,
        mut send: impl FnMut(i64, i64) -> io::Result<()>,
    ) -> io::Result<()> {
        self.written += size as i64;
        while self.written - self.submitted >= INTERVAL {
            send(self.submitted, INTERVAL)?;
            self.submitted += INTERVAL;
        }
        Ok(())
    }
}

pub(super) fn submit(file: &File, offset: i64, length: i64) -> io::Result<()> {
    unsafe extern "C" {
        fn sync_file_range(fd: i32, offset: i64, length: i64, flags: u32) -> i32;
    }
    // WRITE schedules completed ranges; it cannot replace the final durability barrier.
    if unsafe { sync_file_range(file.as_raw_fd(), offset, length, SYNC_FILE_RANGE_WRITE) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[test]
fn every_complete_range_is_submitted_once_and_failure_preserves_the_offset() {
    let mut pending = Pending {
        written: HEADER_BYTES as i64,
        submitted: 0,
    };
    pending
        .written((INTERVAL - HEADER_BYTES as i64 - 1) as usize, |_, _| {
            panic!("partial range")
        })
        .unwrap();
    assert!(
        pending
            .written(1, |_, _| Err(io::Error::other("injected")))
            .is_err()
    );
    let mut ranges = Vec::new();
    pending
        .written(INTERVAL as usize + 1, |o, l| {
            ranges.push((o, l));
            Ok(())
        })
        .unwrap();
    assert_eq!(ranges, [(0, INTERVAL), (INTERVAL, INTERVAL)]);
}
