import { cryptoRng, setTimeoutTimer, systemClock } from "../adapters/system.js";
import type { ClockAdapter, RngAdapter, TimerAdapter } from "../adapters/types.js";
import { KeelError, KeelReplyError, KeelSchemaMismatchError, KeelTransportError } from "../errors.js";
import { errorMessage, hostPlatform } from "../platform.js";
import {
  type HelloPayload,
  KeelReader,
  KeelWriter,
  Kind,
  ReplyStatus,
  codecs,
  decodeCancel,
  decodeEvent,
  decodeObserve,
  decodeRelease,
  decodeStreamCredit,
  decodeTimerFired,
  encodeValue,
  splitHandle,
} from "../wire/index.js";
import type { PortCallPayload } from "../wire/index.js";
import type { PortOutcome, Transport, TransportHandler } from "./transport.js";

/** The wasm ABI version this transport speaks (`keel_abi_version`). */
const ABI_VERSION = 1;
/** Payloads up to this size travel through one reusable scratch buffer in wasm memory. */
const SCRATCH_LIMIT = 64 * 1024;
const SCRATCH_MIN = 1024;

/** What the core's wasm module can be given as: a URL to fetch, its bytes, or a compiled module. */
export type WasmSource = URL | BufferSource | WebAssembly.Module;

/** Options of {@link WasmMainTransport}. */
export interface WasmMainOptions {
  /** The core's `.wasm`. */
  readonly wasm: WasmSource;
  /** The schema hash the bindings were generated from; a different core is refused before `keel_init`. */
  readonly expectedSchemaHash: bigint;
  /** Platform name for `RuntimeConfig`; default `"web"` or `"node"`. */
  readonly platform?: string;
  /** Runs the core in `"dev"` mode (extra devtools log records, SPEC 5.10) instead of `"inproc"`. */
  readonly devtools?: boolean;
  /** Log records below this level (0 trace .. 5 fatal) are not forwarded by the core. Default 2. */
  readonly logLevel?: number;
  /** Replaces `Date.now` behind the `now_ms` import. */
  readonly clock?: ClockAdapter;
  /** Replaces `crypto.getRandomValues` behind the `random` import. */
  readonly rng?: RngAdapter;
  /** Replaces `setTimeout` behind the `timer_set` import. */
  readonly timer?: TimerAdapter;
  /** Receives failures that have no caller: an import handler that threw, a timer or poll that trapped. Default: ignored (the handler's `closed` reports traps). */
  readonly onError?: (error: unknown) => void;
}

/** The exports of a Keel core module (SPEC 7) that this transport uses. */
interface CoreExports {
  readonly memory: WebAssembly.Memory;
  keel_alloc(len: number): number;
  keel_free(ptr: number, len: number): void;
  keel_abi_version(): number;
  keel_schema_hash(): bigint;
  keel_init(ptr: number, len: number): number;
  keel_call(ptr: number, len: number): number;
  keel_call_sync(ptr: number, len: number): number;
  keel_cancel(callId: number): void;
  keel_stream_credit(callId: number, credit: number): void;
  keel_observe(handleLo: number, handleHi: number, signalId: number, on: number): void;
  keel_release(handleLo: number, handleHi: number): void;
  keel_port_reply(ptr: number, len: number): void;
  keel_event(portId: number, methodId: number, ptr: number, len: number): void;
  keel_timer_fired(timerId: number): void;
  keel_poll(): void;
  keel_buf_free(bufPtr: number): void;
  keel_stats_json(): number;
  keel_restore?(ptr: number, len: number): number;
  _initialize?(): void;
}

const REQUIRED_FUNCTIONS = [
  "keel_alloc",
  "keel_free",
  "keel_abi_version",
  "keel_schema_hash",
  "keel_init",
  "keel_call",
  "keel_call_sync",
  "keel_cancel",
  "keel_stream_credit",
  "keel_observe",
  "keel_release",
  "keel_port_reply",
  "keel_event",
  "keel_timer_fired",
  "keel_poll",
  "keel_buf_free",
  "keel_stats_json",
] as const;

