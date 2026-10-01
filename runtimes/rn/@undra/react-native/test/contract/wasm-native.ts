import type { ClockAdapter } from "@undra/runtime";
import { type NativeHostCounters, RecordKind, type UndraNativeModule } from "../../src/native.js";

/*
 * A stand-in for the native module on Node, over the playground core's WebAssembly build, for the
 * contract scenarios (vitest.contract.config.ts). It has the module's JavaScript-visible contract
 * (src/native.ts, ADR-038 decisions 4, 5 and 7): one inbox of `kind u8, len u32, payload` records;
 * what the core says while a host function runs is delivered before that function returns, what it
 * says on its own (here: in `undra_poll` and timers, the wasm core's "core thread") arrives in a
 * later batch; Clock, Rng and Log are answered by the core's own bindings (native in the real
 * module), Timer is the core's; async port methods are queued; a synchronous JavaScript port runs
 * only inside a host function. What it cannot reproduce is native: a panic traps a wasm core instead
 * of being contained (S17 is checked on the device), and there are no real threads.
 */

const CLOCK = 0xcd99c48e;
const RNG = 0x25135bf5;
const LOG = 0x575ff24a;
const TIMER = 0xc2cdd9;

interface CoreExports {
  readonly memory: WebAssembly.Memory;
  undra_alloc(len: number): number;
  undra_free(ptr: number, len: number): void;
  undra_abi_version(): number;
  undra_schema_hash(): bigint;
  undra_schema_json(): number;
  undra_init(ptr: number, len: number): number;
  undra_call(ptr: number, len: number): number;
  undra_call_sync(ptr: number, len: number): number;
  undra_cancel(callId: number): void;
  undra_stream_credit(callId: number, credit: number): void;
  undra_observe(lo: number, hi: number, signalId: number, on: number): void;
  undra_release(lo: number, hi: number): void;
  undra_port_reply(ptr: number, len: number): void;
  undra_event(portId: number, methodId: number, ptr: number, len: number): void;
  undra_timer_fired(timerId: number): void;
  undra_poll(): void;
  undra_buf_free(bufPtr: number): void;
  undra_stats_json(): number;
  undra_snapshot(): number;
  undra_restore(ptr: number, len: number): number;
  _initialize?(): void;
}

/** Options of {@link WasmNative}. */
export interface WasmNativeOptions {
  /** The clock behind the core's Clock port (native in the real module; manual in the scenarios). */
  readonly clock?: ClockAdapter;
  /** The core's namespace, which the real module reads from the core's table. Default `playground_core`. */
  readonly namespace?: string;
}

/**
 * The `abi_version` the real module reports: its core's `UndraApi` table's (2, ADR-044). The wasm core
 * underneath keeps the wasm ABI's own version (`undra_abi_version()` is 1, docs/SPEC.md section 7),
 * which is not what the module stands in for.
 */
const NATIVE_ABI_VERSION = 2;

/** The native module's contract over a wasm core. */
export class WasmNative implements UndraNativeModule {
  sink?: ((batch: ArrayBuffer) => void) | undefined;
  portSync?: ((portId: number, methodId: number, portCallId: number, args: ArrayBuffer) => ArrayBuffer | number) | undefined;
  frame?: (() => void) | undefined;

  /** The instance, for what the scenarios read from the wasm exports. */
  instance!: WebAssembly.Instance;
  /** The core's namespace (the real module's comes from the table). */
  readonly namespace: string;

  readonly #clock: ClockAdapter | undefined;
  #e!: CoreExports;
  #inbox: Array<{ kind: number; payload: Uint8Array }> = [];
  #depth = 0;
  #draining = false;
  #drainPending = false;
  #pollPending = false;
  #running = false;
  #closed = false;
  #ports = new Set<number>();
  #sync = new Set<string>();
  #warned = new Set<number>();
  #timers = new Set<ReturnType<typeof setTimeout>>();
  #records = 0;
  #bytes = 0;
  #wakes = 0;
  #jsSync = 0;
  #unavailableSync = 0;

  private constructor(options: WasmNativeOptions) {
    this.#clock = options.clock;
    this.namespace = options.namespace ?? "playground_core";
  }

