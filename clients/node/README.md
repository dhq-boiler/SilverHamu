# silverhamu for Node.js

Install the ES module in a JavaScript or TypeScript application running Node.js 18 or later. TypeScript with `module: "NodeNext"` reads the included type declarations.

```sh
npm install silverhamu-client@0.2.0
```

Start a silverhamu service and set `CATDB_TOKEN` to its shared token before running this example. [Server quick start](https://github.com/dhq-boiler/SilverHamu#quick-start)

```js
import { Client } from "silverhamu-client";

const client = await Client.connect({ token: process.env.CATDB_TOKEN });
try {
  const key = Uint8Array.of(0xff, 0);
  const version = await client.putCommit(key, Uint8Array.of(1, 2));
  const value = await client.getLatest(key);
  console.log(version, value);
} finally {
  client.close();
}
```

Use the shared token on a trusted local connection. For a TLS account supply `{ host, username, password, database: "main" }` to `Client.connect`. To trust a private CA, pass its PEM text or bytes as `ca`. The client verifies the hostname and the certificate chain and requires TLS 1.3 for accounts. The default address is `127.0.0.1:7400`; `timeoutMs` defaults to 10,000.

`getLatest` returns bytes or `null` for a missing key. An empty value returns zero bytes. `putCommit` and `deleteCommit` wait for a durable commit and return its version as a decimal string. Use `begin` or `beginRead`, transactional `get`, `put`, `delete`, then `commit` or `rollback` on one client. One connection runs requests in order and owns one transaction. Give independent transactions separate clients.

`request({ op: "scan_latest", start: "", rows: 20 })` supports the remaining wire operations, with hex keys and values. A server rejection throws `SilverHamuError` with a `code` property. If the connection fails during a mutation, the result may be unknown. The client discards the connection and leaves the decision to retry to the application.
