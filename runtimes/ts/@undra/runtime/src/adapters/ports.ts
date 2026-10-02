import { UndraPortError } from "../errors.js";
import type { PortImpl } from "../port.js";
import { UndraReader, UndraWriter, codecs, encodeValue } from "../wire/index.js";
import { readHttpRequest, readPanicReport, writeFsError, writeHttpError, writeHttpResponse, writeStorageError } from "./codecs.js";
import { PortIds } from "./ids.js";
import { consoleLog } from "./system.js";
import {
  type Adapters,
  type ClockAdapter,
  type FsAdapter,
  FsError,
  type HttpAdapter,
  HttpError,
  type KvAdapter,
  type LogAdapter,
  type RngAdapter,
  StorageError,
  type TimerAdapter,
  type UndraPanicReport,
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
function readArgs<T>(args: Uint8Array, read: (r: UndraReader) => T): T {
  const r = new UndraReader(args);
  const value = read(r);
  r.finish();
  return value;
}

/** Encodes `value` with `write`: one half of a codec, so that a port ships only what it uses. */
function encodeWith<T>(write: (w: UndraWriter, value: T) => void, value: T): Uint8Array {
  const w = new UndraWriter();
  write(w, value);
  return w.finish();
}

/**
 * Runs `run`, turning a typed failure into `UndraPortError` with the encoded error (the core gets port status
 * 1). `recognize` says which failures are the port's typed error (`undefined`: not one); anything else is
 * rethrown as it is, and the runtime reports it and answers the call as unavailable (status 2).
 */
async function typed<T, E>(run: () => Promise<T>, recognize: (error: unknown) => E | undefined, encodeError: (error: E) => Uint8Array): Promise<T> {
  try {
    return await run();
  } catch (error) {
    const known = recognize(error);
    if (known !== undefined) throw new UndraPortError(encodeError(known));
    throw error;
  }
}

const isHttpError = (error: unknown): HttpError | undefined => (error instanceof HttpError ? error : undefined);
const encodeStorageError = (error: StorageError): Uint8Array => encodeWith(writeStorageError, error);

/** The `Http` port over an {@link HttpAdapter}. An {@link HttpError} becomes the typed error of `Http.request`. */
export function httpPort(http: HttpAdapter): PortImpl {
  return {
    name: "Http",
    sync: false,
    methods: {
      [PortIds.Http.request]: (args) => {
        const request = readArgs(args, readHttpRequest);
        return typed(
          async () => encodeWith(writeHttpResponse, await http.request(request)),
          isHttpError,
          (e) => encodeWith(writeHttpError, e),
        );
      },
    },
  };
}

const stringList = codecs.vec(codecs.string);
const optionBytes = codecs.option(codecs.bytes);

function kvMethods(ids: typeof PortIds.Kv, kv: KvAdapter): PortImpl["methods"] {
  const run = <T>(work: () => Promise<T>) => typed(work, (e) => (e instanceof StorageError ? e : undefined), encodeStorageError);
  return {
    [ids.get]: (args) => {
      const key = readArgs(args, (r) => r.readStr());
      return run(async () => encodeValue(optionBytes, await kv.get(key)));
    },
    [ids.set]: (args) => {
      const [key, value] = readArgs(args, (r) => [r.readStr(), codecs.bytes.decode(r)] as const);
      return run(async () => {
        await kv.set(key, value);
        return EMPTY;
      });
    },
    [ids.delete]: (args) => {
      const key = readArgs(args, (r) => r.readStr());
      return run(async () => {
        await kv.delete(key);
        return EMPTY;
      });
    },
    [ids.list]: (args) => {
      const prefix = readArgs(args, (r) => r.readStr());
      return run(async () => encodeValue(stringList, await kv.list(prefix)));
    },
  };
}

/**
 * The `Kv` port over a {@link KvAdapter}. A {@link StorageError} the adapter rejects with becomes the typed error
 * of the method (ADR-049; `StorageError.from` maps what a storage API throws); any other failure is reported and
 * answered as unavailable.
 */
export function kvPort(kv: KvAdapter): PortImpl {
  return { name: "Kv", sync: false, methods: kvMethods(PortIds.Kv, kv) };
}

/** The `SecureStore` port (same methods and errors as `Kv`, its own ids) over a {@link KvAdapter}. */
export function secureStorePort(store: KvAdapter): PortImpl {
  return { name: "SecureStore", sync: false, methods: kvMethods(PortIds.SecureStore, store) };
}

/**
 * The `Fs` port over an {@link FsAdapter}. An {@link FsError} becomes the typed error of the method (`fsErrorFrom`
 * maps what a file API throws); any other failure is reported and answered as unavailable.
 */
export function fsPort(fs: FsAdapter): PortImpl {
  const run = <T>(work: () => Promise<T>) => typed(work, (e) => (e instanceof FsError ? e : undefined), (e: FsError) => encodeWith(writeFsError, e));
  return {
    name: "Fs",
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
    name: "Timer",
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
    name: "Clock",
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
    name: "Rng",
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
    name: "Log",
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

/** What {@link diagnosticsPort} needs of the runtime that registers it; every member is optional. */
export interface DiagnosticsHost {
  /** Where the report goes when there is no `onPanic`: one line at error level (operation, message, location), so that a panic is never silent. Default: nowhere. */
  readonly log?: LogAdapter;
  /** Receives a failure of `onPanic` (it threw): the report was delivered as far as the core is concerned. Default: the failure propagates out of the port's method. */
  readonly fail?: (error: unknown) => void;
}

/**
 * The `Diagnostics` port (ADR-046 decision 4.2): a native core calls `panicked` once per panic it contained, fire and forget,
 * after its FATAL log record. Decodes the `PanicReport` and passes it to `onPanic`; the call is answered either way (the
 * core ignores the answer). A wasm core never calls it (it traps: see `LoadOptions.onPanic`). `UndraCore` registers it for the
 * native cores it reaches (`remote`, React Native, a custom transport) with `LoadOptions.onPanic` and a {@link DiagnosticsHost}
 * that logs a report nobody asked for and reports a failing handler to `onError`; an embedder with its own transport can
 * register it too.
 */
export function diagnosticsPort(onPanic: ((report: UndraPanicReport) => void) | undefined, host: DiagnosticsHost = {}): PortImpl {
  return {
    name: "Diagnostics",
    sync: true,
    methods: {
      [PortIds.Diagnostics.panicked]: (args) => {
        const report = readArgs(args, readPanicReport);
        try {
          if (onPanic !== undefined) onPanic(report);
          else host.log?.log(4, "undra::panic", `${report.operation === "" ? "panic" : report.operation}: ${report.message} (${report.location})`);
        } catch (thrown) {
          if (host.fail === undefined) throw thrown;
          host.fail(thrown);
        }
        return EMPTY;
      },
    },
  };
}

/**
 * Registers the {@link diagnosticsPort} of a native core in `ports` unless the app registered its own: what `UndraCore` does when it
 * loads one (`remote`, React Native, a custom transport), before the transport starts. Without `onPanic` a report is logged through
 * `log` (default: the console); a handler that throws is reported to `core.report` (`onError`). Here rather than in `UndraCore`, so
 * that a page with a wasm core does not ship it (ADR-052).
 *
 * @internal Called by `UndraCore`.
 */
export function serveDiagnostics(
  core: { report(error: unknown, operation: string): void },
  ports: Map<number, PortImpl>,
  onPanic: ((report: UndraPanicReport) => void) | undefined,
  log: LogAdapter = consoleLog(),
): void {
  if (!ports.has(PortIds.Diagnostics.portId)) {
    ports.set(PortIds.Diagnostics.portId, diagnosticsPort(onPanic, { log, fail: (error) => core.report(error, "onPanic") }));
  }
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

export { type EventSink, emitConnectivity, emitLifecycle, startEventSources } from "./events.js";

export { PortIds };
