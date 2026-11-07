#[test]
fn reused_log_bytes_survive_peer_damage_and_match_original() {
    let a = dir().join("old.db");
    let b = dir().join("new.db");
    for (p, reuse) in [(&a, false), (&b, true)] {
        let mut s = Store::create(p, config(), data(9), BUFFER_BUDGET, &mut noop).unwrap();
        s.reuse_append = reuse;
        s.append(&change(), &mut noop).unwrap();
        s.append(&short(), &mut noop).unwrap();
    }
    assert_eq!(fs::read(&a).unwrap(), fs::read(&b).unwrap());
    let s = Store::open(&b, config(), BUFFER_BUDGET, &mut noop).unwrap();
    let expected = s.rows.clone();
    let offset = config().at(s.root.bank, false, 0, 0);
    drop(s);
    overwrite(&b, offset, &[0; HEAD]);
    let s = Store::open(&b, config(), BUFFER_BUDGET, &mut noop).unwrap();
    assert_eq!(s.rows, expected);
    assert!(s.costs.writes.contains_key("repair-data"));
    drop(s);
    assert_eq!(fs::read(a).unwrap(), fs::read(b).unwrap());
}
