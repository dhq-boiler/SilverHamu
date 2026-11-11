# Java service client

Use Java 17 or later and add this dependency to your application's POM. Maven resolves the client and Gson from Maven Central.

```xml
<dependency>
  <groupId>io.github.dhq-boiler</groupId>
  <artifactId>silverhamu-client</artifactId>
  <version>0.1.0</version>
</dependency>
```

For a local token listener, read the same token file used by the service.

```java
import io.github.dhqboiler.silverhamu.SilverHamuClient;
import java.nio.file.Files;
import java.nio.file.Path;

String token = Files.readString(Path.of(".secrets/silverhamu-token")).replaceFirst("[\\r\\n]+$", "");
try (SilverHamuClient db = SilverHamuClient.connectWithToken("127.0.0.1", 7400, token)) {
    byte[] key = {0x01, (byte) 0xff};
    String version = db.putCommit(key, new byte[]{0x00, 0x2a});
    byte[] value = db.getLatest(key);
}
```

For a TLS account, supply the username, password, database name and an optional PEM CA file. A null CA file uses the JVM's trust store. The client verifies the certificate chain and hostname before sending credentials.

```java
try (SilverHamuClient db = SilverHamuClient.connectWithAccount("localhost", 7400,
        "alice", System.getenv("CATDB_PASSWORD"), "main", Path.of("ca.pem"))) {
    db.begin();
    db.put(new byte[]{1}, new byte[0]);
    db.delete(new byte[]{2});
    String version = db.commit();
}
```

`getLatest`, `putCommit` and `deleteCommit` perform one independent operation each. Inside `begin()` or `beginRead()`, use `get`, `put` and `delete` as permitted, then `commit()` or `rollback()`. Reads return `null` for a missing key and a zero-length array for a stored empty value. Versions are decimal strings and preserve the full unsigned 64-bit range.

Use `request(JsonObject)` for additional protocol operations, including paginated scans. It returns a Gson `JsonObject` with the complete successful response. For a stable scan, begin a read transaction, issue successive `scan` requests with the same range and each returned `next_after` as `after`, and roll back after `next_after` becomes null. An empty page can carry a continuation.

Each request and response is serialized on its connection. Give independent concurrent transactions separate connections. `SilverHamuClient.ServerException.code()` contains the server's error code. A transport or framing failure closes the connection. A submitted write may already be durable, so inspect the application state before retrying. [Service protocol](https://github.com/dhq-boiler/SilverHamu/blob/main/docs/service.md) · [TLS accounts](https://github.com/dhq-boiler/SilverHamu/blob/main/docs/accounts.md)
