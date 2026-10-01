import {
  ALL_SIGNALS,
  CallTarget,
  ChangeOp,
  Kind,
  UndraCore,
  UndraSchemaMismatchError,
  UndraTransportError,
  decodeCall,
  decodeObserve,
  encodeChangeSet,
  type AttachOptions,
  type CallPayload,
  type ChangeEntry,
  type HelloPayload,
  type Transport,
  type TransportHandler,
} from "@undra/runtime";
import { CaptureLog } from "./fakes.js";
import type { ChangeOpName, RecordedEntry, RecordedEvent, RecordedTarget, Recording } from "./recording.js";
import { parseRecording } from "./recording.js";

const REPLY_STATUS: Readonly<Record<string, number>> = { ok: 0, error: 1, panic: 2, cancelled: 3, stream_opened: 4, bad_request: 5 };
const STREAM_FLAG: Readonly<Record<string, number>> = { item: 0, end: 1, error: 2, failed: 3 };
const CHANGE_OP: Readonly<Record<ChangeOpName, ChangeOp>> = { full: ChangeOp.FullValue, patch: ChangeOp.KeyedPatch, lazy_invalidated: ChangeOp.LazyInvalidated };

/** `call_id u32, tag u8, body`: the layout of a Reply (docs/SPEC.md 3.4) and of a StreamItem (3.7), whatever the tag. */
function tagged(callId: number, tag: number, body: Uint8Array): Uint8Array {
  const out = new Uint8Array(5 + body.length);
  new DataView(out.buffer).setUint32(0, callId, true);
  out[4] = tag;
  out.set(body, 5);
  return out;
}

function badRequest(callId: number, reason: string): Uint8Array {
  const text = new TextEncoder().encode(reason);
  const body = new Uint8Array(4 + text.length);
  new DataView(body.buffer).setUint32(0, text.length, true);
  body.set(text, 4);
  return tagged(callId, REPLY_STATUS["bad_request"]!, body);
}

const key = {
  fn: (method: number): string => `f:${method}`,
  method: (handle: bigint, method: number): string => `m:${handle}:${method}`,
  ctor: (type: number, method: number): string => `c:${type}:${method}`,
  page: (handle: bigint, offset: number, limit: number): string => `p:${handle}:${offset}:${limit}`,
};

function targetKey(t: RecordedTarget): string {
  switch (t.kind) {
    case "function":
      return key.fn(t.method);
    case "method":
      return key.method(t.handle, t.method);
    case "constructor":
      return key.ctor(t.type, t.method);
    case "page":
      return key.page(t.handle, t.offset, t.limit);
  }
}

function callKey(c: CallPayload): string {
  switch (c.target) {
    case CallTarget.FreeFunction:
      return key.fn(c.methodId);
    case CallTarget.ObjectMethod:
      return key.method(c.handle, c.methodId);
    case CallTarget.Constructor:
      return key.ctor(c.typeId, c.methodId);
    case CallTarget.LazyListPage:
      return key.page(c.handle, c.offset, c.limit);
  }
}

function describeCall(c: CallPayload): string {
  switch (c.target) {
    case CallTarget.FreeFunction:
      return `function ${c.methodId}`;
    case CallTarget.ObjectMethod:
      return `method ${c.methodId} of ${c.handle}`;
    case CallTarget.Constructor:
      return `constructor ${c.methodId} of type ${c.typeId}`;
    case CallTarget.LazyListPage:
      return `page ${c.offset}+${c.limit} of ${c.handle}`;
  }
}

/** What a call that no recording answers does. */
export type Exhausted = "fail" | "repeat_last";

/** How a {@link RecordedCore} matches the calls the app makes to the recorded ones. */
export interface ReplayOptions {
  /** `"ignore"` (default) matches a call on its target and method alone, in order; `"exact"` also compares the encoded arguments. */
  readonly args?: "exact" | "ignore";
  /** What a call does when the recording has no (more) reply for it: `"fail"` (default) answers a refused request naming the call, `"repeat_last"` answers again with the last recorded reply. */
  readonly exhausted?: Exhausted;
  /** Where the playhead starts, in milliseconds of the session. Default: the time of the first recorded change-set, so that observing a store shows the state the recording first saw. */
  readonly startAtMs?: number;
}

