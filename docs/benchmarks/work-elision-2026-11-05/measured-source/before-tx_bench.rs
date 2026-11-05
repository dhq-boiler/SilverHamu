use super::*;
use crate::work_elision_metrics as meter;
use std::hint::black_box;
use std::time::Instant;
fn key(i:u64)->[u8;8]{i.to_be_bytes()}
fn verify(db:&Database,n:u64){
 let mut tx=db.begin_read().unwrap();let mut after=None;let mut index=0;
 loop{let page=tx.scan_page(b"",Some(&key(n)),after.as_deref(),ScanLimits{max_rows:4096,..ScanLimits::default()}).unwrap();
 for (k,v) in &page.rows {assert_eq!(k,&key(index));assert_eq!(v,&key(index));index+=1;}
 after=page.next_after.clone();if after.is_none(){break;}}
 assert_eq!(index,n);
}
#[test]
#[ignore="explicit work-elision production experiment"]
fn work_elision_points_and_export(){
 let dir=PathBuf::from(std::env::var_os("CATDB_ELISION_OUTPUT").unwrap());std::fs::create_dir(&dir).unwrap();
 let n:u64=std::env::var("CATDB_ELISION_ROWS").unwrap().parse().unwrap();assert!([100000,1000000].contains(&n));
 let limits=ResourceLimits{max_retained_bytes:512*1024*1024,max_buffer_bytes:512*1024*1024,..ResourceLimits::default()};
 let path=dir.join("db.wal");let db=Database::create_with_limits(&path,[93;16],limits).unwrap();
 for start in (0..n).step_by(4096){let mut tx=db.begin().unwrap();for i in start..(start+4096).min(n){tx.put(&key(i),&key(i)).unwrap();}tx.commit().unwrap();}
 let mut old=db.begin_read().unwrap();
 let mut out=std::io::BufWriter::new(OpenOptions::new().write(true).create_new(true).open(dir.join("samples.csv")).unwrap());
 writeln!(out,"case,dependencies,sample,ns,calls,bytes,counted").unwrap();
 for deps in [1,100,10000] {
  let mut tx=db.begin().unwrap();for i in 0..deps {tx.get(&key(i*7919%n)).unwrap();}tx.put(b"~write",b"one").unwrap();
  {let state=db.state().unwrap();let earlier=BTreeSet::new();for sample in 0..192 {
   meter::start();let start=Instant::now();let result=black_box(tx.validate_commit(black_box(&state),black_box(&earlier)));let ns=start.elapsed().as_nanos();let (calls,bytes)=meter::stop();result.unwrap();
   writeln!(out,"validate,{deps},{sample},{ns},{calls},{bytes},{}",meter::counted()).unwrap();
  }}drop(tx);
  for sample in 0..64 {
   let mut tx=db.begin().unwrap();for i in 0..deps {tx.get(&key(i*7919%n)).unwrap();}tx.put(b"~write",&key(sample)).unwrap();
   meter::start();let start=Instant::now();tx.commit().unwrap();let ns=start.elapsed().as_nanos();let (calls,bytes)=meter::stop();
   writeln!(out,"commit,{deps},{sample},{ns},{calls},{bytes},{}",meter::counted()).unwrap();
  }
 }
 let output=dir.join("snapshot.ndb");meter::start();let start=Instant::now();let info=db.export_values(&output,[94;16],b"",Some(&key(n))).unwrap();let ns=start.elapsed().as_nanos();let(calls,bytes)=meter::stop();
 assert_eq!(info.records,n as usize);writeln!(out,"export,0,0,{ns},{calls},{bytes},{}",meter::counted()).unwrap();
 let mut snap=crate::Snapshot::open(&output,[94;16]).unwrap();for start in (0..n).step_by(4096){let rows=snap.read_page(start,4096).unwrap();for (i,v) in rows.iter().enumerate(){assert_eq!(v,&key(start+i as u64));}}
 assert!(old.get(b"~write").unwrap().is_none());verify(&db,n);drop(old);drop(db);
 let db=Database::open_with_limits(&path,limits).unwrap();verify(&db,n);assert_eq!(db.begin_read().unwrap().get(b"~write").unwrap(),Some(key(63).to_vec()));
 out.flush().unwrap();println!("verified points, snapshot and reopen: {n}");
}
