# Service accounts

One TLS listener hosts the primary database (`main`) and optionally additional named databases. An account has one stable ID across those databases. Its grant on each database determines which operations it may use. The embedded Rust and C APIs continue to trust the host process.

## Start the first administrator

Build with `cargo build --release --locked --features service --bin catdb-server`. Prepare a PEM certificate chain and its matching unencrypted PEM private key for the service hostname. The Python client verifies the certificate chain and hostname. For a local trial, generate a temporary certificate:

```sh
umask 077
mkdir -p data .secrets
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj /CN=localhost \
  -addext subjectAltName=DNS:localhost -keyout .secrets/key.pem -out .secrets/cert.pem
python -c "from getpass import getpass; from pathlib import Path; p=getpass('Initial administrator password: '); f=Path('.secrets/bootstrap').open('x', encoding='utf-8'); f.write(p); f.close()"
./target/release/catdb-server --db data/main.ndb --init \
  --tls-cert .secrets/cert.pem --tls-key .secrets/key.pem \
  --bootstrap-admin owner --bootstrap-password-file .secrets/bootstrap
```

Use at least 15 Unicode characters and at most 1,024 UTF-8 bytes. On Windows use private filesystem ACLs and `catdb-server.exe`. Keep the initial password in a password manager. Once the server reports ready, remove the bootstrap password file and **omit both bootstrap flags on subsequent starts**. Supplying them again fails and cannot replace the original administrator. Remote listening also requires `--allow-remote` and a suitable `--bind`.

## Create users and assign grants

From a second terminal, create a reader on `main`. The client prompts for both passwords without echoing them.

```sh
python scripts/catdb-client.py --host localhost --ca-file .secrets/cert.pem \
  --user owner --create-user alice --role reader
python scripts/catdb-client.py --host localhost --ca-file .secrets/cert.pem --user alice
```

Enter one JSON request per line, for example `{"op":"ping"}`. The client uses system trust when you omit `--ca-file`. For unattended clients, `--password-file FILE` reads a private UTF-8 file. Passwords never need to appear in command-line arguments.

| Grant | Operations on the selected database |
|---|---|
| `reader` | Ping, stable read transactions, point and range reads, commit or rollback of a read transaction |
| `writer` | Reader operations plus staged and single-request mutations and supported batch mutations |
| `admin` | Writer operations plus checkpoint, user creation and grant management |

The local bootstrap administrator administers all databases explicitly hosted by this listener. A newly created `admin` administers only databases on which it has that grant.

To host another **existing** transaction database, add `--attach archive=/private/path/archive.ndb` at startup. Select it with `--database archive` at login. Each attached database must have a distinct database ID. A connection stays bound to the selected database. To give the existing Alice account write access on `archive`, log in as its administrator with `--database archive` and send:

```json
{"op":"set_role","username":"alice","role":"writer"}
```

Set `role` to `none` to remove access on that database. Any grant replacement invalidates affected sessions on their next request, including a pending commit. Requests already admitted may finish. Other database grants are independent. Administrators cannot change their own grant or the bootstrap administrator's grant. A new account and its first grant commit together. Concurrent conflicting changes can return `conflict`.

## Authentication and storage

After a TLS 1.3 handshake, send a length-prefixed JSON frame using the existing service framing. The first message is `{"op":"auth","version":2,"username":"alice","password":"…","database":"main"}`. Success includes `protocol: 2` and a stable hex `account_id`; unsuccessful authentication returns `unauthorized`. Database operations then use the existing request format. Account mode rejects legacy token login, plaintext connections and diagnostic-only operations.

The server stores an Argon2id PHC hash with an independently generated 128-bit salt for each password (19 MiB, two iterations, one lane). At most two password hashing or verification operations run concurrently per process. Eight failed authentications from one peer IP throttle further logins for the remainder of a 60-second window; the peer table is bounded. Successful sessions reuse their authorization until a grant changes. The existing connection and deadline limits also apply during login.

The primary data file has a sibling `<db>.accounts` transaction store. Separate key namespaces hold username-to-ID mappings, password credentials keyed by immutable account ID, and grants keyed by database ID plus account ID. A future WebAuthn credential can reference the same account ID; passkey registration and login are not implemented in this release. Each enrolled data file also has a `<db>.account-owner` marker. Use private directories, stop the service before file-level backups, and back up and restore the data, catalog and markers together. Protect private keys separately. Administrators with direct OS file access remain trusted; TLS protects the connection and does not encrypt the database at rest.

The original token listener remains available for unconfigured databases. Enrolled databases reject accidental token-mode startup. The supplied Compose configuration and its healthcheck exercise token mode. For a TLS container mount its certificate, private key and writable data volume, pass the account-mode arguments above, replace the token healthcheck with a certificate-verifying account client issuing `ping`, and ensure UID/GID 10001 can read the mounted secrets.
