use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};

const MAGIC: &[u8; 8] = b"CATDBBAK";
const BLOCK: usize = 64 * 1024;
const BATCH: usize = 1024 * 1024;
const HEADER: usize = 32;
const ENTRY: usize = 5;
static ZERO: [u8; BATCH] = [0; BATCH];

fn invalid(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, reason)
}

fn directory_bytes(size: u64) -> io::Result<usize> {
    if !(super::HEADER_BYTES as u64..=super::MAX_WAL_BYTES).contains(&size) {
        return Err(invalid("backup database size outside supported limits"));
    }
    Ok(size.div_ceil(BLOCK as u64) as usize * ENTRY)
}

fn check_empty(file: &mut File) -> io::Result<()> {
    if file.metadata()?.len() != 0 {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "backup output must be empty",
        ));
    }
    file.rewind()
}

pub(super) fn encode(source: &mut File, archive: &mut File) -> io::Result<()> {
    let size = source.metadata()?.len();
    let mut directory = vec![0; directory_bytes(size)?];
    source.rewind()?;
    check_empty(archive)?;
    archive.seek(SeekFrom::Start((HEADER + directory.len()) as u64))?;
    let mut buffer = vec![0; BATCH.min(size as usize)];
    let mut remaining = size as usize;
    let (entries, _) = directory.as_chunks_mut::<ENTRY>();
    for batch in entries.chunks_mut(BATCH / BLOCK) {
        let count = remaining.min(BATCH);
        let input = &mut buffer[..count];
        source.read_exact(input)?;
        for (block, entry) in input.chunks(BLOCK).zip(batch.iter_mut()) {
            if block != &ZERO[..block.len()] {
                entry[0] = 1;
                entry[1..].copy_from_slice(&crc32c(block).to_le_bytes());
            }
        }
        let mut offset = 0;
        for run in batch.chunk_by(|a, b| a[0] == b[0]) {
            let end = count.min(offset + run.len() * BLOCK);
            if run[0][0] == 1 {
                archive.write_all(&input[offset..end])?;
            }
            offset = end;
        }
        remaining -= count;
    }
    let mut header = [0; HEADER];
    header[..8].copy_from_slice(MAGIC);
    header[8..12].copy_from_slice(&1u32.to_le_bytes());
    header[12..16].copy_from_slice(&(BLOCK as u32).to_le_bytes());
    header[16..24].copy_from_slice(&size.to_le_bytes());
    header[24..28].copy_from_slice(&crate::crc32(&directory).to_le_bytes());
    let check = crate::crc32(&header[..28]);
    header[28..].copy_from_slice(&check.to_le_bytes());
    archive.rewind()?;
    archive.write_all(&header)?;
    archive.write_all(&directory)
}

