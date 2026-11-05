use super::{BufferPermit, MAX_KEY_BYTES, Result, RetiredVersions, State, Version};
use crate::resources::{Budget, RETAINED};
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;

const KEYS: usize = 64;
const METADATA: usize = 192;
const VERSIONS: usize = 128;
const BYTES: usize = 1024 * 1024;

struct Meta {
    sequence: u64,
    charge: usize,
}
struct Entry {
    key: Vec<u8>,
    first: usize,
    end: usize,
    complete: bool,
    remove: usize,
    advance: bool,
}
pub(super) struct Plan {
    target: u64,
    cursor: Option<Vec<u8>>,
    entries: Vec<Entry>,
    metadata: Vec<Meta>,
    prepared: usize,
    retired: RetiredVersions,
    _permit: BufferPermit,
}
impl Plan {
    pub fn allocate(budget: &Arc<Budget>) -> Result<Self> {
        let mut permit = budget.buffer();
        permit.add(
            KEYS * std::mem::size_of::<Entry>()
                + METADATA * std::mem::size_of::<Meta>()
                + VERSIONS * std::mem::size_of::<Version>()
                + (KEYS + 2) * MAX_KEY_BYTES,
        )?;
        Ok(Self {
            target: 0,
            cursor: None,
            entries: Vec::with_capacity(KEYS),
            metadata: Vec::with_capacity(METADATA),
            prepared: 0,
            retired: RetiredVersions {
                values: Vec::with_capacity(VERSIONS),
                charge: 0,
                budget: budget.clone(),
            },
            _permit: permit,
        })
    }
    pub fn ready(&self) -> bool {
        self.prepared != 0
    }
    pub fn capture(&mut self, state: &mut State) -> bool {
        if state.file.is_none() {
            return false;
        }
        if state.gc_target == 0 {
            let floor = state
                .active
                .keys()
                .next()
                .copied()
                .unwrap_or(state.sequence);
            if floor <= state.gc_floor {
                return false;
            }
            state.gc_target = floor;
            state.gc_cursor = None;
        }
        self.target = state.gc_target;
        self.cursor = state.gc_cursor.clone();
        let lower = self.cursor.as_deref().map_or(Unbounded, Excluded);
        for key in state
            .history_keys
            .range::<[u8], _>((lower, Unbounded))
            .take(KEYS)
        {
            let remaining = METADATA - self.metadata.len();
            if remaining < 2 {
                break;
            }
            let versions = &state.rows[key];
            let first = self.metadata.len();
            let count = versions.len().min(VERSIONS + 1).min(remaining);
            self.metadata
                .extend(versions.iter().take(count).map(|v| Meta {
                    sequence: v.sequence,
                    charge: 64 + v.value.as_ref().map_or(0, |value| value.len()),
                }));
            self.entries.push(Entry {
                key: key.clone(),
                first,
                end: self.metadata.len(),
                complete: count == versions.len(),
                remove: 0,
                advance: false,
            });
        }
        if self.entries.is_empty() {
            state.gc_floor = state.gc_target;
            state.gc_target = 0;
            state.gc_cursor = None;
            return false;
        }
        true
    }
    pub fn prepare(&mut self) {
        let mut count = 0;
        let mut bytes = 0;
        for entry in &mut self.entries {
            self.prepared += 1;
            let mut position = entry.first;
            while position + 1 < entry.end && self.metadata[position + 1].sequence <= self.target {
                let charge = self.metadata[position].charge;
                if count == VERSIONS || (count != 0 && bytes + charge > BYTES) {
                    return;
                }
                count += 1;
                bytes += charge;
                entry.remove += 1;
                position += 1;
            }
            entry.advance = position + 1 < entry.end || entry.complete;
            if !entry.advance {
                return;
            }
        }
    }
    pub fn apply(&mut self, state: &mut State) -> bool {
        // Another collector may have advanced the cursor while preparation ran.
        if state.file.is_none() || state.gc_target != self.target || state.gc_cursor != self.cursor
        {
            return false;
        }
        let entries = &self.entries[..self.prepared];
        if entries.iter().any(|entry| {
            state
                .rows
                .get(&entry.key)
                .and_then(|v| v.front())
                .is_none_or(|v| v.sequence != self.metadata[entry.first].sequence)
        }) {
            return false;
        }
        for entry in entries {
            let versions = state.rows.get_mut(&entry.key).unwrap();
            for _ in 0..entry.remove {
                let v = versions.pop_front().unwrap();
                self.retired.charge += 64 + v.value.as_ref().map_or(0, |value| value.len());
                self.retired.values.push(v);
            }
            if !entry.advance {
                break;
            }
            let tombstone = versions.len() == 1
                && versions[0].sequence <= self.target
                && versions[0].value.is_none();
            if tombstone {
                state.rows.remove(&entry.key);
                state
                    .budget
                    .release(RETAINED, 192 + 2 * entry.key.len() + 64);
                state.history_keys.remove(&entry.key);
            } else if versions.len() == 1 && versions[0].value.is_some() {
                state.history_keys.remove(&entry.key);
            }
            state.gc_cursor = Some(entry.key.clone());
        }
        true
    }
    pub fn reclaimed_bytes(&self) -> usize {
        self.retired.charge
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transactions::{Database, ResourceLimits, Transaction};
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    fn history(
        keys: usize,
        versions: usize,
        bytes: usize,
        buffer: usize,
    ) -> (Database, Transaction) {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("artifacts")
            .join(format!(
                "gc-plan-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        let _ = std::fs::remove_file(&path);
        let db = Database::create_with_limits(
            &path,
            [41; 16],
            ResourceLimits {
                max_buffer_bytes: buffer,
                ..ResourceLimits::unlimited()
            },
        )
        .unwrap();
        let mut w = db.begin().unwrap();
        for key in 0..keys {
            w.put(&(key as u32).to_be_bytes(), &vec![1; bytes]).unwrap();
        }
        w.commit().unwrap();
        let old = db.begin_read().unwrap();
        for version in 1..versions {
            let mut w = db.begin().unwrap();
            for key in 0..keys {
                w.put(
                    &(key as u32).to_be_bytes(),
                    &vec![(version + 1) as u8; bytes],
                )
                .unwrap();
            }
            w.commit().unwrap();
        }
        // Dropping normally would run cooperative GC before the plan under test can capture.
        db.state().unwrap().active.remove(&old.snapshot);
        (db, old)
    }
    fn captured(db: &Database) -> Plan {
        let mut plan = Plan::allocate(&db.inner.budget).unwrap();
        assert!(plan.capture(&mut db.state().unwrap()));
        plan.prepare();
        plan
    }
    #[test]
    fn eq_prepared_collection_preserves_appended_and_deleted_values_and_pinned_generation() {
        let (db, setup) = history(3, 2, 16, usize::MAX);
        db.state().unwrap().active.insert(setup.snapshot, 1);
        let mut pinned = db.begin_read().unwrap();
        db.state().unwrap().active.remove(&setup.snapshot);
        let mut plan = captured(&db);
        db.state()
            .unwrap()
            .apply(
                3,
                std::collections::BTreeMap::from([
                    (0u32.to_be_bytes().to_vec(), Some(b"latest".to_vec())),
                    (1u32.to_be_bytes().to_vec(), None),
                    (9u32.to_be_bytes().to_vec(), Some(b"added".to_vec())),
                ]),
            )
            .unwrap();
        assert!(plan.apply(&mut db.state().unwrap()));
        for key in 0..3u32 {
            assert_eq!(pinned.get(&key.to_be_bytes()).unwrap(), Some(vec![2; 16]));
        }
        drop(plan);
        assert_eq!(
            db.begin_read().unwrap().get(&0u32.to_be_bytes()).unwrap(),
            Some(b"latest".to_vec())
        );
        assert_eq!(
            db.begin_read().unwrap().get(&1u32.to_be_bytes()).unwrap(),
            None
        );
        assert_eq!(
            db.begin_read().unwrap().get(&9u32.to_be_bytes()).unwrap(),
            Some(b"added".to_vec())
        );
    }
    #[test]
    fn st_stale_plan_and_persistence_window_cannot_change_retained_data() {
        let (db, _setup) = history(65, 2, 16, usize::MAX);
        let mut plan = captured(&db);
        let mut state = db.state().unwrap();
        let file = state.file.take();
        assert!(!plan.apply(&mut state));
        assert_eq!(plan.reclaimed_bytes(), 0);
        state.file = file;
        drop(state.collect());
        let rows = state.rows.values().map(|v| v.len()).sum::<usize>();
        assert!(!plan.apply(&mut state));
        assert_eq!(state.rows.values().map(|v| v.len()).sum::<usize>(), rows);
        drop(state);
        drop(plan);
        assert_eq!(db.resource_usage().unwrap().current[4], 0);
    }
    #[test]
    fn bv_key_and_version_limits_make_progress_without_skipping_suffixes() {
        for (keys, versions) in [(63, 2), (64, 2), (65, 2), (1, 128), (1, 129), (1, 130)] {
            let (db, _setup) = history(keys, versions, 1, usize::MAX);
            let mut plan = captured(&db);
            let before = db.stats().unwrap().retained_versions;
            assert!(plan.apply(&mut db.state().unwrap()));
            let removed = before - db.stats().unwrap().retained_versions;
            assert_eq!(
                removed,
                if keys == 1 {
                    (versions - 1).min(128)
                } else {
                    keys.min(64)
                }
            );
            drop(plan);
            while db.collect_garbage().unwrap() {}
            assert_eq!(db.stats().unwrap().retained_versions, keys);
        }
    }
    #[test]
    fn bv_byte_limit_and_oversized_values_remain_charged_until_plan_drop() {
        for bytes in [BYTES / 2 - 64, BYTES / 2 - 63, BYTES + 1] {
            let (db, _setup) = history(2, 2, bytes, usize::MAX);
            let before = db.resource_usage().unwrap().current[RETAINED];
            let mut plan = captured(&db);
            assert!(plan.apply(&mut db.state().unwrap()));
            let expected = (64 + bytes) * if bytes == BYTES / 2 - 64 { 2 } else { 1 };
            assert_eq!(plan.reclaimed_bytes(), expected);
            assert_eq!(db.resource_usage().unwrap().current[RETAINED], before);
            drop(plan);
            assert_eq!(
                db.resource_usage().unwrap().current[RETAINED],
                before - expected
            );
            assert_eq!(db.resource_usage().unwrap().current[4], 0);
        }
    }
    #[test]
    fn bv_preparation_reservation_rejects_below_and_accepts_at_the_limit() {
        let unlimited = Budget::new(ResourceLimits::unlimited());
        let plan = Plan::allocate(&unlimited).unwrap();
        let required = unlimited.snapshot().unwrap().current[4];
        drop(plan);
        assert_eq!(unlimited.snapshot().unwrap().current[4], 0);
        for limit in [required - 1, required, required + 1] {
            let budget = Budget::new(ResourceLimits {
                max_buffer_bytes: limit,
                ..ResourceLimits::unlimited()
            });
            let plan = Plan::allocate(&budget);
            assert_eq!(plan.is_ok(), limit >= required);
            drop(plan);
            assert_eq!(budget.snapshot().unwrap().current[4], 0);
        }
    }
    #[test]
    fn eq_tombstone_removal_and_empty_capture_finish_without_leaked_charges() {
        let (db, _setup) = history(1, 2, 16, usize::MAX);
        db.state()
            .unwrap()
            .apply(
                3,
                std::collections::BTreeMap::from([(0u32.to_be_bytes().to_vec(), None)]),
            )
            .unwrap();
        let mut plan = captured(&db);
        assert!(plan.apply(&mut db.state().unwrap()));
        assert_eq!(db.stats().unwrap().visible_records, 0);
        drop(plan);
        let mut empty = Plan::allocate(&db.inner.budget).unwrap();
        assert!(!empty.capture(&mut db.state().unwrap()));
        drop(empty);
        assert_eq!(db.resource_usage().unwrap().current[RETAINED], 0);
        assert_eq!(db.resource_usage().unwrap().current[4], 0);
        assert!(!db.reclamation_pending().unwrap());
    }

    #[test]
    fn er_preparation_budget_refusal_keeps_background_collection_available() {
        let (db, _setup) = history(2, 2, 1, 4096);
        assert!(Plan::allocate(&db.inner.budget).is_err());
        assert_eq!(db.resource_usage().unwrap().current[4], 0);
        db.set_background_reclamation(true).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while db.reclamation_pending().unwrap() && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        db.set_background_reclamation(false).unwrap();
        assert_eq!(db.stats().unwrap().retained_versions, 2);
        assert_eq!(db.resource_usage().unwrap().current[4], 0);
    }
}
