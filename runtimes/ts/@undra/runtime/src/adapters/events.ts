import { UndraWriter } from "../wire/index.js";
import { PortIds } from "./ids.js";
import { APP_STATES, type AppState, type Adapters, NET_KINDS, type NetKind } from "./types.js";

/*
 * Host events: what the Connectivity and Lifecycle sources tell the core. Separate from `ports.ts` (the
 * request/reply ports, loaded on the first call to one of them) because a page starts these two sources
 * with the core (ADR-052).
 */

/** Writes `v` as its `u16` index in `variants` (the encoding half of a unit enum's codec). */
export function writeIndex<T extends string>(w: UndraWriter, name: string, variants: readonly T[], v: T): void {
  const index = variants.indexOf(v);
  if (index < 0) throw new RangeError(`unknown ${name} variant: ${String(v)}`);
  w.writeU16(index);
}

/** Writes a `NetKind` (`NetKindCodec.encode`): the payload of `Connectivity.changed`. */
export function writeNetKind(w: UndraWriter, v: NetKind): void {
  writeIndex(w, "NetKind", NET_KINDS, v);
}

/** Writes an `AppState` (`AppStateCodec.encode`): the payload of `Lifecycle.changed`. */
export function writeAppState(w: UndraWriter, v: AppState): void {
  writeIndex(w, "AppState", APP_STATES, v);
}

/** Where host events go: `UndraCore.event`. */
export interface EventSink {
  event(portId: number, methodId: number, payload: Uint8Array): void;
}

/** Sends `Connectivity.changed(online, kind)` to the core. */
export function emitConnectivity(core: EventSink, online: boolean, kind: NetKind): void {
  const w = new UndraWriter(4);
  w.writeBool(online);
  writeNetKind(w, kind);
  core.event(PortIds.Connectivity.portId, PortIds.Connectivity.changed, w.finish());
}

/** Sends `Lifecycle.changed(state)` to the core. */
export function emitLifecycle(core: EventSink, state: AppState): void {
  const w = new UndraWriter(2);
  writeAppState(w, state);
  core.event(PortIds.Lifecycle.portId, PortIds.Lifecycle.changed, w.finish());
}

/**
 * Connects the Connectivity and Lifecycle adapters to the core: every change
 * they report is sent as an event. Returns the function that disconnects
 * them. `onError` receives failures to send (the core closed meanwhile).
 */
export function startEventSources(
  core: EventSink,
  adapters: Partial<Adapters>,
  onError: (error: unknown) => void,
): () => void {
  const stops: Array<() => void> = [];
  const guarded =
    <A extends unknown[]>(send: (...args: A) => void) =>
    (...args: A): void => {
      try {
        send(...args);
      } catch (error) {
        onError(error);
      }
    };
  if (adapters.connectivity) {
    stops.push(adapters.connectivity.subscribe(guarded((online, kind) => emitConnectivity(core, online, kind))));
  }
  if (adapters.lifecycle) {
    stops.push(adapters.lifecycle.subscribe(guarded((state) => emitLifecycle(core, state))));
  }
  return () => {
    for (const stop of stops.splice(0)) stop();
  };
}
