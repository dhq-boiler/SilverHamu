use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

const N: usize = 64;
const CAP: usize = 4;
const PAGE: u64 = 4096;
const BLOCKS: usize = 4 + 2 * (CAP + 1) * 2;
type Block = [u8; N];
type Image = Vec<Block>;
fn crc(b: &[u8]) -> u32 {
    let mut c = !0u32;
    for &v in b { c ^= u32::from(v); for _ in 0..8 {c=(c>>1)^(0xedb88320 & 0u32.wrapping_sub(c&1));} }
    !c
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Record { kind: u8, bank: u8, epoch: u64, seq: u64, previous: u32, values: [u8;4] }
impl Record {
    fn encode(self) -> Block {
        let mut b=[0;N];b[..8].copy_from_slice(b"CATGEN01");b[8]=self.kind;b[9]=self.bank;
        b[16..24].copy_from_slice(&self.epoch.to_le_bytes());b[24..32].copy_from_slice(&self.seq.to_le_bytes());
        b[32..36].copy_from_slice(&self.previous.to_le_bytes());b[36..40].copy_from_slice(&self.values);
        b[40..48].copy_from_slice(b"COMMIT01");let c=crc(&b[..56]);b[56..60].copy_from_slice(&c.to_le_bytes());b[60..].copy_from_slice(&(!c).to_le_bytes());b
    }
    fn decode(b: &Block) -> Option<Self> {
        let c=u32::from_le_bytes(b[56..60].try_into().unwrap());
        if &b[..8]!=b"CATGEN01" || ![1,2].contains(&b[8]) || b[9]>1 || b[10..16]!=[0;6]
            || &b[40..48]!=b"COMMIT01" || b[48..56]!=[0;8] || c!=crc(&b[..56])
            || u32::from_le_bytes(b[60..].try_into().unwrap())!=!c {return None;}
        let r=Self{kind:b[8],bank:b[9],epoch:u64::from_le_bytes(b[16..24].try_into().unwrap()),seq:u64::from_le_bytes(b[24..32].try_into().unwrap()),previous:u32::from_le_bytes(b[32..36].try_into().unwrap()),values:b[36..40].try_into().unwrap()};
        if r.epoch==0 || (r.kind==2 && (r.previous!=0 || r.values!=[0;4])) {None} else {Some(r)}
    }
    fn checksum(self)->u32 {u32::from_le_bytes(self.encode()[56..60].try_into().unwrap())}
    fn selector(self)->Self {Self{kind:2,previous:0,values:[0;4],..self}}
}
fn root(epoch:u64,lane:usize)->usize {(epoch%2) as usize*2+lane}
fn at(bank:u8, slot:usize, lane:usize)->usize {4+usize::from(bank)*(CAP+1)*2+slot*2+lane}
fn err(s:&str)->io::Error {io::Error::other(s.to_owned())}
#[derive(Debug)]
struct Recovered { selector:Record, last:Record, used:usize, repairs:Vec<(usize,Block,bool)> }
fn recover(image:&[Block])->io::Result<Recovered> {
    if image.len()!=BLOCKS {return Err(err("image length"));}
    let mut roots:Vec<Record>=image[..4].iter().filter_map(Record::decode).filter(|r|r.kind==2).collect();
    roots.sort_by_key(|r|r.epoch);let selector=*roots.last().ok_or_else(||err("no valid selector"))?;
    if roots.iter().any(|r|r.epoch==selector.epoch && *r!=selector) {return Err(err("selectors disagree"));}
    for (i,b) in image[..4].iter().enumerate() {if let Some(r)=Record::decode(b) {if r.kind==2 && i/2!=(r.epoch%2) as usize {return Err(err("selector slot identity"));}}}
    let mut repairs=Vec::new();let mut last=None;let mut used=0;let mut gap=false;
    for slot in 0..=CAP {
        let copies:Vec<Record>=(0..2).filter_map(|lane|Record::decode(&image[at(selector.bank,slot,lane)]))
            .filter(|r|r.kind==1 && r.epoch==selector.epoch && r.bank==selector.bank).collect();
        if copies.is_empty() {if slot==0 {return Err(err("selected checkpoint missing"));}gap=true;continue;}
        if gap {return Err(err("valid record after missing pair"));}
        if copies.len()==2 && copies[0]!=copies[1] {return Err(err("data copies disagree"));}
        let r=copies[0];
        if slot==0 {if r.seq!=selector.seq || r.previous!=0 {return Err(err("checkpoint identity"));}}
        else {let prev:Record=last.unwrap();if prev.seq.checked_add(1)!=Some(r.seq) || r.previous!=prev.checksum() {return Err(err("sequence or chain"));}}
        let b=r.encode();for lane in 0..2 {let i=at(selector.bank,slot,lane);if image[i]!=b {repairs.push((i,b,false));}}
        last=Some(r);used=slot;
    }
    for lane in 0..2 {let i=root(selector.epoch,lane);if image[i]!=selector.encode() {repairs.push((i,selector.encode(),true));}}
    Ok(Recovered{selector,last:last.unwrap(),used,repairs})
}
fn initial()->Image {
    let mut a=vec![[0;N];BLOCKS];let r=Record{kind:1,bank:0,epoch:1,seq:0,previous:0,values:[0;4]};
    for lane in 0..2 {a[root(r.epoch,lane)]=r.selector().encode();a[at(0,0,lane)]=r.encode();}a
}
type Hook<'a> = &'a mut dyn FnMut(&str)->io::Result<()>;
#[derive(Default, Debug)]
struct Costs { categories:BTreeMap<&'static str,(u64,u64)> }
impl Costs {
    fn write(&mut self,k:&'static str,n:usize){self.categories.entry(k).or_default().0+=n as u64;}
    fn sync(&mut self,k:&'static str){self.categories.entry(k).or_default().1+=1;}
}
struct Store { file:File, state:Recovered, visible:Arc<[u8;4]>, poisoned:bool, costs:Costs }
impl Store {
    fn create(path:&Path)->io::Result<Self> {
        let mut file=OpenOptions::new().create_new(true).read(true).write(true).open(path)?;
        let image=initial();let mut buffer=vec![0;BLOCKS*PAGE as usize];
        for (i,b) in image.iter().enumerate(){buffer[i*PAGE as usize..i*PAGE as usize+N].copy_from_slice(b);}
        file.write_all(&buffer)?;file.sync_all()?;File::open(path.parent().unwrap())?.sync_all()?;
        let mut costs=Costs::default();costs.write("initialization",buffer.len());costs.sync("initialization");costs.sync("initialization");
        Ok(Self{file,state:recover(&image)?,visible:Arc::new([0;4]),poisoned:false,costs})
    }
    fn image(file:&mut File)->io::Result<Image> {
        if file.metadata()?.len()!=BLOCKS as u64*PAGE {return Err(err("file length"));}
        let mut image=vec![[0;N];BLOCKS];for (i,b) in image.iter_mut().enumerate(){file.seek(SeekFrom::Start(i as u64*PAGE))?;file.read_exact(b)?;}Ok(image)
    }
    fn open(path:&Path,hook:Hook<'_>)->io::Result<Self> {
        let mut file=OpenOptions::new().read(true).write(true).open(path)?;
        let state=recover(&Self::image(&mut file)?)?;let visible=Arc::new(state.last.values);
        let mut s=Self{file,state,visible,poisoned:true,costs:Costs::default()};
        for meta in [false,true] {
            let repairs:Vec<_>=s.state.repairs.iter().copied().filter(|p|p.2==meta).collect();
            let category=if meta {"repair-selector"} else {"repair-data"};
            for (i,b,_) in &repairs {s.put(*i,b,category,&format!("{category}-{i}"),hook)?;}
            if !repairs.is_empty(){s.sync(category,hook)?;}
        }
        s.state.repairs.clear();s.poisoned=false;hook("open-ready")?;Ok(s)
    }
    fn put(&mut self,index:usize,b:&Block,category:&'static str,point:&str,hook:Hook<'_>)->io::Result<()> {
        self.file.seek(SeekFrom::Start(index as u64*PAGE))?;
        self.file.write_all(&b[..N/2])?;self.costs.write(category,N/2);hook(&format!("{point}.half"))?;
        self.file.write_all(&b[N/2..])?;self.costs.write(category,N/2);hook(&format!("{point}.written"))
    }
    fn sync(&mut self,category:&'static str,hook:Hook<'_>)->io::Result<()> {
        hook(&format!("{category}.before-sync"))?;self.costs.sync(category);self.file.sync_data()?;hook(&format!("{category}.synced"))
    }
    fn append(&mut self,key:usize,value:u8,hook:Hook<'_>)->io::Result<()> {
        if self.poisoned {return Err(err("poisoned"));}
        if key>=4 || self.state.used==CAP {return Err(err("capacity or key"));}
        let seq=self.state.last.seq.checked_add(1).ok_or_else(||err("sequence overflow"))?;
        let mut values=*self.visible;values[key]=value;
        let r=Record{seq,values,previous:self.state.last.checksum(),..self.state.last};let slot=self.state.used+1;
        self.poisoned=true;for lane in 0..2 {self.put(at(r.bank,slot,lane),&r.encode(),"append",&format!("append-{lane}"),hook)?;}
        self.sync("append",hook)?;self.state.last=r;self.state.used=slot;self.visible=Arc::new(values);hook("append.published")?;self.poisoned=false;Ok(())
    }
    fn switch(&mut self,hook:Hook<'_>)->io::Result<()> {
        if self.poisoned {return Err(err("poisoned"));}
        let epoch=self.state.selector.epoch.checked_add(1).ok_or_else(||err("epoch overflow"))?;
        let old=self.state.selector.bank;let new=1-old;
        let r=Record{epoch,bank:new,previous:0,..self.state.last};self.poisoned=true;
        for slot in 0..=CAP {for lane in 0..2 {self.put(at(new,slot,lane),&[0;N],"switch-data","switch-clear",hook)?;}}
        for lane in 0..2 {self.put(at(new,0,lane),&r.encode(),"switch-data",&format!("switch-data-{lane}"),hook)?;}
        self.sync("switch-data",hook)?;
        for lane in 0..2 {self.put(root(epoch,lane),&r.selector().encode(),"switch-selector",&format!("switch-selector-{lane}"),hook)?;}
        self.sync("switch-selector",hook)?;self.state=Recovered{selector:r.selector(),last:r,used:0,repairs:Vec::new()};hook("switch.published")?;
        // Clearing the old bank before both selectors are synced could destroy the only restart target.
        for slot in 0..=CAP {for lane in 0..2 {self.put(at(old,slot,lane),&[0;N],"reclaim","reclaim",hook)?;}}
        self.sync("reclaim",hook)?;self.poisoned=false;Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::{Command,Stdio};
    use std::time::{Duration,Instant};
    fn noop(_: &str)->io::Result<()>{Ok(())}
    fn dir()->PathBuf {let p=std::env::temp_dir().join(format!("catdb-generation-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir(&p).unwrap();p}
    fn fixture()->(PathBuf,Store){let p=dir().join("probe.log");let mut s=Store::create(&p).unwrap();s.append(0,11,&mut noop).unwrap();(p,s)}
    fn damage(p:&Path,index:usize){let mut f=OpenOptions::new().write(true).open(p).unwrap();f.seek(SeekFrom::Start(index as u64*PAGE+N as u64/2)).unwrap();f.write_all(&[0;N/2]).unwrap();f.sync_data().unwrap();}
    fn model_current()->Image {let mut a=initial();let first=Record::decode(&a[at(0,0,0)]).unwrap();let r=Record{seq:1,previous:first.checksum(),values:[11,0,0,0],..first};for lane in 0..2 {a[at(0,1,lane)]=r.encode();}a}
    #[test]
    fn model_repair_interruption_preserves_good_copy_and_detects_conflict(){
        let base=model_current();let good=base[at(0,1,0)];
        for cut in 0..=N {let mut a=base.clone();a[at(0,1,1)]=[0;N];a[at(0,1,1)][..cut].copy_from_slice(&good[..cut]);assert_eq!(recover(&a).unwrap().last.values,[11,0,0,0]);assert_eq!(a[at(0,1,0)],good);}
        let mut a=base;let mut r=Record::decode(&good).unwrap();r.values[0]=99;a[at(0,1,1)]=r.encode();assert!(recover(&a).is_err());
    }
    #[test]
    fn model_all_selector_prefix_pairs_recover_same_acknowledged_values(){
        let mut a=model_current();let old=recover(&a).unwrap();let new=Record{epoch:2,bank:1,previous:0,..old.last};
        for lane in 0..2 {a[at(1,0,lane)]=new.encode();}
        for x in 0..=N {for y in 0..=N {let mut b=a.clone();b[0][..x].copy_from_slice(&new.selector().encode()[..x]);b[1][..y].copy_from_slice(&new.selector().encode()[..y]);let r=recover(&b).unwrap();assert_eq!(r.last.values,[11,0,0,0]);}}
        println!("4225 selector persistence combinations checked; old selector pair remains readable even if both new selectors are partial");
    }
    #[test]
    fn model_old_bank_partial_reclaim_and_stale_records_do_not_change_current_state(){
        let mut a=model_current();let old=recover(&a).unwrap();let new=Record{epoch:2,bank:1,previous:0,..old.last};
        for lane in 0..2 {a[at(1,0,lane)]=new.encode();a[root(new.epoch,lane)]=new.selector().encode();}
        a[at(1,1,0)]=old.last.encode();a[at(1,1,1)]=old.last.encode();
        for cut in 0..=N {let mut b=a.clone();b[at(0,1,0)][..cut].fill(0);assert_eq!(recover(&b).unwrap().last.values,[11,0,0,0]);}
    }
    #[test]
    fn model_selected_missing_checkpoint_is_not_silently_rolled_back(){let mut a=model_current();let r=recover(&a).unwrap();a[0]=Record{epoch:2,bank:1,..r.selector}.encode();assert!(recover(&a).is_err());}
    #[test]
    fn model_second_generation_selector_reuse_preserves_current_checkpoint(){
        let mut a=model_current();let old=recover(&a).unwrap();let current=Record{epoch:2,bank:1,previous:0,..old.last};
        for lane in 0..2{a[at(1,0,lane)]=current.encode();a[root(2,lane)]=current.selector().encode();}
        let next=Record{epoch:3,bank:0,..current};
        for slot in 0..=CAP{for lane in 0..2{a[at(0,slot,lane)]=[0;N];}}
        for lane in 0..2{a[at(0,0,lane)]=next.encode();}
        for x in 0..=N{for y in 0..=N{let mut b=a.clone();b[root(3,0)][..x].copy_from_slice(&next.selector().encode()[..x]);b[root(3,1)][..y].copy_from_slice(&next.selector().encode()[..y]);assert_eq!(recover(&b).unwrap().last.values,[11,0,0,0]);}}
        println!("4225 selector reuse combinations over stale generation-1 selectors passed");
    }
    #[test]
    fn model_single_current_selector_damage_does_not_choose_reclaimed_old_bank(){
        let mut a=model_current();let old=recover(&a).unwrap();let current=Record{epoch:2,bank:1,previous:0,..old.last};
        for lane in 0..2{a[at(1,0,lane)]=current.encode();a[root(2,lane)]=current.selector().encode();}
        for slot in 0..=CAP{for lane in 0..2{a[at(0,slot,lane)]=[0;N];}}
        for lane in 0..2{for cut in 0..=N{let mut b=a.clone();b[root(2,lane)][cut..].fill(0);let r=recover(&b).unwrap();assert_eq!(r.selector.epoch,2);assert_eq!(r.last.values,[11,0,0,0]);}}
    }
    #[test]
    fn model_overwriting_only_two_selectors_can_lose_both_on_interruption(){
        let mut a=model_current();let old=recover(&a).unwrap();let new=Record{epoch:2,bank:1,..old.selector}.encode();
        for lane in 0..2{let index=root(1,lane);a[index][..32].copy_from_slice(&new[..32]);assert!(Record::decode(&a[index]).is_none());}
        assert!(recover(&a).is_err());println!("NEGATIVE CONTROL: overwriting the only selector pair can leave no restart selector");
    }
    #[test]
    fn model_both_copy_damage_is_retained_as_negative_control(){let mut a=model_current();for lane in 0..2 {a[at(0,1,lane)][N/2..].fill(0);}assert_eq!(recover(&a).unwrap().last.values,[0;4]);println!("NEGATIVE CONTROL: both-copy damage can lose acknowledged update, outside contract");}
    #[test]
    fn file_repeated_switches_capacity_overflow_old_snapshot_and_costs(){
        let (p,mut s)=fixture();assert!(Store::create(&p).is_err());let old=s.visible.clone();
        for round in 0..3 {while s.state.used<CAP {s.append(1,20+round,&mut noop).unwrap();}let before=fs::read(&p).unwrap();assert!(s.append(0,99,&mut noop).is_err());assert_eq!(fs::read(&p).unwrap(),before);s.switch(&mut noop).unwrap();s.append(2,30+round,&mut noop).unwrap();}
        assert_eq!(*old,[11,0,0,0]);assert_eq!(*s.visible,[11,22,32,0]);assert_eq!(*Store::open(&p,&mut noop).unwrap().visible,*s.visible);
        let before=fs::read(&p).unwrap();s.state.selector.epoch=u64::MAX;assert!(s.switch(&mut noop).is_err());assert_eq!(fs::read(&p).unwrap(),before);
        s.state.last.seq=u64::MAX;assert!(s.append(0,33,&mut noop).is_err());assert_eq!(fs::read(&p).unwrap(),before);
        println!("code-path costs (write bytes, sync attempts): {:?}",s.costs.categories);
        assert_eq!(s.costs.categories["switch-data"],(2304,3));assert_eq!(s.costs.categories["switch-selector"],(384,3));assert_eq!(s.costs.categories["reclaim"],(1920,3));
    }
    #[test]
    fn file_repair_errors_block_writable_open_and_preserve_good_data(){
        for point in ["repair-data-7.half","repair-data.before-sync","repair-data.synced","repair-selector-3.half","repair-selector.before-sync","repair-selector.synced"] {
            let (p,s)=fixture();drop(s);damage(&p,at(0,1,1));damage(&p,3);
            let mut reached=false;let r=Store::open(&p,&mut |name|{if name==point {reached=true;Err(err("injected phase failure"))}else{Ok(())}});assert!(reached && r.is_err(),"{point}");
            let s=Store::open(&p,&mut noop).unwrap();assert_eq!(*s.visible,[11,0,0,0]);
        }
    }
    #[test]
    fn file_switch_failures_poison_writer_and_reopen_preserves_values(){
        for point in ["switch-data.before-sync","switch-data.synced","switch-selector.before-sync","switch-selector.synced","reclaim.before-sync","reclaim.synced"] {
            let (p,mut s)=fixture();let mut reached=false;
            assert!(s.switch(&mut |name|{if name==point {reached=true;Err(err("injected phase failure"))}else{Ok(())}}).is_err());assert!(reached);assert!(s.append(1,55,&mut noop).is_err());drop(s);assert_eq!(*Store::open(&p,&mut noop).unwrap().visible,[11,0,0,0]);
        }
    }
    #[test]
    fn file_repair_costs_and_clean_reopen(){let(p,s)=fixture();drop(s);damage(&p,7);damage(&p,3);let s=Store::open(&p,&mut noop).unwrap();assert_eq!(s.costs.categories["repair-data"],(64,1));assert_eq!(s.costs.categories["repair-selector"],(64,1));println!("repair costs (write bytes, sync attempts): {:?}",s.costs.categories);assert!(Store::open(&p,&mut noop).unwrap().costs.categories.is_empty());}
    #[test]
    #[ignore]
    fn child_phase(){let p=PathBuf::from(std::env::var_os("GEN_PATH").unwrap());let ready=PathBuf::from(std::env::var_os("GEN_READY").unwrap());let target=std::env::var("GEN_PHASE").unwrap();let mode=std::env::var("GEN_MODE").unwrap();let mut hook=|name:&str| {if name==target {fs::write(&ready,name)?;loop{std::thread::park();}}Ok(())};let mut s=Store::open(&p,&mut hook).unwrap();if mode=="switch" {s.switch(&mut hook).unwrap();}}
    #[test]
    fn file_external_kills_at_repair_and_switch_boundaries(){
        let groups=[("repair",vec!["repair-data-7.half","repair-data-7.written","repair-data.before-sync","repair-data.synced","repair-selector-3.half","repair-selector-3.written","repair-selector.before-sync","repair-selector.synced","open-ready"]),("switch",vec!["switch-clear.half","switch-data-0.half","switch-data-1.written","switch-data.before-sync","switch-data.synced","switch-selector-0.half","switch-selector-0.written","switch-selector-1.half","switch-selector.before-sync","switch-selector.synced","switch.published","reclaim.half","reclaim.before-sync","reclaim.synced"])];
        for(mode,points)in groups{for point in points{for cycle in 0..if mode=="switch" {2}else{1}{
            let(p,mut s)=fixture();
            if cycle==1 {assert!(s.switch(&mut |name|if name=="switch.published"{Err(err("leave old bank for reuse fixture"))}else{Ok(())}).is_err());}
            drop(s);if mode=="repair" {damage(&p,7);damage(&p,3);}let ready=p.with_extension("ready");
            let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","tests::child_phase","--ignored","--nocapture"]).env("GEN_PATH",&p).env("GEN_READY",&ready).env("GEN_PHASE",point).env("GEN_MODE",mode).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn().unwrap();let deadline=Instant::now()+Duration::from_secs(5);
            loop {if fs::read_to_string(&ready).ok().as_deref()==Some(point){break;}if Instant::now()>deadline{let _=child.kill();let _=child.wait();panic!("barrier timeout {point}");}assert!(child.try_wait().unwrap().is_none(),"early child exit {point}");std::thread::sleep(Duration::from_millis(10));}
            child.kill().unwrap();assert!(!child.wait().unwrap().success());let mut s=Store::open(&p,&mut noop).unwrap();assert_eq!(*s.visible,[11,0,0,0],"{point}");s.append(3,44,&mut noop).unwrap();drop(s);assert_eq!(*Store::open(&p,&mut noop).unwrap().visible,[11,0,0,44]);println!("external kill recovered and resumed: {mode}/cycle-{cycle}/{point}");
        }}}
    }
}
