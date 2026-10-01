import {
  type HelloPayload,
  Kind,
  PortStatus,
  ReplyStatus,
  type Transport,
  type TransportHandler,
  UndraReader,
  UndraReplyError,
  UndraSchemaMismatchError,
  UndraTransportError,
  UndraWriter,
  codecs,
  decodeCancel,
  decodeEvent,
  decodeObserve,
  decodeRelease,
  decodeStreamCredit,
  decodeTimerFired,
  encodePortReply,
  encodeValue,
  splitHandle,
} from "@undra/runtime";
import { type NativeHostCounters, RecordKind, type UndraNativeModule, portPlan, startFailure } from "./native.js";

/** The C ABI version this transport speaks (`undra_abi_version`, docs/SPEC.md section 6). */
const ABI_VERSION = 1;
/** Bytes of an inbox record header: `kind u8, len u32`. */
const RECORD_HEADER = 5;
const NO_BYTES = new Uint8Array(0);

/** Options of {@link NativeTransport}. */
export interface NativeTransportOptions {
  /** The installed native module (`globalThis.__undraNative`), or a stand-in for tests. */
  readonly native: UndraNativeModule;
  /** The schema hash of the generated bindings; a core built from another schema is refused before `undra_init`. */
  readonly expectedSchemaHash: bigint;
  /** Platform name for the core's `RuntimeConfig`. Default `"react-native"`. */
  readonly platform?: string;
  /** Run the core in `"dev"` mode (devtools log records, docs/SPEC.md section 5.10) instead of `"inproc"`. */
  readonly devtools?: boolean;
  /** Log records below this level (0 trace .. 5 fatal) are not produced by the core. Default 2. */
  readonly logLevel?: number;
  /** Receives failures that have no caller: a malformed inbox record, a handler that threw. */
  readonly onError?: (error: unknown) => void;
}

/** The `ArrayBuffer` behind a view, for the native module (which only takes `ArrayBuffer`s). */
function bufferOf(bytes: Uint8Array): ArrayBuffer {
  return bytes.buffer as ArrayBuffer;
}

/**
 * The module answered as if there were no core: this runtime's core was stopped under the transport
 * (a reloaded runtime of the same process started its own, ADR-038 decision 11).
 */
function notRunning(): UndraTransportError {
  return new UndraTransportError(
    "closed",
    "the native core of this JavaScript runtime is not running (another runtime of this process started one)",
  );
}

/** `bytes` as an `ArrayBuffer` of exactly its length (no copy when the view is the whole buffer). */
function exactBuffer(bytes: Uint8Array): ArrayBuffer {
  if (bytes.byteOffset === 0 && bytes.byteLength === bytes.buffer.byteLength) return bytes.buffer as ArrayBuffer;
  return bytes.slice().buffer;
}

/**
 * The React Native transport (ADR-038): the TypeScript runtime's {@link Transport} over the
 * native core, reached through the JSI object of `@undra/react-native`'s TurboModule.
 *
 * Like `wasm-main` it is `synchronous`: every message runs the core on the JS thread, and before
 * the native call returns to JavaScript the module hands over everything the core queued
 * meanwhile (its inbox), so the reply of a synchronous method and the change-sets of a call or of
 * `observe` reach the handler before {@link NativeTransport.send} returns. What the core produces
 * on its own threads (async replies, timers, streams, port calls) arrives in batches posted to the
 * JS thread. One inbox for every thread keeps the core's commit order.
 *
 * Ports: `Clock`, `Rng` and `Log` are answered natively (log records are forwarded to the
 * handler), `Timer` is the core's own; async port methods are answered by the handler and sent
 * back with `PortReply`; a synchronous port method implemented in JavaScript is answered only when
 * the core calls it on the JS thread (docs/REACT_NATIVE.md, limits).
 */
export class NativeTransport implements Transport {
  readonly mode = "native";
  readonly synchronous = true;