  /** Instantiates the core (`_initialize` included); `start` runs `undra_init`. */
  static async load(module: WebAssembly.Module, options: WasmNativeOptions = {}): Promise<WasmNative> {
    const native = new WasmNative(options);
    native.instance = await WebAssembly.instantiate(module, native.#imports());
    native.#e = native.instance.exports as unknown as CoreExports;
    native.#e._initialize?.();
    return native;
  }

  // ----- the inbox -------------------------------------------------------------------

  #queue(kind: number, payload: Uint8Array): void {
    if (!this.#running) return;
    this.#inbox.push({ kind, payload });
    this.#records++;
    this.#bytes += payload.length;
    // On "the JS thread inside a host function" the host function drains before it returns.
    if (this.#depth === 0 && !this.#drainPending) {
      this.#drainPending = true;
      this.#wakes++;
      setImmediate(() => {
        this.#drain();
      });
    }
  }

  #drain(): void {
    this.#drainPending = false;
    if (this.#draining || this.#depth > 0) return;
    this.#draining = true;
    try {
      for (let round = 0; round < 1000 && this.#inbox.length > 0; round++) {
        const records = this.#inbox.splice(0);
        let size = 0;
        for (const r of records) size += 5 + r.payload.length;
        const batch = new Uint8Array(size);
        const view = new DataView(batch.buffer);
        let at = 0;
        for (const r of records) {
          batch[at] = r.kind;
          view.setUint32(at + 1, r.payload.length, true);
          batch.set(r.payload, at + 5);
          at += 5 + r.payload.length;
        }
        if (this.sink === undefined) return;
        try {
          this.sink(batch.buffer);
        } catch {
          // The sink reports its own failures.
        }
      }
    } finally {
      this.#draining = false;
    }
  }

  /** A host function: runs `fn` in the core, then drains what it queued (outermost only). */
  #enter<T>(fn: (e: CoreExports) => T): T {
    if (this.#closed) throw new Error("the native core is shut down");
    this.#depth++;
    try {
      return fn(this.#e);
    } finally {
      this.#depth--;
      if (this.#depth === 0) this.#drain();
    }
  }

  // ----- memory ----------------------------------------------------------------------

  #copyOut(ptr: number, len: number): Uint8Array {
    return new Uint8Array(this.#e.memory.buffer, ptr >>> 0, len >>> 0).slice();
  }

  #takeBuf(bufPtr: number): Uint8Array {
    const e = this.#e;
    try {
      const view = new DataView(e.memory.buffer);
      return this.#copyOut(view.getUint32(bufPtr, true), view.getUint32(bufPtr + 4, true));
    } finally {
      e.undra_buf_free(bufPtr);
    }
  }

  #withBytes<T>(bytes: Uint8Array, run: (ptr: number, len: number) => T): T {
    const e = this.#e;
    const size = Math.max(1, bytes.length);
    const ptr = e.undra_alloc(size);
    try {
      new Uint8Array(e.memory.buffer, ptr, bytes.length).set(bytes);
      return run(ptr, bytes.length);
    } finally {
      e.undra_free(ptr, size);
    }
  }

  // ----- the core's imports -----------------------------------------------------------

  #imports(): WebAssembly.Imports {
    return {
      undra: {
        reply: (_callId: number, ptr: number, len: number) => {
          this.#queue(RecordKind.Reply, this.#copyOut(ptr, len));
        },
        changeset: (ptr: number, len: number) => {
          this.#queue(RecordKind.ChangeSet, this.#copyOut(ptr, len));
        },
        stream: (_callId: number, ptr: number, len: number) => {
          this.#queue(RecordKind.StreamItem, this.#copyOut(ptr, len));
        },
        port_call: (portId: number, methodId: number, portCallId: number, ptr: number, len: number) =>
          this.#portCall(portId >>> 0, methodId >>> 0, portCallId >>> 0, this.#copyOut(ptr, len)),
        schedule: () => {
          if (this.#pollPending || this.#closed) return;
          this.#pollPending = true;
          // The "core thread": a turn of its own, not inside any host function.
          setImmediate(() => {
            this.#pollPending = false;
            if (!this.#closed) this.#e.undra_poll();
          });
        },
        timer_set: (timerId: number, lo: number, hi: number) => {
          const delay = (hi >>> 0) * 0x1_0000_0000 + (lo >>> 0);
          const timer = setTimeout(() => {
            this.#timers.delete(timer);
            if (!this.#closed) this.#e.undra_timer_fired(timerId >>> 0);
          }, Math.min(delay, 2_147_483_647));
          this.#timers.add(timer);
        },
        log: (level: number, ptr: number, len: number) => {
          const body = this.#copyOut(ptr, len);
          const record = new Uint8Array(1 + body.length);
          record[0] = level & 0xff;
          record.set(body, 1);
          this.#queue(RecordKind.Log, record);
        },
        now_ms: () => (this.#clock ? this.#clock.nowMs() : Date.now()),
        random: (ptr: number, len: number) => {
          globalThis.crypto.getRandomValues(new Uint8Array(this.#e.memory.buffer, ptr >>> 0, len >>> 0));
        },
      },
    };
  }

  /** `port_call`: the routing of `Host::answerNative` / `Host::answerJs` (cpp/UndraHost.cpp). */
  #portCall(portId: number, methodId: number, portCallId: number, args: Uint8Array): number {
    // Clock, Rng and Log: the core's own bindings answer an "unavailable" (native in the module).
    if (portId === CLOCK || portId === RNG || portId === LOG || portId === TIMER) return 2;
    if (!this.#ports.has(portId)) return 2;
    if (this.#sync.has(`${portId}:${methodId}`)) {
      if (this.#depth > 0 && this.portSync !== undefined) {
        this.#jsSync++;
        const answer = this.portSync(portId, methodId, portCallId, args.slice().buffer);
        if (typeof answer === "number") return 2;
        // The wasm ABI wants the reply in the core before `port_call` returns 0.
        this.#withBytes(new Uint8Array(answer), (ptr, len) => this.#e.undra_port_reply(ptr, len));
        return 0;
      }
      this.#unavailableSync++;
      if (!this.#warned.has(portId)) this.#warned.add(portId);
      return 2;
    }
    const record = new Uint8Array(12 + args.length);
    const view = new DataView(record.buffer);
    view.setUint32(0, portId, true);
    view.setUint32(4, methodId, true);
    view.setUint32(8, portCallId, true);
    record.set(args, 12);
    this.#queue(RecordKind.PortCall, record);
    return 1;
  }

  // ----- UndraNativeModule ------------------------------------------------------------

  abiVersion(): number {
    return NATIVE_ABI_VERSION;
  }
  schemaHash(): bigint {
    return BigInt.asUintN(64, this.#e.undra_schema_hash());
  }
  schemaJson(): string {
    return new TextDecoder().decode(this.#takeBuf(this.#e.undra_schema_json()));
  }
  start(config: ArrayBuffer, byteOffset: number, byteLength: number, ports: readonly number[], syncMethods: readonly number[]): number {
    if (this.#running) return 0x102;
    this.#ports = new Set(ports);
    this.#sync = new Set<string>();
    for (let i = 0; i + 1 < syncMethods.length; i += 2) this.#sync.add(`${syncMethods[i]}:${syncMethods[i + 1]}`);
    this.#running = true;
    const code = this.#enter((e) => this.#withBytes(new Uint8Array(config, byteOffset, byteLength), (ptr, len) => e.undra_init(ptr, len)));
    if (code !== 0) this.#running = false;
    return code;
  }
  shutdown(): void {
    this.#running = false;
    this.#closed = true;
    for (const timer of this.#timers) clearTimeout(timer);
    this.#timers.clear();
    this.#inbox = [];
  }
  call(buffer: ArrayBuffer, byteOffset: number, byteLength: number): number {
    return this.#enter((e) => this.#withBytes(new Uint8Array(buffer, byteOffset, byteLength), (ptr, len) => e.undra_call(ptr, len)));
  }
  callSync(buffer: ArrayBuffer, byteOffset: number, byteLength: number): ArrayBuffer {
    return this.#enter((e) =>
      this.#withBytes(new Uint8Array(buffer, byteOffset, byteLength), (ptr, len) => this.#takeBuf(e.undra_call_sync(ptr, len))).buffer,
    ) as ArrayBuffer;
  }
  cancel(callId: number): void {
    this.#enter((e) => e.undra_cancel(callId));
  }
  streamCredit(callId: number, credit: number): void {
    this.#enter((e) => e.undra_stream_credit(callId, credit));
  }
  observe(handleLo: number, handleHi: number, signalId: number, on: boolean): void {
    this.#enter((e) => e.undra_observe(handleLo | 0, handleHi | 0, signalId | 0, on ? 1 : 0));
  }
  release(handleLo: number, handleHi: number): void {
    this.#enter((e) => e.undra_release(handleLo | 0, handleHi | 0));
  }
  portReply(buffer: ArrayBuffer, byteOffset: number, byteLength: number): void {
    this.#enter((e) => this.#withBytes(new Uint8Array(buffer, byteOffset, byteLength), (ptr, len) => e.undra_port_reply(ptr, len)));
  }
  event(portId: number, methodId: number, buffer: ArrayBuffer, byteOffset: number, byteLength: number): void {
    this.#enter((e) => this.#withBytes(new Uint8Array(buffer, byteOffset, byteLength), (ptr, len) => e.undra_event(portId, methodId, ptr, len)));
  }
  timerFired(timerId: number): void {
    this.#enter((e) => e.undra_timer_fired(timerId));
  }
  snapshot(): ArrayBuffer {
    return this.#takeBuf(this.#e.undra_snapshot()).buffer as ArrayBuffer;
  }
  restore(buffer: ArrayBuffer, byteOffset: number, byteLength: number): number {
    return this.#enter((e) => this.#withBytes(new Uint8Array(buffer, byteOffset, byteLength), (ptr, len) => e.undra_restore(ptr, len)));
  }
  statsJson(): string {
    return new TextDecoder().decode(this.#takeBuf(this.#e.undra_stats_json()));
  }
  hostCounters(): NativeHostCounters {
    return {
      records: this.#records,
      bytes: this.#bytes,
      wakes: this.#wakes,
      dropped: 0,
      nativePortCalls: 0,
      jsSyncPortCalls: this.#jsSync,
      unavailableSyncPortCalls: this.#unavailableSync,
    };
  }
  requestFrame(): boolean {
    return false; // no vsync on Node: the scheduler's timer fallback
  }
}
