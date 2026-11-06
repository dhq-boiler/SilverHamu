use std::sync::Arc;

const N: usize = 128;
type Frame = [u8; N];

fn crc(bytes: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in bytes {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = (c >> 1) ^ (0xedb88320u32 & (0u32.wrapping_sub(c & 1)));
        }
    }
    !c
}

fn frame(epoch: u64, seq: u64, previous: u32, value: u8) -> Frame {
    let mut b = [0u8; N];
    b[..8].copy_from_slice(b"CATPROBE");
    b[8..16].copy_from_slice(&epoch.to_le_bytes());
    b[16..24].copy_from_slice(&seq.to_le_bytes());
    b[24..28].copy_from_slice(&previous.to_le_bytes());
    b[28..32].copy_from_slice(&64u32.to_le_bytes());
    b[32..96].fill(value);
    b[96..104].copy_from_slice(b"COMMIT01");
    b[104..112].copy_from_slice(&epoch.to_le_bytes());
    b[112..120].copy_from_slice(&seq.to_le_bytes());
    let check = crc(&b[..120]);
    b[120..124].copy_from_slice(&check.to_le_bytes());
    b[124..128].copy_from_slice(&(!check).to_le_bytes());
    b
}

fn u64_at(b: &Frame, pos: usize) -> u64 {
    u64::from_le_bytes(b[pos..pos + 8].try_into().unwrap())
}
fn u32_at(b: &Frame, pos: usize) -> u32 {
    u32::from_le_bytes(b[pos..pos + 4].try_into().unwrap())
}

#[derive(Clone, Copy, Debug)]
enum Policy {
    DiscardInvalidTail,
    RejectAmbiguousTail,
}

fn recover(disk: &[Frame], epoch: u64, policy: Policy) -> Result<Vec<u8>, &'static str> {
    let mut values = Vec::new();
    let mut previous = 0;
    for b in disk {
        if *b == [0; N] {
            break;
        }
        let seq = values.len() as u64 + 1;
        let valid = &b[..8] == b"CATPROBE"
            && u64_at(b, 8) == epoch
            && u64_at(b, 16) == seq
            && u32_at(b, 24) == previous
            && u32_at(b, 28) == 64
            && &b[96..104] == b"COMMIT01"
            && u64_at(b, 104) == epoch
            && u64_at(b, 112) == seq
            && u32_at(b, 120) == crc(&b[..120])
            && u32_at(b, 124) == !u32_at(b, 120);
        if !valid {
            return match policy {
                Policy::DiscardInvalidTail => Ok(values),
                Policy::RejectAmbiguousTail => Err("ambiguous tail: no durable independent boundary"),
            };
        }
        previous = u32_at(b, 120);
        values.push(b[32]);
    }
    Ok(values)
}

struct Model {
    disk: Vec<Frame>,
    visible: Arc<Vec<u8>>,
    sync_calls: usize,
}
impl Model {
    fn new(capacity: usize) -> Self {
        Self { disk: vec![[0; N]; capacity], visible: Arc::new(Vec::new()), sync_calls: 0 }
    }
    fn append(&mut self, value: u8, sync_succeeds: bool) -> Result<(), &'static str> {
        let n = self.visible.len();
        if n == self.disk.len() { return Err("capacity"); }
        let previous = if n == 0 { 0 } else { u32_at(&self.disk[n - 1], 120) };
        self.disk[n] = frame(7, n as u64 + 1, previous, value);
        self.sync_calls += 1;
        if !sync_succeeds { return Err("sync failure; writer must be retired"); }
        let mut next = (*self.visible).clone();
        next.push(value);
        self.visible = Arc::new(next);
        Ok(())
    }
}

#[test]
fn eq_st_valid_chain_reopens_and_old_values_survive_publication() {
    let mut m = Model::new(3);
    m.append(11, true).unwrap();
    let old = m.visible.clone();
    m.append(22, true).unwrap();
    assert_eq!(*old, vec![11]);
    assert_eq!(*m.visible, vec![11, 22]);
    assert_eq!(recover(&m.disk, 7, Policy::RejectAmbiguousTail).unwrap(), *m.visible);
    assert_eq!(m.sync_calls, 2);
}

#[test]
fn bv_empty_exact_capacity_and_rejection_do_not_replace_visible_data() {
    let mut m = Model::new(1);
    assert!(recover(&m.disk, 7, Policy::RejectAmbiguousTail).unwrap().is_empty());
    m.append(11, true).unwrap();
    let before = m.disk.clone();
    assert_eq!(m.append(22, true), Err("capacity"));
    assert_eq!(m.disk, before);
    assert_eq!(m.sync_calls, 1);
}

#[test]
fn er_failed_sync_does_not_publish_success() {
    let mut m = Model::new(2);
    m.append(11, true).unwrap();
    assert!(m.append(22, false).is_err());
    assert_eq!(*m.visible, vec![11]);
    // Failure does not imply that none of the new bytes reached storage.
    assert_eq!(recover(&m.disk, 7, Policy::RejectAmbiguousTail).unwrap(), vec![11, 22]);
}

#[test]
fn st_old_generation_reordered_sequence_and_broken_chain_are_rejected() {
    let a = frame(7, 1, 0, 11);
    for b in [frame(6, 2, u32_at(&a, 120), 22), frame(7, 3, u32_at(&a, 120), 22), frame(7, 2, 0, 22)] {
        assert!(recover(&[a, b], 7, Policy::RejectAmbiguousTail).is_err());
    }
}

#[test]
fn er_identical_bytes_cannot_distinguish_interrupted_append_from_damaged_acknowledged_tail() {
    let a = frame(7, 1, 0, 11);
    let b = frame(7, 2, u32_at(&a, 120), 22);
    let mut demonstrated = 0;
    for cut in 1..N {
        let mut interrupted = [0; N];
        interrupted[..cut].copy_from_slice(&b[..cut]);
        let mut acknowledged_then_damaged = b;
        acknowledged_then_damaged[cut..].fill(0);
        assert_eq!(interrupted, acknowledged_then_damaged);
        if cut == 64 {
            println!("cut64_first_frame={:02x?}", a);
            println!("cut64_identical_tail={:02x?}", interrupted);
        }
        if interrupted == b { continue; }
        let disk = [a, interrupted];
        // Prefix acceptance meets the first history but silently loses acknowledged sequence 2 in the second.
        assert_eq!(recover(&disk, 7, Policy::DiscardInvalidTail).unwrap(), vec![11]);
        // Rejection avoids that loss, but is not automatic recovery of the interrupted history.
        assert!(recover(&disk, 7, Policy::RejectAmbiguousTail).is_err());
        demonstrated += 1;
    }
    assert_eq!(demonstrated, 127);
    println!("127 ambiguous tail cuts: speed gate FAILED (expected counterexample)");
}

#[test]
fn er_fragment_order_and_stale_suffix_do_not_make_a_partial_new_frame_valid() {
    let a = frame(7, 1, 0, 11);
    let b = frame(7, 2, u32_at(&a, 120), 22);
    let stale = frame(6, 2, u32_at(&a, 120), 33);
    for mask in 1..15 {
        for base in [[0; N], stale] {
            let mut partial = base;
            for sector in 0..4 {
                if mask & (1 << sector) != 0 {
                    partial[sector * 32..(sector + 1) * 32].copy_from_slice(&b[sector * 32..(sector + 1) * 32]);
                }
            }
            if partial == b { continue; }
            assert!(recover(&[a, partial], 7, Policy::RejectAmbiguousTail).is_err());
        }
    }
}
