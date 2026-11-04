use newdbms::{Snapshot, create};
use std::error::Error;
use std::path::Path;
use std::time::Instant;

const DEMO_GENERATION: [u8; 16] = *b"synthetic-demo01";

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("demo") if args.len() == 4 => {
            let count: usize = args[3].parse()?;
            if count > 1_000_000 {
                return Err("demo count must be at most 1000000".into());
            }
            let path = Path::new(&args[2]);
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            let records: Vec<Vec<u8>> = (0..count)
                .map(|i| {
                    format!(
                        "book-{i:08}\tサンプル書籍{i}\tauthor-{}\tcache/{i}.webp",
                        i % 1000
                    )
                    .into_bytes()
                })
                .collect();
            let begin = Instant::now();
            create(path, DEMO_GENERATION, &records)?;
            println!(
                "synthetic_create_ms={:.3} records={count} bytes={}",
                begin.elapsed().as_secs_f64() * 1000.0,
                path.metadata()?.len()
            );
            measure_read(path, 0, 100)?;
        }
        Some("read") if args.len() == 5 => {
            measure_read(Path::new(&args[2]), args[3].parse()?, args[4].parse()?)?;
        }
        _ => {
            return Err(
                "usage: catdb demo <new-file> <count> | read <demo-file> <start> <limit>".into(),
            );
        }
    }
    Ok(())
}

fn measure_read(path: &Path, start: u64, limit: usize) -> Result<(), Box<dyn Error>> {
    let begin = Instant::now();
    let mut snapshot = Snapshot::open(path, DEMO_GENERATION)?;
    let open_ms = begin.elapsed().as_secs_f64() * 1000.0;
    let begin = Instant::now();
    let records = snapshot.read_page(start, limit)?;
    let read_ms = begin.elapsed().as_secs_f64() * 1000.0;
    let bytes: usize = records.iter().map(Vec::len).sum();
    println!(
        "synthetic_open_ms={open_ms:.3} page_read_ms={read_ms:.3} total_records={} returned={} payload_bytes={bytes}",
        snapshot.len(),
        records.len()
    );
    Ok(())
}
