use silverhamu::transactions::Database;
use std::error::Error;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn key(sequence: u64, side: char) -> String {
    format!("power-cut/{sequence}/{side}")
}

fn exercise(path: &Path, witness: &Path, count: u64) -> Result<()> {
    let mut log = File::create_new(witness)?;
    let id = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let db = Database::create(path, id.to_be_bytes())?;
    writeln!(log, "{id}")?;
    log.sync_all()?;
    for sequence in 1..=count {
        let mut tx = db.begin()?;
        for side in ['a', 'b'] {
            let key = key(sequence, side);
            tx.put(key.as_bytes(), key.as_bytes())?;
        }
        tx.commit()?;
        writeln!(log, "{sequence}")?;
        log.sync_all()?;
        if sequence % 100 == 0 {
            println!("checkpoint start {sequence}");
            db.checkpoint()?;
            println!("checkpoint end {sequence}");
        }
    }
    Ok(())
}

fn verify(path: &Path, witness: &Path) -> Result<()> {
    let log = fs::read_to_string(witness)?;
    // A cut during the final witness append can leave an incomplete last line.
    let (complete, tail) = log.rsplit_once('\n').ok_or("incomplete witness header")?;
    let mut lines = complete.lines();
    let id: u128 = lines.next().ok_or("missing database ID")?.parse()?;
    let mut confirmed = 0u64;
    for line in lines {
        confirmed += 1;
        if line != confirmed.to_string() {
            return Err("non-sequential witness record".into());
        }
    }
    if !(confirmed + 1).to_string().starts_with(tail) {
        return Err("invalid final witness record".into());
    }
    let start = Instant::now();
    let db = Database::open(path)?;
    if db.id() != id.to_be_bytes() {
        return Err("witness belongs to another database".into());
    }
    db.begin()?.get(key(1, 'a').as_bytes())?;
    let ready = start.elapsed();
    for sequence in 1..=confirmed + 1 {
        let mut tx = db.begin()?;
        let mut found = 0;
        for side in ['a', 'b'] {
            let key = key(sequence, side);
            if let Some(value) = tx.get(key.as_bytes())? {
                if value != key.as_bytes() {
                    return Err(format!("wrong value at {key}").into());
                }
                found += 1;
            }
        }
        if found == 1 || (sequence <= confirmed && found != 2) {
            return Err(format!("sequence {sequence}: recovered {found}/2 keys").into());
        }
    }
    println!(
        "confirmed={confirmed} open_to_first_read_ms={:.3}",
        ready.as_secs_f64() * 1000.0
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [mode, db, log, count] if mode == "write" => {
            exercise(Path::new(db), Path::new(log), count.parse()?)
        }
        [mode, db, log] if mode == "verify" => verify(Path::new(db), Path::new(log)),
        _ => Err("usage: power_cut write NEW_DB NEW_WITNESS COUNT | verify DB WITNESS".into()),
    }
}
