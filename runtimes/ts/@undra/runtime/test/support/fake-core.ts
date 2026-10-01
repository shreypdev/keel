import type { Transport, TransportHandler } from "../../src/transport/transport.js";
import { UndraTransportError } from "../../src/errors.js";
import {
  type CallPayload,
  type ChangeEntry,
  ChangeOp,
  type HelloPayload,
  Kind,
  type PortReplyPayload,
  PortStatus,
  ReplyStatus,
  StreamFlag,
  ALL_SIGNALS,
  decodeCall,
  decodeCancel,
  decodeEvent,
  decodeObserve,
  decodePortReply,
  decodeRelease,
  decodeStreamCredit,
  encodeChangeSet,
  encodeReply,
  encodeStreamItem,
} from "../../src/wire/index.js";

/*
 * An in-process stand-in for an Undra core that speaks the logical envelope
 * protocol (`Transport`): the tests script it by method id, let it own stores
 * that answer `Observe` with change-sets, and have it call ports. It records
 * everything the host sends.
 */

/** Answers one call. */
export interface Responder {
  readonly call: CallPayload;
  readonly callId: number;
  ok(body?: Uint8Array): void;
  error(body: Uint8Array): void;
  panic(message: string, backtrace?: string): void;
  badRequest(reason: string): void;
  cancelled(): void;
  /** Sends nothing; the test answers later through `fake.reply(...)`. */
  defer(): void;
}

/** Handles a call to a scripted method. */
export type CallHandler = (call: CallPayload, respond: Responder) => void;

/** A scripted stream: `next` yields items; `fake` sends them as credit allows. */
export interface StreamScript {
  next(): { done: true; error?: Uint8Array } | { done: false; value: Uint8Array };
}

/** Options of {@link FakeCoreTransport}. */
export interface FakeOptions {
  /** In-process semantics: output is delivered inside `send`, and `callSync` exists. Default `false`. */
  readonly synchronous?: boolean;
  readonly schemaHash?: bigint;
  readonly mode?: string;
}

export const SCHEMA = 0x00c0_ffee_0bad_f00dn;

function callIdOf(call: CallPayload): number {
  return call.callId;
}

export class FakeCoreTransport implements Transport {
  readonly mode: string;
  readonly synchronous: boolean;
  readonly schemaHash: bigint;
  readonly sent: Array<{ readonly kind: Kind; readonly payload: Uint8Array }> = [];
  readonly calls: CallPayload[] = [];
  readonly cancelled: number[] = [];
  readonly released: bigint[] = [];
  readonly observed: Array<{ handle: bigint; signalId: number; on: boolean }> = [];
  readonly events: Array<{ portId: number; methodId: number; payload: Uint8Array }> = [];
  readonly credits: Array<{ callId: number; credit: number }> = [];
  readonly timersFired: number[] = [];
  closed = false;
  started = false;
  /** When set, `callSync` is only defined on transports created with `synchronous`. */
  callSync?: (payload: Uint8Array) => Uint8Array;

  #handler: TransportHandler | null = null;
  readonly #methods = new Map<number, CallHandler>();
  readonly #stores = new Map<bigint, Map<number, Uint8Array>>();
  readonly #streams = new Map<number, { script: StreamScript; credit: number; peeked: ReturnType<StreamScript["next"]> | undefined }>();
  readonly #portWaiters = new Map<number, (reply: PortReplyPayload) => void>();
  #nextPortCallId = 1;
  #txn = 0n;
  readonly #queue: Array<() => void> = [];
  #scheduled = false;
  #collector: Array<() => void> | null = null;
  #capture: Uint8Array[] | null = null;

  constructor(options: FakeOptions = {}) {
    this.synchronous = options.synchronous ?? false;
    this.schemaHash = options.schemaHash ?? SCHEMA;
    this.mode = options.mode ?? (this.synchronous ? "wasm-main" : "remote");
    if (this.synchronous) {
      this.callSync = (payload) => {
        const captured: Uint8Array[] = [];
        this.#capture = captured;
        try {
          this.#dispatchCall(decodeCall(payload));
        } finally {
          this.#capture = null;
        }
        const reply = captured[0];
        if (reply === undefined) throw new Error("the scripted handler did not reply synchronously");
        return reply;
      };
    }
  }

  // ----- scripting -------------------------------------------------------------------

  /** Scripts the answer to calls of `methodId`. */
  on(methodId: number, handler: CallHandler): this {
    this.#methods.set(methodId, handler);
    return this;
  }

  /** Makes `methodId` reply Ok with whatever `map` makes of the arguments. */
  echo(methodId: number, map: (args: Uint8Array) => Uint8Array = (a) => a): this {
    return this.on(methodId, (call, r) => {
      r.ok(map("args" in call ? call.args : new Uint8Array(0)));
    });
  }