interface RecordedCall {
  readonly args: Uint8Array;
  readonly callId: number;
  readonly status: number;
  readonly body: Uint8Array;
  readonly items: readonly { readonly flag: number; readonly body: Uint8Array }[];
}

interface ReleaseEvent {
  readonly t: number;
  readonly txn: number;
  readonly entries: readonly RecordedEntry[];
}

const sameBytes = (a: Uint8Array, b: Uint8Array): boolean => a.length === b.length && a.every((x, i) => x === b[i]);

/**
 * A transport that plays a recording instead of reaching a core: replies come from the recorded replies (call ids rewritten), and the
 * recorded change-sets are released by a manual playhead. Under the unchanged `UndraCore`, mirror and generated stores. See {@link RecordedCore}.
 */
export class ReplayTransport implements Transport {
  readonly mode = "replay";
  readonly synchronous = false;

  readonly #hello: HelloPayload;
  readonly #options: ReplayOptions;
  readonly #calls = new Map<string, RecordedCall[]>();
  readonly #last = new Map<string, RecordedCall>();
  readonly #sets: ReleaseEvent[];
  readonly #observed = new Map<bigint, Set<number>>();
  #handler: TransportHandler | undefined;
  #cursor = 0;
  #playhead = 0;
  #closed = false;

  constructor(recording: Recording, options: ReplayOptions = {}) {
    this.#options = options;
    this.#hello = { undraVersion: "replay", schemaHash: recording.schemaHash, platform: "replay", mode: "replay" };
    const pending = new Map<number, { key: string; args: Uint8Array }>();
    const byCall = new Map<number, { call: RecordedCall; key: string; items: { flag: number; body: Uint8Array }[] }>();
    const sets: ReleaseEvent[] = [];
    for (const e of recording.events as readonly RecordedEvent[]) {
      if (e.kind === "call") pending.set(e.call, { key: targetKey(e.target), args: e.args });
      else if (e.kind === "reply") {
        const call = pending.get(e.call);
        if (call === undefined) continue;
        pending.delete(e.call);
        const items: { flag: number; body: Uint8Array }[] = [];
        const recorded: RecordedCall = { args: call.args, callId: e.call, status: REPLY_STATUS[e.status]!, body: e.body, items };
        byCall.set(e.call, { call: recorded, key: call.key, items });
        const list = this.#calls.get(call.key) ?? [];
        this.#calls.set(call.key, list);
        list.push(recorded);
      } else if (e.kind === "stream_item") byCall.get(e.call)?.items.push({ flag: STREAM_FLAG[e.flag]!, body: e.body });
      else if (e.kind === "change_set") sets.push({ t: e.t, txn: e.txn, entries: e.entries });
    }
    this.#sets = sets;
    this.#playhead = options.startAtMs ?? sets[0]?.t ?? 0;
    // What the recording had said by then is history: an observe is answered with it.
    while (this.#cursor < sets.length && sets[this.#cursor]!.t <= this.#playhead) this.#cursor++;
  }

  /** The playhead: milliseconds of the recording released so far. */
  get playhead(): number {
    return this.#playhead;
  }

  /** The time of the last recorded change-set. */
  get durationMs(): number {
    return this.#sets[this.#sets.length - 1]?.t ?? 0;
  }

  /** Change-sets the playhead has not reached yet. */
  get pendingChangeSets(): number {
    return this.#sets.length - this.#cursor;
  }

  async start(handler: TransportHandler): Promise<HelloPayload> {
    this.#handler = handler;
    return this.#hello;
  }

  #entry(e: RecordedEntry): ChangeEntry {
    return { handle: e.handle, signalId: e.signal, op: CHANGE_OP[e.op], value: e.value };
  }

