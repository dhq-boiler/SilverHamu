use std::cell::Cell;
use std::time::Instant;

#[derive(Clone, Copy, Default)]
struct PhaseState {
    enabled: bool,
    stage: usize,
    stamp: Option<Instant>,
    ns: [u128; 10],
}
thread_local! {static PHASE:Cell<PhaseState>=Cell::new(PhaseState::default());}
struct Phase(Option<usize>);
impl Phase {
    fn begin(enabled: bool) {
        if enabled {
            PHASE.with(|s| {
                s.set(PhaseState {
                    enabled: true,
                    stamp: Some(Instant::now()),
                    ..Default::default()
                })
            });
        }
    }
    fn enter(stage: usize) -> Self {
        Self(PHASE.with(|s| {
            let mut m = s.get();
            if !m.enabled {
                return None;
            }
            let old = m.stage;
            let now = Instant::now();
            m.ns[old] += (now - m.stamp.unwrap()).as_nanos();
            m.stamp = Some(now);
            m.stage = stage;
            s.set(m);
            Some(old)
        }))
    }
    fn finish() -> Option<[u128; 10]> {
        PHASE.with(|s| {
            let mut m = s.replace(PhaseState::default());
            if !m.enabled {
                return None;
            }
            m.ns[m.stage] += m.stamp.unwrap().elapsed().as_nanos();
            Some(m.ns)
        })
    }
}
impl Drop for Phase {
    fn drop(&mut self) {
        if let Some(old) = self.0 {
            let p = Self::enter(old);
            std::mem::forget(p);
        }
    }
}
