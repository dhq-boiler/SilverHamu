# C# source bindings

`NewDbmsDatabase.cs` wraps the transactional C ABI and `NewDbmsSnapshot.cs` wraps immutable snapshots. Include the source files in a modern .NET application with implicit usings enabled, and build the native library with `cargo build --locked --release --lib`.

Deploy the matching native library beside your executable or configure the application's native library resolver. The native library is named `silverhamu.dll` on Windows, `libsilverhamu.so` on Linux, and `libsilverhamu.dylib` on macOS. The C# bindings and the `NewDbms` namespace retain their legacy names for compatibility. Update native library deployments from the former `newdbms` filenames to the names above; C ABI symbols and calling conventions are unchanged.

Dispose transactions and database handles deterministically. A database file has one owning process at a time.

## Embedded SQL and CLI

`SilverHamu.Sql` adds SQL tables, indexes, constraints and views on top of the transactional engine. It targets .NET 10 and requires the matching native library. Existing databases retain their storage policy, and new databases use the policy of the underlying Rust build.

Build from the repository root. If Cargo output is in a custom target directory, pass `-p:SilverHamuNativePath=<absolute library path>` to the .NET build.

```sh
cargo build --locked --release --lib
dotnet build dotnet/SilverHamu.Sql.Host -c Release
dotnet dotnet/SilverHamu.Sql.Host/bin/Release/net10.0/SilverHamu.Sql.Host.dll --create example-sql.ndb "CREATE TABLE items (id INT PRIMARY KEY, label TEXT); INSERT INTO items VALUES (1,'book');"
dotnet dotnet/SilverHamu.Sql.Host/bin/Release/net10.0/SilverHamu.Sql.Host.dll example-sql.ndb "SELECT label FROM items WHERE id=1;"
```

The final query returns `book`. To embed SQL, add a project reference to [SilverHamu.Sql](SilverHamu.Sql/SilverHamu.Sql.csproj) and open a dedicated connection:

```csharp
using SilverHamu.Sql;

using var db = await SilverHamuSqlConnection.OpenAsync("example-sql.ndb");
var results = await db.ExecuteAsync("SELECT label FROM items ORDER BY id;");
```

SQL metadata shares the database's keyspace. Raw key-value writers must leave the `_catdb_owner/` prefix to the SQL layer. Use a client version supporting the schema and indexes of any database it updates. Backup and restore of an active embedded database use optional native exports available in the matching native build. The offline `.catbak` archive remains available through the [native CLI](../docs/backup.md).
