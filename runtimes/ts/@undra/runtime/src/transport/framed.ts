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

// ----- what a core that is not in this thread adds to `UndraCore` (reconnects, its ports, the dev notice) ---------------
import type { Adapters } from "../adapters/types.js";
import type { UndraCallError } from "../call-error.js";
import type { AttachOptions, UndraCore } from "../core.js";
import { UndraTransportError } from "../errors.js";
import type { PortImpl } from "../port.js";
import type { HelloPayload } from "../wire/index.js";

/** The `Log` target of the messages `undra dev` addresses to the developer (ADR-053). */
const DEV_NOTICE_TARGET = "undra::dev";
const released = new WeakMap<UndraCore, bigint[]>();

export interface CoreExtension {
  starting(core: UndraCore, options: AttachOptions, adapters: Partial<Adapters>, ports: Map<number, PortImpl>): Promise<() => void>;
  down(core: UndraCore, error: UndraCallError): boolean;
  held(core: UndraCore, handle: bigint): boolean;
  logged(core: UndraCore, options: AttachOptions, target: string, message: string): void;
  reconnecting(core: UndraCore, attempt: number, error: Error): void;
  reconnected(core: UndraCore, hello: HelloPayload): void;
}

export const extension: CoreExtension = {
  async starting(core, options, adapters, ports) {
    const given = (["clock", "rng", "timer"] as const).filter((name) => options.adapters?.[name] != null);
    if (given.length > 0 && core.mode === "wasm-worker") {
      core._internals.log(3, "undra::worker", `adapters.${given.join(", adapters.")} are ignored in wasm-worker mode: set them in LoadOptions.worker.ports`);
    }
    const built = core.mode.startsWith("wasm") ? undefined : await import("../adapters/ports.js");
    built?.serveDiagnostics(core, ports, options.onPanic, adapters.log);
    return () => {
      if (built !== undefined && core.mode === "remote" && options.adapters?.timer) {
        ports.set(
          built.PortIds.Timer.portId,
          built.timerPort(options.adapters.timer, (timerId) => {
            try {
              core.timerFired(timerId);
            } catch (error) {
              core.report(error, "timer");
            }
          }),
        );
      }
    };
  },
  down(core, error) {
    if (error.kind !== "unavailable" || core.mode !== "remote") return false;
    const state = core.connection.peek();
    return state.kind === "reconnecting" || (state.kind === "closed" && state.reason !== "requested");
  },
  held(core, handle) {
    if (core.connection.peek().kind !== "reconnecting") return false;
    let list = released.get(core);
    if (list === undefined) released.set(core, (list = []));
    list.push(handle);
    return true;
  },
  logged(core, options, target, message) {
    if (target !== DEV_NOTICE_TARGET || core.mode !== "remote") return;
    try {
      options.onDevNotice?.(message);
    } catch (error) {
      core.report(error, "onDevNotice");
    }
  },
  reconnecting(core, attempt, error) {
    if (core.closed) return;
    const { failInFlight, setConnection } = core._internals;
    if (attempt === 1) failInFlight(new UndraTransportError("closed", `the connection to the core was lost (${error.message}); reconnecting`, { cause: error }));
    setConnection({ kind: "reconnecting", attempt, error });
  },
  reconnected(core, hello) {
    if (core.closed) return;
    const { transport, observed, setConnection } = core._internals;
    core.hello = hello;
    try {
      for (const handle of released.get(core)?.splice(0) ?? []) transport.release(handle);
      for (const [handle, signals] of observed) for (const signalId of signals) transport.observe(handle, signalId, true);
    } catch (error) {
      core.report(error, "reconnect");
      return;
    }
    setConnection({ kind: "connected" });
  },
};
