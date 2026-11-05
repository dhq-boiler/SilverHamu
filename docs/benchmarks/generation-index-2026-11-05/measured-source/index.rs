use std::collections::HashSet;
use std::ops::Bound;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: u64,
    pub value: Option<u64>,
    pub generation: u64,
}

pub trait Summary: Copy + Default {
    fn combine(generation: u64, left: Self, right: Self) -> Self;
    fn unchanged(self, snapshot: u64) -> bool;
}

#[derive(Clone, Copy, Default)]
pub struct CountOnly;
impl Summary for CountOnly {
    fn combine(_: u64, _: Self, _: Self) -> Self {
        Self
    }
    fn unchanged(self, _: u64) -> bool {
        false
    }
}

#[derive(Clone, Copy, Default)]
pub struct MaxGeneration(u64);
impl Summary for MaxGeneration {
    fn combine(g: u64, l: Self, r: Self) -> Self {
        Self(g.max(l.0).max(r.0))
    }
    fn unchanged(self, snapshot: u64) -> bool {
        self.0 <= snapshot
    }
}

type Link<M> = Option<Arc<Node<M>>>;
struct Node<M: Summary> {
    row: Entry,
    left: Link<M>,
    right: Link<M>,
    count: usize,
    summary: M,
}

fn count<M: Summary>(link: &Link<M>) -> usize {
    link.as_ref().map_or(0, |n| n.count)
}
fn summary<M: Summary>(link: &Link<M>) -> M {
    link.as_ref().map_or_else(M::default, |n| n.summary)
}

pub fn hash(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e3779b97f4a7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d049bb133111eb);
    x ^ (x >> 31)
}

fn node<M: Summary>(row: Entry, left: Link<M>, right: Link<M>) -> Arc<Node<M>> {
    Arc::new(Node {
        row,
        count: count(&left) + count(&right) + usize::from(row.value.is_some()),
        summary: M::combine(row.generation, summary(&left), summary(&right)),
        left,
        right,
    })
}

fn update<M: Summary>(root: &Link<M>, row: Entry, allocations: &mut usize) -> Arc<Node<M>> {
    *allocations += 1;
    let Some(old) = root else {
        return node(row, None, None);
    };
    if row.key == old.row.key {
        return node(row, old.left.clone(), old.right.clone());
    }
    if row.key < old.row.key {
        let left = update(&old.left, row, allocations);
        if hash(left.row.key) < hash(old.row.key) {
            *allocations += 1;
            let right = node(old.row, left.right.clone(), old.right.clone());
            node(left.row, left.left.clone(), Some(right))
        } else {
            node(old.row, Some(left), old.right.clone())
        }
    } else {
        let right = update(&old.right, row, allocations);
        if hash(right.row.key) < hash(old.row.key) {
            *allocations += 1;
            let left = node(old.row, old.left.clone(), right.left.clone());
            node(right.row, Some(left), right.right.clone())
        } else {
            node(old.row, old.left.clone(), Some(right))
        }
    }
}

