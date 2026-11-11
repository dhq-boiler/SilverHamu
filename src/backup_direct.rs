use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};

const ALIGNMENT: usize = 64 * 1024;
const CHUNK: usize = 2 * 1024 * 1024;

pub(super) struct PayloadWriter<'a> {
    buffered: &'a mut File,
    direct: Option<File>,
    offset: u64,
    prefix: usize,
    storage: Vec<u8>,
    start: usize,
    used: usize,
    #[cfg(target_os = "linux")]
    direct_enabled: bool,
}

impl<'a> PayloadWriter<'a> {
    pub(super) fn new(buffered: &'a mut File) -> io::Result<Self> {
        let offset = buffered.stream_position()?;
        let prefix = (ALIGNMENT - offset as usize % ALIGNMENT) % ALIGNMENT;
        let direct = open_direct(buffered)?;
        #[cfg(windows)]
        let mut direct = direct;
        #[cfg(windows)]
        if let Some(file) = &mut direct {
            match file.seek(SeekFrom::Start(offset + prefix as u64)) {
                Ok(_) => {}
                Err(err) if unsupported(&err) => direct = None,
                Err(err) => return Err(err),
            }
        }
        let storage = if direct.is_some() {
            vec![0; CHUNK + ALIGNMENT - 1]
        } else {
            Vec::new()
        };
        let start = storage.as_ptr().align_offset(ALIGNMENT);
        Ok(Self {
            buffered,
            direct,
            offset,
            prefix,
            storage,
            start,
            used: 0,
            #[cfg(target_os = "linux")]
            direct_enabled: false,
        })
    }

    fn close_direct(&mut self) -> io::Result<()> {
        if let Some(direct) = self.direct.take() {
            // Windows reopens a separate handle; its delayed write errors must be checked here.
            #[cfg(windows)]
            direct.sync_all()?;
            #[cfg(target_os = "linux")]
            if self.direct_enabled {
                toggle_direct(&direct, false)?;
                self.direct_enabled = false;
            }
            self.buffered.seek(SeekFrom::Start(self.offset))?;
        }
        Ok(())
    }

    fn write_chunk(&mut self) -> io::Result<()> {
        #[cfg(target_os = "linux")]
        if !self.direct_enabled
            && let Some(direct) = self.direct.as_ref()
        {
            match toggle_direct(direct, true) {
                Ok(()) => self.direct_enabled = true,
                Err(e) if unsupported(&e) || e.kind() == io::ErrorKind::PermissionDenied => {
                    self.close_direct()?
                }
                Err(e) => return Err(e),
            }
        }
        if let Some(direct) = self.direct.as_mut() {
            let data = &self.storage[self.start..self.start + self.used];
            match direct.write_all(data) {
                Ok(()) => {
                    self.offset += self.used as u64;
                    self.used = 0;
                    return Ok(());
                }
                Err(err) if unsupported(&err) => self.close_direct()?,
                Err(err) => return Err(err),
            }
        }
        self.buffered
            .write_all(&self.storage[self.start..self.start + self.used])?;
        self.offset += self.used as u64;
        self.used = 0;
        Ok(())
    }
}

impl Write for PayloadWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        if self.used == CHUNK {
            self.write_chunk()?;
        }
        if self.direct.is_none() {
            return self.buffered.write(bytes);
        }
        if self.prefix != 0 {
            // Keep the cursor-relative header and the unaligned beginning of the payload buffered.
            let count = self
                .buffered
                .write(&bytes[..bytes.len().min(self.prefix)])?;
            self.prefix -= count;
            self.offset += count as u64;
            return Ok(count);
        }
        let count = (CHUNK - self.used).min(bytes.len());
        let begin = self.start + self.used;
        self.storage[begin..begin + count].copy_from_slice(&bytes[..count]);
        self.used += count;
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.used == CHUNK {
            self.write_chunk()?;
        }
        // Padding the last direct write would change the v1 archive length.
        self.close_direct()?;
        if self.used != 0 {
            self.write_chunk()?;
        }
        self.buffered.flush()
    }
}

