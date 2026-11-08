# catdb for Go

Install the Go 1.22+ client from your application's module directory. The client uses the standard library and its package name is `catdb`.

```sh
go get github.com/dhq-boiler/catdb/clients/go@v0.1.0
```

```go
package main

import (
    "log"
    "os"
    "time"

    "github.com/dhq-boiler/catdb/clients/go"
)

func main() {
    c, err := catdb.DialToken("127.0.0.1:7400", os.Getenv("CATDB_TOKEN"), 10*time.Second)
    if err != nil { log.Fatal(err) }
    defer c.Close()
    key := []byte{0, 0xff}
    if _, err := c.PutCommit(key, []byte("hello")); err != nil { log.Fatal(err) }
    value, err := c.GetLatest(key)
    if err != nil { log.Fatal(err) }
    log.Printf("%s", value)
}
```

`DialAccount(address, username, password, database, caPEM, timeout)` uses verified TLS 1.3. Pass `nil` for the system trust store or a PEM-encoded CA bundle. Use the token connection on loopback or through an encrypted tunnel.

`GetLatest`, `PutCommit` and `DeleteCommit` execute one operation. `Begin` or `BeginRead` starts a transaction on this connection. Use `Get`, `Put` and `Delete` within it and end it with `Commit` or `Rollback`. Reserve the connection for the whole transaction. Versions are `uint64`. A missing key returns a nil slice and an empty value returns a non-nil zero-length slice.

`Request(map[string]any{...})` sends a wire operation and returns a `map[string]json.RawMessage`. Use `errors.As` with `*catdb.ServerError` to inspect a server error code. A failed exchange closes the connection and a write with a lost reply may have committed. [Service protocol](https://github.com/dhq-boiler/catdb/blob/main/docs/service.md#wire-format)
