use serde_json::json;
use silverhamu_client::{AccountOptions, Client, Error};
use std::{env, path::Path, time::Duration};

fn required(name: &str) -> String {
    env::var(name).expect(name)
}

fn bytes(name: &str) -> Vec<u8> {
    let text = required(name);
    assert_eq!(text.len() % 2, 0);
    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).unwrap())
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let host = required("CATDB_HOST");
    let port = required("CATDB_PORT").parse()?;
    let timeout = Duration::from_secs(10);
    let mut client = if let Ok(token) = env::var("CATDB_TOKEN") {
        Client::connect_token(&host, port, &token, timeout)?
    } else {
        let username = required("CATDB_USER");
        let password = required("CATDB_PASSWORD");
        let ca = env::var("CATDB_CA_FILE").ok();
        let mut account = AccountOptions::new(&username, &password);
        account.ca_file = ca.as_deref().map(Path::new);
        Client::connect_account(&host, port, account, timeout)?
    };
    client.ping()?;
    if env::var("CATDB_AUTH_ONLY").as_deref() == Ok("1") {
        return Ok(());
    }
    let key = [0xff, 0];
    let value = bytes("CATDB_VALUE");
    assert_eq!(client.get_latest(&key)?, Some(bytes("CATDB_EXPECT")));
    let probe = b"rust\x00";
    client.delete_commit(probe)?;
    assert_eq!(client.get_latest(probe)?, None);
    client.begin()?;
    client.put(probe, b"")?;
    assert!(matches!(client.begin(), Err(Error::Server { code }) if code == "invalid_request"));
    assert_eq!(client.get(probe)?, Some(vec![]));
    client.rollback()?;
    assert_eq!(client.get_latest(probe)?, None);
    client.begin()?;
    client.put(probe, b"")?;
    client.commit()?;
    client.begin_read()?;
    assert_eq!(client.get(probe)?, Some(vec![]));
    assert!(
        matches!(client.put(probe, &value), Err(Error::Server { code }) if code == "invalid_request")
    );
    client.rollback()?;
    assert!(matches!(client.commit(), Err(Error::Server { code }) if code == "invalid_request"));
    client.begin()?;
    client.delete(probe)?;
    assert_eq!(client.get(probe)?, None);
    client.rollback()?;
    assert_eq!(client.get_latest(probe)?, Some(vec![]));
    client.delete_commit(probe)?;
    assert_eq!(client.get_latest(probe)?, None);
    client.put_commit(&key, &value)?;
    client.begin_read()?;
    let scan = client.request(&json!({"op":"scan", "start":"ff00", "rows":1}))?;
    assert_eq!(scan["rows"][0][0], "ff00");
    client.rollback()?;
    client.close();
    client.close();
    assert!(matches!(client.ping(), Err(Error::Closed)));
    println!("Rust interoperation passed");
    Ok(())
}
