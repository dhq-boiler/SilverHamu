#[test]
fn crc_combination_matches_concatenation_at_every_page_length() {
    let prefix = b"CATDB prefix with nonzero CRC";
    let prefix_crc = crate::crc32(prefix);
    let mut body_crc = 0u32;
    let mut joined_crc = prefix_crc;
    let mut body = Vec::new();
    let advance = |crc: u32, byte: u8| {
        let raw = !crc;
        !((raw >> 8) ^ crate::CRC_TABLES[0][((raw ^ (byte as u32)) & 255) as usize])
    };
    for len in 0..=BLOCK {
        assert_eq!(combine_crc(prefix_crc, body_crc, len as u32), joined_crc);
        if len.is_power_of_two() || len == BLOCK - 1 {
            assert_eq!(body_crc, crate::crc32(&body));
            assert_eq!(
                joined_crc,
                crate::crc32(&[prefix.as_slice(), &body].concat())
            );
        }
        let byte = ((len * 193) ^ (len >> 5)) as u8;
        body.push(byte);
        body_crc = advance(body_crc, byte);
        joined_crc = advance(joined_crc, byte);
    }
}
#[test]
fn combined_pages_repair_one_bad_body_and_reject_two() {
    let path = dir().join("damaged.db");
    let cfg = four_config();
    let db = MvccPages::create_direct(&path, cfg, (0..700).map(|i| (key(i), value(i, 0))), None);
    drop(db);
    let image = Dirty::open(&path, cfg, &mut noop).unwrap();
    let offset = u64::from(image.pages.values().next().unwrap().slot) * PAIR;
    drop(image);
    let pool = path.with_extension("pages");
    let at = offset + (HEAD + PREFIX_BYTES + 12 + 9 + 8) as u64;
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&pool)
        .unwrap();
    file.seek(SeekFrom::Start(at)).unwrap();
    let mut byte = [0];
    file.read_exact(&mut byte).unwrap();
    drop(file);
    let damaged = [byte[0] ^ 0x80];
    overwrite(&pool, at, &damaged);
    let restored = Dirty::open(&path, cfg, &mut noop).unwrap();
    assert_eq!(restored.rows, data(700));
    assert!(restored.control.costs.writes.contains_key("repair-page"));
    drop(restored);
    let clean = Dirty::open(&path, cfg, &mut noop).unwrap();
    assert!(!clean.control.costs.writes.contains_key("repair-page"));
    drop(clean);
    overwrite(&pool, at, &damaged);
    overwrite(&pool, at + BLOCK as u64, &damaged);
    assert!(Dirty::open(&path, cfg, &mut noop).is_err());
}
