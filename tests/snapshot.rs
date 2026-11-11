use silverhamu::{MAX_PAGE_BYTES, MAX_PAGE_RECORDS, MAX_RECORD_BYTES, Snapshot, create};
use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const GENERATION: [u8; 16] = [7; 16];
static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct TestFile(PathBuf);

impl TestFile {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "newdbms-{}-{timestamp}-{id}.ndb",
            std::process::id()
        )))
    }
}

impl Drop for TestFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

#[test]
fn eq1_roundtrip_preserves_variable_empty_and_unicode_records() {
    let path = TestFile::new();
    let rows = vec![
        b"abc".to_vec(),
        Vec::new(),
        "日本語の書籍".as_bytes().to_vec(),
        vec![0, 255],
    ];
    create(&path.0, GENERATION, &rows).unwrap();
    let mut db = Snapshot::open(&path.0, GENERATION).unwrap();
    assert_eq!(db.len(), 4);
    assert!(!db.is_empty());
    assert_eq!(db.read_page(0, 100).unwrap(), rows);
}

#[test]
fn bv1_empty_snapshot_returns_empty_pages() {
    let path = TestFile::new();
    create(&path.0, GENERATION, &[] as &[Vec<u8>]).unwrap();
    let mut db = Snapshot::open(&path.0, GENERATION).unwrap();
    assert!(db.is_empty());
    for start in [0, 1, u64::MAX] {
        assert!(db.read_page(start, 10).unwrap().is_empty());
    }
}

#[test]
fn bv2_page_edges_and_zero_limit_are_well_defined() {
    let path = TestFile::new();
    let rows = [b"first".as_slice(), b"middle", b"last"];
    create(&path.0, GENERATION, &rows).unwrap();
    let mut db = Snapshot::open(&path.0, GENERATION).unwrap();
    assert_eq!(db.read_page(0, 1).unwrap(), vec![rows[0]]);
    assert_eq!(db.read_page(1, 1).unwrap(), vec![rows[1]]);
    assert_eq!(db.read_page(2, 1).unwrap(), vec![rows[2]]);
    assert_eq!(db.read_page(2, 2).unwrap(), vec![rows[2]]);
    for start in [3, 4, u64::MAX] {
        assert!(db.read_page(start, 1).unwrap().is_empty());
    }
    assert!(db.read_page(0, 0).unwrap().is_empty());
}

