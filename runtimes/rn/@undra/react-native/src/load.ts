import { type AdapterOverrides, type AttachOptions, type Transport, UndraCore, UndraTransportError } from "@undra/runtime";
import { AppState, Platform, TurboModuleRegistry } from "react-native";
import { nativeAdapterNames, nativeDefaultPorts, reactNativeAdapters } from "./adapters.js";
import { nativeFrameScheduler } from "./frame.js";
import type { NativePlatformDefaults, UndraNativeModule } from "./native.js";
import type { Spec } from "./specs/NativeUndra.js";
import { NativeTransport } from "./transport.js";

/**
 * The core {@link loadNative} loads: the generated entry of its bindings (`UndraPlaygroundCore`,
 * `Undra<Namespace>` in general, ADR-044), or just its namespace and schema hash.
 */
export interface NativeCoreEntry {
  /** The core's namespace (`[core] namespace` of its undra.toml): which native core to load. */
  readonly namespace: string;
  /** The schema hash of the bindings; a core built from another schema is refused before it starts. */
  readonly schemaHash: bigint;
  /**
   * Attaches the core over a transport with the entry's schema hash and makes it the entry's `core`
   * (the generated entries have it). Without it, `loadNative` calls `UndraCore.attach` itself.
   */
  attach?(transport: Transport, options?: Omit<AttachOptions, "expectedSchemaHash">): Promise<UndraCore>;
}

/**
 * Options of {@link loadNative}: those of `UndraCore.attach` but the schema hash (the entry's), plus
 * the core's configuration.
 */
