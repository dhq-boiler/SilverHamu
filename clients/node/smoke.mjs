import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { Client, CatdbError } from "./index.js";

const env = process.env;
const auth = env.CATDB_USER ? { username: env.CATDB_USER, password: env.CATDB_PASSWORD,
  ca: env.CATDB_CA_FILE ? readFileSync(env.CATDB_CA_FILE) : undefined } : { token: env.CATDB_TOKEN };
const client = await Client.connect({ host: env.CATDB_HOST, port: Number(env.CATDB_PORT), ...auth });
try {
  assert.match(await client.ping(), /^\d+$/);
  if (env.CATDB_AUTH_ONLY !== "1") {
    const key = Buffer.from("ff00", "hex");
    assert.deepEqual(await client.getLatest(key), Buffer.from(env.CATDB_EXPECT, "hex"));
    const temporary = Buffer.from("00ff6e6f6465", "hex");
    await client.deleteCommit(temporary);
    assert.equal(await client.getLatest(temporary), null);
    await client.begin();
    await client.put(temporary, Buffer.alloc(0));
    assert.deepEqual(await client.get(temporary), Buffer.alloc(0));
    await client.rollback();
    assert.equal(await client.getLatest(temporary), null);
    await client.begin();
    await client.put(temporary, Buffer.alloc(0));
    await client.commit();
    await client.beginRead();
    assert.deepEqual(await client.get(temporary), Buffer.alloc(0));
    await client.rollback();
    await assert.rejects(client.commit(), err => err instanceof CatdbError && err.code === "invalid_request");
    await client.deleteCommit(temporary);
    await client.putCommit(key, Buffer.from(env.CATDB_VALUE, "hex"));
  }
} finally {
  client.close();
}
