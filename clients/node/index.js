import { once } from "node:events";
import net from "node:net";
import tls from "node:tls";

const MAX_REQUEST = 1024 * 1024;
const MAX_RESPONSE = 2 * 1024 * 1024 + 8192;
const decoder = new TextDecoder("utf-8", { fatal: true });

export class CatdbError extends Error {
  constructor(code) {
    super(`catdb returned ${code}`);
    this.name = "CatdbError";
    this.code = code;
  }
}

function hex(bytes) {
  if (!(bytes instanceof Uint8Array)) throw new TypeError("expected a Uint8Array");
  return Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength).toString("hex");
}

export class Client {
  #socket;
  #reader;
  #rest = Buffer.alloc(0);
  #tail = Promise.resolve();
  #timeout;

  constructor(socket, timeout) {
    this.#socket = socket;
    this.#reader = socket[Symbol.asyncIterator]();
    this.#timeout = timeout;
  }

  static async connect(options) {
    const { host = "127.0.0.1", port = 7400, timeoutMs = 10000, token, username,
      password, database = "main", ca } = options;
    if (!Number.isInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 2147483647) {
      throw new RangeError("timeoutMs must be a positive 32-bit integer");
    }
    const account = username !== undefined;
    if (account ? (typeof username !== "string" || typeof password !== "string" || token !== undefined)
                : (typeof token !== "string" || password !== undefined || ca !== undefined || database !== "main")) {
      throw new TypeError("choose a token or a TLS username and password");
    }
    const socket = account
      ? tls.connect({ host, port, ca: ca instanceof Uint8Array ? Buffer.from(ca) : ca,
          minVersion: "TLSv1.3", rejectUnauthorized: true,
          servername: net.isIP(host) ? undefined : host,
          checkServerIdentity: (_, cert) => tls.checkServerIdentity(host, cert) })
      : net.createConnection({ host, port });
    // A peer can reset an idle socket between two requests.
    socket.on("error", () => socket.destroy());
    socket.setNoDelay(true);
    const deadline = setTimeout(() => socket.destroy(new Error("catdb connection timed out")), timeoutMs);
    try {
      await once(socket, account ? "secureConnect" : "connect");
      clearTimeout(deadline);
      const client = new Client(socket, timeoutMs);
      const reply = await client.request(account
        ? { op: "auth", version: 2, username, password, database }
        : { op: "auth", version: 1, token });
      if (reply.protocol !== (account ? 2 : 1)) throw new Error("unexpected catdb protocol version");
      return client;
    } catch (error) {
      socket.destroy();
      throw error;
    } finally {
      clearTimeout(deadline);
    }
  }

  async #read(size) {
    const output = Buffer.allocUnsafe(size);
    let offset = 0;
    while (offset < size) {
      if (!this.#rest.length) {
        const next = await this.#reader.next();
        if (next.done) throw new Error("catdb connection closed");
        this.#rest = next.value;
      }
      const count = Math.min(size - offset, this.#rest.length);
      this.#rest.copy(output, offset, 0, count);
      this.#rest = this.#rest.subarray(count);
      offset += count;
    }
    return output;
  }

  async #exchange(frame) {
    if (this.#socket.destroyed) throw new Error("catdb connection is closed");
    const deadline = setTimeout(() => this.#socket.destroy(new Error("catdb request timed out")), this.#timeout);
    let reply;
    try {
      this.#socket.write(frame);
      const size = (await this.#read(4)).readUInt32BE();
      if (size === 0 || size > MAX_RESPONSE) throw new Error("invalid catdb response size");
      reply = JSON.parse(decoder.decode(await this.#read(size)));
      if (typeof reply?.ok !== "boolean" || (!reply.ok && typeof reply.error !== "string")) {
        throw new Error("invalid catdb response");
      }
    } catch (cause) {
      this.close();
      throw new Error("catdb response lost or invalid; a submitted mutation may have committed", { cause });
    } finally {
      clearTimeout(deadline);
    }
    if (!reply.ok) throw new CatdbError(reply.error);
    return reply;
  }

  async request(request) {
    const body = Buffer.from(JSON.stringify(request), "utf8");
    if (body.length > MAX_REQUEST) throw new RangeError("catdb request exceeds 1 MiB");
    const frame = Buffer.allocUnsafe(4 + body.length);
    frame.writeUInt32BE(body.length);
    body.copy(frame, 4);
    const response = this.#tail.then(() => this.#exchange(frame));
    this.#tail = response.catch(() => {});
    return response;
  }

  async #value(op, key) {
    const { value } = await this.request({ op, key: hex(key) });
    if (value === null) return null;
    if (typeof value !== "string" || value.length % 2 || /[^0-9a-f]/i.test(value)) {
      this.close();
      throw new Error("invalid catdb value");
    }
    return Buffer.from(value, "hex");
  }

  async #version(request) {
    const { version } = await this.request(request);
    if (typeof version !== "string" || !/^(0|[1-9][0-9]*)$/.test(version)) {
      this.close();
      throw new Error("invalid catdb version");
    }
    return version;
  }

  ping() { return this.#version({ op: "ping" }); }
  getLatest(key) { return this.#value("get_latest", key); }
  putCommit(key, value) { return this.#version({ op: "put_commit", key: hex(key), value: hex(value) }); }
  deleteCommit(key) { return this.#version({ op: "delete_commit", key: hex(key) }); }
  begin() { return this.#version({ op: "begin" }); }
  beginRead() { return this.#version({ op: "begin_read" }); }
  get(key) { return this.#value("get", key); }
  async put(key, value) { await this.request({ op: "put", key: hex(key), value: hex(value) }); }
  async delete(key) { await this.request({ op: "delete", key: hex(key) }); }
  commit() { return this.#version({ op: "commit" }); }
  async rollback() { await this.request({ op: "rollback" }); }
  close() { this.#socket.destroy(); }
}
