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

type Pair = [Frame; 2];
const EPOCH: u64 = 7;
const CAPACITY: usize = 4;
const PAGE: u64 = 4096;
const LANE_B: u64 = 1024 * 1024;

fn valid(b: &Frame) -> bool {
    &b[..8] == b"CATPROBE"
        && u32_at(b, 28) == 64
        && &b[96..104] == b"COMMIT01"
        && u64_at(b, 104) == u64_at(b, 8)
        && u64_at(b, 112) == u64_at(b, 16)
        && u32_at(b, 120) == crc(&b[..120])
        && u32_at(b, 124) == !u32_at(b, 120)
}

fn recover(pairs: &[Pair], epoch: u64) -> Result<Vec<u8>, &'static str> {
    let mut values = Vec::new();
    let mut previous = 0;
    let mut gap = false;
    for pair in pairs {
        let copies: Vec<_> = pair.iter().filter(|b| valid(b) && u64_at(b, 8) == epoch).collect();
        if copies.is_empty() { gap = true; continue; }
        if gap { return Err("valid record after missing pair"); }
        if copies.len() == 2 && copies[0] != copies[1] { return Err("valid copies disagree"); }
        let b = copies[0];
        if u64_at(b, 16) != values.len() as u64 + 1 || u32_at(b, 24) != previous {
            return Err("sequence or chain mismatch");
        }
        values.push(b[32]);
        previous = u32_at(b, 120);
    }
    Ok(values)
}

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
fn offset(index: usize, lane: usize) -> u64 {
    PAGE + index as u64 * PAGE + if lane == 0 { 0 } else { LANE_B }
}
struct Store {
    file: File,
    visible: Arc<Vec<u8>>,
    sync_calls: usize,
    recovery_syncs: usize,
    poisoned: bool,
}
impl Store {
    fn create(path: &Path) -> io::Result<Self> {
        let mut file = OpenOptions::new().create_new(true).read(true).write(true).open(path)?;
        let mut zeros = vec![0; (LANE_B + (CAPACITY as u64 + 1) * PAGE) as usize];
        zeros[..N].copy_from_slice(&frame(EPOCH, 0, 0, CAPACITY as u8));
        file.write_all(&zeros)?;
        file.sync_all()?;
        File::open(path.parent().unwrap())?.sync_all()?;
        Ok(Self { file, visible: Arc::new(Vec::new()), sync_calls: 0, recovery_syncs: 0, poisoned: false })
    }
    fn open(path: &Path) -> io::Result<Self> {
        let mut file = OpenOptions::new().read(true).write(true).open(path)?;
        if file.metadata()?.len() != LANE_B + (CAPACITY as u64 + 1) * PAGE {
            return Err(io::Error::other("file length"));
        }
        let mut header = [0; N];
        file.read_exact(&mut header)?;
        if header != frame(EPOCH, 0, 0, CAPACITY as u8) {
            return Err(io::Error::other("unsupported or damaged generation anchor"));
        }
        let mut pairs = vec![[[0; N]; 2]; CAPACITY];
        for (i, pair) in pairs.iter_mut().enumerate() {
            for (lane, b) in pair.iter_mut().enumerate() {
                file.seek(SeekFrom::Start(offset(i, lane)))?; file.read_exact(b)?;
            }
        }
        let values = recover(&pairs, EPOCH).map_err(io::Error::other)?;
        let mut repaired = false;
        let mut previous = 0;
        for (i, &value) in values.iter().enumerate() {
            let canonical = frame(EPOCH, i as u64 + 1, previous, value);
            previous = u32_at(&canonical, 120);
            for lane in 0..2 {
                if pairs[i][lane] != canonical {
                    file.seek(SeekFrom::Start(offset(i, lane)))?;
                    file.write_all(&canonical)?;
                    repaired = true;
                }
            }
        }
        // Returning a writable store with only one intact copy would spend the next fault's redundancy.
        if repaired { file.sync_data()?; }
        Ok(Self { file, visible: Arc::new(values), sync_calls: 0, recovery_syncs: usize::from(repaired), poisoned: false })
    }
    fn append(&mut self, value: u8, fail_sync: bool, mut hook: impl FnMut(&str, usize)) -> io::Result<()> {
        if self.poisoned { return Err(io::Error::other("poisoned writer")); }
        let index = self.visible.len();
        if index == CAPACITY { return Err(io::Error::other("capacity")); }
        let mut previous = 0;
        for (i, &v) in self.visible.iter().enumerate() { previous = u32_at(&frame(EPOCH, i as u64 + 1, previous, v), 120); }
        let b = frame(EPOCH, index as u64 + 1, previous, value);
        self.poisoned = true;
        for lane in 0..2 {
            self.file.seek(SeekFrom::Start(offset(index, lane)))?;
            self.file.write_all(&b)?;
            hook(if lane == 0 { "after-a" } else { "after-b" }, self.visible.len());
        }
        if fail_sync { return Err(io::Error::other("injected error before sync")); }
        self.sync_calls += 1;
        self.file.sync_data()?;
        hook("after-sync", self.visible.len());
        let mut next = (*self.visible).clone(); next.push(value);
        self.visible = Arc::new(next);
        self.poisoned = false;
        hook("after-publish", self.visible.len());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    fn chain() -> (Frame, Frame) {
        let a = frame(EPOCH, 1, 0, 11);
        (a, frame(EPOCH, 2, u32_at(&a, 120), 22))
    }
    fn dir() -> PathBuf {
        let d = std::env::temp_dir().join(format!("catdb-dual-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        fs::create_dir(&d).unwrap(); d
    }
    #[test]
    fn model_single_copy_suffix_damage_recovers_acknowledged_record() {
        let (a, b) = chain();
        for lane in 0..2 { for cut in 0..N {
            let mut pair = [b, b]; pair[lane][cut..].fill(0);
            assert_eq!(recover(&[[a,a],pair], EPOCH).unwrap(), vec![11,22]);
        }}
        println!("256 one-copy suffix damage cases restored");
    }
    #[test]
    fn model_all_prefix_persistence_pairs_preserve_prior_acknowledgement() {
        let (a, b) = chain();
        for x in 0..=N { for y in 0..=N {
            let mut p = [[0;N];2]; p[0][..x].copy_from_slice(&b[..x]); p[1][..y].copy_from_slice(&b[..y]);
            let r = recover(&[[a,a],p],EPOCH).unwrap();
            assert_eq!(r[0],11);
            assert_eq!(r.len(), if x==N || y==N {2} else {1});
        }}
        println!("16641 prefix persistence combinations passed (not physical crash images)");
    }
    #[test]
    fn model_reordered_fragments_and_stale_generation_are_bounded() {
        let (a,b)=chain(); let stale=frame(6,2,u32_at(&a,120),33);
        for mask in 0..16 {
            let mut p=stale;
            for part in 0..4 { if mask & (1<<part)!=0 {p[part*32..part*32+32].copy_from_slice(&b[part*32..part*32+32]);} }
            let r=recover(&[[a,a],[p,b]],EPOCH).unwrap(); assert_eq!(r,vec![11,22]);
        }
        assert_eq!(recover(&[[a,a],[stale,stale]],EPOCH).unwrap(),vec![11]);
    }
    #[test]
    fn model_conflicting_valid_copies_wrong_order_and_holes_are_rejected() {
        let (a,b)=chain();let other=frame(EPOCH,2,u32_at(&a,120),33);
        assert!(recover(&[[a,a],[b,other]],EPOCH).is_err());
        assert!(recover(&[[[0;N];2],[b,b]],EPOCH).is_err());
        assert!(recover(&[[b,b]],EPOCH).is_err());
        let broken=frame(EPOCH,2,0,22);
        assert!(recover(&[[a,a],[broken,broken]],EPOCH).is_err());
    }
    #[test]
    fn model_both_copy_damage_remains_an_undetectable_counterexample() {
        let (a,b)=chain();let mut torn=b;torn[64..].fill(0);
        assert_eq!(recover(&[[a,a],[torn,torn]],EPOCH).unwrap(),vec![11]);
        println!("NEGATIVE CONTROL: damaging both acknowledged copies still loses sequence 2; outside bounded contract");
    }
    #[test]
    fn file_sync_before_publication_old_values_capacity_and_reopen() {
        let d=dir();let p=d.join("probe.log");let mut s=Store::create(&p).unwrap();
        assert!(Store::create(&p).is_err());assert!(Store::open(&p).unwrap().visible.is_empty());
        s.append(11,false,|stage,n| {if stage!="after-publish" {assert_eq!(n,0)}}).unwrap();
        let old=s.visible.clone();
        for v in [22,33,44] {s.append(v,false,|_,_|{}).unwrap();}
        assert_eq!(s.sync_calls,4);assert_eq!(*old,vec![11]);
        let before=fs::read(&p).unwrap();assert!(s.append(55,false,|_,_|{}).is_err());assert_eq!(before,fs::read(&p).unwrap());
        assert_eq!(*Store::open(&p).unwrap().visible,vec![11,22,33,44]);
    }
    #[test]
    fn file_one_copy_damage_restores_from_other_lane() {
        for lane in 0..2 {
            let d=dir();let p=d.join("probe.log");let mut s=Store::create(&p).unwrap();
            s.append(11,false,|_,_|{}).unwrap();s.append(22,false,|_,_|{}).unwrap();
            s.file.seek(SeekFrom::Start(offset(1,lane)+64)).unwrap();s.file.write_all(&[0;64]).unwrap();s.file.sync_data().unwrap();drop(s);
            let mut repaired=Store::open(&p).unwrap();assert_eq!(*repaired.visible,vec![11,22]);assert_eq!(repaired.recovery_syncs,1);
            assert_eq!(Store::open(&p).unwrap().recovery_syncs,0);
            repaired.file.seek(SeekFrom::Start(offset(1,1-lane)+64)).unwrap();repaired.file.write_all(&[0;64]).unwrap();repaired.file.sync_data().unwrap();drop(repaired);
            assert_eq!(*Store::open(&p).unwrap().visible,vec![11,22]);
        }
    }
    #[test]
    fn file_sync_error_injection_does_not_publish_and_poison_prevents_retry() {
        let d=dir();let p=d.join("probe.log");let mut s=Store::create(&p).unwrap();
        s.append(11,false,|_,_|{}).unwrap();assert!(s.append(22,true,|_,_|{}).is_err());
        assert_eq!(*s.visible,vec![11]);assert!(s.append(33,false,|_,_|{}).is_err());assert_eq!(s.sync_calls,1);
    }
    #[test]
    fn file_damaged_generation_anchor_is_rejected_without_repair() {
        let d=dir();let p=d.join("probe.log");let mut s=Store::create(&p).unwrap();
        s.append(11,false,|_,_|{}).unwrap();s.file.seek(SeekFrom::Start(8)).unwrap();s.file.write_all(&[0;8]).unwrap();s.file.sync_data().unwrap();drop(s);
        let before=fs::read(&p).unwrap();assert!(Store::open(&p).is_err());assert_eq!(before,fs::read(&p).unwrap());
    }
    #[test]
    #[ignore]
    fn child_phase() {
        let p=PathBuf::from(std::env::var_os("DUAL_PATH").unwrap());
        let ready=PathBuf::from(std::env::var_os("DUAL_READY").unwrap());
        let target=std::env::var("DUAL_PHASE").unwrap();let mut s=Store::open(&p).unwrap();
        s.append(22,false,|phase,n| { if phase==target {fs::write(&ready,format!("{phase}|{n}")).unwrap();loop {std::thread::park();}} }).unwrap();
    }
    #[test]
    fn file_parent_kills_children_before_and_after_sync_and_publication() {
        for phase in ["after-a","after-b","after-sync","after-publish"] {
            let d=dir();let p=d.join("probe.log");let ready=d.join("ready");let mut s=Store::create(&p).unwrap();s.append(11,false,|_,_|{}).unwrap();drop(s);
            let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","tests::child_phase","--ignored","--nocapture"]).env("DUAL_PATH",&p).env("DUAL_READY",&ready).env("DUAL_PHASE",phase).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn().unwrap();
            let end=Instant::now()+Duration::from_secs(5);
            let expected=format!("{phase}|{}",if phase=="after-publish" {2}else{1});
            loop {
                if fs::read_to_string(&ready).ok().as_deref()==Some(&expected) {break;}
                if Instant::now()>end {let _=child.kill();let _=child.wait();panic!("child barrier timed out");}
                assert!(child.try_wait().unwrap().is_none(),"child exited before barrier");std::thread::sleep(Duration::from_millis(10));
            }
            child.kill().unwrap();assert!(!child.wait().unwrap().success());
            let r=Store::open(&p).unwrap();assert_eq!(r.visible[0],11);
            if phase=="after-sync" || phase=="after-publish" {assert_eq!(*r.visible,vec![11,22]);}
            else {assert!(r.visible.len()==1 || r.visible.len()==2);}
            println!("external process kill at {phase}: recovered {:?}",r.visible);
        }
    }
}
