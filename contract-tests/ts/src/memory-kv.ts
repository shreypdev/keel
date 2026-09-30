import type { KvAdapter } from "@keel/runtime";

/** One call the core made to the `Kv` port. */
export interface KvOperation {
  readonly op: "get" | "set" | "delete" | "list";
  /** The key (for `list`, the prefix). */
  readonly key: string;
  /** What was stored, for `set`. */
  readonly value?: Uint8Array;
}

/** The `Kv` port of the contract tests: a map in memory that remembers every call made to it. */
export class MemoryKv implements KvAdapter {
  /** Every call, oldest first. */
  readonly operations: KvOperation[] = [];
  readonly #entries = new Map<string, Uint8Array>();

  get(key: string): Promise<Uint8Array | null> {
    this.operations.push({ op: "get", key });
    return Promise.resolve(this.#entries.get(key)?.slice() ?? null);
  }

  set(key: string, value: Uint8Array): Promise<void> {
    this.operations.push({ op: "set", key, value: value.slice() });
    this.#entries.set(key, value.slice());
    return Promise.resolve();
  }

  delete(key: string): Promise<void> {
    this.operations.push({ op: "delete", key });
    this.#entries.delete(key);
    return Promise.resolve();
  }

  list(prefix: string): Promise<string[]> {
    this.operations.push({ op: "list", key: prefix });
    return Promise.resolve([...this.#entries.keys()].filter((key) => key.startsWith(prefix)).sort());
  }

  /** The value stored under `key` right now, without counting as a call. */
  peek(key: string): Uint8Array | undefined {
    return this.#entries.get(key);
  }

  /** The `set` and `delete` calls for `key`, oldest first. */
  writesTo(key: string): KvOperation[] {
    return this.operations.filter((operation) => operation.key === key && (operation.op === "set" || operation.op === "delete"));
  }
}
