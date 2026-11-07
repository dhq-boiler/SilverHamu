use crate::transactions::independent_trace;
#[test]
#[ignore="independent production checkpoint campaign"]
fn independent_campaign() {
    let n:usize=std::env::var("SCALED_ROWS").unwrap().parse().unwrap();
    let ops:usize=std::env::var("SCALED_OPS").unwrap().parse().unwrap();
    let cap:usize=std::env::var("SCALED_CAP").unwrap().parse().unwrap();
    assert_eq!(fs::read_to_string("/sys/fs/cgroup/memory.max").unwrap().trim(),"4294967296");
    let q=fs::read_to_string("/sys/fs/cgroup/cpu.max").unwrap();
    let q:Vec<u64>=q.split_whitespace().map(|p|p.parse().unwrap()).collect();
    assert_eq!(q[0],2*q[1]);
    let enabled=std::env::var("SCALED_VARIANT").unwrap()=="prefix-on";
    let path=dir().join("independent.db");
    let db=Database::create_with_limits(&path,[87;16],ResourceLimits {
        max_transactions:64,max_staged_bytes:32*PAGE,max_dependency_bytes:32*PAGE,
        max_retained_bytes:2*1024*PAGE,max_buffer_bytes:64*PAGE
    }).unwrap();
    for start in (0..n).step_by(1000) {
        let mut t=db.begin().unwrap();
        for i in start..(start+1000).min(n) {t.put(&key(i),&value(i,0)).unwrap();}
        t.commit().unwrap();
    }
    db.checkpoint().unwrap();
    let mut old=db.begin_read().unwrap();
    let base=old.version();
    let clock=Instant::now();
    let now=||clock.elapsed().as_nanos() as u64;
    let (send,recv)=std::sync::mpsc::channel::<usize>();
    let barrier=Barrier::new(5);
    let (attempts,publications)=std::thread::scope(|scope| {
        let worker_db=&db; let worker=scope.spawn(move ||{ let db=worker_db;
            independent_trace::enable(clock);
            let mut result=Vec::new();
            let mut last=0;
            while last<ops/cap-1 {
                let start=now();
                let mut prefix=enabled.then(||db.prepare_checkpoint_prefix(2048).unwrap());
                let ready=now();
                let mut due=recv.recv().unwrap(); while due<=last { due=recv.recv().unwrap(); }
                for pending in recv.try_iter(){due=due.max(pending);}
                let checkpoint=now();
                let mut stages=[0;5];
                let mut observe=|stage|{
                    match stage {
                        CheckpointStage::Written=>stages[0]=now(),
                        CheckpointStage::Synced=>stages[1]=now(),
                        CheckpointStage::TailCopied=>stages[2]=now(),
                        CheckpointStage::TailSynced=>stages[3]=now(),
                        CheckpointStage::Published=>stages[4]=now(),
                        _=>{}
                    }
                    Ok(())
                };
                let info=match prefix.as_mut(){
                    Some(p)=>db.finish_checkpoint(p.draft.take(),false,None,&mut observe),
                    None=>db.checkpoint_with(&mut observe)
                }.unwrap();
                let end=now();
                let [lock,release,..]=independent_trace::take();
                result.push((due,[start,ready,checkpoint,stages[0],stages[1],lock,stages[2],stages[3],stages[4],release,end],info.after_bytes+48));
                last=due;
            }
            result
        });
        let mut threads=Vec::new();
        for lane in 0..4 {
            let db=&db;let send=send.clone();let barrier=&barrier;
            threads.push(scope.spawn(move ||{
                independent_trace::enable(clock);
                let mut records=Vec::new();barrier.wait();
                for id in (lane..ops).step_by(4){
                    let received=now();
                    let w=changes(id,n);
                    loop {
                        let begin=now();let mut t=db.begin().unwrap();
                        for (k,v) in &w {match v {Some(v)=>t.put(k,v).unwrap(),None=>t.delete(k).unwrap()}}
                        let commit=now();let outcome=t.commit();let end=now();
                        let e=independent_trace::take();
                        let seq=match outcome {Ok(s)=>s,Err(TxError::Conflict(_))=>0,Err(e)=>panic!("{e:?}")};
                        records.push((id,received,begin,commit,e,end,seq));
                        if seq>0 {
                            let pos=(seq-base) as usize;
                            if pos%cap==0 && pos<ops {let _=send.send(pos/cap);}
                            break;
                        }
                    }
                }
                records
            }));
        }
        drop(send);barrier.wait();
        let rows=threads.into_iter().flat_map(|t|t.join().unwrap()).collect::<Vec<_>>();
        (rows,worker.join().unwrap())
    });
    let mut expected=BTreeMap::new();
    let mut committed=Vec::new();
    for (id,received,begin,commit,[l,a,s,f,b],end,seq) in attempts {
        println!("ATTEMPT,{id},{received},{begin},{commit},{l},{a},{s},{f},{b},{end},{seq}");
        if seq>0 {committed.push((seq,id));}
    }
    committed.sort_unstable();assert_eq!(committed.len(),ops);
    for (index,(seq,id)) in committed.into_iter().enumerate(){
        assert_eq!(seq,base+1+index as u64);
        for (k,v) in changes(id,n){expected.insert(u64::from_be_bytes(k[..8].try_into().unwrap()) as usize,v);}
    }
    for (due,t,b) in publications {
        println!("PUBLICATION,{due},{},{},{},{},{},{},{},{},{},{},{},{b}",t[0],t[1],t[2],t[3],t[4],t[5],t[6],t[7],t[8],t[9],t[10]);
    }
    check_tx(&mut old,n,&BTreeMap::new());drop(old);
    check_tx(&mut db.begin_read().unwrap(),n,&expected);drop(db);
    let reopened=Database::open(&path).unwrap();let mut r=reopened.begin_read().unwrap();
    assert_eq!(r.version(),base+ops as u64);check_tx(&mut r,n,&expected);
    println!("INDEPENDENT_OK,{ops}");
}
