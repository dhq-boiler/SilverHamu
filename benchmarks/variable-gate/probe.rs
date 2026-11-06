use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

const N: usize = 16384;
const H: usize = 64;
const PAYLOAD: usize = N-H;
use super::{encode_frame, decode_frame, WriteSet, MAX_KEY_BYTES, MAX_VALUE_BYTES, MAX_TRANSACTION_BYTES, MAX_TRANSACTION_WRITES, PREFIX_BYTES, TRAILER_BYTES, FRAME, SEAL};
type Data = BTreeMap<Vec<u8>,Vec<u8>>;
const CAP: usize = 4;
const PAGE: u64 = N as u64;
const BLOCKS: usize = 4 + 2 * (CAP + 1) * 2;
type Block = [u8; N];
type Image = Vec<Block>;
fn crc(b: &[u8]) -> u32 {
    let mut c = !0u32;
    for &v in b { c ^= u32::from(v); for _ in 0..8 {c=(c>>1)^(0xedb88320 & 0u32.wrapping_sub(c&1));} }
    !c
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Record { kind:u8,bank:u8,epoch:u64,seq:u64,previous:u32,payload_len:u32,payload:[u8;PAYLOAD] }
impl Record {
    fn encode(self)->Block {
        let mut b=[0;N];b[..8].copy_from_slice(b"CATVAR01");b[8]=self.kind;b[9]=self.bank;
        b[16..24].copy_from_slice(&self.epoch.to_le_bytes());b[24..32].copy_from_slice(&self.seq.to_le_bytes());b[32..36].copy_from_slice(&self.previous.to_le_bytes());
        b[36..40].copy_from_slice(&self.payload_len.to_le_bytes());b[40..44].copy_from_slice(&(!self.payload_len).to_le_bytes());let n=self.payload_len as usize;
        b[44..48].copy_from_slice(&crc(&self.payload[..n]).to_le_bytes());b[48..56].copy_from_slice(b"COMMIT01");let c=crc(&b[..56]);b[56..60].copy_from_slice(&c.to_le_bytes());b[60..64].copy_from_slice(&(!c).to_le_bytes());b[H..H+n].copy_from_slice(&self.payload[..n]);b
    }
    fn decode(b:&Block)->Option<Self> {
        let u=|p|u32::from_le_bytes(b[p..p+4].try_into().unwrap());let n=u(36) as usize;
        if &b[..8]!=b"CATVAR01" || ![1,2,3].contains(&b[8]) || b[9]>1 || b[10..16]!=[0;6] || &b[48..56]!=b"COMMIT01" || u(56)!=crc(&b[..56]) || u(60)!=!u(56) || u(40)!=!u(36) || n>PAYLOAD {return None;}
        if u(44)!=crc(&b[H..H+n]) {return None;}
        let mut r=Self{kind:b[8],bank:b[9],epoch:u64::from_le_bytes(b[16..24].try_into().unwrap()),seq:u64::from_le_bytes(b[24..32].try_into().unwrap()),previous:u(32),payload_len:n as u32,payload:[0;PAYLOAD]};
        if r.epoch==0 || (r.kind==2 && (n!=0 || r.previous!=0)){return None;}
        r.payload[..n].copy_from_slice(&b[H..H+n]);Some(r)
    }
    fn checksum(self)->u32{u32::from_le_bytes(self.encode()[56..60].try_into().unwrap())}
    fn selector(self)->Self{Self{kind:2,previous:0,payload_len:0,payload:[0;PAYLOAD],..self}}
    fn writes(&self)->io::Result<WriteSet>{
        let n=self.payload_len as usize;
        if self.kind==1 && n==0 {return Ok(WriteSet::new());}
        if n<PREFIX_BYTES+12+TRAILER_BYTES{return Err(err("short inner frame"));}
        let b=&self.payload[..n];let len=u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize;
        if &b[..8]!=FRAME || u32::from_le_bytes(b[12..16].try_into().unwrap())!=!(len as u32) || u32::from_le_bytes(b[16..20].try_into().unwrap())!=crate::crc32(&b[..16]) || len>MAX_TRANSACTION_BYTES || len+PREFIX_BYTES+TRAILER_BYTES!=n{return Err(err("inner length or prefix"));}
        let body=&b[PREFIX_BYTES..PREFIX_BYTES+len];let tail=&b[PREFIX_BYTES+len..];
        if u32::from_le_bytes(tail[..4].try_into().unwrap())!=crate::crc32(body) || &tail[4..]!=SEAL {return Err(err("inner checksum or seal"));}
        let(seq,writes)=decode_frame(body).map_err(|e|err(&format!("inner decode: {e:?}")))?;
        if seq!=self.seq || (self.kind==1 && writes.values().any(Option::is_none)){return Err(err("inner sequence or checkpoint deletion"));}Ok(writes)
    }
}
fn preflight(writes:&WriteSet)->io::Result<usize>{
    if writes.is_empty(){return Ok(0);}
    if writes.len()>MAX_TRANSACTION_WRITES{return Err(err("operation count"));}
    let mut body=12usize;
    for(k,v)in writes{if k.len()>MAX_KEY_BYTES || v.as_ref().is_some_and(|v|v.len()>MAX_VALUE_BYTES){return Err(err("key or value length"));}
        body=body.checked_add(9).and_then(|n|n.checked_add(k.len())).and_then(|n|n.checked_add(v.as_ref().map_or(0,Vec::len))).ok_or_else(||err("length overflow"))?;}
    if body>MAX_TRANSACTION_BYTES{return Err(err("transaction bytes"));}Ok(body+PREFIX_BYTES+TRAILER_BYTES)
}
fn pack(kind:u8,bank:u8,epoch:u64,seq:u64,previous:u32,writes:&WriteSet)->io::Result<(Record,usize,usize)>{
    let len=preflight(writes)?;if len>PAYLOAD{return Err(err("experimental slot capacity"));}
    if kind==3 && len==0{return Err(err("empty transaction record"));}
    let b=if len==0{Vec::new()}else{encode_frame(seq,writes)};
    if b.len()!=len{return Err(err("encoder length mismatch"));}
    let mut r=Record{kind,bank,epoch,seq,previous,payload_len:len as u32,payload:[0;PAYLOAD]};r.payload[..len].copy_from_slice(&b);
    if kind!=2 {r.writes()?;}Ok((r,b.capacity(),b.len()))
}
fn apply(rows:&mut Data,writes:&WriteSet){for(k,v)in writes{if let Some(v)=v{rows.insert(k.clone(),v.clone());}else{rows.remove(k);}}}

fn root(epoch:u64,lane:usize)->usize {(epoch%2) as usize*2+lane}
fn at(bank:u8, slot:usize, lane:usize)->usize {4+usize::from(bank)*(CAP+1)*2+slot*2+lane}
fn err(s:&str)->io::Error {io::Error::other(s.to_owned())}
#[derive(Debug)]
struct Recovered { selector:Record, last:Record, used:usize, repairs:Vec<(usize,Block,bool)>, rows:Data }
fn recover(image:&[Block])->io::Result<Recovered> {
    if image.len()!=BLOCKS {return Err(err("image length"));}
    let mut roots:Vec<Record>=image[..4].iter().filter_map(Record::decode).filter(|r|r.kind==2).collect();
    roots.sort_by_key(|r|r.epoch);let selector=*roots.last().ok_or_else(||err("no valid selector"))?;
    if roots.iter().any(|r|r.epoch==selector.epoch && *r!=selector) {return Err(err("selectors disagree"));}
    for (i,b) in image[..4].iter().enumerate() {if let Some(r)=Record::decode(b) {if r.kind==2 && i/2!=(r.epoch%2) as usize {return Err(err("selector slot identity"));}}}
    let mut rows=Data::new();let mut repairs=Vec::new();let mut last=None;let mut used=0;let mut gap=false;
    for slot in 0..=CAP {
        let copies:Vec<Record>=(0..2).filter_map(|lane|Record::decode(&image[at(selector.bank,slot,lane)]))
            .filter(|r|(r.kind==1 || r.kind==3) && r.epoch==selector.epoch && r.bank==selector.bank).collect();
        if copies.is_empty() {if slot==0 {return Err(err("selected checkpoint missing"));}gap=true;continue;}
        // An envelope CRC over a complete CRC-protected frame cannot prove inner identity.
        let count=copies.len();let mut stale=0;
        let copies:Vec<Record>=copies.into_iter().filter(|r|{
            if r.writes().is_ok(){return true;}
            if r.kind==3 && r.payload_len as usize>=PREFIX_BYTES+12+TRAILER_BYTES{
                let inner_seq=u64::from_le_bytes(r.payload[PREFIX_BYTES..PREFIX_BYTES+8].try_into().unwrap());
                if inner_seq<r.seq && (Record{seq:inner_seq,..*r}).writes().is_ok(){stale+=1;}
            }
            false
        }).collect();
        if copies.is_empty(){
            // Only a fully validated older transaction proves stale payload; arbitrary corruption stays an error.
            if stale==count && slot!=0{gap=true;continue;}
            return Err(err("no valid inner copy"));
        }
        if gap {return Err(err("valid record after missing pair"));}
        if copies.len()==2 && copies[0]!=copies[1] {return Err(err("data copies disagree"));}
        let r=copies[0];
        if slot==0 {if r.kind!=1 || r.seq!=selector.seq || r.previous!=0 {return Err(err("checkpoint identity"));}}
        else {let prev:Record=last.unwrap();if r.kind!=3 || prev.seq.checked_add(1)!=Some(r.seq) || r.previous!=prev.checksum() {return Err(err("sequence or chain"));}}
        let writes=r.writes()?;apply(&mut rows,&writes);
        let b=r.encode();for lane in 0..2 {let i=at(selector.bank,slot,lane);if image[i][..H+r.payload_len as usize]!=b[..H+r.payload_len as usize] {repairs.push((i,b,false));}}
        last=Some(r);used=slot;
    }
    for lane in 0..2 {let i=root(selector.epoch,lane);if image[i][..H]!=selector.encode()[..H] {repairs.push((i,selector.encode(),true));}}
    Ok(Recovered{selector,last:last.unwrap(),used,repairs,rows})
}
fn initial()->Image {
    let mut a=vec![[0;N];BLOCKS];let r=Record{kind:1,bank:0,epoch:1,seq:0,previous:0,payload_len:0,payload:[0;PAYLOAD]};
    for lane in 0..2 {a[root(r.epoch,lane)]=r.selector().encode();a[at(0,0,lane)]=r.encode();}a
}
type Hook<'a> = &'a mut dyn FnMut(&str)->io::Result<()>;
#[derive(Default, Debug)]
struct Costs { categories:BTreeMap<&'static str,(u64,u64)>, frame_capacity_peak:usize, frame_length_peak:usize, checkpoint_clone_bytes:usize }
impl Costs {
    fn write(&mut self,k:&'static str,n:usize){self.categories.entry(k).or_default().0+=n as u64;}
    fn sync(&mut self,k:&'static str){self.categories.entry(k).or_default().1+=1;}
}
struct Store { file:File, state:Recovered, visible:Arc<Data>, poisoned:bool, costs:Costs }
impl Store {
    fn create(path:&Path)->io::Result<Self> {
        let mut file=OpenOptions::new().create_new(true).read(true).write(true).open(path)?;
        let image=initial();let mut buffer=vec![0;BLOCKS*PAGE as usize];
        for (i,b) in image.iter().enumerate(){buffer[i*PAGE as usize..i*PAGE as usize+N].copy_from_slice(b);}
        file.write_all(&buffer)?;file.sync_all()?;File::open(path.parent().unwrap())?.sync_all()?;
        let mut costs=Costs::default();costs.write("initialization",buffer.len());costs.sync("initialization");costs.sync("initialization");
        Ok(Self{file,state:recover(&image)?,visible:Arc::new(Data::new()),poisoned:false,costs})
    }
    fn image(file:&mut File)->io::Result<Image> {
        if file.metadata()?.len()!=BLOCKS as u64*PAGE {return Err(err("file length"));}
        let mut image=vec![[0;N];BLOCKS];for (i,b) in image.iter_mut().enumerate(){file.seek(SeekFrom::Start(i as u64*PAGE))?;file.read_exact(b)?;}Ok(image)
    }
    fn open(path:&Path,hook:Hook<'_>)->io::Result<Self> {
        let mut file=OpenOptions::new().read(true).write(true).open(path)?;
        let mut state=recover(&Self::image(&mut file)?)?;let visible=Arc::new(std::mem::take(&mut state.rows));
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
        let size=Record::decode(b).map_or(N,|r|H+r.payload_len as usize);let half=size/2;
        self.file.write_all(&b[..half])?;self.costs.write(category,half);hook(&format!("{point}.half"))?;
        self.file.write_all(&b[half..size])?;self.costs.write(category,size-half);hook(&format!("{point}.written"))
    }
    fn sync(&mut self,category:&'static str,hook:Hook<'_>)->io::Result<()> {
        hook(&format!("{category}.before-sync"))?;self.costs.sync(category);self.file.sync_data()?;hook(&format!("{category}.synced"))
    }
    fn append(&mut self,writes:&WriteSet,hook:Hook<'_>)->io::Result<()> {
        if self.poisoned {return Err(err("poisoned"));}
        if writes.is_empty(){return Ok(());}
        if self.state.used==CAP{return Err(err("capacity"));}
        let seq=self.state.last.seq.checked_add(1).ok_or_else(||err("sequence overflow"))?;
        let (r,capacity,length)=pack(3,self.state.last.bank,self.state.last.epoch,seq,self.state.last.checksum(),writes)?;
        let mut next=(*self.visible).clone();apply(&mut next,writes);
        self.costs.frame_capacity_peak=self.costs.frame_capacity_peak.max(capacity);self.costs.frame_length_peak=self.costs.frame_length_peak.max(length);
        let slot=self.state.used+1;self.poisoned=true;
        for lane in 0..2 {self.put(at(r.bank,slot,lane),&r.encode(),"append",&format!("append-{lane}"),hook)?;}
        self.sync("append",hook)?;self.state.last=r;self.state.used=slot;self.visible=Arc::new(next);hook("append.published")?;self.poisoned=false;Ok(())
    }
    fn switch(&mut self,hook:Hook<'_>)->io::Result<()> {self.switch_policy(true,hook)}
    fn switch_reuse(&mut self,hook:Hook<'_>)->io::Result<()> {self.switch_policy(false,hook)}
    fn switch_policy(&mut self,clear:bool,hook:Hook<'_>)->io::Result<()> {
        if self.poisoned {return Err(err("poisoned"));}
        let epoch=self.state.selector.epoch.checked_add(1).ok_or_else(||err("epoch overflow"))?;
        let old=self.state.selector.bank;let new=1-old;
        let writes:WriteSet=self.visible.iter().map(|(k,v)|(k.clone(),Some(v.clone()))).collect();
        let (r,capacity,length)=pack(1,new,epoch,self.state.last.seq,0,&writes)?;
        self.costs.frame_capacity_peak=self.costs.frame_capacity_peak.max(capacity);self.costs.frame_length_peak=self.costs.frame_length_peak.max(length);
        self.costs.checkpoint_clone_bytes=self.visible.iter().map(|(k,v)|k.len()+v.len()).sum();self.poisoned=true;
        if clear {for slot in 0..=CAP {for lane in 0..2 {self.put(at(new,slot,lane),&[0;N],"switch-data","switch-clear",hook)?;}}}
        for lane in 0..2 {self.put(at(new,0,lane),&r.encode(),"switch-data",&format!("switch-data-{lane}"),hook)?;}
        self.sync("switch-data",hook)?;
        for lane in 0..2 {self.put(root(epoch,lane),&r.selector().encode(),"switch-selector",&format!("switch-selector-{lane}"),hook)?;}
        self.sync("switch-selector",hook)?;self.state=Recovered{selector:r.selector(),last:r,used:0,repairs:Vec::new(),rows:Data::new()};hook("switch.published")?;
        // Clearing the old bank before both selectors are synced could destroy the only restart target.
        if clear {for slot in 0..=CAP {for lane in 0..2 {self.put(at(old,slot,lane),&[0;N],"reclaim","reclaim",hook)?;}}
            self.sync("reclaim",hook)?;}
        self.poisoned=false;Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::{Command,Stdio};
    use std::time::{Duration,Instant};
    fn noop(_: &str)->io::Result<()>{Ok(())}
    fn dir()->PathBuf {let p=std::env::temp_dir().join(format!("catdb-variable-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir(&p).unwrap();p}
    fn writes()->WriteSet {[(vec![],Some(vec![1;4097])),(b"a".to_vec(),Some(vec![2;33])),(vec![0,255,0],Some(vec![]))].into()}
    fn change()->WriteSet {[(vec![],Some(vec![8;4100])),(b"a".to_vec(),None),(b"new".to_vec(),Some(vec![7;130]))].into()}
    fn expected(w:&WriteSet)->Data {let mut d=Data::new();apply(&mut d,w);d}
    fn fixture()->(PathBuf,Store){let p=dir().join("probe.log");let mut s=Store::create(&p).unwrap();s.append(&writes(),&mut noop).unwrap();(p,s)}
    fn damage(p:&Path,index:usize){let mut f=OpenOptions::new().write(true).open(p).unwrap();f.seek(SeekFrom::Start(index as u64*PAGE)).unwrap();f.write_all(&[0;H]).unwrap();f.sync_data().unwrap();}
    fn model()->Image {let mut a=initial();let base=recover(&a).unwrap();let(r,_,_)=pack(3,0,1,1,base.last.checksum(),&writes()).unwrap();for lane in 0..2{a[at(0,1,lane)]=r.encode();}a}
    #[test]
    fn eq_atomic_changes_empty_noop_old_read_repeated_switch_and_costs(){
        let(p,mut s)=fixture();assert!(Store::create(&p).is_err());let old=s.visible.clone();let before=fs::read(&p).unwrap();let seq=s.state.last.seq;s.append(&WriteSet::new(),&mut noop).unwrap();assert_eq!(seq,s.state.last.seq);assert_eq!(before,fs::read(&p).unwrap());
        let mut want=(*old).clone();apply(&mut want,&change());s.append(&change(),&mut noop).unwrap();assert_eq!(*s.visible,want);assert_eq!(*old,expected(&writes()));
        for _ in 0..3{s.switch(&mut noop).unwrap();s.append(&change(),&mut noop).unwrap();assert_eq!(*Store::open(&p,&mut noop).unwrap().visible,want);}
        assert_eq!(*old,expected(&writes()));println!("variable costs: {:?}",s.costs);
    }
    #[test]
    fn bv_page_and_exact_slot_boundaries_preserve_rejected_state(){
        let p=dir().join("bound.log");let mut s=Store::create(&p).unwrap();
        for n in [4095,4096,4097,8192,PAYLOAD-54]{if s.state.used==CAP{s.switch(&mut noop).unwrap();}let w:WriteSet=[(vec![1],Some(vec![3;n]))].into();s.append(&w,&mut noop).unwrap();assert_eq!(*Store::open(&p,&mut noop).unwrap().visible,expected(&w));}
        let before=fs::read(&p).unwrap();let w:WriteSet=[(vec![1],Some(vec![3;PAYLOAD-53]))].into();assert!(s.append(&w,&mut noop).is_err());assert_eq!(before,fs::read(&p).unwrap());
        for n in [1023,1024,1025]{let w:WriteSet=[(vec![0;n],Some(vec![]))].into();assert_eq!(preflight(&w).is_ok(),n<=MAX_KEY_BYTES);if n<=MAX_KEY_BYTES{let(r,_,_)=pack(3,0,1,1,0,&w).unwrap();assert_eq!(r.writes().unwrap(),w);}}
        for n in [MAX_VALUE_BYTES-1,MAX_VALUE_BYTES,MAX_VALUE_BYTES+1]{let w:WriteSet=[(vec![],Some(vec![0;n]))].into();assert_eq!(preflight(&w).is_ok(),n<=MAX_VALUE_BYTES);}
        let mut w:WriteSet=(0..MAX_TRANSACTION_WRITES).map(|i|(i.to_be_bytes().to_vec(),None)).collect();assert!(preflight(&w).is_ok());w.insert(MAX_TRANSACTION_WRITES.to_be_bytes().to_vec(),None);assert!(preflight(&w).is_err());
    }
    #[test]
    fn bv_checkpoint_capacity_and_log_capacity_reject_before_writes(){
        let p=dir().join("cap.log");let mut s=Store::create(&p).unwrap();for k in [1,2]{s.append(&[(vec![k],Some(vec![k;9000]))].into(),&mut noop).unwrap();}
        let before=fs::read(&p).unwrap();assert!(s.switch(&mut noop).is_err());assert_eq!(before,fs::read(&p).unwrap());s.append(&[(vec![2],None)].into(),&mut noop).unwrap();s.switch(&mut noop).unwrap();
        for _ in 0..CAP{s.append(&[(vec![1],Some(vec![1]))].into(),&mut noop).unwrap();}let before=fs::read(&p).unwrap();assert!(s.append(&change(),&mut noop).is_err());assert_eq!(before,fs::read(&p).unwrap());assert_eq!(*Store::open(&p,&mut noop).unwrap().visible,*s.visible);
    }
    #[test]
    fn er_partial_copies_and_repair_keep_whole_transaction(){
        let base=model();let r=Record::decode(&base[at(0,1,0)]).unwrap();let end=H+r.payload_len as usize;let cuts=[0,1,63,64,65,4095,4096,4097,end-1,end];
        for x in cuts{for y in cuts{let mut a=base.clone();for(lane,cut)in [(0,x),(1,y)]{a[at(0,1,lane)][cut..].fill(0);}let got=recover(&a).unwrap().rows;assert!(got.is_empty() || got==expected(&writes()));if x==end || y==end{assert_eq!(got,expected(&writes()));}}}
        for cut in cuts{let mut a=base.clone();a[at(0,1,1)][cut..].fill(0);assert_eq!(recover(&a).unwrap().rows,expected(&writes()));}
        let mut a=base.clone();for lane in 0..2{a[at(0,1,lane)].fill(0);}assert!(recover(&a).unwrap().rows.is_empty());println!("negative control: both copies lost can discard acknowledged transaction");
        let mut a=base;let(other,_,_)=pack(3,0,1,1,r.previous,&change()).unwrap();a[at(0,1,1)]=other.encode();assert!(recover(&a).is_err());
    }
    #[test]
    fn er_untrusted_lengths_inner_corruption_and_padding(){
        let mut a=model();let i=at(0,1,1);a[i][36..40].copy_from_slice(&u32::MAX.to_le_bytes());a[i][40..44].copy_from_slice(&0u32.to_le_bytes());let c=crc(&a[i][..56]);a[i][56..60].copy_from_slice(&c.to_le_bytes());a[i][60..64].copy_from_slice(&(!c).to_le_bytes());assert_eq!(recover(&a).unwrap().rows,expected(&writes()));
        let mut a=model();for lane in 0..2{let i=at(0,1,lane);let mut r=Record::decode(&a[i]).unwrap();r.payload[8..12].copy_from_slice(&u32::MAX.to_le_bytes());a[i]=r.encode();}assert!(recover(&a).is_err());
        let mut a=model();for b in &mut a{if let Some(r)=Record::decode(b){b[H+r.payload_len as usize..].fill(0xEF);}}assert!(recover(&a).unwrap().repairs.is_empty());
    }
    #[test]
    fn st_selector_prefixes_keep_variable_checkpoint_across_reuse(){
        let mut a=model();for epoch in [2,3]{let old=recover(&a).unwrap();let bank=1-old.selector.bank;let w=old.rows.iter().map(|(k,v)|(k.clone(),Some(v.clone()))).collect();let(r,_,_)=pack(1,bank,epoch,old.last.seq,0,&w).unwrap();for slot in 0..=CAP{for lane in 0..2{a[at(bank,slot,lane)]=[0;N];}}for lane in 0..2{a[at(bank,0,lane)]=r.encode();}
            for x in 0..=H{for y in 0..=H{let mut b=a.clone();for(lane,n)in [(0,x),(1,y)]{b[root(epoch,lane)][..n].copy_from_slice(&r.selector().encode()[..n]);}assert_eq!(recover(&b).unwrap().rows,expected(&writes()));}}
            for lane in 0..2{a[root(epoch,lane)]=r.selector().encode();}for slot in 0..=CAP{for lane in 0..2{a[at(1-bank,slot,lane)]=[0;N];}}
        }println!("8450 selector prefix combinations passed");
    }
    #[test]
    fn er_phase_failures_poison_writer_and_repair_before_writable(){
        for point in ["append.before-sync","append.synced","switch-data.before-sync","switch-selector.synced","reclaim.before-sync"]{let(p,mut s)=fixture();let old=s.visible.clone();let mut hit=false;let mut hook=|name:&str|if name==point{hit=true;Err(err("injected phase failure"))}else{Ok(())};let result=if point.starts_with("append"){s.append(&change(),&mut hook)}else{s.switch(&mut hook)};assert!(hit && result.is_err());assert_eq!(*s.visible,*old);assert!(s.append(&change(),&mut noop).is_err());drop(s);let got=Store::open(&p,&mut noop).unwrap();let mut next=expected(&writes());apply(&mut next,&change());assert!(*got.visible==*old || *got.visible==next);}
        let(p,s)=fixture();drop(s);damage(&p,7);damage(&p,3);assert!(Store::open(&p,&mut |n|if n=="repair-data.before-sync"{Err(err("repair interrupted"))}else{Ok(())}).is_err());let s=Store::open(&p,&mut noop).unwrap();assert_eq!(*s.visible,expected(&writes()));println!("repair costs: {:?}",s.costs);assert!(Store::open(&p,&mut noop).unwrap().costs.categories.is_empty());
    }
    #[test]
    #[ignore]
    fn child_phase(){let p=PathBuf::from(std::env::var_os("VAR_PATH").unwrap());let ready=PathBuf::from(std::env::var_os("VAR_READY").unwrap());let target=std::env::var("VAR_PHASE").unwrap();let mode=std::env::var("VAR_MODE").unwrap();let mut hook=|name:&str|{if name==target{fs::write(&ready,name)?;loop{std::thread::park();}}Ok(())};let mut s=Store::open(&p,&mut hook).unwrap();if mode=="switch"{s.switch(&mut hook).unwrap();}else if mode=="append"{s.append(&change(),&mut hook).unwrap();}}
    #[test]
    fn er_external_kills_preserve_acknowledged_and_atomic_pending_changes(){
        let groups=[("append",vec!["append-0.half","append-0.written","append-1.half","append-1.written","append.before-sync","append.synced","append.published"]),("repair",vec!["repair-data-7.half","repair-data-7.written","repair-data.before-sync","repair-data.synced","repair-selector-3.half","repair-selector-3.written","repair-selector.before-sync","repair-selector.synced","open-ready"]),("switch",vec!["switch-clear.half","switch-data-0.half","switch-data-1.written","switch-data.before-sync","switch-data.synced","switch-selector-0.half","switch-selector-0.written","switch-selector-1.half","switch-selector.before-sync","switch-selector.synced","switch.published","reclaim.half","reclaim.before-sync","reclaim.synced"])];
        for(mode,points)in groups{for point in points{let(p,s)=fixture();drop(s);if mode=="repair"{damage(&p,7);damage(&p,3);}let ready=p.with_extension("ready");let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","transactions::variable_gate::tests::child_phase","--ignored","--nocapture"]).env("VAR_PATH",&p).env("VAR_READY",&ready).env("VAR_PHASE",point).env("VAR_MODE",mode).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn().unwrap();let deadline=Instant::now()+Duration::from_secs(10);loop{if fs::read_to_string(&ready).ok().as_deref()==Some(point){break;}if Instant::now()>deadline{let _=child.kill();let _=child.wait();panic!("barrier timeout {point}");}assert!(child.try_wait().unwrap().is_none(),"early exit {point}");std::thread::sleep(Duration::from_millis(5));}child.kill().unwrap();assert!(!child.wait().unwrap().success());let mut s=Store::open(&p,&mut noop).unwrap();let old=expected(&writes());let mut next=old.clone();apply(&mut next,&change());assert!(*s.visible==old || (mode=="append" && *s.visible==next),"{point}");if point=="append.synced" || point=="append.published"{assert_eq!(*s.visible,next);}s.append(&[(b"resume".to_vec(),Some(vec![3]))].into(),&mut noop).unwrap();assert_eq!(*Store::open(&p,&mut noop).unwrap().visible,*s.visible);println!("external kill recovered: {mode}/{point}");}}
    }
    #[test]
    fn bv_production_total_bytes_and_sequence_limits(){
        let mut w=WriteSet::new();for k in 0..4{w.insert(vec![k],Some(vec![0;MAX_VALUE_BYTES]));}
        let current=preflight(&w).unwrap()-PREFIX_BYTES-TRAILER_BYTES;
        let remaining=MAX_TRANSACTION_BYTES-current-10;w.insert(vec![4],Some(vec![0;remaining]));assert_eq!(preflight(&w).unwrap(),MAX_TRANSACTION_BYTES+PREFIX_BYTES+TRAILER_BYTES);
        w.get_mut(&vec![4]).unwrap().as_mut().unwrap().push(0);assert!(preflight(&w).is_err());
        let(p,mut s)=fixture();let before=fs::read(&p).unwrap();s.state.selector.epoch=u64::MAX;assert!(s.switch(&mut noop).is_err());assert_eq!(fs::read(&p).unwrap(),before);s.state.last.seq=u64::MAX;assert!(s.append(&change(),&mut noop).is_err());assert_eq!(fs::read(&p).unwrap(),before);
    }
    #[test]
    fn er_valid_outer_checksums_do_not_hide_invalid_inner_operations(){
        for(offset,bytes)in [(8,0u32.to_le_bytes()),(8,(MAX_TRANSACTION_WRITES as u32+1).to_le_bytes()),(13,(MAX_KEY_BYTES as u32+1).to_le_bytes()),(17,u32::MAX.to_le_bytes())]{
            let mut a=model();let mut r=Record::decode(&a[at(0,1,0)]).unwrap();let n=r.payload_len as usize;let body_end=n-TRAILER_BYTES;r.payload[PREFIX_BYTES+offset..PREFIX_BYTES+offset+4].copy_from_slice(&bytes);let c=crate::crc32(&r.payload[PREFIX_BYTES..body_end]);r.payload[body_end..body_end+4].copy_from_slice(&c.to_le_bytes());for lane in 0..2{a[at(0,1,lane)]=r.encode();}assert!(recover(&a).is_err());
        }
    }
    #[test]
    fn er_one_copy_damage_each_meaningful_byte_and_sync_before_publish(){
        let a=model();let end=H+Record::decode(&a[at(0,1,0)]).unwrap().payload_len as usize;
        for pos in 0..end{let mut b=a.clone();b[at(0,1,1)][pos]^=1;assert_eq!(recover(&b).unwrap().rows,expected(&writes()));}
        let(p,mut s)=fixture();let old=s.visible.clone();let mut points=Vec::new();s.append(&change(),&mut |point|{points.push(point.to_owned());assert_eq!(*old,expected(&writes()));Ok(())}).unwrap();assert!(points.iter().position(|p|p=="append.synced").unwrap()<points.iter().position(|p|p=="append.published").unwrap());assert_eq!(*Store::open(&p,&mut noop).unwrap().visible,*s.visible);
        damage(&p,at(0,2,1));drop(s);let reopened=Store::open(&p,&mut noop).unwrap();assert_eq!(reopened.costs.categories["repair-data"].1,1);println!("fresh single-copy repair costs: {:?}",reopened.costs);println!("{end} meaningful byte single-bit corruptions restored from peer");
    }

    #[test]
    fn reuse_eq_same_workload_costs_and_old_snapshots(){
        let mut results=Vec::new();
        for clear in [true,false]{let(p,mut s)=fixture();let old=s.visible.clone();s.append(&change(),&mut noop).unwrap();for _ in 0..3{s.switch_policy(clear,&mut noop).unwrap();s.append(&change(),&mut noop).unwrap();assert_eq!(*Store::open(&p,&mut noop).unwrap().visible,*s.visible);}assert_eq!(*old,expected(&writes()));assert_eq!(s.file.metadata().unwrap().len(),393216);
            let categories=&s.costs.categories;let bytes:u64=["switch-data","switch-selector","reclaim"].iter().map(|k|categories.get(k).map_or(0,|v|v.0)).sum();let syncs:u64=["switch-data","switch-selector","reclaim"].iter().map(|k|categories.get(k).map_or(0,|v|v.1)).sum();assert_eq!((bytes,syncs),if clear{(1009650,9)}else{(26610,6)});println!("reuse comparison clear={clear}: {:?}; transition_bytes={bytes} transition_syncs={syncs}; reopen_match=true",s.costs);results.push((*s.visible).clone());}
        assert_eq!(results[0],results[1]);
    }
    fn short()->WriteSet {[(vec![],Some(vec![4])),(b"a".to_vec(),None),(vec![0,255,0],None),(b"new".to_vec(),Some(vec![]))].into()}
    fn reused_fixture()->(PathBuf,Store){let(p,mut s)=fixture();s.append(&change(),&mut noop).unwrap();s.switch_reuse(&mut noop).unwrap();s.append(&writes(),&mut noop).unwrap();s.switch_reuse(&mut noop).unwrap();(p,s)}
    #[test]
    fn reuse_st_repeated_long_short_records_capacity_and_epoch_exhaustion(){
        let(p,mut s)=reused_fixture();let old=s.visible.clone();for round in 0..8{let w=if round%2==0{short()}else{change()};while s.state.used<CAP{s.append(&w,&mut noop).unwrap();}let before=fs::read(&p).unwrap();assert!(s.append(&w,&mut noop).is_err());assert_eq!(fs::read(&p).unwrap(),before);s.switch_reuse(&mut noop).unwrap();let reopened=Store::open(&p,&mut noop).unwrap();assert_eq!(*reopened.visible,*s.visible);assert!(reopened.costs.categories.is_empty());}
        let mut want=expected(&writes());apply(&mut want,&change());apply(&mut want,&writes());assert_eq!(*old,want);let before=fs::read(&p).unwrap();s.state.selector.epoch=u64::MAX;assert!(s.switch_reuse(&mut noop).is_err());assert_eq!(fs::read(&p).unwrap(),before);
    }
    #[test]
    fn reuse_model_recovers_torn_reuse_after_full_inner_validation(){
        let mut a=model();let mut rejected=0;
        for epoch in 2..=4{let current=recover(&a).unwrap();let bank=1-current.selector.bank;let w:WriteSet=current.rows.iter().map(|(k,v)|(k.clone(),Some(v.clone()))).collect();let(r,_,_)=pack(1,bank,epoch,current.last.seq,0,&w).unwrap();let b=r.encode();let end=H+r.payload_len as usize;
            for lane in 0..2{for cut in [0,1,32,63,64,65,4095,4096,end-1,end]{let mut partial=a.clone();partial[at(bank,0,lane)][..cut].copy_from_slice(&b[..cut]);assert_eq!(recover(&partial).unwrap().rows,current.rows);}}
            for lane in 0..2{a[at(bank,0,lane)][..end].copy_from_slice(&b[..end]);}
            for x in 0..=H{for y in 0..=H{let mut partial=a.clone();for(lane,cut)in [(0,x),(1,y)]{partial[root(epoch,lane)][..cut].copy_from_slice(&r.selector().encode()[..cut]);}assert_eq!(recover(&partial).unwrap().rows,current.rows);}}
            for lane in 0..2{a[root(epoch,lane)][..H].copy_from_slice(&r.selector().encode()[..H]);}
            let w=if epoch%2==0{short()}else{change()};let(t,_,_)=pack(3,bank,epoch,r.seq+1,r.checksum(),&w).unwrap();let b=t.encode();let end=H+t.payload_len as usize;
            for x in [0,1,32,63,64,65,end-1,end]{for y in [0,1,32,63,64,65,end-1,end]{let mut partial=a.clone();for(lane,cut)in [(0,x),(1,y)]{partial[at(bank,1,lane)][..cut].copy_from_slice(&b[..cut]);}let mut next=current.rows.clone();apply(&mut next,&w);match recover(&partial){Ok(got)=>{assert!(got.rows==current.rows || got.rows==next);if x==end || y==end{assert_eq!(got.rows,next);}},Err(e)=>{assert!(x<end && y<end,"valid peer must recover");assert_eq!(e.to_string(),"no valid inner copy");rejected+=1;println!("REJECTED reuse model: epoch={epoch} left_prefix={x} right_prefix={y} frame_end={end}: {e}");}}}}
            for lane in 0..2{a[at(bank,1,lane)][..end].copy_from_slice(&b[..end]);}
            for lane in 0..2{let mut damaged=a.clone();damaged[root(epoch,lane)][..H].fill(0);assert_eq!(recover(&damaged).unwrap().rows,recover(&a).unwrap().rows);}
        }assert_eq!(rejected,0);println!("Reuse model: {rejected} refused interrupted states; 12675 selector prefixes, 60 checkpoint prefixes, 192 append prefix pairs, 6 selector damage cases examined");
    }
    #[test]
    fn reuse_er_interrupted_switch_retry_after_more_updates(){
        for point in ["switch-data-0.half","switch-data-1.written","switch-data.before-sync","switch-data.synced","switch-selector-0.half","switch-selector-0.written","switch-selector-1.half","switch-selector.before-sync","switch-selector.synced","switch.published"]{
            let(p,mut s)=reused_fixture();let before=s.visible.clone();let mut hit=false;assert!(s.switch_reuse(&mut |n|if n==point{hit=true;Err(err("phase failure"))}else{Ok(())}).is_err());assert!(hit);assert!(s.append(&short(),&mut noop).is_err());drop(s);let mut s=Store::open(&p,&mut noop).unwrap();assert_eq!(*s.visible,*before);s.append(&short(),&mut noop).unwrap();let want=s.visible.clone();s.switch_reuse(&mut noop).unwrap();drop(s);assert_eq!(*Store::open(&p,&mut noop).unwrap().visible,*want);
        }
    }
    #[test]
    fn reuse_er_damage_repair_negative_control_and_capacity(){
        let(p,mut s)=reused_fixture();s.append(&short(),&mut noop).unwrap();let want=s.visible.clone();let bank=s.state.selector.bank;let used=s.state.used;let epoch=s.state.selector.epoch;drop(s);damage(&p,at(bank,used,1));damage(&p,root(epoch,1));let mut hit=false;assert!(Store::open(&p,&mut |n|if n=="repair-data.before-sync"{hit=true;Err(err("repair interrupted"))}else{Ok(())}).is_err());assert!(hit);let mut s=Store::open(&p,&mut noop).unwrap();assert_eq!(*s.visible,*want);s.switch_reuse(&mut noop).unwrap();s.append(&change(),&mut noop).unwrap();let mut a=Store::image(&mut s.file).unwrap();for lane in 0..2{a[at(s.state.selector.bank,1,lane)][..H].fill(0);}assert_ne!(recover(&a).unwrap().rows,*s.visible);println!("negative control: both-copy loss remains outside the contract");
        let p=dir().join("capacity.log");let mut s=Store::create(&p).unwrap();for k in [1,2]{s.append(&[(vec![k],Some(vec![k;9000]))].into(),&mut noop).unwrap();}let before=fs::read(&p).unwrap();assert!(s.switch_reuse(&mut noop).is_err());assert_eq!(fs::read(&p).unwrap(),before);
    }
    #[test]
    #[ignore]
    fn reuse_child(){let p=PathBuf::from(std::env::var_os("REUSE_PATH").unwrap());let ready=PathBuf::from(std::env::var_os("REUSE_READY").unwrap());let target=std::env::var("REUSE_PHASE").unwrap();let mode=std::env::var("REUSE_MODE").unwrap();let mut hook=|name:&str|{if name==target{fs::write(&ready,name)?;loop{std::thread::park();}}Ok(())};let mut s=Store::open(&p,&mut hook).unwrap();if mode=="switch"{s.switch_reuse(&mut hook).unwrap();}else if mode=="append"{s.append(&short(),&mut hook).unwrap();}}
    #[test]
    fn reuse_er_external_kills_then_update_retry_and_reopen(){
        let groups=[("switch",vec!["switch-data-0.half","switch-data-1.written","switch-data.before-sync","switch-data.synced","switch-selector-0.half","switch-selector-0.written","switch-selector-1.half","switch-selector.before-sync","switch-selector.synced","switch.published"]),("append",vec!["append-0.half","append-0.written","append-1.half","append-1.written","append.before-sync","append.synced","append.published"]),("repair",vec!["repair-data.before-sync","repair-data.synced","repair-selector.before-sync","repair-selector.synced","open-ready"])];
        for(mode,points)in groups{for point in points{for cycle in 0..2{let(p,mut s)=reused_fixture();if cycle==1{s.append(&short(),&mut noop).unwrap();s.switch_reuse(&mut noop).unwrap();}let old=s.visible.clone();let mut next=(*old).clone();apply(&mut next,&short());let bank=s.state.selector.bank;let epoch=s.state.selector.epoch;drop(s);if mode=="repair"{damage(&p,at(bank,0,1));damage(&p,root(epoch,1));}let ready=p.with_extension("ready");let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact","transactions::variable_gate::tests::reuse_child","--ignored","--nocapture"]).env("REUSE_PATH",&p).env("REUSE_READY",&ready).env("REUSE_PHASE",point).env("REUSE_MODE",mode).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn().unwrap();let deadline=Instant::now()+Duration::from_secs(10);loop{if fs::read_to_string(&ready).ok().as_deref()==Some(point){break;}if Instant::now()>deadline{let _=child.kill();let _=child.wait();panic!("reuse barrier timeout {point}");}assert!(child.try_wait().unwrap().is_none(),"early exit {point}");std::thread::sleep(Duration::from_millis(5));}child.kill().unwrap();assert!(!child.wait().unwrap().success());let mut s=Store::open(&p,&mut noop).unwrap();assert!(*s.visible==*old || (mode=="append" && *s.visible==next));if point=="append.synced" || point=="append.published"{assert_eq!(*s.visible,next);}s.append(&change(),&mut noop).unwrap();let want=s.visible.clone();s.switch_reuse(&mut noop).unwrap();assert_eq!(*Store::open(&p,&mut noop).unwrap().visible,*want);println!("reuse external kill recovered/retried: {mode}/{cycle}/{point}");}}}
    }

    #[test]
    fn reuse_negative_nested_crc_does_not_bind_inner_sequence(){
        let w=writes();let(a,_,_)=pack(3,0,1,1,0,&w).unwrap();let(b,_,_)=pack(3,0,3,3,0,&w).unwrap();let n=a.payload_len as usize;assert_ne!(a.payload,b.payload);assert_eq!(crc(&a.payload[..n]),crc(&b.payload[..n]));let mut torn=b.encode();torn[H..H+n].copy_from_slice(&a.payload[..n]);let decoded=Record::decode(&torn).expect("outer check accepts this constructed tear");assert!(decoded.writes().is_err());
        let mut image=initial();let base=recover(&image).unwrap();let(old,_,_)=pack(3,0,1,0,base.last.checksum(),&w).unwrap();let(new,_,_)=pack(3,0,1,1,base.last.checksum(),&w).unwrap();let mut torn=new.encode();torn[H..H+n].copy_from_slice(&old.payload[..n]);image[at(0,1,0)]=new.encode();image[at(0,1,1)]=torn;assert_eq!(recover(&image).unwrap().rows,expected(&w));image[at(0,1,0)]=torn;assert!(recover(&image).unwrap().rows.is_empty());println!("negative control: outer CRC alone accepts stale payload; full inner validation selects current peer or recognizes a fully validated older transaction");
    }

    #[test]
    fn reuse_er_stale_classification_rejects_future_and_holes(){
        let w=writes();let mut image=initial();let base=recover(&image).unwrap();let(new,_,_)=pack(3,0,1,1,base.last.checksum(),&w).unwrap();let(future,_,_)=pack(3,0,1,2,base.last.checksum(),&w).unwrap();let n=new.payload_len as usize;let mut torn=new.encode();torn[H..H+n].copy_from_slice(&future.payload[..n]);for lane in 0..2{image[at(0,1,lane)]=torn;}assert!(recover(&image).is_err());
        let(old,_,_)=pack(3,0,1,0,base.last.checksum(),&w).unwrap();torn[H..H+n].copy_from_slice(&old.payload[..n]);for lane in 0..2{image[at(0,1,lane)]=torn;image[at(0,2,lane)]=future.encode();}assert!(recover(&image).is_err());
        let(checkpoint,_,_)=pack(1,0,1,0,0,&w).unwrap();let mut invalid=checkpoint.encode();invalid[H..H+n].copy_from_slice(&future.payload[..n]);let mut image=initial();for lane in 0..2{image[at(0,0,lane)]=invalid;}assert!(recover(&image).is_err());
    }

}
