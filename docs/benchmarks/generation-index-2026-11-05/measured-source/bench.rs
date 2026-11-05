mod index;
use index::*;
use std::collections::BTreeMap;
use std::hint::black_box;
use std::io::Write;
use std::ops::Bound::*;
use std::time::Instant;

fn emit(out: &mut impl Write, method: &str, case: &str, samples: &[u128], bytes: usize) {
    for (i, ns) in samples.iter().enumerate() {
        writeln!(out, "{method},{case},{i},{ns},{bytes}").unwrap();
    }
}

fn queries(n: u64, generation: u64, case: &str) -> Vec<(Range, u64)> {
    (0..96)
        .map(|i| {
            let key = hash(i) % n * 2;
            match case {
                "stable_wide" => (Range::all(), generation),
                "stable_narrow" => (
                    Range {
                        start: Included(key),
                        end: Excluded(key.saturating_add(200)),
                    },
                    generation,
                ),
                "stale_wide" => (
                    Range {
                        start: Included(key),
                        end: Excluded(key.saturating_add(n / 2)),
                    },
                    1,
                ),
                "stale_narrow" => (
                    Range {
                        start: Included(key),
                        end: Excluded(key.saturating_add(200)),
                    },
                    1,
                ),
                "stale_upper_half" => (
                    Range {
                        start: Included(n),
                        end: Unbounded,
                    },
                    1,
                ),
                _ => unreachable!(),
            }
        })
        .collect()
}

fn run<M: Summary>(
    out: &mut impl Write,
    name: &str,
    source: &[Entry],
    edits: &[Entry],
    expected: &[(u64, u64)],
) {
    let start = Instant::now();
    let old = Tree::<M>::new(black_box(source));
    emit(
        out,
        name,
        "build",
        &[start.elapsed().as_nanos()],
        old.retained_bytes(&old),
    );
    let mut tree = old.clone();
    let mut samples = Vec::new();
    let mut written = 0;
    for row in edits {
        let start = Instant::now();
        let (next, bytes) = tree.update(black_box(*row)).unwrap();
        tree = next;
        samples.push(start.elapsed().as_nanos());
        written += bytes;
    }
    emit(out, name, "update", &samples, written);
    assert_eq!(tree.read(0, usize::MAX), expected);
    assert_eq!(
        old.read(0, usize::MAX),
        source
            .iter()
            .map(|r| (r.key, r.value.unwrap()))
            .collect::<Vec<_>>()
    );
    let pinned = tree.retained_bytes(&old);
    emit(out, name, "retained_pinned", &[0], pinned);
    emit(
        out,
        name,
        "retained_current",
        &[0],
        tree.retained_bytes(&tree),
    );
    samples.clear();
    for i in 0..4096 {
        let rank = hash(i) as usize % expected.len();
        let start = Instant::now();
        let page = black_box(tree.read(black_box(rank), 100));
        samples.push(start.elapsed().as_nanos());
        assert_eq!(page, expected[rank..expected.len().min(rank + 100)]);
    }
    emit(out, name, "rank100", &samples, pinned);
    let mut reference: BTreeMap<u64, Entry> = source.iter().map(|r| (r.key, *r)).collect();
    for row in edits {
        reference.insert(row.key, *row);
    }
    for case in [
        "stable_wide",
        "stable_narrow",
        "stale_wide",
        "stale_narrow",
        "stale_upper_half",
    ] {
        samples.clear();
        for (range, snapshot) in
            queries(source.len() as u64, edits.last().unwrap().generation, case)
        {
            let answer = reference
                .range((range.start, range.end))
                .any(|(_, r)| r.generation > snapshot);
            let start = Instant::now();
            let got = black_box(tree.conflict(black_box(range), black_box(snapshot)));
            samples.push(start.elapsed().as_nanos());
            assert_eq!(got, answer);
        }
        emit(out, name, case, &samples, pinned);
        if name == "count_only" {
            samples.clear();
            for (range, snapshot) in
                queries(source.len() as u64, edits.last().unwrap().generation, case)
            {
                let answer = reference
                    .range((range.start, range.end))
                    .any(|(_, r)| r.generation > snapshot);
                let start = Instant::now();
                let got = black_box(
                    tree.conflict_with_global_guard(black_box(range), black_box(snapshot)),
                );
                samples.push(start.elapsed().as_nanos());
                assert_eq!(got, answer);
            }
            emit(out, "count_global_guard", case, &samples, pinned);
        }
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    assert_eq!(
        args.len(),
        5,
        "bench <rows> <local|spread> <plain-first|max-first> <new-output.csv>"
    );
    let n: u64 = args[1].parse().unwrap();
    assert!(n >= 1000);
    assert!(["local", "spread"].contains(&args[2].as_str()));
    assert!(["plain-first", "max-first"].contains(&args[3].as_str()));
    let mut out = std::io::BufWriter::new(
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args[4])
            .unwrap(),
    );
    writeln!(out, "method,case,sample,ns,bytes").unwrap();
    let source: Vec<_> = (0..n)
        .map(|i| Entry {
            key: i * 2,
            value: Some(i),
            generation: 1,
        })
        .collect();
    let mut oracle: BTreeMap<_, _> = source.iter().map(|r| (r.key, *r)).collect();
    let edits: Vec<_> = (0..2400)
        .map(|i| {
            let region = if args[2] == "local" { n / 100 } else { n };
            Entry {
                key: (hash(i) % region) * 2 + u64::from(i % 3 == 0),
                value: (i % 3 != 1).then_some(i),
                generation: i + 2,
            }
        })
        .collect();
    for row in &edits {
        oracle.insert(row.key, *row);
    }
    let expected: Vec<_> = oracle
        .values()
        .filter_map(|r| r.value.map(|v| (r.key, v)))
        .collect();
    if args[3] == "plain-first" {
        run::<CountOnly>(&mut out, "count_only", &source, &edits, &expected);
        run::<MaxGeneration>(&mut out, "count_max", &source, &edits, &expected);
    } else {
        run::<MaxGeneration>(&mut out, "count_max", &source, &edits, &expected);
        run::<CountOnly>(&mut out, "count_only", &source, &edits, &expected);
    }
    let mut samples = Vec::new();
    for i in 0..4096 {
        let rank = hash(i) as usize % expected.len();
        let start = Instant::now();
        let page = black_box(expected[rank..expected.len().min(rank + 100)].to_vec());
        samples.push(start.elapsed().as_nanos());
        assert_eq!(page, expected[rank..expected.len().min(rank + 100)]);
    }
    emit(
        &mut out,
        "sorted_vec",
        "rank100",
        &samples,
        expected.len() * std::mem::size_of::<(u64, u64)>(),
    );
    for case in [
        "stable_wide",
        "stable_narrow",
        "stale_wide",
        "stale_narrow",
        "stale_upper_half",
    ] {
        samples.clear();
        for (range, snapshot) in queries(n, edits.last().unwrap().generation, case) {
            let answer = oracle
                .values()
                .any(|r| range.contains(r.key) && r.generation > snapshot);
            let start = Instant::now();
            let snapshot = black_box(snapshot);
            let got = black_box(
                oracle
                    .range((black_box(range.start), black_box(range.end)))
                    .any(|(_, r)| r.generation > snapshot),
            );
            samples.push(start.elapsed().as_nanos());
            assert_eq!(got, answer);
        }
        emit(&mut out, "btree_map", case, &samples, 0);
    }
    out.flush().unwrap();
    println!(
        "verified {n} rows, {} edits, {} live rows; {}",
        edits.len(),
        expected.len(),
        args[4]
    );
}
