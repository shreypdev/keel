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
import { WasmHost } from "./wasm-main.js";

export type { WasmMainOptions, WasmSource } from "./wasm-main.js";

/**
 * Runs an Undra core in this thread (the `wasm-main` mode): the in-process host plus `send(kind, payload)`, which takes
 * the framed messages a worker's script or an embedder passes along and decodes each into the host's call. `UndraCore.load`
 * runs the host itself, which has no payload decoder (ADR-057); this class is what the worker script, recovery, tests and
 * `UndraCore.attach(new WasmMainTransport(..))` use.
 */
export class WasmMainTransport extends WasmHost implements Transport {
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
        this._restore(payload);
        return;
      default:
        throw new UndraTransportError("protocol", `cannot send a ${Kind[kind] ?? String(kind)} message to a wasm core`);
    }
  }
}