  #wanted(handle: bigint, signal: number): boolean {
    const signals = this.#observed.get(handle);
    return signals !== undefined && (signals.has(ALL_SIGNALS) || signals.has(signal));
  }

  /** Releases every recorded change-set up to `t` (for the signals that are observed). */
  #releaseUntil(t: number): void {
    while (this.#cursor < this.#sets.length && this.#sets[this.#cursor]!.t <= t) {
      const set = this.#sets[this.#cursor++]!;
      const entries = set.entries.filter((e) => this.#wanted(e.handle, e.signal)).map((e) => this.#entry(e));
      if (entries.length > 0) this.#handler?.changeSet(encodeChangeSet({ txnId: BigInt(set.txn), entries }));
    }
  }

  /** Moves the playhead forward by `ms` and releases the change-sets it passes. */
  advance(ms: number): void {
    this.#playhead += Math.max(0, ms);
    this.#releaseUntil(this.#playhead);
  }

  /** Releases everything that is left. */
  playAll(): void {
    this.#playhead = Math.max(this.#playhead, this.durationMs);
    this.#releaseUntil(this.#playhead);
  }

  #observe(handle: bigint, signal: number, on: boolean): void {
    let signals = this.#observed.get(handle);
    if (!on) {
      if (signal === ALL_SIGNALS) this.#observed.delete(handle);
      else signals?.delete(signal);
      return;
    }
    if (signals === undefined) this.#observed.set(handle, (signals = new Set()));
    signals.add(signal);
    // The core answers an observe with what the signals hold: here, everything recorded up to the playhead, in order.
    const entries: ChangeEntry[] = [];
    for (let i = 0; i < this.#cursor; i++) {
      for (const e of this.#sets[i]!.entries) {
        if (e.handle === handle && (signal === ALL_SIGNALS || e.signal === signal)) entries.push(this.#entry(e));
      }
    }
    if (entries.length > 0) this.#handler?.changeSet(encodeChangeSet({ txnId: 0n, entries }));
  }

  #answer(call: CallPayload): void {
    const handler = this.#handler;
    if (handler === undefined) return;
    const k = callKey(call);
    const queue = this.#calls.get(k) ?? [];
    let found: RecordedCall | undefined;
    const at = this.#options.args === "exact" && call.target !== CallTarget.LazyListPage ? queue.findIndex((c) => sameBytes(c.args, call.args)) : queue.length > 0 ? 0 : -1;
    if (at >= 0) {
      found = queue.splice(at, 1)[0];
      if (found !== undefined) this.#last.set(k, found);
    } else if (this.#options.exhausted === "repeat_last") found = this.#last.get(k);
    if (found === undefined) {
      queueMicrotask(() =>
        handler.reply(
          badRequest(call.callId, `RecordedCore: the recording has no reply for ${describeCall(call)}${this.#options.args === "exact" ? " with these arguments" : ""}`),
        ),
      );
      return;
    }
    const recorded = found;
    queueMicrotask(() => {
      handler.reply(tagged(call.callId, recorded.status, recorded.body));
      for (const item of recorded.items) handler.streamItem(tagged(call.callId, item.flag, item.body));
    });
  }

  send(kind: Kind, payload: Uint8Array): void {
    if (this.#closed) throw new UndraTransportError("closed", "the recorded core is closed");
    switch (kind) {
      case Kind.Call:
        this.#answer(decodeCall(payload));
        break;
      case Kind.Observe: {
        const o = decodeObserve(payload);
        this.#observe(o.handle, o.signalId, o.on);
        break;
      }
      default:
        // Release, Cancel, StreamCredit, PortReply, Event, TimerFired, Restore: nothing to do for a recording.
        break;
    }
  }

  close(): void {
    this.#closed = true;
  }
}

