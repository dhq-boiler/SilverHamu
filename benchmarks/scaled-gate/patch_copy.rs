#[derive(Default)]
struct PatchCounts {
    used: u64,
    fallback: u64,
    bytes: u64,
}

fn page_body_end(data: &[u8], addr: &Address, cfg: Config) -> Option<usize> {
    let u = |n| u32::from_le_bytes(data[n..n + 4].try_into().unwrap());
    let q = |n| u64::from_le_bytes(data[n..n + 8].try_into().unwrap());
    let size = u(36) as usize;
    if &data[..8] != b"CATBIG01"
        || &data[88..96] != b"BIGSEAL1"
        || u(8) != 1
        || u(12) > 1
        || q(16) != addr.generation
        || q(24) != addr.generation
        || u(32) != addr.slot
        || u(44) != cfg.checkpoints
        || u(48) != cfg.logs
        || u(76) != !(size as u32)
        || u(80) != addr.digest
        || u(84) != !addr.digest
        || crate::crc32(&data[..80]) != addr.digest
        || data[68..72] != [0; 4]
        || !(PREFIX_BYTES + 12 + TRAILER_BYTES..=BLOCK - HEAD).contains(&size)
    {
        return None;
    }
    let end = HEAD + size - TRAILER_BYTES;
    let body = HEAD + PREFIX_BYTES;
    let len = end - body;
    if &data[HEAD..HEAD + 8] != FRAME
        || u(HEAD + 8) != len as u32
        || u(HEAD + 12) != !(len as u32)
        || u(HEAD + 16) != crate::crc32(&data[HEAD..HEAD + 16])
        || q(body) != addr.generation
        || u(body + 8) as usize != addr.rows
        || addr.rows == 0
        || addr.rows > MAX_OPS
        || &data[end + 4..end + TRAILER_BYTES] != SEAL
    {
        return None;
    }
    let crc = crate::crc32(&data[body..end]);
    if crc != u(end) || combine_crc(crate::crc32(&data[HEAD..body]), crc, len as u32) != u(72) {
        return None;
    }
    Some(end)
}
fn patch_copy(
    pool: &mut File,
    addr: &Address,
    cfg: Config,
    overlay: &Data,
    lower: &[u8],
    upper: Option<&[u8]>,
    page: &mut LinearPage,
    counts: &mut PatchCounts,
) -> io::Result<bool> {
    let lo = tagged(1, lower);
    let hi = upper.map_or_else(|| vec![2], |k| tagged(1, k));
    let mut edits = overlay.range(lo..hi).peekable();
    if edits.peek().is_none() {
        return Ok(false);
    }
    page.bytes.resize(BLOCK, 0);
    let mut end = None;
    for lane in 0..2 {
        {
            let _phase = Phase::enter(7);
            pool.seek(SeekFrom::Start(
                u64::from(addr.slot) * PAIR + lane * BLOCK as u64,
            ))?;
            pool.read_exact(&mut page.bytes)?;
        }
        counts.bytes += BLOCK as u64;
        let _phase = Phase::enter(8);
        end = page_body_end(&page.bytes, addr, cfg);
        if end.is_some() {
            break;
        }
    }
    let end = end.ok_or_else(|| err("no valid source page for patch"))?;
    let _phase = Phase::enter(9);
    let mut pos = HEAD + PREFIX_BYTES + 12;
    for _ in 0..addr.rows {
        if pos + 9 > end || page.bytes[pos] != 1 {
            return Err(err("invalid patch source operation"));
        }
        let n = u32::from_le_bytes(page.bytes[pos + 1..pos + 5].try_into().unwrap()) as usize;
        let size = u32::from_le_bytes(page.bytes[pos + 5..pos + 9].try_into().unwrap()) as usize;
        let key_at = pos + 9;
        let value_at = key_at + n;
        pos = value_at + size;
        if pos > end || n > MAX_KEY_BYTES {
            return Err(err("invalid patch source lengths"));
        }
        if let Some((tagged_key, v)) = edits.peek() {
            match tagged_key[1..].cmp(&page.bytes[key_at..value_at]) {
                std::cmp::Ordering::Less => return Ok(false),
                std::cmp::Ordering::Equal => {
                    if v.first() != Some(&1) || v.len() != size + 1 {
                        return Ok(false);
                    }
                    page.bytes[value_at..pos].copy_from_slice(&v[1..]);
                    edits.next();
                }
                _ => {}
            }
        }
    }
    if pos != end {
        return Err(err("patch source length mismatch"));
    }
    if edits.next().is_some() {
        return Ok(false);
    }
    page.bytes.truncate(end);
    page.count = addr.rows;
    Ok(true)
}
