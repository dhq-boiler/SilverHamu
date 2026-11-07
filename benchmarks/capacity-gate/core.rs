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
        let slot=self.state.used+1;self.poisoned=true;hook("append.prepared")?;
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
        self.costs.checkpoint_clone_bytes=self.visible.iter().map(|(k,v)|k.len()+v.len()).sum();self.poisoned=true;hook("switch.prepared")?;
        if clear {for slot in 0..=CAP {for lane in 0..2 {self.put(at(new,slot,lane),&[0;N],"switch-data","switch-clear",hook)?;}}}
        for lane in 0..2 {self.put(at(new,0,lane),&r.encode(),"switch-data",&format!("switch-data-{lane}"),hook)?;}
        self.sync("switch-data",hook)?;
        for lane in 0..2 {self.put(root(epoch,lane),&r.selector().encode(),"switch-selector",&format!("switch-selector-{lane}"),hook)?;}
        self.sync("switch-selector",hook)?;self.state=Recovered{selector:r.selector(),last:r,used:0,repairs:Vec::new(),rows:Data::new()};hook("switch.published")?;
        // Clearing the old bank before both selectors are synced could destroy the only restart target.
        if clear {hook("switch.reclaim-start")?;for slot in 0..=CAP {for lane in 0..2 {self.put(at(old,slot,lane),&[0;N],"reclaim","reclaim",hook)?;}}
            self.sync("reclaim",hook)?;}
        self.poisoned=false;Ok(())
    }
}