pub(super) fn decode(archive: &mut File, destination: &mut File) -> io::Result<()> {
    let archive_size = archive.metadata()?.len();
    archive.rewind()?;
    let mut header = [0; HEADER];
    archive.read_exact(&mut header)?;
    if &header[..8] != MAGIC
        || header[8..12] != 1u32.to_le_bytes()
        || header[12..16] != (BLOCK as u32).to_le_bytes()
        || header[28..] != crate::crc32(&header[..28]).to_le_bytes()
    {
        return Err(invalid("invalid backup header or version"));
    }
    let size = u64::from_le_bytes(header[16..24].try_into().unwrap());
    let mut directory = vec![0; directory_bytes(size)?];
    archive.read_exact(&mut directory)?;
    if header[24..28] != crate::crc32(&directory).to_le_bytes() {
        return Err(invalid("invalid backup block directory checksum"));
    }
    let mut expected = (HEADER + directory.len()) as u64;
    let (blocks, _) = directory.as_chunks::<ENTRY>();
    for (i, entry) in blocks.iter().enumerate() {
        match entry[0] {
            0 if entry[1..] == [0; 4] => {}
            1 => expected += (size - (i * BLOCK) as u64).min(BLOCK as u64),
            _ => return Err(invalid("invalid backup block type")),
        }
    }
    if expected != archive_size {
        return Err(invalid("invalid backup archive length"));
    }
    check_empty(destination)?;
    destination.set_len(size)?;
    let mut buffer = vec![0; BATCH.min(size as usize)];
    let mut remaining = size as usize;
    for run in blocks.chunk_by(|a, b| a[0] == b[0]) {
        for batch in run.chunks(BATCH / BLOCK) {
            let count = remaining.min(batch.len() * BLOCK);
            if batch[0][0] == 0 {
                // A hole or unwritten extent would defer allocation work to future commits.
                destination.write_all(&ZERO[..count])?;
            } else {
                let raw = &mut buffer[..count];
                archive.read_exact(raw)?;
                for (block, entry) in raw.chunks(BLOCK).zip(batch) {
                    if entry[1..] != crc32c(block).to_le_bytes() {
                        return Err(invalid("invalid backup payload checksum"));
                    }
                }
                destination.write_all(raw)?;
            }
            remaining -= count;
        }
    }
    Ok(())
}

const CASTAGNOLI_TABLE: [u32; 256] = {
    let mut table = [0; 256];
    let mut i = 0;
    while i < 256 {
        let mut crc = i as u32;
        let mut bit = 0;
        while bit < 8 {
            crc = (crc >> 1) ^ if crc & 1 == 0 { 0 } else { 0x82f63b78 };
            bit += 1;
        }
        table[i] = crc;
        i += 1;
    }
    table
};

fn portable_crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc = (crc >> 8) ^ CASTAGNOLI_TABLE[((crc as u8) ^ byte) as usize];
    }
    !crc
}

fn crc32c(bytes: &[u8]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    if std::is_x86_feature_detected!("sse4.2") {
        // The intrinsic must only be reached on CPUs with the CRC32 instruction.
        return unsafe { hardware_crc32c(bytes) };
    }
    portable_crc32c(bytes)
}

#[cfg(target_arch = "x86_64")]
const fn transform(mut value: u32, basis: &[u32; 32]) -> u32 {
    let mut result = 0;
    while value != 0 {
        let bit = value.trailing_zeros() as usize;
        result ^= basis[bit];
        value &= value - 1;
    }
    result
}

