import { type KvAdapter, StorageError } from "@undra/runtime";

/** The kinds of operation of the `Kv` port. */
export type KvKind = "get" | "set" | "delete" | "list";

/** One call the core made to the `Kv` port. */
export interface KvOperation {
  readonly op: KvKind;
  /** The key (for `list`, the prefix). */
  readonly key: string;
  /** What was stored, for `set` (also for a `set` that failed). */
  readonly value?: Uint8Array;
  /** The `StorageError` the harness answered with; absent for a success. */
  readonly failure?: StorageError;
}

/** A failure the test asked for (see {@link MemoryKv.fail}). */
interface Rule {
  readonly kind: KvKind | null;
  readonly key: string | null;
  readonly error: StorageError;
  /** How many more operations it fails; `null` until the store heals. */
  remaining: number | null;
}

/** Operations held back until the test releases them (see {@link MemoryKv.hold}). */
interface Hold {
  readonly kind: KvKind;
  readonly key: string;
  readonly released: Promise<void>;
}

/**
 * The `Kv` port of the contract tests (scenarios.md, "Adapters"): a map in memory that remembers every call
 * made to it, in order, and fails on demand (ADR-049).
 *
 * A failure is a `StorageError`, which the runtime's `kvPort` answers as the port's typed error (reply status
 * 1), exactly as the platform's own adapters do. {@link MemoryKv.fail} makes the next `times` operations of one
 * kind (or of one key) fail, or every one of them until {@link MemoryKv.heal}.
 */
export class MemoryKv implements KvAdapter {
  /** Every call, oldest first, failed ones included. */
  readonly operations: KvOperation[] = [];
  readonly #entries: Map<string, Uint8Array>;
  readonly #rules: Rule[] = [];
  readonly #holds: Hold[] = [];

  /** A store holding `entries` (S14.8: build B starts from what build A persisted). */
  constructor(entries: ReadonlyMap<string, Uint8Array> = new Map()) {
    this.#entries = new Map([...entries].map(([key, value]) => [key, value.slice()]));
  }

  // ----- scripting ---------------------------------------------------------------------

  /**
   * Makes operations of `kind` (every kind if `null`) on `key` (every key if omitted) fail with `error`: the next
   * `times` of them, or every one until {@link MemoryKv.heal} if `times` is omitted.
   */
  fail(kind: KvKind | null, error: StorageError, options: { readonly key?: string; readonly times?: number } = {}): void {
    this.#rules.push({ kind, key: options.key ?? null, error, remaining: options.times ?? null });
  }

  /** Removes every failure the test asked for. */
  heal(): void {
    this.#rules.length = 0;
  }

  /**
   * Holds back every `kind` operation on `key` until the returned function is called (they are recorded when they
   * arrive and answered on release). A harness aid: it fixes the order of what the core does at load (S14.8).
   */
  hold(kind: KvKind, key: string): () => void {
    let release!: () => void;
    const released = new Promise<void>((resolve) => {
      release = resolve;
    });
    const hold: Hold = { kind, key, released };
    this.#holds.push(hold);
    return () => {
      const at = this.#holds.indexOf(hold);
      if (at >= 0) this.#holds.splice(at, 1);
      release();
    };
  }

  // ----- reading -----------------------------------------------------------------------

  /** The value stored under `key` right now, without counting as a call. */
  peek(key: string): Uint8Array | undefined {
    return this.#entries.get(key);
  }

