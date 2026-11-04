# C# source bindings

`NewDbmsDatabase.cs` wraps the transactional C ABI and `NewDbmsSnapshot.cs` wraps immutable snapshots. Include the source files in a modern .NET application with implicit usings enabled, and build the native library with `cargo build --locked --release --lib`.

Deploy the matching native library beside your executable or configure the application's native library resolver. The native library is named `newdbms.dll` on Windows, `libnewdbms.so` on Linux, and `libnewdbms.dylib` on macOS. These bindings and the `NewDbms` namespace retain their legacy names for compatibility.

Dispose transactions and database handles deterministically. Do not let an embedded handle and a service process own the same database concurrently. These are local native bindings, not a TCP service client. Application-specific entities, archive import, and UI integration are excluded from this repository.
