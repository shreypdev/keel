// PROTOTYPE (ADR-057 lever d4): drives a transport that only has `send(kind, payload)` (the remote and worker transports, a
// custom one) through the typed channel `UndraCore` speaks. Loaded with those transports, never by a `wasm-main` page.
import { Kind, encodeCancel, encodeEvent, encodeObserve, encodeRelease, encodeStreamCredit, encodeTimerFired } from "../wire/index.js";
import type { Channel, Transport } from "./transport.js";

export { mirrorWaiters } from "../mirror-waiters.js";

/** `transport` with every {@link Channel} method: its own where it has one, an encoded `send` otherwise. */
export function framed<T extends Transport>(transport: T): T & Channel {
  const t = transport as T & Partial<Channel>;
  t.sendCall ??= (head, tail) => {
    if (tail === undefined || tail.length === 0) return transport.send(Kind.Call, head);
    const joined = new Uint8Array(head.length + tail.length);
    joined.set(head);
    joined.set(tail, head.length);
    transport.send(Kind.Call, joined);
  };
  t.observe ??= (handle, signalId, on) => transport.send(Kind.Observe, encodeObserve({ handle, signalId, on }));
  t.release ??= (handle) => transport.send(Kind.Release, encodeRelease({ handle }));
  t.cancel ??= (callId) => transport.send(Kind.Cancel, encodeCancel({ callId }));
  t.streamCredit ??= (callId, credit) => transport.send(Kind.StreamCredit, encodeStreamCredit({ callId, credit }));
  t.event ??= (portId, methodId, payload) => transport.send(Kind.Event, encodeEvent({ portId, methodId, payload }));
  t.timerFired ??= (timerId) => transport.send(Kind.TimerFired, encodeTimerFired({ timerId }));
  t.portReply ??= (reply) => transport.send(Kind.PortReply, reply);
  return t as T & Channel;
}