  /** Every key and a copy of its value now. */
  entries(): Map<string, Uint8Array> {
    return new Map([...this.#entries].map(([key, value]) => [key, value.slice()]));
  }

  /** The keys stored right now that start with `prefix`. */
  keys(prefix = ""): string[] {
    return [...this.#entries.keys()].filter((key) => key.startsWith(prefix)).sort();
  }

  /** The `set` and `delete` calls for `key` that succeeded, oldest first. */
  writesTo(key: string): KvOperation[] {
    return this.operations.filter((operation) => operation.key === key && operation.failure === undefined && (operation.op === "set" || operation.op === "delete"));
  }

  // ----- the port ----------------------------------------------------------------------

  get(key: string): Promise<Uint8Array | null> {
    return this.#perform("get", key, undefined, () => this.#entries.get(key)?.slice() ?? null);
  }

  set(key: string, value: Uint8Array): Promise<void> {
    const copy = value.slice();
    return this.#perform("set", key, copy, () => {
      this.#entries.set(key, copy);
    });
  }

  delete(key: string): Promise<void> {
    return this.#perform("delete", key, undefined, () => {
      this.#entries.delete(key);
    });
  }

  list(prefix: string): Promise<string[]> {
    return this.#perform("list", prefix, undefined, () => this.keys(prefix));
  }

  /** Records one operation and fails it with the first rule that applies, or performs it with `body`. */
  async #perform<T>(kind: KvKind, key: string, value: Uint8Array | undefined, body: () => T): Promise<T> {
    const hold = this.#holds.find((h) => h.kind === kind && h.key === key);
    if (hold !== undefined) await hold.released;
    const at = this.#rules.findIndex((rule) => (rule.kind === null || rule.kind === kind) && (rule.key === null || rule.key === key));
    const rule = this.#rules[at];
    if (rule !== undefined) {
      if (rule.remaining !== null) {
        rule.remaining -= 1;
        if (rule.remaining <= 0) this.#rules.splice(at, 1);
      }
      this.operations.push({ op: kind, key, ...(value !== undefined && { value }), failure: rule.error });
      throw rule.error;
    }
    this.operations.push({ op: kind, key, ...(value !== undefined && { value }) });
    return body();
  }
}

/** The keys and layouts of what the query client persists (ADR-037), as the scenarios read them. */
export const Persisted = {
  /** The offline queue, format 2: `format u16 = 2, schema_hash u64, count u32, count x item`. */
  queueKey: "undra.query.queue2",
  /** The dead-letter queue. */
  deadLetterKey: "undra.query.queue.dead",
  /** The prefix of the cache entries: `undra.query.cache2.<query id>.<fnv1a64 of the encoded arguments>`. */
  cachePrefix: "undra.query.cache2.",

  /** The number of items of a format-2 queue (the `u32` at offset 10), or `undefined` when the value is not one. */
  queueCount(value: Uint8Array | undefined): number | undefined {
    if (value === undefined || value.length < 14 || value[0] !== 2 || value[1] !== 0) return undefined;
    return new DataView(value.buffer, value.byteOffset, value.byteLength).getUint32(10, true);
  },

  /** The `u64` at `offset` (a fingerprint), or `undefined` when the value is too short. */
  fingerprint(value: Uint8Array | undefined, offset: number): bigint | undefined {
    if (value === undefined || value.length < offset + 8) return undefined;
    return new DataView(value.buffer, value.byteOffset, value.byteLength).getBigUint64(offset, true);
  },

  /** `undra.types.<fingerprint as 16 hex digits>`: where a closure description is stored. */
  typesKey(fingerprint: bigint): string {
    return `undra.types.${fingerprint.toString(16).padStart(16, "0")}`;
  },

  /** The key of a cache entry of query `queryId` with encoded arguments `args`. */
  cacheKey(queryId: number, args: Uint8Array): string {
    return `undra.query.cache2.${(queryId >>> 0).toString(16).padStart(8, "0")}.${fnv1a64(args).toString(16).padStart(16, "0")}`;
  },
} as const;

/** FNV-1a, 64 bits, over bytes. */
export function fnv1a64(bytes: Uint8Array): bigint {
  let hash = 0xcbf2_9ce4_8422_2325n;
  for (const byte of bytes) {
    hash ^= BigInt(byte);
    hash = BigInt.asUintN(64, hash * 0x0000_0100_0000_01b3n);
  }
  return hash;
}

export { StorageError };
