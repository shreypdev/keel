import { installNative, loadNative, nativePlatformDefaults, reactNativeHttp, type NativeTransport } from '@undra/react-native';
import type { HttpAdapter, UndraCore, UndraPanicReport } from '@undra/runtime';
import { BigList, Device, Notes, Todos, UndraPlaygroundCore, kvGet, kvPut } from '@playground/core';

/** What the screens share: the core and its long-lived stores. */
export interface Playground {
  /** The native core, through @undra/react-native. */
  readonly core: UndraCore;
  /** The to-do list. */
  readonly todos: Todos;
  /** The 10,000-row list. */
  readonly bigList: BigList;
  /** The `Connectivity` and `Lifecycle` reports the core received (the platform module of the core). */
  readonly device: Device;
  /** Notes kept in SQLite through the native `Db` port (ADR-048); the Notes screen opens its database. */
  readonly notes: Notes;
  /** Milliseconds from `loadNative` to the stores being observed. */
  readonly startupMs: number;
  /** A fresh token of this launch, which the checks write into the stores (and the device script looks for). */
  readonly nonce: string;
  /** Every panic report the core handed to `onPanic` (ADR-046), oldest first: what an app would send to its crash reporter. */
  readonly panics: readonly UndraPanicReport[];
}

/** A panic report as the lines the Bench screen shows (and the device log carries): what panicked, where, in which build. */
export function describePanic(report: UndraPanicReport): string {
  const frames = report.frames.slice(0, 3).map(f => `0x${f.address.toString(16)}${f.symbol === null ? '' : ` ${f.symbol}`}${f.file === null ? '' : ` ${f.file}:${f.line ?? '?'}`}`);
  return [
    report.message,
    `at ${report.location === '' ? 'no location' : report.location}, in ${report.operation === '' ? 'nothing' : report.operation}, thread ${report.thread}`,
    `${report.namespace} ${report.coreVersion}, schema 0x${report.schemaHash.toString(16)}, image ${report.imageId === '' ? 'unknown' : `${report.imageId.slice(0, 12)}…`}`,
    `${report.frames.length} frame${report.frames.length === 1 ? '' : 's'}${frames.length > 0 ? `: ${frames.join(' | ')}` : ''}`,
  ].join('\n');
}

/** Where the app's evidence lines go: the device log (`UNDRA-RN ...`) and the Bench screen. */
export type Log = (line: string) => void;

/** The header the app's `Http` override adds to every request the core makes. */
export const PLAYGROUND_HEADER = 'x-undra-playground';

/** The `Kv` key whose value one launch writes and the next one reads (the restart check of the device script). */
const RESTART_KEY = 'rn.checks.restart';

/**
 * The sample of overriding a default (ADR-038 amendment B): every other port is the package's own (`Kv`,
 * `SecureStore`, `Fs`, `Db` and `Connectivity` native, `Lifecycle` from `AppState`, `WebSocket` and `Sse` from React
 * Native's networking), but `Http` is wrapped to add a header to the core's requests. It still goes through the
 * package's default, `reactNativeHttp()` (React Native's `fetch`).
 */
function taggedHttp(inner: HttpAdapter = reactNativeHttp()): HttpAdapter {
  return {
    request: req => inner.request({ ...req, headers: [...req.headers, { name: PLAYGROUND_HEADER, value: 'rn' }] }),
  };
}

const text = new TextEncoder();
const fromBytes = (bytes: Uint8Array): string => new TextDecoder().decode(bytes);

/**
 * Loads the native core `playground_core` (its pod, PlaygroundCore, on iOS; libplayground_core.so on
 * Android) through the generated entry, so it is `UndraPlaygroundCore.core`, and creates the stores
 * the screens show.
 */
export async function startUndra(log: Log, onPanic?: (report: UndraPanicReport) => void): Promise<Playground> {
  const started = performance.now();
  const panics: UndraPanicReport[] = [];
  const nonce = `${Date.now().toString(36)}${Math.floor(Math.random() * 0xffffff).toString(36)}`;
  const core = await loadNative(UndraPlaygroundCore, {
    adapters: { http: taggedHttp() },
    onError: error => log(`UNDRA-RN error ${String(error)}`),
    // Each panic the native core contained (ADR-046): where an app would call its crash reporter. The core reports it
    // through its Diagnostics port; the module queues it for the JS thread, once.
    onPanic: report => {
      panics.push(report);
      log(`UNDRA-RN PANIC ${report.operation} ${report.message} at ${report.location}`);
      onPanic?.(report);
    },
  });
  log(
    `UNDRA-RN loaded core=${UndraPlaygroundCore.namespace} mode=${core.mode} platform=${core.hello.platform} schema=0x${core.hello.schemaHash.toString(16)} abi=${core.hello.undraVersion} hermes=${String(
      typeof (globalThis as { HermesInternal?: unknown }).HermesInternal === 'object',
    )}`,
  );
  const defaults = nativePlatformDefaults(UndraPlaygroundCore.namespace);
  log(
    `UNDRA-RN defaults native=${defaults.ports.length} kv=${defaults.kv ?? '-'} fs=${defaults.fs ?? '-'} secure=${defaults.secureStore ?? '-'} db=${defaults.db ?? '-'}${defaults.error ? ` error=${defaults.error}` : ''}`,
  );

  // What the previous launch left in Kv, then this launch's token: the device script kills the app between two
  // launches and compares (a Kv round trip that survives the process).
  const previous = await kvGet(RESTART_KEY, core);
  log(`UNDRA-RN KV previous=${previous === null ? 'none' : fromBytes(previous)}`);
  await kvPut(RESTART_KEY, text.encode(nonce), core);
  log(`UNDRA-RN KV wrote=${nonce}`);

  const [todos, bigList, device, notes] = await Promise.all([Todos.create(core), BigList.create(core), Device.create(core), Notes.create(core)]);
  // Every report the core receives, as the core's store shows it (the device script waits for these lines).
  const showNet = (): void =>
    log(`UNDRA-RN CONNECTIVITY online=${String(device.online.get())} kind=${device.netKind.get()} reports=${device.connectivityReports.get()}`);
  const showLife = (): void => log(`UNDRA-RN LIFECYCLE state=${device.appState.get()} reports=${device.lifecycleReports.get()}`);
  device.connectivityReports.subscribe(showNet);
  device.lifecycleReports.subscribe(showLife);
  showNet();
  showLife();

  const startupMs = performance.now() - started;
  log(
    `UNDRA-RN stores observed: todos=${todos.todos.get().length} biglist rows=${bigList.items.get().length} count=${bigList.count.get()} in ${startupMs.toFixed(1)} ms`,
  );
  return { core, todos, bigList, device, notes, startupMs, nonce, panics };
}

/** The transport's native counters, when the core is the native one. */
export function nativeCounters(core: UndraCore): ReturnType<NativeTransport['counters']> | null {
  return core.mode === 'native' ? installNative(UndraPlaygroundCore.namespace).hostCounters() : null;
}
