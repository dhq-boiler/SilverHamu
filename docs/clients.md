# Client libraries

Start a catdb service with the [quick start](../README.md#quick-start), then choose a client from this checkout. Each guide includes installation, a first write and read, and TLS account configuration.

| Language | Runtime | Install or reference |
|---|---|---|
| [Python](../clients/python/README.md) | Python 3.10+ | Install `catdb-service-client` and import `catdb_client`. |
| [JavaScript and TypeScript](../clients/node/README.md) | Node.js 18+ | Run `npm install catdb-client@0.1.0`. The package includes TypeScript declarations. |
| [Go](../clients/go/README.md) | Go 1.22+ | Run `go get github.com/dhq-boiler/catdb/clients/go@v0.1.0`. |
| [C#](../clients/dotnet/README.md) | .NET 10 | Use `Catdb.Client` or add a project reference to `clients/dotnet/Catdb.Client/Catdb.Client.csproj`. |
| [Java](../clients/java/README.md) | Java 17+ | Add the Maven dependency `io.github.dhq-boiler:catdb-client:0.1.0`. |
| [Ruby](../clients/ruby/README.md) | Ruby 3.1+ | Run `gem install catdb-client -v 0.1.0` and load it with `require "catdb"`. |

Each client provides point reads, durable single-key writes and deletes, read-only and read/write transactions, commit and rollback. Keys and values are bytes. An absent key and an empty value are distinct. A raw request method exposes other server operations, including paginated range scans. Keep a multi-page snapshot on the same connection and use separate connections for independent concurrent transactions. [Wire operations, limits and error codes](service.md#wire-format)

Use a shared token on loopback or through an encrypted tunnel. For named accounts, use TLS 1.3 with hostname verification and either a trusted system certificate or the service's CA. A server rejection carries a machine-readable error code. If a connection breaks after submitting a write, reconnect and check the application state before deciding whether to retry. [Service accounts and database grants](accounts.md)

For in-process use, the Rust crate, C ABI and existing C# native bindings open the database directly. [Rust example](../examples/basic.rs) · [C# native bindings](../dotnet/README.md)

## Running the interoperability checks

The [language clients workflow](../.github/workflows/language-clients.yml) builds the server and smoke programs from the checkout. After those build steps, run `python tests/language_clients.py`. It starts two disposable servers, passes a binary value from one language to the next, and checks each client against rejected credentials, an untrusted certificate and a hostname mismatch. Use `--clients` with one or more of `python node go dotnet java ruby` to run a subset.
