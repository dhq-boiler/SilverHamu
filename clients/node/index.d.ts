type Connection = { host?: string; port?: number; timeoutMs?: number };
export type ConnectOptions = Connection & (
  | { token: string; username?: never; password?: never; ca?: never; database?: never }
  | { username: string; password: string; database?: string; ca?: string | Uint8Array; token?: never }
);
export type Request = { op: string; [key: string]: unknown };
export type Reply = { ok: true; [key: string]: unknown };
export class CatdbError extends Error {
  constructor(code: string);
  readonly code: string;
}
export class Client {
  private constructor();
  static connect(options: ConnectOptions): Promise<Client>;
  request(request: Request): Promise<Reply>;
  ping(): Promise<string>;
  getLatest(key: Uint8Array): Promise<Uint8Array | null>;
  putCommit(key: Uint8Array, value: Uint8Array): Promise<string>;
  deleteCommit(key: Uint8Array): Promise<string>;
  begin(): Promise<string>;
  beginRead(): Promise<string>;
  get(key: Uint8Array): Promise<Uint8Array | null>;
  put(key: Uint8Array, value: Uint8Array): Promise<void>;
  delete(key: Uint8Array): Promise<void>;
  commit(): Promise<string>;
  rollback(): Promise<void>;
  close(): void;
}
