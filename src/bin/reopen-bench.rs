use silverhamu::transactions::Database;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os().nth(1).ok_or("database path required")?;
    let began = std::time::Instant::now();
    let db = Database::open(path)?;
    let elapsed = began.elapsed().as_nanos();
    let stats = db.stats()?;
    println!(
        "{{\"open_ns\":{elapsed},\"rows\":{},\"version\":{},\"wal_bytes\":{}}}",
        stats.visible_records, stats.version, stats.wal_bytes
    );
    Ok(())
}