async function instantiate(source: WasmSource, imports: WebAssembly.Imports): Promise<WebAssembly.Instance> {
  if (typeof WebAssembly !== "object") {
    throw new KeelTransportError("unsupported", "WebAssembly is not available on this platform");
  }
  try {
    if (source instanceof WebAssembly.Module) return await WebAssembly.instantiate(source, imports);
    if (source instanceof URL) {
      const response = await fetch(source);
      if (!response.ok) throw new Error(`GET ${source.href} answered ${response.status}`);
      if (
        typeof WebAssembly.instantiateStreaming === "function" &&
        response.headers.get("content-type")?.startsWith("application/wasm") === true
      ) {
        return (await WebAssembly.instantiateStreaming(response, imports)).instance;
      }
      return (await WebAssembly.instantiate(await response.arrayBuffer(), imports)).instance;
    }
    return (await WebAssembly.instantiate(source, imports)).instance;
  } catch (cause) {
    throw new KeelTransportError("handshake", `could not instantiate the wasm core: ${errorMessage(cause)}`, {
      cause,
    });
  }
}

/**
 * The `log` import passes `(level, ptr, len)`. Its bytes are the record's
 * `target` and `message` as two wire strings; anything else is taken as the
 * UTF-8 text of the message (a panic hook has no target to give).
 */
function parseLog(raw: Uint8Array): { readonly target: string; readonly message: string } {
  try {
    const r = new KeelReader(raw);
    const target = r.readStr();
    const message = r.readStr();
    r.finish();
    return { target, message };
  } catch {
    return { target: "keel", message: new TextDecoder().decode(raw) };
  }
}

/**
 * Runs a Keel core in this thread, through the wasm ABI of docs/SPEC.md
 * section 7, and implements the `keel` import object it needs: `reply`,
 * `changeset`, `stream`, `port_call`, `schedule` (`keel_poll` from a
 * microtask), `timer_set` (`setTimeout`, then `keel_timer_fired`), `log`,
 * `now_ms` and `random`.
 *
 * This is the `wasm-main` mode: calls cross into wasm synchronously, so
 * `callSync` works and a reply to a synchronous method is available before
 * `send` returns. The same class runs inside the worker of the `wasm-worker`
 * mode.
 *
 * Memory: views over the module's memory are recreated whenever it grows,
 * payloads are copied out of the core inside every callback (they are only
 * valid during it), and no import handler ever throws into wasm, since a JS
 * exception crossing wasm frames would leave the core's lock held.
 */
export class WasmMainTransport implements Transport {
  readonly mode = "wasm-main";
  readonly synchronous = true;

  readonly #options: WasmMainOptions;
  readonly #clock: ClockAdapter;
  #rng: RngAdapter | null;
  readonly #timer: TimerAdapter;
  #handler: TransportHandler | null = null;
  #instance: WebAssembly.Instance | null = null;
  #exports: CoreExports | null = null;
  #buffer: ArrayBuffer | null = null;
  #u8 = new Uint8Array(0);
  #view = new DataView(new ArrayBuffer(0));
  #depth = 0;
  #scratchPtr = 0;
  #scratchCap = 0;
  #pollScheduled = false;
  #closed = false;
  #dead: KeelTransportError | null = null;

  /** @param options See {@link WasmMainOptions}. */
  constructor(options: WasmMainOptions) {
    this.#options = options;
    this.#clock = options.clock ?? systemClock();
    this.#timer = options.timer ?? setTimeoutTimer();
    this.#rng = options.rng ?? null;
  }

  /** The instantiated module (after `start`); for devtools and tests. */
  get instance(): WebAssembly.Instance | null {
    return this.#instance;
  }

