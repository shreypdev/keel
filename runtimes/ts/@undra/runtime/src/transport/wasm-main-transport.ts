import { UndraTransportError } from "../errors.js";
import {
  Kind,
  decodeCancel,
  decodeEvent,
  decodeObserve,
  decodeRelease,
  decodeStreamCredit,
  decodeTimerFired,
} from "../wire/index.js";
import type { Transport } from "./transport.js";
import { WasmHost, type WasmMainOptions } from "./wasm-main.js";
import { restoreInto, takeSnapshot, twin } from "./wasm-snapshot.js";
import { msg } from "../messages.js";

export type { WasmMainOptions, WasmSource } from "./wasm-main.js";

/**
 * Runs an Undra core in this thread (the `wasm-main` mode): the in-process host plus `send(kind, payload)`, which takes
 * the framed messages a worker's script or an embedder passes along and decodes each into the host's call, and the snapshot
 * operations as methods (over the functions of `wasm-snapshot.ts`). `UndraCore.load` runs the host itself, which has no payload
 * decoder (ADR-057); this class is what the worker script, recovery, tests and `UndraCore.attach(new WasmMainTransport(..))` use.
 */
export class WasmMainTransport extends WasmHost implements Transport {
  /** @param options See {@link WasmMainOptions}. */
  constructor(options: WasmMainOptions) {
    super(options, options, options.onError);
  }

  send(kind: Kind, payload: Uint8Array): void {
    switch (kind) {
      case Kind.Call:
        this.sendCall(payload);
        return;
      case Kind.Cancel:
        this.cancel(decodeCancel(payload).callId);
        return;
      case Kind.StreamCredit: {
        const { callId, credit } = decodeStreamCredit(payload);
        this.streamCredit(callId, credit);
        return;
      }
      case Kind.Observe: {
        const { handle, signalId, on } = decodeObserve(payload);
        this.observe(handle, signalId, on);
        return;
      }
      case Kind.Release:
        this.release(decodeRelease(payload).handle);
        return;
      case Kind.Event: {
        const { portId, methodId, payload: body } = decodeEvent(payload);
        this.event(portId, methodId, body);
        return;
      }
      case Kind.PortReply:
        this.portReply(payload);
        return;
      case Kind.TimerFired:
        this.timerFired(decodeTimerFired(payload).timerId);
        return;
      case Kind.Restore:
        restoreInto(this, payload);
        return;
      default:
        throw new UndraTransportError("protocol", msg(190, Kind[kind] ?? String(kind)));
    }
  }

  /** The persisted state of every store (`undra_snapshot`, SPEC 5.9). Rejects `UndraTransportError` when the core is closed or exports no `undra_snapshot`. */
  snapshot(): Promise<Uint8Array> {
    try {
      return Promise.resolve(takeSnapshot(this));
    } catch (error) {
      return Promise.reject(error);
    }
  }

  /** `undra_snapshot`, copied out of wasm memory, at once; throws `UndraTransportError` when the core cannot be asked. */
  takeSnapshot(): Uint8Array {
    return takeSnapshot(this);
  }

  /**
   * Rebuilds the stores from `bytes` (`undra_restore`). The change-sets of the observed signals the core re-delivers during the
   * restore (ADR-023) have reached the handler when this resolves. Rejects with `UndraRestoreError` when the core refuses the
   * bytes (it is unchanged).
   */
  restore(bytes: Uint8Array): Promise<void> {
    try {
      restoreInto(this, bytes);
      return Promise.resolve();
    } catch (error) {
      return Promise.reject(error);
    }
  }

  /**
   * A new transport over the same compiled module (no recompile) and options, not started: what a restart after a trap runs on
   * (ADR-049, `crashRecovery`). This one stays dead.
   */
  twin(): this {
    return twin(this);
  }
}
