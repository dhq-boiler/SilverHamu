#[path = "../benchmarks/generation-index/index.rs"]
mod index;
use index::*;
use std::collections::BTreeMap;
use std::ops::Bound::*;

fn rows(n: u64) -> Vec<Entry> {
    (0..n)
        .map(|i| Entry {
            key: i * 2,
            value: Some(i),
            generation: 1,
        })
        .collect()
}

#[test]
fn eq_st_mutations_preserve_every_value_rank_and_old_root() {
    let source = rows(2048);
    let old = Tree::<MaxGeneration>::new(&source);
    let mut tree = old.clone();
    let mut reference: BTreeMap<_, _> = source.iter().map(|r| (r.key, *r)).collect();
    for i in 0..1600 {
        let key = hash(i) % 5000;
        let row = Entry {
            key,
            value: (i % 3 != 0).then_some(i),
            generation: i + 2,
        };
        tree = tree.update(row).unwrap().0;
        reference.insert(key, row);
        if i % 100 == 0 {
            let expected: Vec<_> = reference
                .values()
                .filter_map(|r| r.value.map(|v| (r.key, v)))
                .collect();
            assert_eq!(tree.read(0, usize::MAX), expected);
            for rank in (0..expected.len() + 100).step_by(97) {
                assert_eq!(
                    tree.read(rank, 100),
                    expected
                        .iter()
                        .skip(rank)
                        .take(100)
                        .copied()
                        .collect::<Vec<_>>()
                );
            }
            assert_eq!(
                old.read(0, usize::MAX),
                source
                    .iter()
                    .map(|r| (r.key, r.value.unwrap()))
                    .collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn bv_empty_rank_endpoints_and_deleted_only_subtrees() {
    let empty = Tree::<MaxGeneration>::new(&[]);
    assert_eq!(empty.len(), 0);
    assert!(empty.read(usize::MAX, 100).is_empty());
    assert!(!empty.conflict(Range::all(), 0));
    let mut tree = Tree::<MaxGeneration>::new(&rows(130));
    for key in 0..130 {
        tree = tree
            .update(Entry {
                key: key * 2,
                value: None,
                generation: 2,
            })
            .unwrap()
            .0;
    }
    assert_eq!(tree.len(), 0);
    assert!(tree.read(0, 100).is_empty());
    assert!(tree.conflict(Range::all(), 1));
    assert!(!tree.conflict(Range::all(), 2));
    assert!(
        tree.update(Entry {
            key: 0,
            value: Some(1),
            generation: 1
        })
        .is_err()
    );
    let boundary = Tree::<MaxGeneration>::new(&[
        Entry {
            key: 0,
            value: Some(0),
            generation: 1,
        },
        Entry {
            key: u64::MAX,
            value: Some(1),
            generation: 2,
        },
    ]);
    assert_eq!(boundary.read(1, 100), vec![(u64::MAX, 1)]);
    assert!(boundary.read(2, 100).is_empty());
    assert!(boundary.read(0, 0).is_empty());
}

#[test]
fn cc_range_bounds_and_snapshot_generations_match_full_scan() {
    let mut entries = rows(40);
    entries.extend([
        Entry {
            key: 81,
            value: Some(81),
            generation: 2,
        },
        Entry {
            key: 83,
            value: None,
            generation: 3,
        },
    ]);
    let mut tree = Tree::<MaxGeneration>::new(&entries);
    tree = tree
        .update(Entry {
            key: 4,
            value: None,
            generation: 4,
        })
        .unwrap()
        .0;
    entries[2] = Entry {
        key: 4,
        value: None,
        generation: 4,
    };
    let bounds: Vec<_> = std::iter::once(Unbounded)
        .chain(
            [0, 1, 4, 5, 40, 78, 81, 83, 84, u64::MAX]
                .into_iter()
                .flat_map(|k| [Included(k), Excluded(k)]),
        )
        .collect();
    for &start in &bounds {
        for &end in &bounds {
            for snapshot in 0..6 {
                let range = Range { start, end };
                let expected = entries
                    .iter()
                    .any(|r| range.contains(r.key) && r.generation > snapshot);
                assert_eq!(
                    tree.conflict(range, snapshot),
                    expected,
                    "{range:?}, snapshot={snapshot}"
                );
            }
        }
    }
}

#[test]
fn cc_randomized_insert_delete_resurrect_and_same_generation_match_oracle() {
    let mut a = Tree::<MaxGeneration>::new(&[]);
    let mut b = Tree::<CountOnly>::new(&[]);
    let mut oracle = BTreeMap::new();
    for i in 0..3000u64 {
        let row = Entry {
            key: hash(i) % 800,
            value: (i % 3 != 0).then_some(i),
            generation: 1 + i / 4,
        };
        a = a.update(row).unwrap().0;
        b = b.update(row).unwrap().0;
        oracle.insert(row.key, row);
        for offset in 0..4 {
            let x = hash(i + offset) % 900;
            let y = hash(i + offset + 1) % 900;
            let range = Range {
                start: Included(x.min(y)),
                end: Excluded(x.max(y)),
            };
            let snapshot = hash(i + offset + 2) % (row.generation + 2);
            let expected = oracle
                .values()
                .any(|r| range.contains(r.key) && r.generation > snapshot);
            assert_eq!(a.conflict(range, snapshot), expected);
            assert_eq!(b.conflict(range, snapshot), expected);
            assert_eq!(b.conflict_with_global_guard(range, snapshot), expected);
        }
        assert_eq!(a.read(i as usize % 800, 100), b.read(i as usize % 800, 100));
    }
}

#[test]
fn st_memory_counts_pinned_roots_and_releases_unpinned_paths() {
    let old = Tree::<MaxGeneration>::new(&rows(100));
    let current = old
        .update(Entry {
            key: 50,
            value: None,
            generation: 2,
        })
        .unwrap()
        .0;
    assert_eq!(
        old.retained_bytes(&old),
        100 * Tree::<MaxGeneration>::node_bytes()
    );
    assert!(current.retained_bytes(&old) > current.retained_bytes(&current));
    assert_eq!(
        current.retained_bytes(&current),
        100 * Tree::<MaxGeneration>::node_bytes()
    );
}
