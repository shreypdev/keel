import { Kind, encodeCancel, encodeEvent, encodeObserve, encodeRelease, encodeStreamCredit, encodeTimerFired } from "../wire/index.js";
import type { CoreTransport, Transport } from "./transport.js";

/*
 * The adapter between `UndraCore` and a transport that frames its messages (ADR-057). A core whose host is in this thread
 * passes its control messages as calls (`observe`, `release`, `cancel`, ...) and nothing is encoded; a transport that has only
 * `send(kind, payload)` (`remote`, the worker's, React Native's, a test double) is wrapped once, in `attach`, by
 * `framed(transport)`, which encodes each call into the payload `send` always received: the same bytes reach the same `send`
 * as before the typed channel existed. The remote and worker transports import this module, so it arrives with them; a
 * custom transport's `attach` loads it as one small chunk.
 */

/** What a wrapped transport answers beyond the control messages: forwarded as it has them (the core tells the modes apart by them). */
const FORWARDED = ["callSync", "callSyncParts", "stats", "snapshot", "restore", "portAdded", "restart"] as const;

/**
 * `transport` driven through the typed channel `UndraCore` speaks: its own optional members as they are, and every control
 * message as the encoding of `send`. It does not touch `transport`: the wrapper holds it.
 */
export function framed(transport: Transport): CoreTransport {
  const t = transport;
  const wrapper = {
    mode: t.mode,
    synchronous: t.synchronous,
    start: (handler) => t.start(handler),
    close: () => {
      t.close();
    },
    // A head without a tail is a whole payload the caller does not reuse; with one, the head is the core's reused header: both
    // go out in one fresh array, which is what `send` always got.
    sendCall: (head, tail) => {
      if (t.sendCall !== undefined) return t.sendCall(head, tail);
      if (tail === undefined) return t.send(Kind.Call, head);
      const payload = new Uint8Array(head.length + tail.length);
      payload.set(head);
      payload.set(tail, head.length);
      t.send(Kind.Call, payload);
    },
    observe: (handle, signalId, on) => {
      t.observe ? t.observe(handle, signalId, on) : t.send(Kind.Observe, encodeObserve({ handle, signalId, on }));
    },
    release: (handle) => {
      t.release ? t.release(handle) : t.send(Kind.Release, encodeRelease({ handle }));
    },
    cancel: (callId) => {
      t.cancel ? t.cancel(callId) : t.send(Kind.Cancel, encodeCancel({ callId }));
    },
    streamCredit: (callId, credit) => {
      t.streamCredit ? t.streamCredit(callId, credit) : t.send(Kind.StreamCredit, encodeStreamCredit({ callId, credit }));
    },
    event: (portId, methodId, payload) => {
      t.event ? t.event(portId, methodId, payload) : t.send(Kind.Event, encodeEvent({ portId, methodId, payload }));
    },
    timerFired: (timerId) => {
      t.timerFired ? t.timerFired(timerId) : t.send(Kind.TimerFired, encodeTimerFired({ timerId }));
    },
    portReply: (reply) => {
      t.portReply ? t.portReply(reply) : t.send(Kind.PortReply, reply);
    },
  } satisfies CoreTransport;
  for (const name of FORWARDED) {
    // Present as the transport has it (the core tells the modes apart by them), read at each call: the transport stays the
    // transport, whatever replaces one of its methods later.
    if (t[name] !== undefined) (wrapper as Record<string, unknown>)[name] = (...args: unknown[]) => Reflect.apply(t[name] as () => unknown, t, args);
  }
  return wrapper;
}