#[test]
fn bv3_record_limit_allows_maximum_and_rejects_next_byte_before_creation() {
    let path = TestFile::new();
    let row = vec![42; MAX_RECORD_BYTES];
    create(&path.0, GENERATION, &[&row]).unwrap();
    let mut db = Snapshot::open(&path.0, GENERATION).unwrap();
    assert_eq!(db.read_page(0, 1).unwrap()[0], row);
    let oversized = TestFile::new();
    let row = vec![42; MAX_RECORD_BYTES + 1];
    assert_eq!(
        create(&oversized.0, GENERATION, &[row]).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
    assert!(!oversized.0.exists());
}

#[test]
fn bv4_page_record_limit_allows_maximum_and_rejects_next_record() {
    let path = TestFile::new();
    let rows = vec![Vec::<u8>::new(); MAX_PAGE_RECORDS + 1];
    create(&path.0, GENERATION, &rows).unwrap();
    let mut db = Snapshot::open(&path.0, GENERATION).unwrap();
    assert_eq!(
        db.read_page(0, MAX_PAGE_RECORDS).unwrap().len(),
        MAX_PAGE_RECORDS
    );
    assert_eq!(
        db.read_page(0, MAX_PAGE_RECORDS + 1).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
}

#[test]
fn bv5_page_byte_limit_allows_maximum_and_rejects_next_byte() {
    let path = TestFile::new();
    let row = vec![1; MAX_RECORD_BYTES];
    let rows = [&row[..], &row[..], &row[..], &row[..], &[2_u8][..]];
    create(&path.0, GENERATION, &rows).unwrap();
    let mut db = Snapshot::open(&path.0, GENERATION).unwrap();
    let page = db.read_page(0, 4).unwrap();
    assert_eq!(page.iter().map(Vec::len).sum::<usize>(), MAX_PAGE_BYTES);
    assert_eq!(
        db.read_page(0, 5).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
    assert_eq!(db.read_page(4, 1).unwrap(), vec![vec![2]]);
}

#[test]
fn er1_incomplete_or_extended_files_are_rejected() {
    let source = TestFile::new();
    create(&source.0, GENERATION, &[b"record"]).unwrap();
    let bytes = fs::read(&source.0).unwrap();
    for len in [0, 1, 63, 64, 71, bytes.len() - 1, bytes.len() + 1] {
        let damaged = TestFile::new();
        let mut changed = bytes.clone();
        changed.resize(len, 0);
        fs::write(&damaged.0, changed).unwrap();
        assert!(
            Snapshot::open(&damaged.0, GENERATION).is_err(),
            "length {len}"
        );
    }
}

#[test]
fn er1_magic_version_checksum_and_reserved_corruption_are_rejected() {
    let source = TestFile::new();
    create(&source.0, GENERATION, &[b"record"]).unwrap();
    let bytes = fs::read(&source.0).unwrap();
    for at in [0, 8, 12, 32, 40, 48, 56, 60] {
        let damaged = TestFile::new();
        let mut changed = bytes.clone();
        changed[at] ^= 1;
        fs::write(&damaged.0, changed).unwrap();
        assert!(
            Snapshot::open(&damaged.0, GENERATION).is_err(),
            "header byte {at}"
        );
    }
}

#[test]
fn er2_corrupt_intermediate_offsets_are_rejected_on_read() {
    let source = TestFile::new();
    create(&source.0, GENERATION, &[b"one", b"two", b"end"]).unwrap();
    let bytes = fs::read(&source.0).unwrap();
    let first = u64::from_le_bytes(bytes[64..72].try_into().unwrap());
    for offset in [0, u64::MAX, first, first + 3, first + 15] {
        let damaged = TestFile::new();
        let mut changed = bytes.clone();
        changed[72..80].copy_from_slice(&offset.to_le_bytes());
        fs::write(&damaged.0, changed).unwrap();
        let mut db = Snapshot::open(&damaged.0, GENERATION).unwrap();
        assert_eq!(
            db.read_page(0, 3).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
    }
}

#[test]
fn er2_payload_and_record_checksum_corruption_are_rejected_on_read() {
    let source = TestFile::new();
    create(&source.0, GENERATION, &[b"record"]).unwrap();
    let bytes = fs::read(&source.0).unwrap();
    for at in [80, bytes.len() - 1] {
        let damaged = TestFile::new();
        let mut changed = bytes.clone();
        changed[at] ^= 1;
        fs::write(&damaged.0, changed).unwrap();
        let mut db = Snapshot::open(&damaged.0, GENERATION).unwrap();
        assert_eq!(
            db.read_page(0, 1).unwrap_err().kind(),
            ErrorKind::InvalidData
        );
    }
}

#[test]
fn er2_record_checksum_matches_standard_crc32_reference_vector() {
    let path = TestFile::new();
    create(&path.0, GENERATION, &[b"123456789"]).unwrap();
    let bytes = fs::read(&path.0).unwrap();
    assert_eq!(&bytes[bytes.len() - 4..], &0xcbf4_3926_u32.to_le_bytes());
}

#[test]
fn st1_recreating_existing_snapshot_preserves_original_bytes() {
    let path = TestFile::new();
    create(&path.0, GENERATION, &[b"original"]).unwrap();
    let original = fs::read(&path.0).unwrap();
    assert_eq!(
        create(&path.0, [9; 16], &[b"replacement"])
            .unwrap_err()
            .kind(),
        ErrorKind::AlreadyExists
    );
    assert_eq!(fs::read(&path.0).unwrap(), original);
}

#[test]
fn st2_snapshot_with_wrong_generation_is_rejected() {
    let path = TestFile::new();
    create(&path.0, GENERATION, &[b"record"]).unwrap();
    assert!(Snapshot::open(&path.0, [8; 16]).is_err());
    assert!(Snapshot::open(&path.0, GENERATION).is_ok());
}
