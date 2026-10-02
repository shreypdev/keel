import type { PortImpl } from "../port.js";
import type { RestartResult } from "../recovery.js";
import type { HelloPayload, Kind, PortCallPayload } from "../wire/index.js";

/*
 * The seam between `UndraCore` and the thing that runs the Undra core (docs/SPEC.md
 * sections 3.2, 7 and 11). A transport moves *logical envelopes*: a `Kind` and
 * its payload bytes (SPEC 3.3 to 3.8). The wasm transports map them onto the
 * ABI exports; the worker and WebSocket transports frame them with the 23-byte
 * envelope header. `UndraCore` never sees the difference.
 */

/** How the host answers a `PortCall` (SPEC 6.3): now, later, or never. */
export type PortOutcome =
  /** The reply is ready: `reply` is a complete `PortReply` payload (`port_call_id u32, status u8, body`). */
  | { readonly kind: "sync"; readonly reply: Uint8Array }
  /** The host will send a `PortReply` envelope later with `Transport.send`. */
  | { readonly kind: "async" }
  /** Nobody implements the port (or the method); the core sees `PortError::Unavailable`. */
  | { readonly kind: "unavailable" };

/**
 * What a transport calls when the core says something. `UndraCore` implements
 * this. Handlers run inside the core's callbacks (the core lock may be held),
 * so none of them may call back into the transport synchronously except
 * `send(Kind.PortReply, ..)` from `portCall`.
 */
export interface TransportHandler {
  /** A `Reply` payload (SPEC 3.4). */
  reply(payload: Uint8Array): void;
  /** A `ChangeSet` payload (SPEC 3.5). */
  changeSet(payload: Uint8Array): void;
  /** A `StreamItem` payload (SPEC 3.7). */
  streamItem(payload: Uint8Array): void;
  /** The core calls a platform port; answer with a {@link PortOutcome}. */
  portCall(call: PortCallPayload): PortOutcome;
  /** A log record from the core (`level` as the `Log` port defines it: 0 trace .. 5 fatal). */
  log(level: number, target: string, message: string): void;
  /** The channel is gone (connection closed, worker died, wasm trapped). Not called after the host closed the transport itself. */
  closed(error: Error): void;
  /**
   * Only for a transport that reconnects by itself (`remote`, ADR-051): the channel dropped (or a
   * retry failed) and the transport will try again. `attempt` counts from 1, and attempt 1 is the
   * loss itself: whatever was in flight has failed for good. `closed` is called only when the
   * transport gives up.
   */
  reconnecting?(attempt: number, error: Error): void;
  /** Only for a transport that reconnects: the channel is back and the core's `Hello` checked. The host observes its stores again. */
  reconnected?(hello: HelloPayload): void;
  /** Only for a transport that reconnects: whether the host holds objects it expects the core to still have (it asks the server to resume them). */
  holdsObjects?(): boolean;
  /**
   * The ports the host serves. A transport whose core runs elsewhere (`wasm-worker`) reads them when it starts: it
   * forwards their calls to the host, and refuses a synchronous one, which the core could not wait for (ADR-049).
   */
  ports?(): ReadonlyMap<number, PortImpl>;
}

/**
 * A way to reach an Undra core. The three implementations are
 * {@link WasmMainTransport}, {@link WasmWorkerTransport} and
 * {@link RemoteTransport}; tests and embedders can supply their own through
 * `UndraCore.attach`.
 */
