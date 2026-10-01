import { loadNative, nativePlatformDefaults, reactNativeHttp, type NativeTransport } from '@undra/react-native';
import type { HttpAdapter, UndraCore } from '@undra/runtime';
import { BigList, Device, Todos, UndraIds, kvGet, kvPut } from '@playground/core';

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
  /** Milliseconds from `loadNative` to the stores being observed. */
  readonly startupMs: number;
  /** A fresh token of this launch, which the checks write into the stores (and the device script looks for). */
  readonly nonce: string;
}

/** Where the app's evidence lines go: the device log (`UNDRA-RN ...`) and the Bench screen. */
export type Log = (line: string) => void;

/** The header the app's `Http` override adds to every request the core makes. */
export const PLAYGROUND_HEADER = 'x-undra-playground';

/** The `Kv` key whose value one launch writes and the next one reads (the restart check of the device script). */
const RESTART_KEY = 'rn.checks.restart';

/**
 * The sample of overriding a default (ADR-038 amendment B): every other port is the package's own (`Kv`,
 * `SecureStore`, `Fs` and `Connectivity` native, `Lifecycle` from `AppState`), but `Http` is wrapped to add a header
 * to the core's requests. It still goes through the package's default, `reactNativeHttp()` (React Native's `fetch`).
 */
function taggedHttp(inner: HttpAdapter = reactNativeHttp()): HttpAdapter {
  return {
    request: req => inner.request({ ...req, headers: [...req.headers, { name: PLAYGROUND_HEADER, value: 'rn' }] }),
  };
}

const text = new TextEncoder();
const fromBytes = (bytes: Uint8Array): string => new TextDecoder().decode(bytes);

/** Loads the native core (it is linked into the app) and creates the stores the screens show. */
export async function startUndra(log: Log): Promise<Playground> {
  const started = performance.now();
  const nonce = `${Date.now().toString(36)}${Math.floor(Math.random() * 0xffffff).toString(36)}`;
  const core = await loadNative({
    expectedSchemaHash: UndraIds.schemaHash,
    adapters: { http: taggedHttp() },
    onError: error => log(`UNDRA-RN error ${String(error)}`),
  });
  log(
    `UNDRA-RN loaded mode=${core.mode} platform=${core.hello.platform} schema=0x${core.hello.schemaHash.toString(16)} hermes=${String(
      typeof (globalThis as { HermesInternal?: unknown }).HermesInternal === 'object',
    )}`,
  );
  const defaults = nativePlatformDefaults();
  log(`UNDRA-RN defaults native=${defaults.ports.length} kv=${defaults.kv ?? '-'} fs=${defaults.fs ?? '-'} secure=${defaults.secureStore ?? '-'}${defaults.error ? ` error=${defaults.error}` : ''}`);

  // What the previous launch left in Kv, then this launch's token: the device script kills the app between two
  // launches and compares (a Kv round trip that survives the process).
  const previous = await kvGet(RESTART_KEY, core);
  log(`UNDRA-RN KV previous=${previous === null ? 'none' : fromBytes(previous)}`);
  await kvPut(RESTART_KEY, text.encode(nonce), core);
  log(`UNDRA-RN KV wrote=${nonce}`);

  const [todos, bigList, device] = await Promise.all([Todos.create(core), BigList.create(core), Device.create(core)]);
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
  return { core, todos, bigList, device, startupMs, nonce };
}

/** The transport's native counters, when the core is the native one. */
export function nativeCounters(core: UndraCore): ReturnType<NativeTransport['counters']> | null {
  const native = (globalThis as { __undraNative?: { hostCounters(): ReturnType<NativeTransport['counters']> } }).__undraNative;
  return core.mode === 'native' && native !== undefined ? native.hostCounters() : null;
}
