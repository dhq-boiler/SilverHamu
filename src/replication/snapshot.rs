use crate::transactions::{Database, Error, ScanLimits, Transaction};
use ring::digest::{SHA256, digest};
use std::{io, sync::Arc};

pub(super) const CHUNK: usize = 128 * 1024;
pub(super) const MAX_IMAGE: usize = 128 * 1024 * 1024;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Snapshot {
    pub index: u64,
    pub term: u64,
    pub bytes: Arc<Vec<u8>>,
    pub hash: String,
}

impl Snapshot {
    pub fn new(index: u64, term: u64, bytes: Vec<u8>) -> Self {
        let hash = crate::service::hex(digest(&SHA256, &bytes).as_ref());
        Self {
            index,
            term,
            bytes: Arc::new(bytes),
            hash,
        }
    }

    pub fn capture(mut read: Transaction, index: u64, term: u64) -> Result<Self, Error> {
        let mut after = None;
        let mut image = Vec::new();
        loop {
            let page = read.scan_page(b"", None, after.as_deref(), ScanLimits::default())?;
            for (key, value) in page.rows {
                if image.len() + 8 + key.len() + value.len() > MAX_IMAGE {
                    return Err(Error::Limit("snapshot image capacity"));
                }
                for field in [key, value] {
                    image.extend_from_slice(&(field.len() as u32).to_be_bytes());
                    image.extend_from_slice(&field);
                }
            }
            after = page.next_after;
            if after.is_none() {
                break;
            }
        }
        Ok(Self::new(index, term, image))
    }

    pub(super) fn validate(&self) -> io::Result<()> {
        let mut rows = Rows::new(&self.bytes);
        while rows.next_row()?.is_some() {}
        Ok(())
    }

    // The journal keeps the install marker until every page is durable. Repeating
    // a partial installation must also remove keys absent from the new image.
    pub fn install(&self, db: &Database) -> Result<(), Error> {
        let mut rows = Rows::new(&self.bytes);
        let mut target = rows.next_row()?;
        let mut after = None;
        loop {
            let page =
                db.begin_read()?
                    .scan_page(b"", None, after.as_deref(), ScanLimits::default())?;
            let mut tx = db.begin()?;
            for (key, _) in page.rows {
                while target.is_some_and(|(k, _)| k < key.as_slice()) {
                    target = rows.next_row()?;
                }
                if target.is_none_or(|(k, _)| k != key.as_slice()) {
                    tx.delete(&key)?;
                }
            }
            tx.commit()?;
            after = page.next_after;
            if after.is_none() {
                break;
            }
        }
        let mut tx = db.begin()?;
        let mut size = 0;
        let mut rows = Rows::new(&self.bytes);
        let mut read = db.begin_read()?;
        while let Some((k, v)) = rows.next_row()? {
            if size >= CHUNK {
                drop(read);
                tx.commit()?;
                tx = db.begin()?;
                read = db.begin_read()?;
                size = 0;
            }
            if read.get(k)?.as_deref() != Some(v) {
                tx.put(k, v)?;
            }
            size += k.len() + v.len() + 128;
        }
        tx.commit()?;
        Ok(())
    }
}

struct Rows<'a> {
    bytes: &'a [u8],
    previous: Option<&'a [u8]>,
}
impl<'a> Rows<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            previous: None,
        }
    }
    fn field(&mut self, limit: usize) -> io::Result<&'a [u8]> {
        let (length, rest) = self
            .bytes
            .split_at_checked(4)
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        let length = u32::from_be_bytes(length.try_into().unwrap()) as usize;
        if length > limit {
            return Err(io::Error::other("oversized snapshot field"));
        }
        let (field, rest) = rest
            .split_at_checked(length)
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        self.bytes = rest;
        Ok(field)
    }
    fn next_row(&mut self) -> io::Result<Option<(&'a [u8], &'a [u8])>> {
        if self.bytes.is_empty() {
            return Ok(None);
        }
        let key = self.field(crate::transactions::MAX_KEY_BYTES)?;
        let value = self.field(256 * 1024)?;
        if self.previous.is_some_and(|k| k >= key) {
            return Err(io::Error::other("unordered snapshot"));
        }
        self.previous = Some(key);
        Ok(Some((key, value)))
    }
}
