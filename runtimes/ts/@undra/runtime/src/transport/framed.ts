import { TIMER_PORT } from "../adapters/port-literals.js";
import type { Adapters } from "../adapters/types.js";
import type { UndraCallError } from "../call-error.js";
import type { AttachOptions, UndraCore } from "../core.js";
import { UndraError, UndraTransportError } from "../errors.js";
import { onDemand } from "../on-demand.js";
import type { PortImpl } from "../port.js";
import { type HelloPayload, Kind, encodeCancel, encodeEvent, encodeObserve, encodeRelease, encodeStreamCredit, encodeTimerFired } from "../wire/index.js";
import type { CoreTransport, Transport } from "./transport.js";
import { msg } from "../messages.js";

// The observe waiters of a core that answers later arrive with the transport that needs them (a re-export from a module with code puts the module in this chunk).
export { mirrorWaiters } from "../mirror-waiters.js";

/*
 * The adapter between `UndraCore` and a transport that frames its messages (ADR-057). A core whose host is in this thread
 * passes its control messages as calls (`observe`, `release`, `cancel`, ...) and nothing is encoded; a transport that has only
 * `send(kind, payload)` (`remote`, the worker's, React Native's, a test double) is wrapped once, in `attach`, by
 * `framed(transport)`, which encodes each call into the payload `send` always received: the same bytes reach the same `send`
 * as before the typed channel existed. The remote and worker transports import this module, so it arrives with them; a
 * custom transport's `attach` loads it as one small chunk.
 *
 * The same module is what a core that is not in this thread adds to `UndraCore` (`extension`): reconnecting after the connection
 * dropped (ADR-051), the connection-down test of `report`, the dev notice (ADR-053), the ports of a native core and the warning
 * about the adapters a worker ignores. An in-process core has none of it, so none of it is in a `wasm-main` page's first chunk.
 */

/** The control messages a transport passes as calls (ADR-057): all of them, or none (`send` frames them). */
const CONTROL = ["observe", "release", "cancel", "streamCredit", "event", "timerFired", "portReply"] as const;

/**
 * The typed channel `UndraCore` speaks over `transport`, a transport that is not the in-process host: the transport itself when it
 * passes every control message as a call and has `sendCall`, `framed(transport)` when it passes none (or all of them but frames its
 * calls with `send`). A transport with some of the seven and not the others is refused, `UndraError("options")` naming both lists,
 * before it starts: the core would otherwise call a method it does not have at the first release or cancel.
 */
export function channel(transport: Transport | CoreTransport): CoreTransport {
  const t = transport as Partial<CoreTransport>;
  const has = CONTROL.filter((name) => t[name] !== undefined);
  if (has.length > 0 && has.length < CONTROL.length) {
    throw new UndraError("options", msg(246, has.join(", "), CONTROL.filter((name) => t[name] === undefined).join(", ")));
  }
  return has.length > 0 && t.sendCall !== undefined ? (transport as CoreTransport) : framed(transport as Transport);
}

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

// ----- what a core outside this thread adds to `UndraCore` --------------------------------------------------------------

/** The `Log` target of the messages `undra dev` addresses to the developer (ADR-053); see `AttachOptions.onDevNotice`. */
const DEV_NOTICE_TARGET = "undra::dev";

/** References given back while a connection was down, per core, one per reference: released in the core once it is back. */
const releasedWhileDown = new WeakMap<UndraCore, bigint[]>();

/** What `UndraCore` asks of a core that is not in this thread: installed as `UndraCore._ext` by `attach`. */
export interface CoreExtension {
  /**
   * Before the transport starts: in `wasm-worker` mode the warning about adapters that do not reach the worker (ADR-049), and, for
   * a native core, the `Diagnostics` port its panics are reported through (ADR-046 decision 4.2) and the Timer port of an explicit
   * adapter, which is registered by the function this resolves with once the Hello is checked. Built from a module that loads on
   * demand (`adapters/ports.js`) and awaited, so the ports are there before the first message after the Hello can reach them.
   */
  starting(core: UndraCore, options: AttachOptions, adapters: Partial<Adapters>, ports: Map<number, PortImpl>): Promise<() => void>;
  /**
   * Whether `error` is the connection of a `remote` core being down, which `UndraCore.connection` already reports: the core is
   * `reconnecting`, or `closed` for a reason other than the app's own `close()`. A wasm core that trapped is not a connection, and
   * a core the app closed is a programming error: both are still reported.
   */
  down(core: UndraCore, error: UndraCallError): boolean;
  /** Keeps a reference given back while the connection is down (the core keeps the object meanwhile, ADR-051); `false` when it is up. */
  held(core: UndraCore, handle: bigint): boolean;
  /** Hands a dev server's message to `onDevNotice` (only a `remote` core is told: ADR-053). */
  logged(core: UndraCore, options: AttachOptions, target: string, message: string): void;
  /** The connection dropped and the transport reconnects: what was in flight is lost, the core stays open. */
  reconnecting(core: UndraCore, attempt: number, error: Error): void;
  /**
   * The connection is back: release what was released meanwhile and observe what the app observes again. The core answers each
   * `Observe` with the current values, so every mirror converges.
   */
  reconnected(core: UndraCore, hello: HelloPayload): void;
}

/** The extension of a core outside this thread. */
export const extension: CoreExtension = {
  async starting(core, options, adapters, ports) {
    const given = (["clock", "rng", "timer"] as const).filter((name) => options.adapters?.[name] != null);
    if (given.length > 0 && core.mode === "wasm-worker") {
      core._log(3, "undra::worker", msg(173, given.join(", adapters.")));
    }
    // A wasm core needs neither port: it traps instead of reporting.
    const built = core.mode.startsWith("wasm") ? undefined : await onDemand("port adapters", () => import("../adapters/ports.js"));
    built?.serveDiagnostics(core, ports, options.onPanic, adapters.log);
    return () => {
      if (built !== undefined && core.mode === "remote" && options.adapters?.timer) {
        // A native core normally times itself; an explicit Timer adapter is a request to serve its Timer port.
        ports.set(
          TIMER_PORT,
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
    let list = releasedWhileDown.get(core);
    if (list === undefined) releasedWhileDown.set(core, (list = []));
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
    if (attempt === 1) {
      core._failInFlight(new UndraTransportError("closed", msg(174, error.message), { cause: error }));
    }
    core._setConnection({ kind: "reconnecting", attempt, error });
  },
  reconnected(core, hello) {
    if (core.closed) return;
    core.hello = hello;
    try {
      for (const handle of releasedWhileDown.get(core)?.splice(0) ?? []) core._transport.release(handle);
      for (const [handle, signals] of core._observed) {
        for (const signalId of signals) core._transport.observe(handle, signalId, true);
      }
    } catch (error) {
      // The connection dropped again already: not connected after all. The transport reports the loss and the next reconnect replays.
      core.report(error, "reconnect");
      return;
    }
    core._setConnection({ kind: "connected" });
  },
};
