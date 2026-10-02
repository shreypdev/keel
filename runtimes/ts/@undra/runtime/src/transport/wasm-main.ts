import { cryptoRng, setTimeoutTimer, systemClock } from "../adapters/system.js";
import type { ClockAdapter, RngAdapter, TimerAdapter } from "../adapters/types.js";
import { UndraError, UndraReplyError, UndraSchemaMismatchError, UndraTransportError } from "../errors.js";
import { errorMessage, hostPlatform } from "../platform.js";

import {
  type HelloPayload,
  UndraReader,
  UndraWriter,
  ReplyStatus,
  codecs,
  encodeValue,
  splitHandle,
} from "../wire/index.js";
import type { PortCallPayload } from "../wire/index.js";
import type { Channel, PortOutcome, TransportHandler } from "./transport.js";

/** The error of a call the core refused without a reply (`undra_call` returned `code`). */
function refused(code: number): UndraReplyError {
  return new UndraReplyError(ReplyStatus.BadRequest, encodeValue(codecs.string, `the core refused the call (undra_call returned ${code})`));
}

/** The wasm ABI version this transport speaks (`undra_abi_version`). */
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
  /** The schema hash the bindings were generated from; a different core is refused before `undra_init`. */
  readonly expectedSchemaHash: bigint;
  /** Platform name for `RuntimeConfig`; default `"web"` or `"node"`. */
  readonly platform?: string;
  /** Runs the core in `"dev"` mode (extra devtools log records, SPEC 5.10) instead of `"inproc"`. */
  readonly devtools?: boolean;
  /** Log records below this level (0 trace .. 5 fatal) are not forwarded by the core. Default 2. */
  readonly logLevel?: number;
  /** Replaces `Date.now` behind the `now_ms` import. */
  readonly clock?: ClockAdapter;
  /**
   * Replaces `crypto.getRandomValues` behind the `random` import. It must fill the whole buffer with
   * cryptographically secure bytes or throw: a throw makes the core's `Rng` unavailable (ADR-049).
   */
  readonly rng?: RngAdapter;
  /** Replaces `setTimeout` behind the `timer_set` import. */
  readonly timer?: TimerAdapter;
  /** Receives failures that have no caller: an import handler that threw, a timer or poll that trapped. Default: ignored (the handler's `closed` reports traps). */
  readonly onError?: (error: unknown) => void;
}

/** The exports of an Undra core module (SPEC 7) that this transport uses. */
export interface CoreExports {
  readonly memory: WebAssembly.Memory;
  undra_alloc(len: number): number;
  undra_free(ptr: number, len: number): void;
  undra_abi_version(): number;
  undra_schema_hash(): bigint;
  undra_init(ptr: number, len: number): number;
  undra_call(ptr: number, len: number): number;
  undra_call_sync(ptr: number, len: number): number;
  undra_cancel(callId: number): void;
  undra_stream_credit(callId: number, credit: number): void;
  undra_observe(handleLo: number, handleHi: number, signalId: number, on: number): void;
  undra_release(handleLo: number, handleHi: number): void;
  undra_port_reply(ptr: number, len: number): void;
  undra_event(portId: number, methodId: number, ptr: number, len: number): void;
  undra_timer_fired(timerId: number): void;
  undra_poll(): void;
  undra_buf_free(bufPtr: number): void;
  undra_stats_json(): number;
  undra_snapshot?(): number;
  undra_restore?(ptr: number, len: number): number;
  _initialize?(): void;
}

const REQUIRED_FUNCTIONS = [
  "undra_alloc",
  "undra_free",
  "undra_abi_version",
  "undra_schema_hash",
  "undra_init",
  "undra_call",
  "undra_call_sync",
  "undra_cancel",
  "undra_stream_credit",
  "undra_observe",
  "undra_release",
  "undra_port_reply",
  "undra_event",
  "undra_timer_fired",
  "undra_poll",
  "undra_buf_free",
  "undra_stats_json",
] as const;

