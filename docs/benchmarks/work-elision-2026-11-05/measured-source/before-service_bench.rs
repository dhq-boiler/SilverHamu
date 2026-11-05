use super::*;
use crate::work_elision_metrics as meter;
#[test]
#[ignore="explicit work-elision service-dispatch experiment"]
fn work_elision_single_requests(){
 let dir=PathBuf::from(std::env::var_os("CATDB_ELISION_OUTPUT").unwrap());std::fs::create_dir(&dir).unwrap();
 let path=dir.join("db.wal");let db=Database::create_with_limits(&path,[95;16],ResourceLimits::default()).unwrap();
 let mut out=std::io::BufWriter::new(std::fs::OpenOptions::new().write(true).create_new(true).open(dir.join("samples.csv")).unwrap());
 writeln!(out,"case,dependencies,sample,ns,calls,bytes,counted").unwrap();
 for size in [8,256*1024]{
  let value="ab".repeat(size);let request=json!({"op":"put_commit","key":"01","value":value});
  for sample in 0..64{
   meter::start();let start=Instant::now();let result=dispatch(&db,&mut None,&request);let ns=start.elapsed().as_nanos();let(calls,bytes)=meter::stop();assert_eq!(result.unwrap()["ok"],true);
   writeln!(out,"put_commit,{size},{sample},{ns},{calls},{bytes},{}",meter::counted()).unwrap();
  }
  let request=json!({"op":"get_latest","key":"01"});
  for sample in 0..192{
   meter::start();let start=Instant::now();let result=dispatch(&db,&mut None,&request);let ns=start.elapsed().as_nanos();let(calls,bytes)=meter::stop();let response=result.unwrap();assert_eq!(response["value"],value);
   writeln!(out,"get_latest,{size},{sample},{ns},{calls},{bytes},{}",meter::counted()).unwrap();
  }
 }
 drop(db);let db=Database::open(&path).unwrap();assert_eq!(db.begin_read().unwrap().get(&[1]).unwrap(),Some(vec![0xab;256*1024]));out.flush().unwrap();
 println!("verified single-request values and reopen");
}