#[derive(Clone)]
pub struct Tree<M: Summary> {
    root: Link<M>,
    generation: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Range {
    pub start: Bound<u64>,
    pub end: Bound<u64>,
}
impl Range {
    pub fn all() -> Self {
        Self {
            start: Bound::Unbounded,
            end: Bound::Unbounded,
        }
    }
    fn below(self, key: u64) -> bool {
        match self.start {
            Bound::Included(x) => key < x,
            Bound::Excluded(x) => key <= x,
            Bound::Unbounded => false,
        }
    }
    fn above(self, key: u64) -> bool {
        match self.end {
            Bound::Included(x) => key > x,
            Bound::Excluded(x) => key >= x,
            Bound::Unbounded => false,
        }
    }
    pub fn contains(self, key: u64) -> bool {
        !self.below(key) && !self.above(key)
    }
}

impl<M: Summary> Tree<M> {
    pub fn new(rows: &[Entry]) -> Self {
        assert!(rows.windows(2).all(|w| w[0].key < w[1].key));
        let mut left = vec![None; rows.len()];
        let mut right = vec![None; rows.len()];
        let mut stack: Vec<usize> = Vec::new();
        for (i, row) in rows.iter().enumerate() {
            let mut previous = None;
            while stack
                .last()
                .is_some_and(|&j| hash(rows[j].key) > hash(row.key))
            {
                previous = stack.pop();
            }
            left[i] = previous;
            if let Some(&j) = stack.last() {
                right[j] = Some(i);
            }
            stack.push(i);
        }
        fn build<M: Summary>(
            i: Option<usize>,
            rows: &[Entry],
            l: &[Option<usize>],
            r: &[Option<usize>],
        ) -> Link<M> {
            i.map(|i| node(rows[i], build(l[i], rows, l, r), build(r[i], rows, l, r)))
        }
        Self {
            root: build(stack.first().copied(), rows, &left, &right),
            generation: rows.iter().map(|r| r.generation).max().unwrap_or(0),
        }
    }
    pub fn len(&self) -> usize {
        count(&self.root)
    }
    pub fn update(&self, row: Entry) -> Result<(Self, usize), &'static str> {
        if row.generation < self.generation {
            return Err("generation regression");
        }
        let mut allocations = 0;
        let root = Some(update(&self.root, row, &mut allocations));
        Ok((
            Self {
                root,
                generation: row.generation,
            },
            allocations * Self::node_bytes(),
        ))
    }
    pub fn read(&self, rank: usize, limit: usize) -> Vec<(u64, u64)> {
        fn visit<M: Summary>(
            root: &Link<M>,
            skip: &mut usize,
            limit: usize,
            out: &mut Vec<(u64, u64)>,
        ) {
            let Some(n) = root else { return };
            if out.len() == limit {
                return;
            }
            if *skip >= n.count {
                *skip -= n.count;
                return;
            }
            visit(&n.left, skip, limit, out);
            if let Some(value) = n.row.value {
                if *skip > 0 {
                    *skip -= 1;
                } else if out.len() < limit {
                    out.push((n.row.key, value));
                }
            }
            if out.len() < limit {
                visit(&n.right, skip, limit, out);
            }
        }
        let mut out = Vec::with_capacity(limit.min(self.len().saturating_sub(rank)));
        visit(&self.root, &mut { rank }, limit, &mut out);
        out
    }
    pub fn conflict_with_global_guard(&self, range: Range, snapshot: u64) -> bool {
        self.generation > snapshot && self.conflict(range, snapshot)
    }
    pub fn conflict(&self, range: Range, snapshot: u64) -> bool {
        fn visit<M: Summary>(root: &Link<M>, range: Range, snapshot: u64) -> bool {
            let Some(n) = root else { return false };
            if n.summary.unchanged(snapshot) {
                return false;
            }
            if range.below(n.row.key) {
                return visit(&n.right, range, snapshot);
            }
            if range.above(n.row.key) {
                return visit(&n.left, range, snapshot);
            }
            // Tombstones must remain visible to conflict detection even when their live count is zero.
            n.row.generation > snapshot
                || visit(&n.left, range, snapshot)
                || visit(&n.right, range, snapshot)
        }
        visit(&self.root, range, snapshot)
    }
    pub fn node_bytes() -> usize {
        std::mem::size_of::<Node<M>>() + 2 * std::mem::size_of::<usize>()
    }
    pub fn retained_bytes(&self, old: &Self) -> usize {
        fn visit<M: Summary>(link: &Link<M>, seen: &mut HashSet<usize>) {
            if let Some(n) = link {
                if seen.insert(Arc::as_ptr(n) as usize) {
                    visit(&n.left, seen);
                    visit(&n.right, seen);
                }
            }
        }
        let mut seen = HashSet::new();
        visit(&self.root, &mut seen);
        visit(&old.root, &mut seen);
        seen.len() * Self::node_bytes()
    }
}