  /** Makes `methodId` open a stream of `script` (status 4, then items as credit allows, then the end or the error). */
  stream(methodId: number, script: (call: CallPayload) => StreamScript): this {
    return this.on(methodId, (call, r) => {
      const callId = callIdOf(call);
      this.#streams.set(callId, { script: script(call), credit: 0, peeked: undefined });
      this.#send(() => this.#handler?.reply(encodeReply({ callId, status: ReplyStatus.StreamOpened })));
      this.#pumpStream(callId);
      r.defer();
    });
  }

  /** Gives the fake a store: `Observe` answers with a change-set carrying these values. */
  store(handle: bigint, values: ReadonlyMap<number, Uint8Array>): void {
    this.#stores.set(handle, new Map(values));
  }

  /** Changes a stored value and, when the store is observed, emits it as a change-set. */
  setSignal(handle: bigint, signalId: number, value: Uint8Array): void {
    this.#stores.get(handle)?.set(signalId, value);
    this.emitChangeSet([{ handle, signalId, op: ChangeOp.FullValue, value }]);
  }

  // ----- what the core sends ---------------------------------------------------------

  /** Delivers a change-set (one message). */
  emitChangeSet(entries: readonly ChangeEntry[]): void {
    this.#send(() => this.#handler?.changeSet(encodeChangeSet({ txnId: ++this.#txn, entries })));
  }

  /** Delivers raw change-set bytes (for malformed input). */
  emitRawChangeSet(payload: Uint8Array): void {
    this.#send(() => this.#handler?.changeSet(payload));
  }

  /** Delivers a `Reply` for `callId` (for calls the test deferred). */
  reply(callId: number, status: ReplyStatus.Ok | ReplyStatus.Error, body: Uint8Array = new Uint8Array(0)): void {
    this.#send(() => this.#handler?.reply(encodeReply({ callId, status, body })));
  }

  /** Delivers a raw `Reply` payload. */
  emitRawReply(payload: Uint8Array): void {
    this.#send(() => this.#handler?.reply(payload));
  }

