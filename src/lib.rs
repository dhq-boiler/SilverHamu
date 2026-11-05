use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

mod ffi;
mod resources;
#[cfg(feature = "service")]
pub mod service;
mod transaction_ffi;
pub mod transactions;

const MAGIC: &[u8; 8] = b"NDBMS001";
const HEADER_SIZE: u64 = 64;
pub const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_PAGE_RECORDS: usize = 4096;
pub const MAX_PAGE_BYTES: usize = 64 * 1024 * 1024;

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn input(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

const CRC_TABLES: [[u32; 256]; 8] = crc_tables();

const fn crc_tables() -> [[u32; 256]; 8] {
    let mut tables = [[0; 256]; 8];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0_u32.wrapping_sub(crc & 1));
            bit += 1;
        }
        tables[0][i] = crc;
        i += 1;
    }
    let mut slice = 1;
    while slice < 8 {
        let mut i = 0;
        while i < 256 {
            let previous = tables[slice - 1][i];
            tables[slice][i] = (previous >> 8) ^ tables[0][(previous & 255) as usize];
            i += 1;
        }
        slice += 1;
    }
    tables
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    let (blocks, tail) = bytes.as_chunks::<8>();
    for block in blocks {
        let low = crc ^ u32::from_le_bytes(block[..4].try_into().unwrap());
        crc = CRC_TABLES[7][(low & 255) as usize]
            ^ CRC_TABLES[6][((low >> 8) & 255) as usize]
            ^ CRC_TABLES[5][((low >> 16) & 255) as usize]
            ^ CRC_TABLES[4][(low >> 24) as usize]
            ^ CRC_TABLES[3][block[4] as usize]
            ^ CRC_TABLES[2][block[5] as usize]
            ^ CRC_TABLES[1][block[6] as usize]
            ^ CRC_TABLES[0][block[7] as usize];
    }
    for &byte in tail {
        crc = (crc >> 8) ^ CRC_TABLES[0][((crc ^ u32::from(byte)) & 255) as usize];
    }
    !crc
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

/// Writes a new immutable snapshot. An existing destination is never overwritten.
/// A failed write can leave an incomplete destination; callers must not publish it.
pub fn create<T: AsRef<[u8]>>(
    path: impl AsRef<Path>,
    generation: [u8; 16],
    records: &[T],
) -> io::Result<()> {
    let count = u64::try_from(records.len()).map_err(|_| input("too many records"))?;
    let payload_start = count
        .checked_add(1)
        .and_then(|n| n.checked_mul(8))
        .and_then(|n| n.checked_add(HEADER_SIZE))
        .ok_or_else(|| input("index length overflow"))?;
    let mut file_len = payload_start;
    for record in records {
        let len = record.as_ref().len();
        if len > MAX_RECORD_BYTES {
            return Err(input("record exceeds size limit"));
        }
        file_len = file_len
            .checked_add(len as u64 + 4)
            .ok_or_else(|| input("file length overflow"))?;
    }

    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut writer = BufWriter::new(file);
    writer.write_all(&[0; HEADER_SIZE as usize])?;
    let mut offset = payload_start;
    writer.write_all(&offset.to_le_bytes())?;
    for record in records {
        offset += record.as_ref().len() as u64 + 4;
        writer.write_all(&offset.to_le_bytes())?;
    }
    for record in records {
        let bytes = record.as_ref();
        writer.write_all(bytes)?;
        writer.write_all(&crc32(bytes).to_le_bytes())?;
    }
    writer.flush()?;
    writer.get_ref().sync_all()?;

    let mut header = [0; HEADER_SIZE as usize];
    header[..8].copy_from_slice(MAGIC);
    header[8..12].copy_from_slice(&1_u32.to_le_bytes());
    header[12..16].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
    header[16..32].copy_from_slice(&generation);
    header[32..40].copy_from_slice(&count.to_le_bytes());
    header[40..48].copy_from_slice(&payload_start.to_le_bytes());
    header[48..56].copy_from_slice(&file_len.to_le_bytes());
    let crc = crc32(&header[..56]);
    header[56..60].copy_from_slice(&crc.to_le_bytes());
    writer.seek(SeekFrom::Start(0))?;
    writer.write_all(&header)?;
    writer.flush()?;
    writer.get_ref().sync_all()
}

