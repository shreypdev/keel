import { UndraCore } from "../../src/core.js";
import { LazyList } from "../../src/lazy.js";
import type { UndraUnhandledError } from "../../src/call-error.js";
import {
  CallTarget,
  type Codec,
  UndraReader,
  codecs,
  encodeLazyInvalidated,
  encodeLazyPage,
  encodeLazyValue,
} from "../../src/wire/index.js";
import { FakeCoreTransport, SCHEMA, type Responder } from "./fake-core.js";
import { track } from "./harness.js";

/*
 * A page server for the lazy-list tests: what the core does for a `Lazy<T>` signal (ADR-043), written for
 * `FakeCoreTransport`. It holds the rows and a version, answers page calls (target 3, which the fake routes to
 * method id -1), records them, and builds the three payloads the core sends.
 */

/** One page call the host made. */
export interface PageCall {
  readonly handle: bigint;
  readonly offset: number;
  readonly limit: number;
  readonly callId: number;
}

export class LazyServer<T> {
  /** The page calls received, in order. */
  readonly calls: PageCall[] = [];
  /** The answers to hold back: with `hold`, a call is recorded and not answered until `release`. */
  readonly held: Array<{ call: PageCall; respond: Responder }> = [];
  hold = false;
  /** When set, the next page reply is this body instead of the real page (hostile replies). */
  override: ((call: PageCall) => Uint8Array | undefined) | null = null;
  /** Called with each page call before it is answered. */
  onCall: ((call: PageCall) => void) | null = null;
  version: bigint;
  rows: T[];

  constructor(
    readonly codec: Codec<T>,
    rows: T[],
    readonly handle: bigint = 0x0001_0000_0001n,
    version = 1n,
  ) {
    this.rows = rows;
    this.version = version;
  }

  install(fake: FakeCoreTransport): this {
    fake.on(-1, (call, respond) => {
      if (call.target !== CallTarget.LazyListPage) return respond.badRequest("not a page call");
      const page: PageCall = { handle: call.handle, offset: call.offset, limit: call.limit, callId: call.callId };
      this.calls.push(page);
      this.onCall?.(page);
      if (call.handle !== this.handle) return respond.badRequest(`stale handle ${call.handle}`);
      if (this.hold) {
        this.held.push({ call: page, respond });
        respond.defer();
        return;
      }
      respond.ok(this.override?.(page) ?? this.page(page.offset, page.limit));
    });
    return this;
  }

  /** The reply to a page call now. */
  page(offset: number, limit: number): Uint8Array {
    const items = this.rows.slice(offset, offset + limit);
    return encodeLazyPage(this.codec, { version: this.version, total: this.rows.length, items });
  }

  /** The reply to a page call as of `version` and `rows`, whatever the server holds now. */
  pageAt(version: bigint, rows: readonly T[], offset: number, limit: number): Uint8Array {
    return encodeLazyPage(this.codec, { version, total: rows.length, items: rows.slice(offset, offset + limit) });
  }

  /** The value of the signal (`FullValue`). */
  value(): Uint8Array {
    return encodeLazyValue({ handle: this.handle, len: this.rows.length, version: this.version });
  }

  /** The value of op 2. */
  invalidated(): Uint8Array {
    return encodeLazyInvalidated({ len: this.rows.length, version: this.version });
  }

  /** Changes the rows and bumps the version, as a commit does. */
  change(edit: (rows: T[]) => void): this {
    edit(this.rows);
    this.version++;
    return this;
  }

  /** Answers the held call `index` with the page as the server has it now (or with `body`). */
  release(index = 0, body?: Uint8Array): void {
    const entry = this.held[index];
    if (entry === undefined) throw new Error(`no held call ${index}`);
    entry.respond.ok(body ?? this.page(entry.call.offset, entry.call.limit));
  }

  /** Releases every held call, oldest first. */
  releaseAll(): void {
    for (const entry of this.held.splice(0)) entry.respond.ok(this.page(entry.call.offset, entry.call.limit));
  }
}

/** A core on a fake transport (synchronous: the in-process kind, with `callSync`; or asynchronous: `remote`), with the errors it reports. */
export async function lazyCore(options: { synchronous: boolean }): Promise<{
  fake: FakeCoreTransport;
  core: UndraCore;
  errors: UndraUnhandledError[];
}> {
  const fake = new FakeCoreTransport({ synchronous: options.synchronous });
  current = fake;
  const errors: UndraUnhandledError[] = [];
  const core = track(
    await UndraCore.attach(fake, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: { http: null, timer: null, log: { log() {} } },
      onError: (error) => errors.push(error),
    }),
  );
  return { fake, core, errors };
}

/** A list of numbers over a fresh core, as a generated store builds it, with the server installed and the value applied. */
export async function numbersList(
  rows: number,
  options: { synchronous: boolean; handle?: bigint },
): Promise<{
  fake: FakeCoreTransport;
  core: UndraCore;
  errors: UndraUnhandledError[];
  server: LazyServer<number>;
  list: LazyList<number>;
}> {
  const base = await lazyCore(options);
  const server = new LazyServer(codecs.i32, Array.from({ length: rows }, (_, i) => i * 10), options.handle).install(base.fake);
  const list = new LazyList<number>(base.core, codecs.i32);
  list.applyFull(new UndraReader(server.value()));
  return { ...base, server, list };
}

/** Lets the microtask queue (a list's flush and an asynchronous reply's continuation) run. */
export async function settle(rounds = 3): Promise<void> {
  for (let i = 0; i < rounds; i++) await Promise.resolve();
}

/** The fake of the core `lazyCore` made last: `drain` waits for its deliveries. */
let current: FakeCoreTransport | null = null;

/** Waits until the fake has delivered everything queued and the microtasks they start have run (replies that ask for more included). */
export async function drain(): Promise<void> {
  for (let round = 0; round < 4; round++) {
    await settle();
    await current?.settle();
  }
  await settle();
}
