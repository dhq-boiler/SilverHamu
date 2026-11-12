# silverhamu-client for Rust

A synchronous Rust 1.89+ client for the SilverHamu TCP service. It is independent
of the embedded storage engine. The API and protocol are experimental.

Install from crates.io:

```sh
cargo add silverhamu-client@0.2.1
```

Or add the dependency to `Cargo.toml`:

```toml
[dependencies]
silverhamu-client = "0.2.1"
```

```rust,no_run
use silverhamu_client::Client;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = std::env::var("CATDB_TOKEN")?;
    let mut client = Client::connect_token("127.0.0.1", 7400, &token, Duration::from_secs(10))?;
    let key = [0, 255];
    let version: u64 = client.put_commit(&key, b"hello")?;
    let value: Option<Vec<u8>> = client.get_latest(&key)?;
    assert_eq!(value.as_deref(), Some(b"hello".as_slice()));
    println!("committed version {version}");
    Ok(())
}
```

Token connections are plaintext: use loopback or an encrypted tunnel. Named accounts
always use TLS 1.3 with certificate and hostname verification:

```rust,no_run
use silverhamu_client::{AccountOptions, Client};
use std::{path::Path, time::Duration};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let password = std::env::var("CATDB_PASSWORD")?;
    let mut account = AccountOptions::new("owner", &password);
    account.database = "main";
    account.ca_file = Some(Path::new("ca.pem"));
    let mut client = Client::connect_account("localhost", 7400, account, Duration::from_secs(10))?;
    client.ping()?;
    Ok(())
}
```

Omit `ca_file` to use the platform's system trust store. An explicit PEM CA bundle
replaces system roots. TLS verification cannot be disabled.

`get_latest`, `put_commit` and `delete_commit` perform single operations. `ping`
and successful commits return `u64` versions. Missing keys return `None`; stored
empty values return `Some(Vec::new())`.

Use `begin` or `begin_read`, then `get`, `put` and `delete`, followed by `commit`
or `rollback`. A transaction belongs to its connection. Use separate clients for
independent concurrent transactions. Mutating methods require `&mut Client`.
`begin_read` is read-only; the server rejects writes in that transaction.

```rust,no_run
use silverhamu_client::Client;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = std::env::var("CATDB_TOKEN")?;
    let mut client = Client::connect_token("127.0.0.1", 7400, &token, Duration::from_secs(10))?;
    client.begin()?;
    client.put(b"key", b"value")?;
    assert_eq!(client.get(b"key")?, Some(b"value".to_vec()));
    client.rollback()?;

    client.begin_read()?;
    let page = client.request(&serde_json::json!({"op":"scan", "start":"", "rows":100}))?;
    client.rollback()?;
    Ok(())
}
```

`request(&serde_json::Value)` exposes all wire operations. Raw versions are decimal
strings; raw keys and values are hexadecimal. Keep paginated snapshot scans on the
same transaction and connection. [Wire operations and limits](../../docs/service.md#wire-format)

Match `Error::Server { code }` for a machine-readable rejection. Ordinary server
errors leave the connection available; `unauthorized` closes it. Invalid input or
an oversized local request returns `Error::InvalidInput` without sending it.

Transport errors, malformed frames and malformed typed replies return
`Error::Connection` and close the connection. **A submitted write may already have
committed when its reply is lost.** The client never reconnects or replays a request.
Check application state before deciding to retry. `Error::Closed` identifies later
use of a closed client. `close()` is idempotent; dropping the client closes the socket
and the server discards any uncommitted transaction.

Timeouts must be positive. The connect budget is shared across resolved addresses;
each request has a single deadline covering its writes, TLS handshake if needed,
and response reads, including partial frames. OS hostname resolution itself is
blocking and is not interrupted by this timeout.

From the repository root:

```sh
cargo test --locked --manifest-path clients/rust/Cargo.toml
cargo build --locked --manifest-path clients/rust/Cargo.toml --example smoke
cargo build --locked --features service --bin silverhamu-server
python tests/language_clients.py --clients rust
```

The interoperability check verifies binary values, transactions, token/TLS
authentication and rejected credentials, untrusted CAs and hostname mismatches.

## Releasing from GitHub Actions

Update the version in `clients/rust/Cargo.toml` and refresh `clients/rust/Cargo.lock`,
then merge the release changes into `main`. Run **Publish Rust client to crates.io**
(`publish-rust.yml`) from `main` with that version. Leave `publish` unchecked for a
validation-only run, or enable it to publish after all checks pass.

The Linux build job checks metadata, formatting, tests, Clippy, token/TLS
interoperability and `cargo publish --dry-run`. It uploads the tested `.crate` and
records its SHA-256 in the run summary. The separate publish job checks the artifact,
reproduces it from the same commit and checks its SHA-256 before requesting a
short-lived crates.io token. The downloaded registry version must match that checksum.
An already published, identical, non-yanked version succeeds without another upload;
a different package at the same version fails. Bump the version for any release change.

One-time configuration for this repository:

- GitHub environment: `crates-io`, with deployments restricted to the `main` branch.
- [crates.io Trusted Publishing](https://crates.io/crates/silverhamu-client/settings):
  repository owner `dhq-boiler`, repository `SilverHamu`, workflow `publish-rust.yml`,
  environment `crates-io`.

The workflow uses OIDC, so it does not need a long-lived `CARGO_REGISTRY_TOKEN` secret.
The **Build client release artifacts** workflow also includes the Rust `.crate` in
the shared client downloads and checksums. Rust releases run on Linux; the quarantined
Windows Rust tooling is not used.
