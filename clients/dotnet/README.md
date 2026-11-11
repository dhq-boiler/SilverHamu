# silverhamu for .NET

SilverHamu.Client connects a .NET 10 application to the silverhamu TCP service. Install the package from your application directory.

```sh
dotnet package add SilverHamu.Client --version 0.1.0
```

```csharp
using SilverHamu;

using var client = SilverHamuClient.ConnectToken("127.0.0.1", 7400, Environment.GetEnvironmentVariable("CATDB_TOKEN")!);
byte[] key = [0, 255];
ulong version = client.PutCommit(key, "hello"u8);
byte[]? value = client.GetLatest(key);
```

`ConnectAccount(host, port, username, password, database: "main", caFile: null)` uses TLS 1.3 and verifies the host certificate. Use the system trust store or pass a PEM CA file. The plaintext token connection is intended for loopback or an encrypted tunnel.

`GetLatest`, `PutCommit` and `DeleteCommit` execute a single operation. Start a transaction with `Begin` or `BeginRead`, use `Get`, `Put` and `Delete`, then call `Commit` or `Rollback`. A transaction belongs to its connection. Use separate clients for concurrent transactions. Missing keys return `null` and stored empty values return zero-length arrays.

`Request(new { op = "ping" })` returns a `JsonElement` for any wire operation. Server errors expose `SilverHamuServerException.Code`. An interrupted exchange closes the connection and a submitted write may have committed. [Wire protocol](https://github.com/dhq-boiler/SilverHamu/blob/main/docs/service.md#wire-format)
