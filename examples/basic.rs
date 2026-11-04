use newdbms::transactions::{Database, ResourceLimits};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // This ID identifies this example database; it is not a credential.
    let db = Database::create_with_limits(
        "example.ndb", *b"catdb-example-01", ResourceLimits::default(),
    )?;
    let mut tx = db.begin()?;
    tx.put(b"hello", b"world")?;
    tx.commit()?;

    let mut reader = db.begin()?;
    assert_eq!(reader.get(b"hello")?, Some(b"world".to_vec()));
    reader.rollback();
    Ok(())
}