#[cfg(target_arch = "x86_64")]
const QUARTER_SHIFT: [u32; 32] = {
    let mut basis = [0; 32];
    let mut i = 0;
    while i < basis.len() {
        let word = 1u32 << i;
        basis[i] = (word >> 8) ^ CASTAGNOLI_TABLE[(word & 255) as usize];
        i += 1;
    }
    let mut bytes = 1;
    while bytes < BLOCK / 4 {
        let previous = basis;
        let mut i = 0;
        while i < basis.len() {
            basis[i] = transform(previous[i], &previous);
            i += 1;
        }
        bytes *= 2;
    }
    basis
};

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.2")]
unsafe fn hardware_crc32c(bytes: &[u8]) -> u32 {
    use std::arch::x86_64::{_mm_crc32_u8, _mm_crc32_u64};
    let (words, tail) = bytes.as_chunks::<8>();
    if bytes.len() == BLOCK {
        // A single CRC dependency chain leaves most of the processor's CRC throughput unused.
        let quarter = BLOCK / 32;
        let mut crc = [u64::from(!0u32); 4];
        for i in 0..quarter {
            for (lane, state) in crc.iter_mut().enumerate() {
                *state = _mm_crc32_u64(*state, u64::from_le_bytes(words[lane * quarter + i]));
            }
        }
        let mut result = !(crc[0] as u32);
        for &next in &crc[1..] {
            result = transform(result, &QUARTER_SHIFT) ^ !(next as u32);
        }
        return result;
    }
    let mut crc = u64::from(!0u32);
    for &word in words {
        crc = _mm_crc32_u64(crc, u64::from_le_bytes(word));
    }
    let mut crc = crc as u32;
    for &byte in tail {
        crc = _mm_crc32_u8(crc, byte);
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "catdb-backup-archive-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn write_archive(&self, bytes: &[u8]) -> Vec<u8> {
            let path = self.0.join("source");
            fs::write(&path, bytes).unwrap();
            let archive = self.0.join("archive");
            encode(
                &mut File::open(path).unwrap(),
                &mut File::create(&archive).unwrap(),
            )
            .unwrap();
            fs::read(archive).unwrap()
        }

        fn restore(&self, bytes: &[u8]) -> io::Result<Vec<u8>> {
            let archive = self.0.join("input");
            fs::write(&archive, bytes)?;
            let output = self.0.join("restored");
            decode(&mut File::open(archive)?, &mut File::create(&output)?)?;
            fs::read(output)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn physical_bytes_round_trip_across_io_boundaries_and_zero_blocks_take_no_payload() {
        let fixture = Fixture::new();
        for len in [super::super::HEADER_BYTES, BLOCK, BATCH + BLOCK + 13] {
            let mut input = vec![0; len];
            let archive = fixture.write_archive(&input);
            assert_eq!(archive.len(), HEADER + len.div_ceil(BLOCK) * ENTRY);
            assert_eq!(fixture.restore(&archive).unwrap(), input);
            for i in (0..len).step_by(2 * BLOCK) {
                let end = (i + BLOCK).min(len);
                for (j, byte) in input[i..end].iter_mut().enumerate() {
                    *byte = (j % 251 + 1) as u8;
                }
            }
            input[len - 1] = 0xff;
            assert_eq!(
                fixture.restore(&fixture.write_archive(&input)).unwrap(),
                input
            );
        }
    }

    #[test]
    fn header_directory_payload_order_and_exact_end_are_checked() {
        let fixture = Fixture::new();
        let mut input = vec![7; BLOCK * 3];
        input[BLOCK..2 * BLOCK].fill(0);
        input[2 * BLOCK..].fill(13);
        let original = fixture.write_archive(&input);
        let payload = HEADER + ENTRY * 3;
        for pos in [0, 8, 12, 16, 24, 28, HEADER + ENTRY, payload] {
            let mut bad = original.clone();
            bad[pos] ^= 1;
            assert!(fixture.restore(&bad).is_err(), "corruption at {pos}");
        }
        let mut swapped = original.clone();
        swapped[payload..].rotate_left(BLOCK);
        assert!(fixture.restore(&swapped).is_err());
        assert!(fixture.restore(&original[..31]).is_err());
        assert!(fixture.restore(&original[..original.len() - 1]).is_err());
        let mut trailing = original.clone();
        trailing.push(0);
        assert!(fixture.restore(&trailing).is_err());

        for size in [0, super::super::MAX_WAL_BYTES + 1, u64::MAX] {
            let mut bad = original.clone();
            bad[16..24].copy_from_slice(&size.to_le_bytes());
            let check = crate::crc32(&bad[..28]);
            bad[28..32].copy_from_slice(&check.to_le_bytes());
            assert!(fixture.restore(&bad).is_err());
        }
    }

    #[test]
    fn archive_crc32c_matches_castagnoli_on_all_block_tails() {
        assert_eq!(crc32c(b"123456789"), 0xe3069283);
        let values: Vec<u8> = (0..255).collect();
        for i in 0..values.len() {
            assert_eq!(crc32c(&values[..i]), portable_crc32c(&values[..i]));
        }
        let full: Vec<_> = values.iter().copied().cycle().take(BLOCK + 1).collect();
        for len in [BLOCK - 1, BLOCK, BLOCK + 1] {
            assert_eq!(crc32c(&full[..len]), portable_crc32c(&full[..len]));
        }
    }
}