/**
 * `undra_alloc` (SPEC 7) traps when it cannot satisfy a request, so it never returns 0. A 0 is a
 * module that broke that contract, and copying a payload to linear address 0 would overwrite the
 * bottom of its shadow stack: fail (the caller's trap path closes the transport) instead.
 */
function allocate(e: CoreExports, len: number): number {
  const ptr = e.undra_alloc(len);
  if (ptr === 0) throw new Error(`undra_alloc(${len}) returned 0 instead of trapping`);
  return ptr;
}

/** Compiles (unless it is compiled already) and instantiates `source`; the compiled module is kept for a restart (ADR-049: no recompile). */
async function instantiate(source: WasmSource, imports: WebAssembly.Imports): Promise<WebAssembly.WebAssemblyInstantiatedSource> {
  if (typeof WebAssembly !== "object") {
    throw new UndraTransportError("unsupported", "WebAssembly is not available on this platform");
  }
  try {
    if (source instanceof WebAssembly.Module) return { module: source, instance: await WebAssembly.instantiate(source, imports) };
    if (source instanceof URL) {
      const response = await fetch(source);
      if (!response.ok) throw new Error(`GET ${source.href} answered ${response.status}`);
      if (
        typeof WebAssembly.instantiateStreaming === "function" &&
        response.headers.get("content-type")?.startsWith("application/wasm") === true
      ) {
        return await WebAssembly.instantiateStreaming(response, imports);
      }
      return await WebAssembly.instantiate(await response.arrayBuffer(), imports);
    }
    return await WebAssembly.instantiate(source, imports);
  } catch (cause) {
    throw new UndraTransportError("handshake", `could not instantiate the wasm core: ${errorMessage(cause)}`, {
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
    const r = new UndraReader(raw);
    const target = r.readStr();
    const message = r.readStr();
    r.finish();
    return { target, message };
  } catch {
    return { target: "undra", message: new TextDecoder().decode(raw) };
  }
}

/**
 * Runs an Undra core in this thread, through the wasm ABI of docs/SPEC.md
 * section 7, and implements the `undra` import object it needs: `reply`,
 * `changeset`, `stream`, `port_call`, `schedule` (`undra_poll` from a
 * microtask), `timer_set` (`setTimeout`, then `undra_timer_fired`), `log`,
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
export class WasmHost implements Channel {
  readonly mode = "wasm-main";
  readonly synchronous = true;

  /** @internal */
  readonly _options: WasmMainOptions;
  private readonly _clock: ClockAdapter;
  private _rng: RngAdapter | null;
  private readonly _onError: ((error: unknown) => void) | undefined;
  private readonly _timer: TimerAdapter;
  private _handler: TransportHandler | null = null;
  private _instance: WebAssembly.Instance | null = null;
  private _exports: CoreExports | null = null;
  private _buffer: ArrayBuffer | null = null;
  private _u8 = new Uint8Array(0);
  private _view = new DataView(new ArrayBuffer(0));
  private _depth = 0;
  private _scratchPtr = 0;
  private _scratchCap = 0;
  private _pollScheduled = false;
  private _closed = false;
  private _dead: UndraTransportError | null = null;
  /** The compiled module, kept so that a restart instantiates it again without compiling (ADR-049, `twin`). */
  /** @internal */
  _module: WebAssembly.Module | null = null;

  /**
   * @param options See {@link WasmMainOptions}.
   * @param adapters Clock, Rng and Timer adapters that replace the ones in `options` (what `UndraCore.load` passes).
   * @param onError Replaces `options.onError`.
   */
  constructor(options: WasmMainOptions, adapters: { readonly clock?: ClockAdapter; readonly rng?: RngAdapter; readonly timer?: TimerAdapter } = options, onError = options.onError) {
    this._options = options;
    this._clock = adapters.clock ?? systemClock();
    this._timer = adapters.timer ?? setTimeoutTimer();
    this._rng = adapters.rng ?? null;
    this._onError = onError;
  }

  /** The instantiated module (after `start`); for devtools and tests. */
  get instance(): WebAssembly.Instance | null {
    return this._instance;
  }

  async start(handler: TransportHandler): Promise<HelloPayload> {
    this._handler = handler;
    const { module, instance } = await instantiate(this._options.wasm, this._imports());
    if (this._closed) throw new UndraTransportError("closed", "the core is closed");
    this._module = module;
    this._instance = instance;
    const exported = instance.exports as unknown as Record<string, unknown>;
    const missing = [
      ...(exported.memory instanceof WebAssembly.Memory ? [] : ["memory"]),
      ...REQUIRED_FUNCTIONS.filter((name) => typeof exported[name] !== "function"),
    ];
    if (missing.length > 0) {
      throw new UndraTransportError("handshake", `the module is not an Undra core: it does not export ${missing.join(", ")}`);
    }
    this._exports = exported as unknown as CoreExports;
    const { abi, schemaHash } = this._run((e) => {
      e._initialize?.();
      return { abi: e.undra_abi_version(), schemaHash: BigInt.asUintN(64, e.undra_schema_hash()) };
    });
    if (abi !== ABI_VERSION) {
      throw new UndraTransportError("handshake", `the core speaks wasm ABI ${abi}, this runtime speaks ${ABI_VERSION}`);
    }
    if (schemaHash !== this._options.expectedSchemaHash) {
      throw new UndraSchemaMismatchError(this._options.expectedSchemaHash, schemaHash);
    }
    const mode = this._options.devtools === true ? "dev" : "inproc";
    const platform = this._options.platform ?? hostPlatform();
    const config = new UndraWriter(32);
    config.writeStr(platform);
    config.writeStr(mode);
    config.writeU8(0); // core_threads: the host drives the executor (undra_poll)
    config.writeU8(0); // blocking_threads: no pool on wasm
    config.writeU8(this._options.logLevel ?? 2);
    const code = this._invoke(config.finish(), (e, ptr, len) => e.undra_init(ptr, len));
    if (code !== 0) throw new UndraTransportError("handshake", `undra_init failed with code ${code}`);
    return { undraVersion: `wasm-abi-${abi}`, schemaHash, platform: "wasm", mode };
  }

  observe(handle: bigint, signalId: number, on: boolean): void {
    const { lo, hi } = splitHandle(handle);
    this._run((e) => e.undra_observe(lo, hi, signalId, on ? 1 : 0));
  }

  release(handle: bigint): void {
    const { lo, hi } = splitHandle(handle);
    this._run((e) => e.undra_release(lo, hi));
  }

  cancel(callId: number): void {
    this._run((e) => e.undra_cancel(callId));
  }

  streamCredit(callId: number, credit: number): void {
    this._run((e) => e.undra_stream_credit(callId, credit));
  }

  event(portId: number, methodId: number, payload: Uint8Array): void {
    this._invoke(payload, (e, ptr, len) => e.undra_event(portId, methodId, ptr, len));
  }

  timerFired(timerId: number): void {
    this._run((e) => e.undra_timer_fired(timerId));
  }

  portReply(reply: Uint8Array): void {
    this._invoke(reply, (e, ptr, len) => e.undra_port_reply(ptr, len));
  }

  callSync(payload: Uint8Array): Uint8Array {
    return this._invoke(payload, (e, ptr, len) => this._takeBuf(e, e.undra_call_sync(ptr, len)));
  }

  callSyncParts(head: Uint8Array, tail: Uint8Array): Uint8Array {
    return this._invoke(head, (e, ptr, len) => this._takeBuf(e, e.undra_call_sync(ptr, len)), tail);
  }

  sendCall(head: Uint8Array, tail?: Uint8Array): void {
    const code = this._invoke(head, (e, ptr, len) => e.undra_call(ptr, len), tail);
    if (code !== 0) throw refused(code);
  }

  stats(): Promise<string | null> {
    try {
      const json = this._run((e) => new TextDecoder().decode(this._takeBuf(e, e.undra_stats_json())));
      return Promise.resolve(json);
    } catch (error) {
      return Promise.reject(error);
    }
  }

  close(): void {
    this._closed = true;
    this._handler = null;
    this._exports = null;
  }

  // ----- memory ----------------------------------------------------------------------

  /** The current byte view of the module's memory; recreated when memory grew (the old buffer is detached). */
  private _bytes(): Uint8Array {
    const memory = (this._exports as CoreExports).memory;
    if (memory.buffer !== this._buffer) {
      this._buffer = memory.buffer;
      this._u8 = new Uint8Array(memory.buffer);
      this._view = new DataView(memory.buffer);
    }
    return this._u8;
  }

  /** A copy of `[ptr, ptr + len)` of wasm memory. Import arguments are `i32`, so they arrive signed. */
  private _copyOut(ptr: number, len: number): Uint8Array {
    const start = ptr >>> 0;
    const end = start + (len >>> 0);
    const bytes = this._bytes();
    if (end > bytes.length) {
      throw new RangeError(`the core handed out [${start}, ${end}) beyond its ${bytes.length} bytes of memory`);
    }
    return bytes.slice(start, end);
  }

  /** Copies the bytes of an `UndraBuf { ptr, len, cap }` and frees it. */
  /** @internal */
  _takeBuf(e: CoreExports, bufPtr: number): Uint8Array {
    if (bufPtr === 0) throw new UndraTransportError("protocol", "the core returned a null UndraBuf");
    try {
      this._bytes();
      const ptr = this._view.getUint32(bufPtr, true);
      const len = this._view.getUint32(bufPtr + 4, true);
      return this._copyOut(ptr, len);
    } finally {
      e.undra_buf_free(bufPtr);
    }
  }

  // ----- calling into wasm -----------------------------------------------------------

  private _live(): CoreExports {
    if (this._dead !== null) throw this._dead;
    if (this._exports === null) {
      throw new UndraTransportError("closed", this._closed ? "the core is closed" : "the core is not started");
    }
    return this._exports;
  }

  /** Runs `call` with a copy of `bytes` in wasm memory. */
  /** @internal */
  _invoke<R>(bytes: Uint8Array, call: (e: CoreExports, ptr: number, len: number) => R, tail?: Uint8Array): R {
    const e = this._live();
    // A payload copied while another export is running (a sync port reply
    // from inside `port_call`) must not reuse the scratch buffer the outer
    // call may still be reading.
    const nested = this._depth > 0;
    this._depth++;
    let owned = 0;
    let ptr = 0;
    try {
      const len = tail === undefined ? bytes.length : bytes.length + tail.length;
      if (!nested && len <= SCRATCH_LIMIT) {
        ptr = this._scratch(e, len);
      } else {
        owned = Math.max(len, 1);
        ptr = allocate(e, owned);
      }
      const memory = this._bytes();
      memory.set(bytes, ptr >>> 0);
      if (tail !== undefined) memory.set(tail, (ptr >>> 0) + bytes.length);
      return call(e, ptr, len);
    } catch (error) {
      throw this._classify(error);
    } finally {
      this._depth--;
      if (owned > 0 && this._dead === null) e.undra_free(ptr, owned);
    }
  }

  /** Runs `call` for an export that takes no buffer. */
  /** @internal */
  _run<R>(call: (e: CoreExports) => R): R {
    const e = this._live();
    this._depth++;
    try {
      return call(e);
    } catch (error) {
      throw this._classify(error);
    } finally {
      this._depth--;
    }
  }

  private _scratch(e: CoreExports, len: number): number {
    if (this._scratchCap < len) {
      if (this._scratchPtr !== 0) e.undra_free(this._scratchPtr, this._scratchCap);
      let cap = SCRATCH_MIN;
      while (cap < len) cap *= 2;
      this._scratchPtr = allocate(e, cap);
      this._scratchCap = cap;
    }
    return this._scratchPtr;
  }

  /**
   * An exception that escapes an export is a trap (`panic=abort` compiles
   * panics to `unreachable`) or an engine failure (stack overflow, out of
   * memory). The instance cannot be trusted afterwards: the transport dies and
   * the handler hears about it once.
   */
  private _classify(error: unknown): unknown {
    if (error instanceof UndraError) return error;
    if (this._dead !== null) return this._dead;
    const dead = new UndraTransportError("trap", `the wasm core trapped: ${errorMessage(error)}`, { cause: error });
    this._dead = dead;
    const handler = this._handler;
    if (handler !== null) queueMicrotask(() => handler.closed(dead));
    return dead;
  }

  private _report(error: unknown): void {
    this._onError?.(error);
  }

  // ----- the `undra` import object ----------------------------------------------------

  private _imports(): WebAssembly.Imports {
    /** Wraps an import so that nothing escapes into wasm; `fallback` is what the core sees instead. */
    const guard =
      <A extends unknown[], R>(fn: (...args: A) => R, fallback: R) =>
      (...args: A): R => {
        try {
          return fn(...args);
        } catch (error) {
          this._report(error);
          return fallback;
        }
      };
    return {
      undra: {
        reply: guard((_callId: number, ptr: number, len: number) => {
          this._handler?.reply(this._copyOut(ptr, len));
        }, undefined),
        changeset: guard((ptr: number, len: number) => {
          this._handler?.changeSet(this._copyOut(ptr, len));
        }, undefined),
        stream: guard((_callId: number, ptr: number, len: number) => {
          this._handler?.streamItem(this._copyOut(ptr, len));
        }, undefined),
        port_call: guard(
          (portId: number, methodId: number, portCallId: number, ptr: number, len: number) =>
            this._portCall({ portId: portId >>> 0, methodId: methodId >>> 0, portCallId: portCallId >>> 0, args: this._copyOut(ptr, len) }),
          2,
        ),
        schedule: guard(() => {
          this._schedulePoll();
        }, undefined),
        timer_set: guard((timerId: number, delayLo: number, delayHi: number) => {
          const id = timerId >>> 0;
          this._timer.set(id, (delayHi >>> 0) * 0x1_0000_0000 + (delayLo >>> 0), (fired) => {
            this._timerFired(fired);
          });
        }, undefined),
        log: guard((level: number, ptr: number, len: number) => {
          const { target, message } = parseLog(this._copyOut(ptr, len));
          this._handler?.log(level & 0xff, target, message);
        }, undefined),
        now_ms: guard(() => this._clock.nowMs(), 0),
        // No CSPRNG (no WebCrypto, or an Rng adapter that throws): the guard writes nothing, so the core finds
        // its canary untouched and answers `Rng.fill` unavailable (ADR-049) instead of using zeros. Never a fallback.
        random: guard((ptr: number, len: number) => {
          const start = ptr >>> 0;
          (this._rng ??= cryptoRng()).fill(this._bytes().subarray(start, start + (len >>> 0)));
        }, undefined),
      },
    };
  }

  private _portCall(call: PortCallPayload): number {
    const handler = this._handler;
    if (handler === null) return 2;
    const outcome: PortOutcome = handler.portCall(call);
    switch (outcome.kind) {
      case "sync":
        // The reply must be in the core before `port_call` returns 0.
        this._invoke(outcome.reply, (e, ptr, len) => e.undra_port_reply(ptr, len));
        return 0;
      case "async":
        return 1;
      case "unavailable":
        return 2;
    }
  }

  private _schedulePoll(): void {
    if (this._pollScheduled || this._closed) return;
    this._pollScheduled = true;
    queueMicrotask(() => {
      this._pollScheduled = false;
      if (this._closed || this._dead !== null) return;
      try {
        this._run((e) => e.undra_poll());
      } catch (error) {
        this._report(error);
      }
    });
  }

  private _timerFired(timerId: number): void {
    if (this._closed || this._dead !== null) return;
    try {
      this._run((e) => e.undra_timer_fired(timerId));
    } catch (error) {
      this._report(error);
    }
  }
}
