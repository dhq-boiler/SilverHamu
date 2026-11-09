# catdb Owner

Owner is a Windows x64 SQL editor and database manager. Open a local catdb file directly, connect to a service, or explore the in-memory demo. The SQL editor uses the shared `Catdb.Sql` implementation.

## Build and run

Install the .NET 10 SDK, the .NET MAUI Windows workload, and the Rust toolchain. From the repository root:

```powershell
./scripts/build-owner-native.ps1
dotnet build apps/Catdb.Owner/Catdb.Owner.csproj -c Release
```

Open `apps/Catdb.Owner.sln` in Visual Studio, select the Catdb.Owner project and run the Windows target. For local storage use **新規DB** to create a database or **DBファイルを開く** to open an existing file. The native library must match the source revision used to build the app.

## Service connections

Under **サービス接続**, select TLS account login for an account-enabled listener. Supply the certificate hostname, port, database, username, and password. Owner requires TLS 1.3 and verifies the certificate against the Windows trust store. Token login uses the legacy TCP v1 protocol; use a secure tunnel for remote token connections.

Servers that advertise SQL execute SQL on the server. A token connection to a legacy KV server uses the client-side SQL engine. An account connection that does not advertise SQL offers only the KV operations authorized on that database. View-only SQL tokens can list and query their granted views. [Service accounts](../../docs/accounts.md)

## Data operations

Use `BEGIN`, `COMMIT`, and `ROLLBACK` for multi-statement work. In local file mode, the backup action saves the committed state to a new `.catbak` archive. Restore validates the archive and opens a new database file. The value-range snapshot command exports just the values in a selected key range. The result grid can export a query result to CSV.

## Tests

```powershell
dotnet test apps/Catdb.Owner.Sql.Tests/Catdb.Owner.Sql.Tests.csproj
```

The tests create temporary databases and use synthetic data. To run the additional process-termination recovery check with PowerShell on .NET 10, build the test project, then run `tests/run-owner-index-recovery.ps1` with the same configuration.
