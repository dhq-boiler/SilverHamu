import { Client, type ConnectOptions } from "./index.js";

export async function writeAndRead(options: ConnectOptions, key: Uint8Array, value: Uint8Array) {
  const client = await Client.connect(options);
  try {
    const version: string = await client.putCommit(key, value);
    const stored: Uint8Array | null = await client.getLatest(key);
    return { version, stored };
  } finally {
    client.close();
  }
}
