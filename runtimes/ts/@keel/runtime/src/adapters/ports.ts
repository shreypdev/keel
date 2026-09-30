import { KeelPortError } from "../errors.js";
import type { PortImpl } from "../port.js";
import { KeelReader, KeelWriter, codecs, encodeValue } from "../wire/index.js";
import { AppStateCodec, FsErrorCodec, HttpErrorCodec, HttpRequestCodec, HttpResponseCodec, NetKindCodec } from "./codecs.js";
import { PortIds } from "./ids.js";
import {
  type AppState,
  type Adapters,
  type ClockAdapter,
  type FsAdapter,
  FsError,
  type HttpAdapter,
  HttpError,
  type KvAdapter,
  type LogAdapter,
  type NetKind,
  type RngAdapter,
  type TimerAdapter,
} from "./types.js";

/*
 * `PortImpl` builders for the standard ports (SPEC 8): each wraps a typed
 * adapter with the argument and result codecs of its methods, so a platform
 * only writes `fetch` or `IndexedDB` code. `standardPorts` picks the ones the
 * runtime registers for a set of adapters.
 */

const EMPTY = new Uint8Array(0);
const MAX_RNG_BYTES = 1 << 24;

/** Decodes `args` with `read`, requiring that all of them are consumed. */
function readArgs<T>(args: Uint8Array, read: (r: KeelReader) => T): T {
  const r = new KeelReader(args);
  const value = read(r);
  r.finish();
  return value;
}

/** Runs `run`, turning a typed failure into `KeelPortError` with the encoded error. */
async function typed<T, E extends Error>(
  run: () => Promise<T>,
  isTyped: (error: unknown) => error is E,
  encodeError: (error: E) => Uint8Array,
): Promise<T> {
  try {
    return await run();
  } catch (error) {
    if (isTyped(error)) throw new KeelPortError(encodeError(error));
    throw error;
  }
}

const isHttpError = (error: unknown): error is HttpError => error instanceof HttpError;
const isFsError = (error: unknown): error is FsError => error instanceof FsError;

/** The `Http` port over an {@link HttpAdapter}. An {@link HttpError} becomes the typed error of `Http.request`. */
export function httpPort(http: HttpAdapter): PortImpl {
  return {
    sync: false,
    methods: {
      [PortIds.Http.request]: (args) => {
        const request = readArgs(args, (r) => HttpRequestCodec.decode(r));
        return typed(
          async () => encodeValue(HttpResponseCodec, await http.request(request)),
          isHttpError,
          (e) => encodeValue(HttpErrorCodec, e),
        );
      },
    },
  };
}

const stringList = codecs.vec(codecs.string);
const optionBytes = codecs.option(codecs.bytes);

function kvMethods(ids: typeof PortIds.Kv, kv: KvAdapter): PortImpl["methods"] {
  return {
    [ids.get]: async (args) => encodeValue(optionBytes, await kv.get(readArgs(args, (r) => r.readStr()))),
    [ids.set]: async (args) => {
      const [key, value] = readArgs(args, (r) => [r.readStr(), codecs.bytes.decode(r)] as const);
      await kv.set(key, value);
      return EMPTY;
    },
    [ids.delete]: async (args) => {
      await kv.delete(readArgs(args, (r) => r.readStr()));
      return EMPTY;
    },
    [ids.list]: async (args) => encodeValue(stringList, await kv.list(readArgs(args, (r) => r.readStr()))),
  };
}

/** The `Kv` port over a {@link KvAdapter}. */
export function kvPort(kv: KvAdapter): PortImpl {
  return { sync: false, methods: kvMethods(PortIds.Kv, kv) };
}

/** The `SecureStore` port (same methods as `Kv`, its own ids) over a {@link KvAdapter}. */
export function secureStorePort(store: KvAdapter): PortImpl {
  return { sync: false, methods: kvMethods(PortIds.SecureStore, store) };
}

/** The `Fs` port over an {@link FsAdapter}. An {@link FsError} becomes the typed error of the method. */
export function fsPort(fs: FsAdapter): PortImpl {
  const run = <T>(work: () => Promise<T>) => typed(work, isFsError, (e) => encodeValue(FsErrorCodec, e));
  return {
    sync: false,
    methods: {
      [PortIds.Fs.read]: (args) => {
        const path = readArgs(args, (r) => r.readStr());
        return run(async () => encodeValue(codecs.bytes, await fs.read(path)));
      },
      [PortIds.Fs.write]: (args) => {
        const [path, data] = readArgs(args, (r) => [r.readStr(), codecs.bytes.decode(r)] as const);
        return run(async () => {
          await fs.write(path, data);
          return EMPTY;
        });
      },
      [PortIds.Fs.delete]: (args) => {
        const path = readArgs(args, (r) => r.readStr());
        return run(async () => {
          await fs.delete(path);
          return EMPTY;
        });
      },
      [PortIds.Fs.list]: (args) => {
        const dir = readArgs(args, (r) => r.readStr());
        return run(async () => encodeValue(stringList, await fs.list(dir)));
      },
    },
  };
}

