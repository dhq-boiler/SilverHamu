trait RowCollector {
    fn count(&self) -> usize;
    fn push_row(&mut self, key: &[u8], value: &[u8]);
    fn reset(&mut self);
}
impl RowCollector for WriteSet {
    fn count(&self) -> usize {
        self.len()
    }
    fn push_row(&mut self, k: &[u8], v: &[u8]) {
        self.insert(k.to_vec(), Some(v.to_vec()));
    }
    fn reset(&mut self) {
        self.clear();
    }
}

struct LinearPage {
    bytes: Vec<u8>,
    count: usize,
}
impl LinearPage {
    fn new() -> Self {
        let mut bytes = Vec::with_capacity(BLOCK);
        bytes.resize(HEAD + PREFIX_BYTES + 12, 0);
        Self { bytes, count: 0 }
    }
    fn seal(&mut self, record: &Record) -> io::Result<&[u8]> {
        if self.count == 0 || self.count > MAX_OPS || self.bytes.len() + TRAILER_BYTES > BLOCK {
            return Err(err("invalid linear page"));
        }
        let data = &mut self.bytes;
        let start = HEAD + PREFIX_BYTES;
        let length = (data.len() - start) as u32;
        data[HEAD..HEAD + 8].copy_from_slice(FRAME);
        data[HEAD + 8..HEAD + 12].copy_from_slice(&length.to_le_bytes());
        data[HEAD + 12..HEAD + 16].copy_from_slice(&(!length).to_le_bytes());
        let prefix_crc = crate::crc32(&data[HEAD..HEAD + 16]);
        data[HEAD + 16..start].copy_from_slice(&prefix_crc.to_le_bytes());
        data[start..start + 8].copy_from_slice(&record.seq.to_le_bytes());
        data[start + 8..start + 12].copy_from_slice(&(self.count as u32).to_le_bytes());
        let checksum = crate::crc32(&data[start..]);
        data.extend_from_slice(&checksum.to_le_bytes());
        data.extend_from_slice(SEAL);
        let (header, payload) = data.split_at_mut(HEAD);
        record.fill_header(header, payload);
        Ok(data)
    }
}
impl RowCollector for LinearPage {
    fn count(&self) -> usize {
        self.count
    }
    fn push_row(&mut self, key: &[u8], value: &[u8]) {
        self.bytes.push(1);
        self.bytes
            .extend_from_slice(&(key.len() as u32).to_le_bytes());
        self.bytes
            .extend_from_slice(&(value.len() as u32).to_le_bytes());
        self.bytes.extend_from_slice(key);
        self.bytes.extend_from_slice(value);
        self.count += 1;
    }
    fn reset(&mut self) {
        self.bytes.truncate(HEAD + PREFIX_BYTES + 12);
        self.count = 0;
    }
}
enum PageSource<'a> {
    Tree(&'a WriteSet),
    Linear(&'a mut LinearPage),
}