  /** Delivers a `StreamItem`. */
  emitStreamItem(callId: number, flag: StreamFlag, body: Uint8Array = new Uint8Array(0)): void {
    this.#send(() =>
      this.#handler?.streamItem(
        flag === StreamFlag.End ? encodeStreamItem({ callId, flag }) : encodeStreamItem({ callId, flag, body }),
      ),
    );
  }

  /** Delivers a raw `StreamItem` payload (for malformed input). */
  emitRawStreamItem(payload: Uint8Array): void {
    this.#send(() => this.#handler?.streamItem(payload));
  }

  /** Delivers a log record. */
  emitLog(level: number, target: string, message: string): void {
    this.#send(() => this.#handler?.log(level, target, message));
  }

  /** The channel dies. */
  fail(error: Error = new UndraTransportError("closed", "the connection was lost")): void {
    this.#send(() => this.#handler?.closed(error));
  }

  /**
   * The core calls a port. Resolves with the `PortReply` the host produced
   * (immediately for a synchronous answer, later for an asynchronous one).
   */
  callPort(portId: number, methodId: number, args: Uint8Array = new Uint8Array(0)): Promise<PortReplyPayload> {
    const portCallId = this.#nextPortCallId++;
    return new Promise((resolve) => {
      this.#send(() => {
        const outcome = (this.#handler as TransportHandler).portCall({ portId, methodId, portCallId, args });
        if (outcome.kind === "sync") resolve(decodePortReply(outcome.reply));
        else if (outcome.kind === "unavailable") {
          resolve({ portCallId, status: PortStatus.Unavailable, body: new Uint8Array(0) });
        } else this.#portWaiters.set(portCallId, resolve);
      });
    });
  }

  /** Runs `fn` so that everything it emits is delivered within one macrotask, back to back. */
  burst(fn: () => void): void {
    if (this.synchronous) {
      fn();
      return;
    }
    const collected: Array<() => void> = [];
    this.#collector = collected;
    try {
      fn();
    } finally {
      this.#collector = null;
    }
    this.#queue.push(() => {
      for (const step of collected) step();
    });
    this.#pump();
  }

  // ----- Transport -------------------------------------------------------------------

  start(handler: TransportHandler): Promise<HelloPayload> {
    this.#handler = handler;
    this.started = true;
    return Promise.resolve({ undraVersion: "fake", schemaHash: this.schemaHash, platform: "fake", mode: "test" });
  }

  send(kind: Kind, payload: Uint8Array): void {
    if (this.closed) throw new UndraTransportError("closed", "the fake core is closed");
    this.sent.push({ kind, payload });
    switch (kind) {
      case Kind.Call:
        this.#dispatchCall(decodeCall(payload));
        return;
      case Kind.Cancel: {
        const { callId } = decodeCancel(payload);
        this.cancelled.push(callId);
        this.#streams.delete(callId);
        return;
      }
      case Kind.StreamCredit: {
        const credit = decodeStreamCredit(payload);
        this.credits.push(credit);
        const stream = this.#streams.get(credit.callId);
        if (stream !== undefined) {
          stream.credit += credit.credit;
          this.#pumpStream(credit.callId);
        }
        return;
      }
      case Kind.Observe: {
        const observe = decodeObserve(payload);
        this.observed.push(observe);
        const values = this.#stores.get(observe.handle);
        if (!observe.on || values === undefined) return;
        const entries: ChangeEntry[] = [];
        for (const [signalId, value] of values) {
          if (observe.signalId === ALL_SIGNALS || observe.signalId === signalId) {
            entries.push({ handle: observe.handle, signalId, op: ChangeOp.FullValue, value });
          }
        }
        if (entries.length > 0) this.emitChangeSet(entries);
        return;
      }
      case Kind.Release:
        this.released.push(decodeRelease(payload).handle);
        return;
      case Kind.Event:
        this.events.push(decodeEvent(payload));
        return;
      case Kind.PortReply: {
        const reply = decodePortReply(payload);
        const waiter = this.#portWaiters.get(reply.portCallId);
        this.#portWaiters.delete(reply.portCallId);
        waiter?.(reply);
        return;
      }
      case Kind.TimerFired:
        this.timersFired.push(new DataView(payload.buffer, payload.byteOffset, payload.byteLength).getUint32(0, true));
        return;
      default:
        return;
    }
  }

  stats(): Promise<string | null> {
    return Promise.resolve(JSON.stringify({ live_handles: this.#stores.size, platform: "fake" }));
  }

  close(): void {
    this.closed = true;
  }

  // ----- internals -------------------------------------------------------------------

  #dispatchCall(call: CallPayload): void {
    this.calls.push(call);
    const methodId = "methodId" in call ? call.methodId : -1;
    const handler = this.#methods.get(methodId);
    const callId = callIdOf(call);
    const respond: Responder = {
      call,
      callId,
      ok: (body = new Uint8Array(0)) => {
        this.#answer(encodeReply({ callId, status: ReplyStatus.Ok, body }));
      },
      error: (body) => {
        this.#answer(encodeReply({ callId, status: ReplyStatus.Error, body }));
      },
      panic: (message, backtrace = "") => {
        this.#answer(encodeReply({ callId, status: ReplyStatus.Panic, message, backtrace }));
      },
      badRequest: (reason) => {
        this.#answer(encodeReply({ callId, status: ReplyStatus.BadRequest, reason }));
      },
      cancelled: () => {
        this.#answer(encodeReply({ callId, status: ReplyStatus.Cancelled }));
      },
      defer: () => {},
    };
    if (handler === undefined) {
      respond.badRequest(`unknown method 0x${methodId.toString(16)}`);
      return;
    }
    handler(call, respond);
  }

  #answer(reply: Uint8Array): void {
    if (this.#capture !== null) {
      this.#capture.push(reply);
      return;
    }
    this.#send(() => this.#handler?.reply(reply));
  }

  /** Sends the items the stream has credit for, and its end or error as soon as it is reached (no credit needed). */
  #pumpStream(callId: number): void {
    this.#send(() => {
      const stream = this.#streams.get(callId);
      if (stream === undefined) return;
      for (;;) {
        const step = stream.peeked ?? stream.script.next();
        stream.peeked = undefined;
        if (step.done) {
          this.#streams.delete(callId);
          const item =
            step.error === undefined
              ? encodeStreamItem({ callId, flag: StreamFlag.End })
              : encodeStreamItem({ callId, flag: StreamFlag.Error, body: step.error });
          this.#handler?.streamItem(item);
          return;
        }
        if (stream.credit <= 0) {
          stream.peeked = step;
          return;
        }
        stream.credit--;
        this.#handler?.streamItem(encodeStreamItem({ callId, flag: StreamFlag.Item, body: step.value }));
      }
    });
  }

  /** Delivers `fn` inline for a synchronous transport, else on a later macrotask, in order. */
  #send(fn: () => void): void {
    if (this.synchronous) {
      fn();
    } else if (this.#collector !== null) {
      this.#collector.push(fn);
    } else {
      this.#queue.push(fn);
      this.#pump();
    }
  }

  #pump(): void {
    if (this.#scheduled) return;
    this.#scheduled = true;
    setTimeout(() => {
      this.#scheduled = false;
      this.#queue.shift()?.();
      if (this.#queue.length > 0) this.#pump();
    }, 0);
  }

  /** Waits until every queued delivery has happened. */
  async settle(): Promise<void> {
    while (this.#queue.length > 0 || this.#scheduled) {
      await new Promise<void>((resolve) => setTimeout(resolve, 0));
    }
    await Promise.resolve();
  }
}