#[cfg(windows)]
fn open_direct(buffered: &File) -> io::Result<Option<File>> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReOpenFile(handle: RawHandle, access: u32, share: u32, flags: u32) -> RawHandle;
    }
    const GENERIC_WRITE: u32 = 0x40000000;
    const SHARE_READ_WRITE_DELETE: u32 = 7;
    const FILE_FLAG_NO_BUFFERING: u32 = 0x20000000;
    // Reopening the owned handle avoids resolving the temporary filename a second time.
    let handle = unsafe {
        ReOpenFile(
            buffered.as_raw_handle(),
            GENERIC_WRITE,
            SHARE_READ_WRITE_DELETE,
            FILE_FLAG_NO_BUFFERING,
        )
    };
    if handle as isize == -1 {
        let err = io::Error::last_os_error();
        return if unsupported(&err) || err.kind() == io::ErrorKind::PermissionDenied {
            Ok(None)
        } else {
            Err(err)
        };
    }
    Ok(Some(unsafe { File::from_raw_handle(handle) }))
}

#[cfg(windows)]
fn unsupported(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(1 | 50 | 87))
}

#[cfg(target_os = "linux")]
fn open_direct(buffered: &File) -> io::Result<Option<File>> {
    // A second open would track writeback errors separately from the final sync_all.
    // The buffered handle already has write access when the descriptor limit is reached.
    Ok(buffered.try_clone().ok())
}

#[cfg(target_os = "linux")]
fn toggle_direct(file: &File, on: bool) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let fd = file.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    let flags = if on {
        flags | libc::O_DIRECT
    } else {
        flags & !libc::O_DIRECT
    };
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn unsupported(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(libc::EINVAL | libc::EOPNOTSUPP | libc::ENOSYS)
    )
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::transactions::temporary_path;
    use std::fs;

    fn verify_output(force_fallback: bool) {
        let path = temporary_path(&std::env::temp_dir().join("silverhamu-direct-test"));
        let mut output = File::options()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let prefix = [0xc5; 13];
        output.write_all(&prefix).unwrap();
        let mut writer = PayloadWriter::new(&mut output).unwrap();
        let data: Vec<_> = (0..(3 * CHUNK + ALIGNMENT + 17))
            .map(|i| (i % 251) as u8)
            .collect();
        if writer.direct.is_some() && force_fallback {
            let split = writer.prefix + CHUNK + 1;
            writer.write_all(&data[..split]).unwrap();
            assert_eq!(writer.used, 1);
            // On NTFS this makes the next full direct write fail with ERROR_INVALID_PARAMETER.
            let first = writer.storage[writer.start];
            writer.start = if writer.start == 0 {
                1
            } else {
                writer.start - 1
            };
            writer.storage[writer.start] = first;
            writer.write_all(&data[split..]).unwrap();
        } else {
            writer.write_all(&data).unwrap();
        }
        writer.flush().unwrap();
        writer.write_all(b"end").unwrap();
        writer.flush().unwrap();
        drop(writer);
        output.sync_all().unwrap();
        drop(output);
        let actual = fs::read(&path).unwrap();
        fs::remove_file(path).unwrap();
        assert_eq!(actual.len(), prefix.len() + data.len() + 3);
        assert_eq!(&actual[..prefix.len()], &prefix);
        assert!(&actual[prefix.len()..prefix.len() + data.len()] == data.as_slice());
        assert_eq!(&actual[prefix.len() + data.len()..], b"end");
    }

    #[test]
    fn flush_writes_the_unaligned_tail_and_allows_another_write() {
        verify_output(false);
    }

    #[test]
    fn buffered_retry_preserves_bytes_already_written_by_the_direct_handle() {
        verify_output(true);
    }
}
