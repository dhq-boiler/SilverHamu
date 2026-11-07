use std::cell::RefCell;
#[derive(Default)]
struct CommitSample {
    stamp: Option<std::time::Instant>,
    phase: usize,
    ns: [u128; 6],
    attempts: u32,
}
thread_local! {static COMMIT_SAMPLE:RefCell<CommitSample>=RefCell::new(CommitSample::default());}
impl CommitSample {
    fn begin() {
        COMMIT_SAMPLE.with(|s| *s.borrow_mut() = Self::default());
    }
    fn set(phase: usize) {
        COMMIT_SAMPLE.with(|s| {
            let mut s = s.borrow_mut();
            let now = std::time::Instant::now();
            if let Some(before) = s.stamp {
                let current = s.phase;
                s.ns[current] += (now - before).as_nanos();
            }
            s.stamp = Some(now);
            s.phase = phase;
        });
    }
    fn attempt() {
        Self::set(0);
        COMMIT_SAMPLE.with(|s| s.borrow_mut().attempts += 1);
    }
    fn stop() {
        Self::set(5);
        COMMIT_SAMPLE.with(|s| s.borrow_mut().stamp = None);
    }
    fn result() -> ([u128; 6], u32) {
        COMMIT_SAMPLE.with(|s| {
            let s = s.borrow();
            (s.ns, s.attempts)
        })
    }
}