  async start(handler: TransportHandler): Promise<HelloPayload> {
    this.#handler = handler;
    const instance = await instantiate(this.#options.wasm, this.#imports());
    this.#instance = instance;
    const exported = instance.exports as unknown as Record<string, unknown>;
    const missing = [
      ...(exported.memory instanceof WebAssembly.Memory ? [] : ["memory"]),
      ...REQUIRED_FUNCTIONS.filter((name) => typeof exported[name] !== "function"),
    ];
    if (missing.length > 0) {
      throw new KeelTransportError("handshake", `the module is not a Keel core: it does not export ${missing.join(", ")}`);
    }
    this.#exports = exported as unknown as CoreExports;
    const { abi, schemaHash } = this.#run((e) => {
      e._initialize?.();
      return { abi: e.keel_abi_version(), schemaHash: BigInt.asUintN(64, e.keel_schema_hash()) };
    });
    if (abi !== ABI_VERSION) {
      throw new KeelTransportError("handshake", `the core speaks wasm ABI ${abi}, this runtime speaks ${ABI_VERSION}`);
    }
    if (schemaHash !== this.#options.expectedSchemaHash) {
      throw new KeelSchemaMismatchError(this.#options.expectedSchemaHash, schemaHash);
    }
    const mode = this.#options.devtools === true ? "dev" : "inproc";
    const platform = this.#options.platform ?? hostPlatform();
    const config = new KeelWriter(32);
    config.writeStr(platform);
    config.writeStr(mode);
    config.writeU8(0); // core_threads: the host drives the executor (keel_poll)
    config.writeU8(0); // blocking_threads: no pool on wasm
    config.writeU8(this.#options.logLevel ?? 2);
    const code = this.#invoke(config.finish(), (e, ptr, len) => e.keel_init(ptr, len));
    if (code !== 0) throw new KeelTransportError("handshake", `keel_init failed with code ${code}`);
    return { keelVersion: `wasm-abi-${abi}`, schemaHash, platform: "wasm", mode };
  }

  send(kind: Kind, payload: Uint8Array): void {
    switch (kind) {
      case Kind.Call: {
        const code = this.#invoke(payload, (e, ptr, len) => e.keel_call(ptr, len));
        if (code !== 0) {
          throw new KeelReplyError(
            ReplyStatus.BadRequest,
            encodeValue(codecs.string, `the core refused the call (keel_call returned ${code})`),
          );
        }
        return;
      }
      case Kind.Cancel: {
        const { callId } = decodeCancel(payload);
        this.#run((e) => e.keel_cancel(callId));
        return;
      }
      case Kind.StreamCredit: {
        const { callId, credit } = decodeStreamCredit(payload);
        this.#run((e) => e.keel_stream_credit(callId, credit));
        return;
      }
      case Kind.Observe: {
        const { handle, signalId, on } = decodeObserve(payload);
        const { lo, hi } = splitHandle(handle);
        this.#run((e) => e.keel_observe(lo, hi, signalId, on ? 1 : 0));
        return;
      }
      case Kind.Release: {
        const { lo, hi } = splitHandle(decodeRelease(payload).handle);
        this.#run((e) => e.keel_release(lo, hi));
        return;
      }
      case Kind.Event: {
        const event = decodeEvent(payload);
        this.#invoke(event.payload, (e, ptr, len) => e.keel_event(event.portId, event.methodId, ptr, len));
        return;
      }
      case Kind.PortReply:
        this.#invoke(payload, (e, ptr, len) => e.keel_port_reply(ptr, len));
        return;
      case Kind.TimerFired: {
        const { timerId } = decodeTimerFired(payload);
        this.#run((e) => e.keel_timer_fired(timerId));
        return;
      }
      case Kind.Restore: {
        const code = this.#invoke(payload, (e, ptr, len) => {
          if (e.keel_restore === undefined) {
            throw new KeelTransportError("unsupported", "the core does not export keel_restore");
          }
          return e.keel_restore(ptr, len);
        });
        if (code !== 0) throw new KeelTransportError("protocol", `keel_restore failed with code ${code}`);
        return;
      }
      default:
        throw new KeelTransportError("protocol", `cannot send a ${Kind[kind] ?? String(kind)} message to a wasm core`);
    }
  }

  callSync(payload: Uint8Array): Uint8Array {
    return this.#invoke(payload, (e, ptr, len) => this.#takeBuf(e, e.keel_call_sync(ptr, len)));
  }

  stats(): Promise<string | null> {
    try {
      const json = this.#run((e) => new TextDecoder().decode(this.#takeBuf(e, e.keel_stats_json())));
      return Promise.resolve(json);
    } catch (error) {
      return Promise.reject(error);
    }
  }

  close(): void {
    this.#closed = true;
    this.#handler = null;
    this.#exports = null;
  }

  // ----- memory ----------------------------------------------------------------------

  /** The current byte view of the module's memory; recreated when memory grew (the old buffer is detached). */
  #bytes(): Uint8Array {
    const memory = (this.#exports as CoreExports).memory;
    if (memory.buffer !== this.#buffer) {
      this.#buffer = memory.buffer;
      this.#u8 = new Uint8Array(memory.buffer);
      this.#view = new DataView(memory.buffer);
    }
    return this.#u8;
  }

  /** A copy of `[ptr, ptr + len)` of wasm memory. Import arguments are `i32`, so they arrive signed. */
  #copyOut(ptr: number, len: number): Uint8Array {
    const start = ptr >>> 0;
    const end = start + (len >>> 0);
    const bytes = this.#bytes();
    if (end > bytes.length) {
      throw new RangeError(`the core handed out [${start}, ${end}) beyond its ${bytes.length} bytes of memory`);
    }
    return bytes.slice(start, end);
  }

  /** Copies the bytes of a `KeelBuf { ptr, len, cap }` and frees it. */
  #takeBuf(e: CoreExports, bufPtr: number): Uint8Array {
    if (bufPtr === 0) throw new KeelTransportError("protocol", "the core returned a null KeelBuf");
    try {
      this.#bytes();
      const ptr = this.#view.getUint32(bufPtr, true);
      const len = this.#view.getUint32(bufPtr + 4, true);
      return this.#copyOut(ptr, len);
    } finally {
      e.keel_buf_free(bufPtr);
    }
  }

  // ----- calling into wasm -----------------------------------------------------------

  #live(): CoreExports {
    if (this.#dead !== null) throw this.#dead;
    if (this.#exports === null) {
      throw new KeelTransportError("closed", this.#closed ? "the core is closed" : "the core is not started");
    }
    return this.#exports;
  }

  /** Runs `call` with a copy of `bytes` in wasm memory. */
  #invoke<R>(bytes: Uint8Array, call: (e: CoreExports, ptr: number, len: number) => R): R {
    const e = this.#live();
    // A payload copied while another export is running (a sync port reply
    // from inside `port_call`) must not reuse the scratch buffer the outer
    // call may still be reading.
    const nested = this.#depth > 0;
    this.#depth++;
    let owned = 0;
    let ptr = 0;
    try {
      const len = bytes.length;
      if (!nested && len <= SCRATCH_LIMIT) {
        ptr = this.#scratch(e, len);
      } else {
        owned = Math.max(len, 1);
        ptr = e.keel_alloc(owned);
      }
      this.#bytes().set(bytes, ptr >>> 0);
      return call(e, ptr, len);
    } catch (error) {
      throw this.#classify(error);
    } finally {
      this.#depth--;
      if (owned > 0 && this.#dead === null) e.keel_free(ptr, owned);
    }
  }

  /** Runs `call` for an export that takes no buffer. */
  #run<R>(call: (e: CoreExports) => R): R {
    const e = this.#live();
    this.#depth++;
    try {
      return call(e);
    } catch (error) {
      throw this.#classify(error);
    } finally {
      this.#depth--;
    }
  }

  #scratch(e: CoreExports, len: number): number {
    if (this.#scratchCap < len) {
      if (this.#scratchPtr !== 0) e.keel_free(this.#scratchPtr, this.#scratchCap);
      let cap = SCRATCH_MIN;
      while (cap < len) cap *= 2;
      this.#scratchPtr = e.keel_alloc(cap);
      this.#scratchCap = cap;
    }
    return this.#scratchPtr;
  }

  /**
   * An exception that escapes an export is a trap (`panic=abort` compiles
   * panics to `unreachable`) or an engine failure (stack overflow, out of
   * memory). The instance cannot be trusted afterwards: the transport dies and
   * the handler hears about it once.
   */
  #classify(error: unknown): unknown {
    if (error instanceof KeelError) return error;
    if (this.#dead !== null) return this.#dead;
    const dead = new KeelTransportError("trap", `the wasm core trapped: ${errorMessage(error)}`, { cause: error });
    this.#dead = dead;
    const handler = this.#handler;
    if (handler !== null) queueMicrotask(() => handler.closed(dead));
    return dead;
  }

  #report(error: unknown): void {
    this.#options.onError?.(error);
  }

  // ----- the `keel` import object ----------------------------------------------------

  #imports(): WebAssembly.Imports {
    /** Wraps an import so that nothing escapes into wasm; `fallback` is what the core sees instead. */
    const guard =
      <A extends unknown[], R>(fn: (...args: A) => R, fallback: R) =>
      (...args: A): R => {
        try {
          return fn(...args);
        } catch (error) {
          this.#report(error);
          return fallback;
        }
      };
    return {
      keel: {
        reply: guard((_callId: number, ptr: number, len: number) => {
          this.#handler?.reply(this.#copyOut(ptr, len));
        }, undefined),
        changeset: guard((ptr: number, len: number) => {
          this.#handler?.changeSet(this.#copyOut(ptr, len));
        }, undefined),
        stream: guard((_callId: number, ptr: number, len: number) => {
          this.#handler?.streamItem(this.#copyOut(ptr, len));
        }, undefined),
        port_call: guard(
          (portId: number, methodId: number, portCallId: number, ptr: number, len: number) =>
            this.#portCall({ portId: portId >>> 0, methodId: methodId >>> 0, portCallId: portCallId >>> 0, args: this.#copyOut(ptr, len) }),
          2,
        ),
        schedule: guard(() => {
          this.#schedulePoll();
        }, undefined),
        timer_set: guard((timerId: number, delayLo: number, delayHi: number) => {
          const id = timerId >>> 0;
          this.#timer.set(id, (delayHi >>> 0) * 0x1_0000_0000 + (delayLo >>> 0), (fired) => {
            this.#timerFired(fired);
          });
        }, undefined),
        log: guard((level: number, ptr: number, len: number) => {
          const { target, message } = parseLog(this.#copyOut(ptr, len));
          this.#handler?.log(level & 0xff, target, message);
        }, undefined),
        now_ms: guard(() => this.#clock.nowMs(), 0),
        random: guard((ptr: number, len: number) => {
          const start = ptr >>> 0;
          (this.#rng ??= cryptoRng()).fill(this.#bytes().subarray(start, start + (len >>> 0)));
        }, undefined),
      },
    };
  }

  #portCall(call: PortCallPayload): number {
    const handler = this.#handler;
    if (handler === null) return 2;
    const outcome: PortOutcome = handler.portCall(call);
    switch (outcome.kind) {
      case "sync":
        // The reply must be in the core before `port_call` returns 0.
        this.#invoke(outcome.reply, (e, ptr, len) => e.keel_port_reply(ptr, len));
        return 0;
      case "async":
        return 1;
      case "unavailable":
        return 2;
    }
  }

  #schedulePoll(): void {
    if (this.#pollScheduled || this.#closed) return;
    this.#pollScheduled = true;
    queueMicrotask(() => {
      this.#pollScheduled = false;
      if (this.#closed || this.#dead !== null) return;
      try {
        this.#run((e) => e.keel_poll());
      } catch (error) {
        this.#report(error);
      }
    });
  }

  #timerFired(timerId: number): void {
    if (this.#closed || this.#dead !== null) return;
    try {
      this.#run((e) => e.keel_timer_fired(timerId));
    } catch (error) {
      this.#report(error);
    }
  }
}
