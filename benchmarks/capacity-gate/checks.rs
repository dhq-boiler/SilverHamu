#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::{Command,Stdio};
    use std::time::{Duration,Instant};
    fn noop(_: &str)->io::Result<()>{Ok(())}
    fn writes()->WriteSet{[(vec![],Some(vec![1;4097])),(b"a".to_vec(),Some(vec![2;33])),(vec![0,255,0],Some(vec![]))].into()}
    fn change()->WriteSet{[(vec![],Some(vec![8;4100])),(b"a".to_vec(),None),(b"new".to_vec(),Some(vec![7;130]))].into()}
    fn short()->WriteSet{[(vec![],Some(vec![4])),(b"a".to_vec(),None),(vec![0,255,0],None),(b"new".to_vec(),Some(vec![]))].into()}
    fn dir()->PathBuf{let root=std::env::var_os("CAP_ROOT").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);let p=root.join(format!("cap-{CAP}-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));fs::create_dir(&p).unwrap();p}
    fn fixture()->(PathBuf,Store){let p=dir().join("data.log");let mut s=Store::create(&p).unwrap();s.append(&writes(),&mut noop).unwrap();s.append(&change(),&mut noop).unwrap();for _ in 0..2{while s.state.used<CAP{s.append(&change(),&mut noop).unwrap();}s.switch_reuse(&mut noop).unwrap();}while s.state.used<CAP-1{s.append(&change(),&mut noop).unwrap();}(p,s)}
    fn damage(p:&Path,index:usize){let mut f=OpenOptions::new().write(true).open(p).unwrap();f.seek(SeekFrom::Start(index as u64*PAGE)).unwrap();f.write_all(&[0;H]).unwrap();f.sync_data().unwrap();}
    #[test]
    fn boundaries_repeated_reuse_and_old_generation(){
        let(p,mut s)=fixture();assert_eq!(s.state.used,CAP-1);let old=s.visible.clone();let want=(*old).clone();assert!(Store::create(&p).is_err());s.append(&short(),&mut noop).unwrap();assert_eq!(s.state.used,CAP);let before=fs::read(&p).unwrap();assert!(s.append(&change(),&mut noop).is_err());assert_eq!(fs::read(&p).unwrap(),before);s.append(&WriteSet::new(),&mut noop).unwrap();assert_eq!(fs::read(&p).unwrap(),before);
        for round in 0..3{s.switch_reuse(&mut noop).unwrap();for _ in 0..CAP{s.append(&if round%2==0{change()}else{short()},&mut noop).unwrap();}assert_eq!(*Store::open(&p,&mut noop).unwrap().visible,*s.visible);}
        assert_eq!(*old,want);assert_eq!(s.file.metadata().unwrap().len(),BLOCKS as u64*PAGE);let before=fs::read(&p).unwrap();s.state.selector.epoch=u64::MAX;assert!(s.switch_reuse(&mut noop).is_err());assert_eq!(fs::read(&p).unwrap(),before);
    }
    #[test]
    fn tail_prefix_model_preserves_atomicity_and_valid_peer(){
        let(_,mut s)=fixture();let image=Store::image(&mut s.file).unwrap();let old=(*s.visible).clone();let mut next=old.clone();apply(&mut next,&short());let(r,_,_)=pack(3,s.state.last.bank,s.state.last.epoch,s.state.last.seq+1,s.state.last.checksum(),&short()).unwrap();let bytes=r.encode();let end=H+r.payload_len as usize;let cuts=[0,1,32,63,64,65,end-1,end];
        for x in cuts{for y in cuts{let mut a=image.clone();for(lane,n)in [(0,x),(1,y)]{a[at(r.bank,CAP,lane)][..n].copy_from_slice(&bytes[..n]);}let got=recover(&a).unwrap().rows;assert!(got==old || got==next);if x==end || y==end{assert_eq!(got,next);}}}println!("CAP_MODEL,{CAP},64");
    }
    #[test]
    fn damaged_tail_repair_and_interrupted_switch_retry(){
        let(p,mut s)=fixture();s.append(&short(),&mut noop).unwrap();let old=s.visible.clone();let bank=s.state.selector.bank;drop(s);damage(&p,at(bank,CAP,1));let mut reached=false;assert!(Store::open(&p,&mut |n|if n=="repair-data.before-sync"{reached=true;Err(err("repair interrupted"))}else{Ok(())}).is_err());assert!(reached);let mut s=Store::open(&p,&mut noop).unwrap();assert_eq!(*s.visible,*old);
        assert!(s.switch_reuse(&mut |n|if n=="switch-data.synced"{Err(err("switch interrupted"))}else{Ok(())}).is_err());assert!(s.append(&change(),&mut noop).is_err());drop(s);let mut s=Store::open(&p,&mut noop).unwrap();assert_eq!(*s.visible,*old);s.switch_reuse(&mut noop).unwrap();s.append(&change(),&mut noop).unwrap();assert_eq!(*Store::open(&p,&mut noop).unwrap().visible,*s.visible);
    }
    #[test]
    #[ignore = "cross-capacity child helper"]
    fn foreign_open(){let p=PathBuf::from(std::env::var_os("CAP_PATH").unwrap());let from:usize=std::env::var("CAP_FROM").unwrap().parse().unwrap();assert_ne!(CAP,from);assert!(Store::open(&p,&mut noop).is_err());}
    #[test]
    fn wrong_capacity_rejects_before_modifying_file(){let(p,s)=fixture();drop(s);let before=fs::read(&p).unwrap();for other in [4,16,64]{if other==CAP{continue;}let status=Command::new(std::env::current_exe().unwrap()).args(["--exact",&format!("transactions::capacity_gate::cap{other}::tests::foreign_open"),"--ignored"]).env("CAP_PATH",&p).env("CAP_FROM",CAP.to_string()).stdout(Stdio::null()).stderr(Stdio::inherit()).status().unwrap();assert!(status.success());assert_eq!(fs::read(&p).unwrap(),before);}println!("CAP_FOREIGN,{CAP},2");}
    #[test]
    #[ignore = "external crash child helper"]
    fn child_phase(){let p=PathBuf::from(std::env::var_os("CAP_PATH").unwrap());let ready=PathBuf::from(std::env::var_os("CAP_READY").unwrap());let target=std::env::var("CAP_PHASE").unwrap();let mode=std::env::var("CAP_MODE").unwrap();let mut hook=|name:&str|{if name==target{fs::write(&ready,name)?;loop{std::thread::park();}}Ok(())};let mut s=Store::open(&p,&mut hook).unwrap();if mode=="append"{s.append(&short(),&mut hook).unwrap();}else if mode=="switch"{s.switch_reuse(&mut hook).unwrap();}}
    #[test]
    fn external_exit_at_tail_switch_and_repair(){
        for(mode,points)in [("append",vec!["append-0.half","append-1.written","append.before-sync","append.synced","append.published"]),("switch",vec!["switch-data-0.half","switch-data.synced","switch-selector-0.half","switch-selector.synced","switch.published"]),("repair",vec!["repair-data.before-sync","repair-data.synced","open-ready"]) ]{for point in points{
            let(p,mut s)=fixture();if mode=="repair"{s.append(&short(),&mut noop).unwrap();}let old=s.visible.clone();let mut next=(*old).clone();apply(&mut next,&short());let bank=s.state.selector.bank;drop(s);if mode=="repair"{damage(&p,at(bank,CAP,1));}let ready=p.with_extension("ready");let mut child=Command::new(std::env::current_exe().unwrap()).args(["--exact",&format!("transactions::capacity_gate::cap{CAP}::tests::child_phase"),"--ignored","--nocapture"]).env("CAP_PATH",&p).env("CAP_READY",&ready).env("CAP_PHASE",point).env("CAP_MODE",mode).stdout(Stdio::null()).stderr(Stdio::inherit()).spawn().unwrap();let deadline=Instant::now()+Duration::from_secs(10);loop{if fs::read_to_string(&ready).ok().as_deref()==Some(point){break;}if Instant::now()>deadline{let _=child.kill();let _=child.wait();panic!("barrier timeout {point}");}assert!(child.try_wait().unwrap().is_none());std::thread::sleep(Duration::from_millis(5));}child.kill().unwrap();assert!(!child.wait().unwrap().success());let mut s=Store::open(&p,&mut noop).unwrap();assert!(*s.visible==*old || (mode=="append" && *s.visible==next));if point=="append.synced" || point=="append.published"{assert_eq!(*s.visible,next);}if s.state.used==CAP{s.switch_reuse(&mut noop).unwrap();}s.append(&change(),&mut noop).unwrap();assert_eq!(*Store::open(&p,&mut noop).unwrap().visible,*s.visible);println!("CAP_KILL,{CAP},{mode},{point}");}}
    }
    #[test]
    #[ignore = "explicit bounded capacity timing experiment"]
    fn capacity_speed(){
        let trial:usize=std::env::var("CAP_TRIAL").unwrap().parse().unwrap();let p=dir().join("timed.log");let start=Instant::now();let mut s=Store::create(&p).unwrap();let init_ns=start.elapsed().as_nanos();s.append(&writes(),&mut noop).unwrap();s.append(&change(),&mut noop).unwrap();let old=s.visible.clone();let original=(*old).clone();let mut updates=Vec::new();for i in 0..32{let mut w=change();w.get_mut(&vec![]).unwrap().as_mut().unwrap()[0]=i;updates.push(w);}
        // Equal warmup must end at the same occupancy boundary for every capacity.
        for i in 0..126{if s.state.used==CAP{s.switch_reuse(&mut noop).unwrap();}s.append(&updates[i%32],&mut noop).unwrap();}assert_eq!(s.state.used,CAP);
        let mut expected=(*s.visible).clone();let mut rows=Vec::with_capacity(2048);let before_bytes:u64=s.costs.categories.values().map(|v|v.0).sum();let before_sync:u64=s.costs.categories.values().map(|v|v.1).sum();let phase=Instant::now();
        for i in 0..2048{let start=Instant::now();let mut sw=0;if s.state.used==CAP{let t=Instant::now();s.switch_reuse(&mut noop).unwrap();sw=t.elapsed().as_nanos();}let t=Instant::now();s.append(&updates[i%32],&mut noop).unwrap();let ap=t.elapsed().as_nanos();rows.push((i,start.elapsed().as_nanos(),ap,sw));}let elapsed=phase.elapsed().as_nanos();
        for i in 0..2048{apply(&mut expected,&updates[i%32]);}assert_eq!(*s.visible,expected);assert_eq!(*old,original);assert_eq!(s.state.used,CAP);let bytes=s.costs.categories.values().map(|v|v.0).sum::<u64>()-before_bytes;let syncs=s.costs.categories.values().map(|v|v.1).sum::<u64>()-before_sync;assert_eq!(syncs,(2048+2*2048/CAP) as u64);assert_eq!(rows.iter().filter(|r|r.3>0).count(),2048/CAP);let size=s.file.metadata().unwrap().len();drop(s);
        let mut reopen=Vec::with_capacity(100);for i in 0..100{let t=Instant::now();let s=Store::open(&p,&mut noop).unwrap();let ns=t.elapsed().as_nanos();assert_eq!(*s.visible,expected);assert_eq!(s.state.used,CAP);assert!(s.costs.categories.is_empty());drop(s);reopen.push((i,ns));}
        println!("CAP_SUMMARY,{CAP},{trial},{elapsed},{bytes},{syncs},{size},{init_ns},2048,true");for(i,total,ap,sw)in rows{println!("CAP_UPDATE,{CAP},{trial},{i},{total},{ap},{sw}");}for(i,ns)in reopen{println!("CAP_REOPEN,{CAP},{trial},{i},{ns}");}
    }
}