export interface NativeLoadOptions extends Omit<AttachOptions, "expectedSchemaHash"> {
  /** Run the core in `"dev"` mode (devtools log records, docs/SPEC.md section 5.10). */
  readonly devtools?: boolean;
  /** Core log threshold, 0 trace .. 5 fatal. Default 2. */
  readonly logLevel?: number;
  /** Platform name for the core's `RuntimeConfig`. Default `"react-native-<Platform.OS>"`. */
  readonly platform?: string;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * The JSI object of the native core `namespace`: installs it on first use
 * (`UndraNative.install(namespace)`) and returns `globalThis.__undraNative[namespace]`. Throws
 * `UndraTransportError("unsupported")` when the TurboModule is not linked into the app, or when the app
 * has no core of that namespace (or one the module refuses: another C ABI version).
 */
export function installNative(namespace: string): UndraNativeModule {
  const g = globalThis as { __undraNative?: Record<string, UndraNativeModule | undefined> };
  if (g.__undraNative?.[namespace] === undefined) {
    // Looked up here rather than at import, so importing this package never touches the registry.
    const NativeUndra = TurboModuleRegistry.get<Spec>("UndraNative");
    if (NativeUndra === null || NativeUndra === undefined) {
      throw new UndraTransportError(
        "unsupported",
        "the UndraNative TurboModule is not linked into this app: add @undra/react-native as a dependency, run `pod install` (iOS) and rebuild (docs/REACT_NATIVE.md)",
      );
    }
    try {
      NativeUndra.install(namespace);
    } catch (error) {
      // The module's own message says what is missing and how to build it.
      throw new UndraTransportError("unsupported", messageOf(error), { cause: error });
    }
  }
  const native = g.__undraNative?.[namespace];
  if (native === undefined) {
    throw new UndraTransportError("unsupported", `UndraNative.install("${namespace}") did not install __undraNative.${namespace}`);
  }
  return native;
}

/**
 * The standard ports the module answers natively on this device, and where they keep their data
 * (ADR-038 amendment B): `{ ports, kv, fs, secureStore }`, or `{ ports: [], error }` when the platform
 * has none (on Android: the package's Java library or its context is missing). Asked of the module of
 * the core `namespace`, which it installs on first use like {@link loadNative}; the answer is the
 * device's, the same for every core.
 */
export function nativePlatformDefaults(namespace: string): NativePlatformDefaults {
  return platformDefaultsOf(installNative(namespace));
}

function platformDefaultsOf(native: UndraNativeModule): NativePlatformDefaults {
  try {
    return typeof native.platformDefaults === "function" ? native.platformDefaults() : { ports: [] };
  } catch (error) {
    return { ports: [], error: messageOf(error) };
  }
}

/**
 * Loads a native core, linked into the app with `@undra/react-native` (ADR-038), through its generated
 * entry, and resolves with the running {@link UndraCore}. The core becomes the entry's `core` (the one
 * every generated class and function of its bindings uses by default), and the first core loaded
 * becomes `UndraCore.shared` (for `@undra/runtime/react`'s `useUndra`).
 *
 * ```ts
 * import { loadNative } from "@undra/react-native";            // first: it installs what Hermes lacks
 * import { Todos, UndraAcmePay } from "@acme/pay-core";        // the generated bindings
 *
 * await loadNative(UndraAcmePay);
 * const todos = await Todos.create();                           // on UndraAcmePay.core
 * ```
 *
 * The entry may also be `{ namespace, schemaHash }` (`UndraIds.namespace`, `UndraIds.schemaHash`). An app
 * may load several cores, one per namespace (ADR-044); loading a namespace again while its core is open
 * (or still loading) resolves with the same core, and after `close()` starts a new one.
 *
 * Every standard port has a default (ADR-038 amendment B): `Kv`, `SecureStore`, `Fs` and the
 * `Connectivity` source are the module's, native and off the JS thread (files in the app's private
 * storage, the Keychain or the Android Keystore, `NWPathMonitor` or `ConnectivityManager`); `Http` is
 * React Native's `fetch` and `Lifecycle` its `AppState`; `Clock`, `Rng`, `Log` and `Timer` are native.
 * A value in `adapters` (or an implementation in `ports`) replaces a default, and `null` removes it.
 * Replace a native default here: `core.registerPort` after the load does not reach a port the module
 * answers itself. The native defaults keep their data in one place per app, shared by its cores.
 *
 * Rejects with `UndraSchemaMismatchError` when the core was built from another schema (checked before
 * the core starts), with `UndraTransportError` when the module is not linked, the app has no core of
 * that namespace, or the core does not start, and with the entry's own error when the entry already
 * has a core that `loadNative` did not load.
 */
export function loadNative(entry: NativeCoreEntry, options: NativeLoadOptions = {}): Promise<UndraCore> {
  const pending = loading.get(entry.namespace);
  if (pending !== undefined) {
    return pending.then(
      (core) => (core.closed ? start(entry, options) : core),
      () => start(entry, options),
    );
  }
  return start(entry, options);
}

/** The last load of each namespace, until it fails. */
const loading = new Map<string, Promise<UndraCore>>();

function start(entry: NativeCoreEntry, options: NativeLoadOptions): Promise<UndraCore> {
  const namespace = entry.namespace;
  let native: UndraNativeModule;
  try {
    native = installNative(namespace);
  } catch (error) {
    return Promise.reject(error);
  }
  const adapters = { ...reactNativeAdapters(), ...options.adapters };
  let core: UndraCore | undefined;
  // What the module and the frame scheduler find wrong has no caller to reject: it is the runtime's to report
  // (ADR-032, amendment A), which logs it at error level and hands `onError` an `UndraUnhandledError`. Before
  // the core exists (the handshake is still running) it is only logged.
  const report =
    (operation: string) =>
    (error: unknown): void => {
      if (core !== undefined) {
        core.report(error, operation);
      } else {
        adapters.log?.log(4, "undra::react-native", `${operation} failed before the core was up: ${messageOf(error)}`);
      }
    };
  const offered = platformDefaultsOf(native);
  if (offered.error !== undefined && offered.error !== "") {
    adapters.log?.log(3, "undra::react-native", `the native default ports are off on this device: ${offered.error}`);
  }
  const nativePorts = nativeDefaultPorts(offered, options);
  // A port the module answers natively has no JavaScript adapter: `UndraCore.attach` would otherwise fill it from
  // the web's defaults where an app polyfills what they look for (`navigator.onLine` and a global
  // `addEventListener` give a second Connectivity source, reporting next to the native one).
  const attachAdapters: AdapterOverrides = { ...adapters };
  for (const name of nativeAdapterNames(nativePorts)) Object.assign(attachAdapters, { [name]: null });
  const transport = new NativeTransport({
    namespace,
    native,
    nativePorts,
    expectedSchemaHash: entry.schemaHash,
    platform: options.platform ?? `react-native-${Platform.OS}`,
    ...(options.devtools !== undefined && { devtools: options.devtools }),
    ...(options.logLevel !== undefined && { logLevel: options.logLevel }),
    onError: report("the native module"),
  });
  const schedule = nativeFrameScheduler(native, {
    isActive: () => AppState.currentState === "active",
    onError: report("the frame scheduler"),
  });
  const attach: Omit<AttachOptions, "expectedSchemaHash"> = {
    ...options,
    adapters: attachAdapters,
    mirror: { schedule, ...options.mirror },
  };
  const attached =
    entry.attach !== undefined
      ? entry.attach(transport, attach)
      : UndraCore.attach(transport, { ...attach, expectedSchemaHash: entry.schemaHash });
  const started = attached.then((ready) => {
    core = ready;
    return ready;
  });
  loading.set(namespace, started);
  started.catch(() => {
    if (loading.get(namespace) === started) loading.delete(namespace);
  });
  return started;
}
