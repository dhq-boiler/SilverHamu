use std::cell::RefCell;
use std::os::unix::fs::FileExt;
use std::sync::Condvar;

#[derive(Default)]
struct TurnQueue {
    state: Mutex<(u64, u64)>,
    ready: Condvar,
}
struct Turn<'a>(&'a TurnQueue);
impl TurnQueue {
    fn enter(&self) -> Turn<'_> {
        let mut state = self.state.lock().unwrap();
        let mine = state.0;
        state.0 = mine.checked_add(1).unwrap();
        while state.1 != mine {
            state = self.ready.wait(state).unwrap();
        }
        Turn(self)
    }
}
impl Drop for Turn<'_> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap();
        state.1 += 1;
        self.0.ready.notify_all();
    }
}

thread_local! {static SCHED:RefCell<Option<File>>=const{RefCell::new(None)};}
#[derive(Copy, Clone)]
struct Stamp {
    wall: u128,
    cpu: u64,
    runnable: u64,
}
impl Stamp {
    fn capture(clock: Instant) -> Self {
        SCHED.with(|cell| {
            let mut file = cell.borrow_mut();
            let file =
                file.get_or_insert_with(|| File::open("/proc/thread-self/schedstat").unwrap());
            let mut buf = [0; 128];
            let count = file.read_at(&mut buf, 0).unwrap();
            let mut parts = std::str::from_utf8(&buf[..count])
                .unwrap()
                .split_whitespace();
            let cpu = parts.next().unwrap().parse().unwrap();
            let runnable = parts.next().unwrap().parse().unwrap();
            Self {
                wall: clock.elapsed().as_nanos(),
                cpu,
                runnable,
            }
        })
    }
}
struct Span {
    stage: &'static str,
    a: Stamp,
    b: Stamp,
}
struct Trace {
    lite: bool,
    last: Stamp,
    spans: Vec<Span>,
}
impl Trace {
    fn stamp(clock: Instant, lite: bool) -> Stamp {
        if lite {
            Stamp {
                wall: clock.elapsed().as_nanos(),
                cpu: 0,
                runnable: 0,
            }
        } else {
            Stamp::capture(clock)
        }
    }
    fn start(clock: Instant, lite: bool) -> Self {
        Self {
            lite,
            last: Self::stamp(clock, lite),
            spans: Vec::with_capacity(16),
        }
    }
    fn end(trace: &mut Option<Self>, clock: Instant, stage: &'static str) {
        if let Some(t) = trace {
            assert!(t.spans.len() < 128, "trace capacity");
            let now = Self::stamp(clock, t.lite);
            assert!(
                now.wall >= t.last.wall && now.cpu >= t.last.cpu && now.runnable >= t.last.runnable
            );
            t.spans.push(Span {
                stage,
                a: t.last,
                b: now,
            });
            t.last = now;
        }
    }
}
#[test]
fn ticket_handoff_drains_in_registration_order() {
    let queue = TurnQueue::default();
    let first = queue.enter();
    let seen = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for idx in 0..4 {
            let q = &queue;
            let seen = &seen;
            scope.spawn(move || {
                let _turn = q.enter();
                seen.lock().unwrap().push(idx);
            });
            let until = Instant::now() + Duration::from_secs(3);
            while queue.state.lock().unwrap().0 != idx + 2 {
                assert!(Instant::now() < until);
                std::thread::yield_now();
            }
        }
        drop(first);
    });
    assert_eq!(*seen.lock().unwrap(), [0, 1, 2, 3]);
}