/**
 * The `Timer` port over a {@link TimerAdapter}, for a core that asks the host
 * for timers through a port (a native core with a foreign Timer binding, SPEC
 * 5.8). `fire` must deliver `TimerFired`, normally `(id) => core.timerFired(id)`.
 * A wasm core owns its timers through the `timer_set` import instead.
 */
export function timerPort(timer: TimerAdapter, fire: (timerId: number) => void): PortImpl {
  return {
    sync: true,
    methods: {
      [PortIds.Timer.set]: (args) => {
        const [timerId, delayMs] = readArgs(args, (r) => [r.readU32(), r.readU64()] as const);
        timer.set(timerId, Number(delayMs), fire);
        return EMPTY;
      },
    },
  };
}

/** The `Clock` port over a {@link ClockAdapter}; registering it overrides the core's built-in clock. */
export function clockPort(clock: ClockAdapter): PortImpl {
  return {
    sync: true,
    methods: {
      [PortIds.Clock.nowMs]: () => encodeValue(codecs.i64, BigInt(Math.trunc(clock.nowMs()))),
      [PortIds.Clock.monotonicNs]: () => encodeValue(codecs.u64, BigInt.asUintN(64, clock.monotonicNs())),
    },
  };
}

/** The `Rng` port over an {@link RngAdapter}; registering it overrides the core's built-in generator. */
export function rngPort(rng: RngAdapter): PortImpl {
  return {
    sync: true,
    methods: {
      [PortIds.Rng.fill]: (args) => {
        const len = readArgs(args, (r) => r.readU32());
        if (len > MAX_RNG_BYTES) throw new RangeError(`Rng.fill: ${len} bytes requested, the limit is ${MAX_RNG_BYTES}`);
        const out = new Uint8Array(len);
        rng.fill(out);
        return encodeValue(codecs.bytes, out);
      },
    },
  };
}

/** The `Log` port over a {@link LogAdapter}; registering it overrides the core's built-in log binding. */
export function logPort(log: LogAdapter): PortImpl {
  return {
    sync: true,
    methods: {
      [PortIds.Log.log]: (args) => {
        const [level, target, message] = readArgs(args, (r) => [r.readU8(), r.readStr(), r.readStr()] as const);
        log.log(level, target, message);
        return EMPTY;
      },
    },
  };
}

/**
 * The ports the runtime registers for `adapters` without being asked: Http,
 * Kv, SecureStore and Fs. Clock, Rng and Log stay with the core's built-in
 * bindings (SPEC 7) and Timer with the wasm `timer_set` import; register
 * {@link clockPort}, {@link rngPort}, {@link logPort} or {@link timerPort}
 * yourself to override them.
 */
export function standardPorts(adapters: Partial<Adapters>): Map<number, PortImpl> {
  const ports = new Map<number, PortImpl>();
  if (adapters.http) ports.set(PortIds.Http.portId, httpPort(adapters.http));
  if (adapters.kv) ports.set(PortIds.Kv.portId, kvPort(adapters.kv));
  if (adapters.secureStore) ports.set(PortIds.SecureStore.portId, secureStorePort(adapters.secureStore));
  if (adapters.fs) ports.set(PortIds.Fs.portId, fsPort(adapters.fs));
  return ports;
}

/** Where host events go: `KeelCore.event`. */
export interface EventSink {
  event(portId: number, methodId: number, payload: Uint8Array): void;
}

/** Sends `Connectivity.changed(online, kind)` to the core. */
export function emitConnectivity(core: EventSink, online: boolean, kind: NetKind): void {
  const w = new KeelWriter(4);
  w.writeBool(online);
  NetKindCodec.encode(w, kind);
  core.event(PortIds.Connectivity.portId, PortIds.Connectivity.changed, w.finish());
}

/** Sends `Lifecycle.changed(state)` to the core. */
export function emitLifecycle(core: EventSink, state: AppState): void {
  core.event(PortIds.Lifecycle.portId, PortIds.Lifecycle.changed, encodeValue(AppStateCodec, state));
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