export interface Transport {
  /** Name of the mode, for messages and `UndraModeError` (`"wasm-main"`, `"wasm-worker"`, `"remote"`). */
  readonly mode: string;
  /**
   * `true` when the core runs on the calling thread inside `send`: everything
   * the core emits in response (replies, change-sets) has been delivered to the
   * handler by the time `send` returns. Only such transports offer `callSync`.
   */
  readonly synchronous: boolean;
  /**
   * Connects, performs the handshake and returns the core's `Hello`. Rejects
   * with `UndraSchemaMismatchError` when the core was built from another
   * schema, with `UndraTransportError` for anything else.
   */
  start(handler: TransportHandler): Promise<HelloPayload>;
  /**
   * Sends a host-to-core message: `Call`, `PortReply`, `Cancel`,
   * `StreamCredit`, `Observe`, `Release`, `Event`, `TimerFired` or `Restore`.
   * Throws `UndraTransportError` when the channel is closed and
   * `UndraReplyError` (status 5) when a `Call` is refused without a reply.
   */
  send(kind: Kind, payload: Uint8Array): void;
  /** Runs a `Call` and returns the `Reply` payload (SPEC 6 `undra_call_sync`). Present only when `synchronous`. */
  callSync?(payload: Uint8Array): Uint8Array;
  /**
   * `send(Kind.Call, head ++ tail)` for a core that runs in this thread, without joining the two: `head` is the `Call`
   * header (SPEC 3.3: 17 bytes for a method or function, which the caller reuses for its next call), `tail` the encoded
   * arguments, absent when `head` is the whole payload (a page call, a constructor's header). The transport must have
   * copied both before it returns and keep neither. Optional: a transport that has it is called through it; one that has
   * not gets the joined payload through `send` (ADR-056, ADR-057). Only `wasm-main` has it.
   */
  sendCall?(head: Uint8Array, tail?: Uint8Array): void;
  /** `callSync(head ++ tail)`, with the same contract as {@link Transport.sendCall}. */
  callSyncParts?(head: Uint8Array, tail: Uint8Array): Uint8Array;
  /**
   * The control messages as calls (ADR-057), for a transport whose core runs in this thread: `UndraCore` calls these
   * instead of `send(kind, payload)`, so nothing is encoded only to be decoded again in the same thread. `Observe`:
   * starts or stops observing `signalId` of the store `handle` (`ALL_SIGNALS` for every one).
   *
   * A transport that has one of the seven must have all of them (`observe`, `release`, `cancel`, `streamCredit`, `event`,
   * `timerFired`, `portReply`): `attach` refuses one that has some and not the others with `UndraError("options")` naming
   * both. One that has none is wrapped by the runtime (`transport/framed.ts`), which encodes each call into the payload
   * `send` always received, byte for byte. As for {@link Transport.sendCall}: whatever a method is
   * given it must copy before it returns.
   */
  observe?(handle: bigint, signalId: number, on: boolean): void;
  /** `Release`: the host drops one reference to the object behind `handle`. */
  release?(handle: bigint): void;
  /** `Cancel`: cancels the in-flight call or stream `callId`. */
  cancel?(callId: number): void;
  /** `StreamCredit`: grants the stream `callId` `credit` more items. */
  streamCredit?(callId: number, credit: number): void;
  /** `Event`: a host-to-core event of an event port (`Connectivity.changed`, ...). */
  event?(portId: number, methodId: number, payload: Uint8Array): void;
  /** `TimerFired`: a timer the core set through a foreign `Timer` port is due. */
  timerFired?(timerId: number): void;
  /** `PortReply`: the host's answer to a `PortCall` it answered later; `reply` is a complete `PortReply` payload. */
  portReply?(reply: Uint8Array): void;
  /** The core's statistics as JSON (`undra_stats_json`), or `null` when the transport cannot ask. */
  stats?(): Promise<string | null>;
  /**
   * The persisted state of every store (`undra_snapshot`, SPEC 5.9) as opaque bytes. Present on the
   * wasm transports only: a transport without it makes `UndraCore.snapshot` reject with
   * `UndraModeError`. Rejects with `UndraTransportError` when the channel is closed.
   */
  snapshot?(): Promise<Uint8Array>;
  /**
   * Rebuilds the stores from `bytes` (`undra_restore`) and resolves once the core has applied them;
   * every change-set the restore produced has reached the handler by then. Rejects with
   * `UndraRestoreError` when the core refuses the bytes (it is unchanged), with
   * `UndraTransportError` when the channel is closed or cannot restore. Absent on a transport that
   * cannot restore (`UndraCore.restore` then rejects with `UndraModeError`). A `Restore` envelope
   * sent through {@link Transport.send} does the same without the acknowledgement.
   */
  restore?(bytes: Uint8Array): Promise<void>;
  /** Releases the channel. Idempotent; the handler's `closed` is not called. */
  close(): void;
  /**
   * The host registers `impl` for `portId` (`registerPort`), before it does. Throws `UndraError("options")` for a port
   * this transport cannot serve: a synchronous one in `wasm-worker`, whose core cannot wait for the host's thread
   * (ADR-049); otherwise a started worker is told to forward the port's calls.
   */
  portAdded?(portId: number, impl: PortImpl): void;
  /**
   * Only for a wasm transport loaded with recovery (ADR-049): after the handler heard of a trap (`closed` with an
   * `UndraTransportError("trap")`), brings the core back: the same compiled module instantiated again, initialised,
   * and the last snapshot restored with its generation floor raised to `generationFloor`. Rejects with an
   * `UndraTransportError` (`"trap"` when the new instance traps too). A transport the host does not restart is
   * closed with `close()`.
   */
  restart?(generationFloor: number): Promise<RestartResult>;
}

/**
 * What `UndraCore` drives (ADR-057): a {@link Transport} whose control messages are calls, either its own (the in-process
 * host) or the encodings of `send` that `framed()` makes. `send` is not part of it: a core never frames a message itself.
 * Internal to the package.
 */
export interface CoreTransport extends Omit<Transport, "send" | "sendCall" | "observe" | "release" | "cancel" | "streamCredit" | "event" | "timerFired" | "portReply"> {
  /** `Call`: the header, or the whole payload, and apart the encoded arguments (see {@link Transport.sendCall}). */
  sendCall(head: Uint8Array, tail?: Uint8Array): void;
  observe(handle: bigint, signalId: number, on: boolean): void;
  release(handle: bigint): void;
  cancel(callId: number): void;
  streamCredit(callId: number, credit: number): void;
  event(portId: number, methodId: number, payload: Uint8Array): void;
  timerFired(timerId: number): void;
  portReply(reply: Uint8Array): void;
}