/** Options of {@link RecordedCore.load}. */
export interface RecordedCoreOptions extends ReplayOptions, Pick<AttachOptions, "onError" | "shared" | "mirror" | "observeTimeoutMs"> {
  /** The schema hash of the bindings (`UndraIds.schemaHash`): a recording of another schema is refused with `UndraSchemaMismatchError`. */
  readonly expectedSchemaHash: bigint;
}

/**
 * A core that replays a recording: the generated stores run on the recorded replies and change-sets with no core behind them, for previews of
 * states that are expensive to reach. Interaction the recording does not hold fails with a typed refusal naming the call (`UndraCallError.refused`);
 * for an app that has to react, use {@link PreviewCore}, the real core with fakes.
 *
 * ```ts
 * const recorded = await RecordedCore.load(parseRecording(text), { expectedSchemaHash: UndraIds.schemaHash });
 * const todos = new Todos(recorded.core);     // the recorded constructor reply
 * await recorded.advance(300);                // the change-sets up to 300 ms into the session
 * ```
 */
export class RecordedCore {
  /** The core: the generated bindings take it as `ctx`, or it becomes `UndraCore.shared`. */
  readonly core: UndraCore;
  /** Where the recorded core's log records go. */
  readonly log: CaptureLog;
  readonly #transport: ReplayTransport;

  private constructor(core: UndraCore, transport: ReplayTransport, log: CaptureLog) {
    this.core = core;
    this.#transport = transport;
    this.log = log;
  }

  /**
   * Loads `recording` (a parsed one or its JSON text).
   *
   * @throws UndraSchemaMismatchError when the recording belongs to another schema.
   */
  static async load(recording: Recording | string, options: RecordedCoreOptions): Promise<RecordedCore> {
    const parsed = typeof recording === "string" ? parseRecording(recording) : recording;
    if (parsed.schemaHash !== options.expectedSchemaHash) throw new UndraSchemaMismatchError(options.expectedSchemaHash, parsed.schemaHash);
    const transport = new ReplayTransport(parsed, options);
    const log = new CaptureLog();
    const core = await UndraCore.attach(transport, {
      expectedSchemaHash: options.expectedSchemaHash,
      // Nothing touches the platform: every default adapter is removed.
      adapters: { http: null, kv: null, secureStore: null, fs: null, connectivity: null, lifecycle: null, log, clock: null, rng: null, timer: null },
      observeTimeoutMs: options.observeTimeoutMs ?? 1000,
      ...(options.onError !== undefined && { onError: options.onError }),
      ...(options.shared !== undefined && { shared: options.shared }),
      ...(options.mirror !== undefined && { mirror: options.mirror }),
    });
    return new RecordedCore(core, transport, log);
  }

  /** The playhead, in milliseconds. */
  get playhead(): number {
    return this.#transport.playhead;
  }

  /** The time of the last recorded change-set. */
  get durationMs(): number {
    return this.#transport.durationMs;
  }

  /** Moves the playhead forward by `ms`, releases the recorded change-sets it passes for the signals that are observed, and waits until the mirror has applied them. */
  async advance(ms: number): Promise<void> {
    this.#transport.advance(ms);
    await settleCore();
  }

  /** Plays the recording to its end. */
  async playAll(): Promise<void> {
    this.#transport.playAll();
    await settleCore();
  }

  /** Closes the core. */
  close(): void {
    this.core.close();
  }
}

/**
 * Lets the microtasks and the zero-delay tasks a core queued run, so what it produced is applied. In a visible page it also waits for a
 * frame, which is when the mirror applies what the core produced on its own (docs/SPEC.md section 11.1).
 */
export async function settleCore(): Promise<void> {
  for (let i = 0; i < 3; i++) await new Promise<void>((resolve) => setTimeout(resolve, 0));
  const doc = (globalThis as { document?: { visibilityState?: string } }).document;
  const raf = (globalThis as { requestAnimationFrame?: (cb: () => void) => number }).requestAnimationFrame;
  if (raf !== undefined && doc?.visibilityState === "visible") {
    await new Promise<void>((resolve) => raf(() => resolve()));
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
  }
}
