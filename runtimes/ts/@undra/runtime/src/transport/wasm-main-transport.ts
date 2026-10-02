// PROTOTYPE (ADR-057 lever d4): the public transport class: the in-process host plus the framed `send(kind, payload)` that
// the worker script and custom drivers use. `UndraCore.load` runs the host itself, which carries no payload decoder.
import { UndraReplyError, UndraTransportError } from "../errors.js";
import { Kind, ReplyStatus, codecs, decodeCancel, decodeEvent, decodeObserve, decodeRelease, decodeStreamCredit, decodeTimerFired, encodeValue, splitHandle } from "../wire/index.js";
import type { Transport } from "./transport.js";
import { WasmHost } from "./wasm-main.js";
import { restoreInto, takeSnapshot, twin } from "./wasm-snapshot.js";

export type { WasmMainOptions, WasmSource } from "./wasm-main.js";

function refused(code: number): UndraReplyError {
  return new UndraReplyError(ReplyStatus.BadRequest, encodeValue(codecs.string, `the core refused the call (undra_call returned ${code})`));
}

export class WasmMainTransport extends WasmHost implements Transport {
  send(kind: Kind, payload: Uint8Array): void {
    switch (kind) {
      case Kind.Call: {
        const code = this._invoke(payload, (e, ptr, len) => e.undra_call(ptr, len));
        if (code !== 0) throw refused(code);
        return;
      }
      case Kind.Cancel: {
        const { callId } = decodeCancel(payload);
        this._run((e) => e.undra_cancel(callId));
        return;
      }
      case Kind.StreamCredit: {
        const { callId, credit } = decodeStreamCredit(payload);
        this._run((e) => e.undra_stream_credit(callId, credit));
        return;
      }
      case Kind.Observe: {
        const { handle, signalId, on } = decodeObserve(payload);
        const { lo, hi } = splitHandle(handle);
        this._run((e) => e.undra_observe(lo, hi, signalId, on ? 1 : 0));
        return;
      }
      case Kind.Release: {
        const { lo, hi } = splitHandle(decodeRelease(payload).handle);
        this._run((e) => e.undra_release(lo, hi));
        return;
      }
      case Kind.Event: {
        const event = decodeEvent(payload);
        this._invoke(event.payload, (e, ptr, len) => e.undra_event(event.portId, event.methodId, ptr, len));
        return;
      }
      case Kind.PortReply:
        this._invoke(payload, (e, ptr, len) => e.undra_port_reply(ptr, len));
        return;
      case Kind.TimerFired: {
        const { timerId } = decodeTimerFired(payload);
        this._run((e) => e.undra_timer_fired(timerId));
        return;
      }
      case Kind.Restore:
        restoreInto(this, payload);
        return;
      default:
        throw new UndraTransportError("protocol", `cannot send a ${Kind[kind] ?? String(kind)} message to a wasm core`);
    }
  }


  snapshot(): Promise<Uint8Array> {
    try {
      return Promise.resolve(takeSnapshot(this));
    } catch (error) {
      return Promise.reject(error);
    }
  }

  takeSnapshot(): Uint8Array {
    return takeSnapshot(this);
  }

  restore(bytes: Uint8Array): Promise<void> {
    try {
      restoreInto(this, bytes);
      return Promise.resolve();
    } catch (error) {
      return Promise.reject(error);
    }
  }

  twin(): this {
    return twin(this);
  }
}
