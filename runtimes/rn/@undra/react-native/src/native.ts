/**
 * The JSI object the native module installs as `globalThis.__undraNative` (ADR-038, decisions 1
 * and 5): one host function per C ABI entry (docs/SPEC.md section 6). Bytes go in as
 * `(ArrayBuffer, byteOffset, byteLength)` and are borrowed by the core for the call; bytes come back
 * as `ArrayBuffer`s that JavaScript owns. Handles are passed as two unsigned 32-bit halves.
 *
 * `NativeTransport` is the only intended caller. Tests pass a fake with the same shape.
 */
export interface UndraNativeModule {
  /** `undra_abi_version()`. */
  abiVersion(): number;
  /** `undra_schema_hash()`; works before `start`. */
  schemaHash(): bigint;
  /** `undra_schema_json()`: the canonical schema JSON; works before `start`. */
  schemaJson(): string;
  /**
   * Registers the ports, then `undra_init`s the core with an encoded `RuntimeConfig`. `ports` are
   * the schema's non-event port ids; `syncMethods` holds `(portId, methodId)` pairs of its
   * synchronous methods. Returns 0, an `undra_init` code (1 to 5) or a host code
   * (`0x100` another core of this process is running, `0x101` ABI mismatch, `0x102` already started).
   */
  start(config: ArrayBuffer, byteOffset: number, byteLength: number, ports: readonly number[], syncMethods: readonly number[]): number;
  /** `undra_shutdown()`; throws when called from inside a JavaScript sync port. */
  shutdown(): void;
  /**
   * `undra_call`: 0 accepted (the reply arrives in the inbox), 5 refused. Every entry that reaches
   * the core answers as the C ABI does with no core (5, ignored, `undefined`, 6) without calling it
   * when this runtime's core is not running: the process's core may be another runtime's.
   */
  call(buffer: ArrayBuffer, byteOffset: number, byteLength: number): number;
  /** `undra_call_sync`: the `Reply` payload; `undefined` when this runtime's core is not running. */
  callSync(buffer: ArrayBuffer, byteOffset: number, byteLength: number): ArrayBuffer | undefined;
  /** `undra_cancel`. */
  cancel(callId: number): void;
  /** `undra_stream_credit`. */
  streamCredit(callId: number, credit: number): void;
  /** `undra_observe`. */
  observe(handleLo: number, handleHi: number, signalId: number, on: boolean): void;
  /** `undra_release`. */
  release(handleLo: number, handleHi: number): void;
  /** `undra_port_reply`. */
  portReply(buffer: ArrayBuffer, byteOffset: number, byteLength: number): void;
  /** `undra_event`. */
  event(portId: number, methodId: number, buffer: ArrayBuffer, byteOffset: number, byteLength: number): void;
  /** `undra_timer_fired`. */
  timerFired(timerId: number): void;
  /** `undra_snapshot`: a `Snapshot` payload; `undefined` when this runtime's core is not running. */
  snapshot(): ArrayBuffer | undefined;
  /** `undra_restore`: 0 or a `restore_code` (6, unavailable, when this runtime's core is not running). */
  restore(buffer: ArrayBuffer, byteOffset: number, byteLength: number): number;
  /** `undra_stats_json`. */
  statsJson(): string;
  /** The native host's own counters. */
  hostCounters(): NativeHostCounters;
  /** Arms the vsync source: `frame` is called at the next display frame. `false` when there is none. */
  requestFrame(): boolean;

  /** Set by the transport: receives each inbox batch (`kind u8, len u32, payload` records). */
  sink?: ((batch: ArrayBuffer) => void) | undefined;
  /**
   * Set by the transport: answers a synchronous port method on the JS thread, from inside a core
   * callback. Returns a complete `PortReply` payload, or 2 (unavailable).
   */
  portSync?: ((portId: number, methodId: number, portCallId: number, args: ArrayBuffer) => ArrayBuffer | number) | undefined;
  /** Set by the frame scheduler: called at the display frame `requestFrame` asked for. */
  frame?: (() => void) | undefined;
}

/** What the native host counted (`hostCounters()`). */
export interface NativeHostCounters {
  /** Inbox records queued. */
  readonly records: number;
  /** Payload bytes those records carried (the one copy each). */
  readonly bytes: number;
  /** Drains posted to the JS thread from other threads. */
  readonly wakes: number;
  /** Records lost to memory exhaustion. */
  readonly dropped: number;
  /** Clock, Rng and Log calls answered natively. */
  readonly nativePortCalls: number;
  /** JavaScript sync port calls answered on the JS thread. */
  readonly jsSyncPortCalls: number;
  /** JavaScript sync port calls made from a core thread, answered "unavailable". */
  readonly unavailableSyncPortCalls: number;
}

/** Inbox record kinds: the envelope kinds of docs/SPEC.md section 3.2 for what each carries. */
export const RecordKind = {
  /** A `Reply` payload. */
  Reply: 2,
  /** A `ChangeSet` payload. */
  ChangeSet: 3,
  /** `port_id u32, method_id u32, port_call_id u32, args`. */
  PortCall: 4,
  /** A `StreamItem` payload. */
  StreamItem: 8,
  /** `level u8, target String, message String`. */
  Log: 13,
} as const;

/** Start codes of the native host beyond `undra_init`'s. */
export const NativeStartCode = {
  /** Another core of this process is running (one core per process). */
  Busy: 0x100,
  /** The linked core speaks another C ABI version. */
  AbiMismatch: 0x101,
  /** `start` was called twice. */
  AlreadyStarted: 0x102,
} as const;

/** What each nonzero start code means, for error messages. */
export function startFailure(code: number): string {
  switch (code) {
    case 1:
      return "another embedder already initialised the core in this process (undra_init code 1)";
    case 2:
      return "the core refused the RuntimeConfig (undra_init code 2)";
    case 3:
      return "undra_init was given a null callback (code 3)";
    case 4:
      return "the core could not start its thread (undra_init code 4)";
    case 5:
      return "undra_init panicked (code 5)";
    case NativeStartCode.Busy:
      return "another Undra core is running in this process; there is one per process";
    case NativeStartCode.AbiMismatch:
      return "the linked core speaks another C ABI version";
    case NativeStartCode.AlreadyStarted:
      return "this core is already started";
    default:
      return `undra_init failed with code ${code}`;
  }
}

/** The schema's ports as the native host registers them. */
export interface PortPlan {
  /** Every port that is not an event port. */
  readonly ports: number[];
  /** `(portId, methodId)` pairs of the synchronous methods of those ports. */
  readonly syncMethods: number[];
}

interface SchemaPorts {
  readonly ports?: ReadonlyArray<{
    readonly port_id?: unknown;
    readonly kind?: unknown;
    readonly methods?: ReadonlyArray<{ readonly method_id?: unknown; readonly is_async?: unknown }>;
  }>;
}

/** Reads the port plan from the core's schema JSON (`undra_schema_json`). */
export function portPlan(schemaJson: string): PortPlan {
  const schema = JSON.parse(schemaJson) as SchemaPorts;
  const ports: number[] = [];
  const syncMethods: number[] = [];
  for (const port of schema.ports ?? []) {
    if (typeof port.port_id !== "number" || port.kind === "event") continue;
    ports.push(port.port_id);
    for (const method of port.methods ?? []) {
      if (typeof method.method_id === "number" && method.is_async !== true) {
        syncMethods.push(port.port_id, method.method_id);
      }
    }
  }
  return { ports, syncMethods };
}