  readonly #native: UndraNativeModule;
  readonly #options: NativeTransportOptions;
  #handler: TransportHandler | null = null;
  #started = false;
  #closed = false;
  /** The `sink` and `portSync` this transport installed on the module, to remove only its own. */
  readonly #sink = (batch: ArrayBuffer): void => {
    this.#deliver(batch);
  };
  readonly #portSyncFn = (portId: number, methodId: number, portCallId: number, args: ArrayBuffer): ArrayBuffer | number =>
    this.#portSync(portId, methodId, portCallId, args);

  /** @param options See {@link NativeTransportOptions}. */
  constructor(options: NativeTransportOptions) {
    this.#native = options.native;
    this.#options = options;
  }

  start(handler: TransportHandler): Promise<HelloPayload> {
    try {
      return Promise.resolve(this.#start(handler));
    } catch (error) {
      return Promise.reject(error);
    }
  }

  #start(handler: TransportHandler): HelloPayload {
    if (this.#closed) throw new UndraTransportError("closed", "the transport is closed");
    if (this.#started) throw new UndraTransportError("handshake", "the transport is already started");
    const native = this.#native;
    const abi = native.abiVersion();
    if (abi !== ABI_VERSION) {
      throw new UndraTransportError("handshake", `the core speaks C ABI ${abi}, this runtime speaks ${ABI_VERSION}`);
    }
    // The schema check comes before `undra_init` (docs/SPEC.md section 11), as on Swift: a core
    // built from another schema is never started.
    const schemaHash = BigInt.asUintN(64, BigInt(native.schemaHash()));
    if (schemaHash !== this.#options.expectedSchemaHash) {
      throw new UndraSchemaMismatchError(this.#options.expectedSchemaHash, schemaHash);
    }
    const plan = portPlan(native.schemaJson());
    const mode = this.#options.devtools === true ? "dev" : "inproc";
    const config = new UndraWriter(48);
    config.writeStr(this.#options.platform ?? "react-native");
    config.writeStr(mode);
    config.writeU8(1); // core_threads: the undra-core thread
    config.writeU8(0); // blocking_threads: the runtime's default
    config.writeU8(this.#options.logLevel ?? 2);
    const bytes = config.finish();
    this.#handler = handler;
    // Installed before `start`, which delivers what `undra_init` produced. A start that fails
    // (another core of this process is running) puts back what was there: the running core's
    // transport keeps its sink, or every batch of that core would be dropped from now on.
    const previous = { sink: native.sink, portSync: native.portSync };
    native.sink = this.#sink;
    native.portSync = this.#portSyncFn;
    let code: number;
    try {
      code = native.start(bufferOf(bytes), bytes.byteOffset, bytes.byteLength, plan.ports, plan.syncMethods);
    } catch (error) {
      this.#detach(previous);
      throw error;
    }
    if (code !== 0) {
      this.#detach(previous);
      throw new UndraTransportError("handshake", `the native core did not start: ${startFailure(code)}`);
    }
    this.#started = true;
    return { undraVersion: `c-abi-${abi}`, schemaHash, platform: this.#options.platform ?? "react-native", mode };
  }

  send(kind: Kind, payload: Uint8Array): void {
    const native = this.#live();
    switch (kind) {
      case Kind.Call: {
        const code = native.call(bufferOf(payload), payload.byteOffset, payload.byteLength);
        if (code !== 0) {
          throw new UndraReplyError(
            ReplyStatus.BadRequest,
            encodeValue(codecs.string, `the core refused the call (undra_call returned ${code})`),
          );
        }
        return;
      }
      case Kind.Cancel:
        native.cancel(decodeCancel(payload).callId);
        return;
      case Kind.StreamCredit: {
        const { callId, credit } = decodeStreamCredit(payload);
        native.streamCredit(callId, credit);
        return;
      }
      case Kind.Observe: {
        const { handle, signalId, on } = decodeObserve(payload);
        const { lo, hi } = splitHandle(handle);
        native.observe(lo, hi, signalId, on);
        return;
      }
      case Kind.Release: {
        const { lo, hi } = splitHandle(decodeRelease(payload).handle);
        native.release(lo, hi);
        return;
      }
      case Kind.Event: {
        const event = decodeEvent(payload);
        native.event(event.portId, event.methodId, bufferOf(event.payload), event.payload.byteOffset, event.payload.byteLength);
        return;
      }
      case Kind.PortReply:
        native.portReply(bufferOf(payload), payload.byteOffset, payload.byteLength);
        return;
      case Kind.TimerFired:
        native.timerFired(decodeTimerFired(payload).timerId);
        return;
      case Kind.Restore: {
        const code = native.restore(bufferOf(payload), payload.byteOffset, payload.byteLength);
        if (code !== 0) throw new UndraTransportError("protocol", `undra_restore failed with code ${code}`);
        return;
      }
      default:
        throw new UndraTransportError("protocol", `cannot send a ${Kind[kind] ?? String(kind)} message to a native core`);
    }
  }

  callSync(payload: Uint8Array): Uint8Array {
    const reply = this.#live().callSync(bufferOf(payload), payload.byteOffset, payload.byteLength);
    if (reply === undefined) throw notRunning();
    return new Uint8Array(reply);
  }

  stats(): Promise<string | null> {
    if (this.#closed || !this.#started) return Promise.resolve(null);
    try {
      return Promise.resolve(this.#native.statsJson());
    } catch (error) {
      return Promise.reject(error);
    }
  }

  /** Every store as a `Snapshot` payload (docs/SPEC.md section 5.9), to hand back with `Kind.Restore`. */
  snapshot(): Uint8Array {
    const snapshot = this.#live().snapshot();
    if (snapshot === undefined) throw notRunning();
    return new Uint8Array(snapshot);
  }

  /** The native host's counters: inbox records and bytes, wakes, native and JavaScript port calls. */
  counters(): NativeHostCounters {
    return this.#native.hostCounters();
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#detach();
    if (this.#started) {
      this.#started = false;
      try {
        this.#native.shutdown();
      } catch (error) {
        this.#report(error);
      }
    }
  }

  // ----- internals -------------------------------------------------------------------

  #live(): UndraNativeModule {
    if (this.#closed) throw new UndraTransportError("closed", "the core is closed");
    if (!this.#started) throw new UndraTransportError("closed", "the core is not started");
    return this.#native;
  }

  /**
   * Stops receiving from the module. Only this transport's own `sink` and `portSync` are removed
   * (a transport that failed to start, or one closed late, must not take a running core's), and
   * `restore` is what goes back in their place.
   */
  #detach(restore: { sink?: UndraNativeModule["sink"]; portSync?: UndraNativeModule["portSync"] } = {}): void {
    this.#handler = null;
    const native = this.#native;
    if (native.sink === this.#sink) native.sink = restore.sink;
    if (native.portSync === this.#portSyncFn) native.portSync = restore.portSync;
  }

  #report(error: unknown): void {
    try {
      this.#options.onError?.(error);
    } catch {
      // The reporter failed; nothing more to do.
    }
  }

  /** One inbox batch: `kind u8, len u32, payload` records, delivered in order. Never throws. */
  #deliver(batch: ArrayBuffer): void {
    const bytes = new Uint8Array(batch);
    const view = new DataView(batch);
    let at = 0;
    while (at < bytes.length) {
      const handler = this.#handler;
      if (handler === null) return;
      if (bytes.length - at < RECORD_HEADER) {
        this.#report(new UndraTransportError("protocol", "the native inbox ended inside a record header"));
        return;
      }
      const kind = bytes[at] as number;
      const len = view.getUint32(at + 1, true);
      const start = at + RECORD_HEADER;
      if (bytes.length - start < len) {
        this.#report(new UndraTransportError("protocol", "the native inbox ended inside a record"));
        return;
      }
      const payload = bytes.subarray(start, start + len);
      at = start + len;
      try {
        switch (kind) {
          case RecordKind.Reply:
            handler.reply(payload);
            break;
          case RecordKind.ChangeSet:
            handler.changeSet(payload);
            break;
          case RecordKind.StreamItem:
            handler.streamItem(payload);
            break;
          case RecordKind.PortCall:
            this.#portCall(handler, payload);
            break;
          case RecordKind.Log:
            this.#log(handler, payload);
            break;
          default:
            this.#report(new UndraTransportError("protocol", `unknown native inbox record kind ${kind}`));
        }
      } catch (error) {
        this.#report(error);
      }
    }
  }

  /** An async port method: the handler answers now, later, or not at all ("unavailable"). */
  #portCall(handler: TransportHandler, payload: Uint8Array): void {
    if (payload.length < 12) {
      this.#report(new UndraTransportError("protocol", "a native PortCall record is truncated"));
      return;
    }
    const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
    const portId = view.getUint32(0, true);
    const methodId = view.getUint32(4, true);
    const portCallId = view.getUint32(8, true);
    const outcome = handler.portCall({ portId, methodId, portCallId, args: payload.subarray(12) });
    switch (outcome.kind) {
      case "sync":
        this.#portReply(outcome.reply);
        return;
      case "async":
        return; // UndraCore sends the PortReply when the promise settles
      case "unavailable":
        this.#portReply(encodePortReply({ portCallId, status: PortStatus.Unavailable, body: NO_BYTES }));
        return;
    }
  }

  #portReply(reply: Uint8Array): void {
    if (this.#closed) return;
    try {
      this.#native.portReply(bufferOf(reply), reply.byteOffset, reply.byteLength);
    } catch (error) {
      this.#report(error);
    }
  }

  /** A synchronous port method, called by the native module on the JS thread inside a core callback. */
  #portSync(portId: number, methodId: number, portCallId: number, args: ArrayBuffer): ArrayBuffer | number {
    const handler = this.#handler;
    if (handler === null) return 2;
    try {
      const outcome = handler.portCall({ portId, methodId, portCallId, args: new Uint8Array(args) });
      return outcome.kind === "sync" ? exactBuffer(outcome.reply) : 2;
    } catch (error) {
      this.#report(error);
      return 2;
    }
  }

  /** A log record: the core's (the native Log port forwards it) or the native host's own. */
  #log(handler: TransportHandler, payload: Uint8Array): void {
    const r = new UndraReader(payload);
    const level = r.readU8();
    const target = r.readStr();
    const message = r.readStr();
    handler.log(level, target, message);
  }
}