/// A snapshot must remain immutable while this handle is open.
/// Opening validates metadata; individual records are validated when read.
pub struct Snapshot {
    file: File,
    count: u64,
    payload_start: u64,
    file_len: u64,
}

impl Snapshot {
    pub fn open(path: impl AsRef<Path>, expected_generation: [u8; 16]) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let file_len = file.metadata()?.len();
        let mut header = [0; HEADER_SIZE as usize];
        file.read_exact(&mut header)?;
        if &header[..8] != MAGIC
            || header[8..12] != 1_u32.to_le_bytes()
            || header[12..16] != (HEADER_SIZE as u32).to_le_bytes()
            || header[60..64] != [0; 4]
        {
            return Err(invalid("unsupported snapshot header"));
        }
        if header[56..60] != crc32(&header[..56]).to_le_bytes() {
            return Err(invalid("header checksum mismatch"));
        }
        if header[16..32] != expected_generation {
            return Err(invalid("snapshot generation mismatch"));
        }
        let count = u64_at(&header, 32);
        let payload_start = u64_at(&header, 40);
        let expected_start = count
            .checked_add(1)
            .and_then(|n| n.checked_mul(8))
            .and_then(|n| n.checked_add(HEADER_SIZE))
            .ok_or_else(|| invalid("index length overflow"))?;
        let minimum_len = count
            .checked_mul(4)
            .and_then(|n| n.checked_add(payload_start))
            .ok_or_else(|| invalid("payload length overflow"))?;
        if payload_start != expected_start
            || file_len != u64_at(&header, 48)
            || file_len < minimum_len
        {
            return Err(invalid("invalid snapshot length"));
        }
        let mut offset = [0; 8];
        file.read_exact(&mut offset)?;
        if u64::from_le_bytes(offset) != payload_start {
            return Err(invalid("invalid first offset"));
        }
        file.seek(SeekFrom::Start(payload_start - 8))?;
        file.read_exact(&mut offset)?;
        if u64::from_le_bytes(offset) != file_len {
            return Err(invalid("invalid terminal offset"));
        }
        Ok(Self {
            file,
            count,
            payload_start,
            file_len,
        })
    }

    pub fn len(&self) -> u64 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn read_page(&mut self, start: u64, limit: usize) -> io::Result<Vec<Vec<u8>>> {
        if limit > MAX_PAGE_RECORDS {
            return Err(input("page exceeds record count limit"));
        }
        if start >= self.count || limit == 0 {
            return Ok(Vec::new());
        }
        let count = (self.count - start).min(limit as u64) as usize;
        let mut index = vec![0; (count + 1) * 8];
        self.file.seek(SeekFrom::Start(HEADER_SIZE + start * 8))?;
        self.file.read_exact(&mut index)?;
        let offsets: Vec<u64> = index
            .as_chunks::<8>()
            .0
            .iter()
            .map(|b| u64::from_le_bytes(*b))
            .collect();
        let mut payload_bytes = 0_u64;
        for pair in offsets.windows(2) {
            if pair[0] < self.payload_start || pair[1] > self.file_len {
                return Err(invalid("record offset out of bounds"));
            }
            let size = pair[1]
                .checked_sub(pair[0])
                .filter(|n| *n >= 4 && *n <= MAX_RECORD_BYTES as u64 + 4)
                .ok_or_else(|| invalid("invalid record span"))?;
            payload_bytes += size - 4;
        }
        if payload_bytes > MAX_PAGE_BYTES as u64 {
            return Err(input("page exceeds byte limit"));
        }
        let span = usize::try_from(offsets[count] - offsets[0])
            .map_err(|_| invalid("page length overflow"))?;
        let mut bytes = vec![0; span];
        self.file.seek(SeekFrom::Start(offsets[0]))?;
        self.file.read_exact(&mut bytes)?;
        let mut records = Vec::with_capacity(count);
        for pair in offsets.windows(2) {
            let begin = (pair[0] - offsets[0]) as usize;
            let end = (pair[1] - offsets[0]) as usize;
            let payload = &bytes[begin..end - 4];
            if bytes[end - 4..end] != crc32(payload).to_le_bytes() {
                return Err(invalid("record checksum mismatch"));
            }
            records.push(payload.to_vec());
        }
        Ok(records)
    }
}

#[cfg(test)]
#[path = "../benchmarks/work-elision/metrics.rs"]
mod work_elision_metrics;
