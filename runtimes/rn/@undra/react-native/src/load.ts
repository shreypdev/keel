import { type AttachOptions, UndraCore, UndraTransportError } from "@undra/runtime";
import { AppState, Platform, TurboModuleRegistry } from "react-native";
import { nativeDefaultPorts, reactNativeAdapters } from "./adapters.js";
import { nativeFrameScheduler } from "./frame.js";
import type { NativePlatformDefaults, UndraNativeModule } from "./native.js";
import type { Spec } from "./specs/NativeUndra.js";
import { NativeTransport } from "./transport.js";

/** Options of {@link loadNative}: those of `UndraCore.attach`, plus the core's configuration. */
export interface NativeLoadOptions extends AttachOptions {
  /** Run the core in `"dev"` mode (devtools log records, docs/SPEC.md section 5.10). */
  readonly devtools?: boolean;
  /** Core log threshold, 0 trace .. 5 fatal. Default 2. */
  readonly logLevel?: number;
  /** Platform name for the core's `RuntimeConfig`. Default `"react-native-<Platform.OS>"`. */
  readonly platform?: string;
}

/**
 * The JSI object of the native module: installs it on first use (`UndraNative.install()`) and
 * returns `globalThis.__undraNative`. Throws `UndraTransportError("unsupported")` when the
 * TurboModule is not linked into the app.
 */
export function installNative(): UndraNativeModule {
  const g = globalThis as { __undraNative?: UndraNativeModule };
  if (g.__undraNative === undefined) {
    // Looked up here rather than at import, so importing this package never touches the registry.
    const NativeUndra = TurboModuleRegistry.get<Spec>("UndraNative");
    if (NativeUndra === null || NativeUndra === undefined) {
      throw new UndraTransportError(
        "unsupported",
        "the UndraNative TurboModule is not linked into this app: add @undra/react-native as a dependency, run `pod install` (iOS) and rebuild (docs/REACT_NATIVE.md)",
      );
    }
    NativeUndra.install();
  }
  if (g.__undraNative === undefined) {
    throw new UndraTransportError("unsupported", "UndraNative.install() did not install __undraNative");
  }
  return g.__undraNative;
}

/**
 * The standard ports the module answers natively on this device, and where they keep their data
 * (ADR-038 amendment B): `{ ports, kv, fs, secureStore }`, or `{ ports: [], error }` when the platform
 * has none (on Android: the package's Java library or its context is missing). Installs the module
 * on first use, like {@link loadNative}.
 */
export function nativePlatformDefaults(): NativePlatformDefaults {
  return platformDefaultsOf(installNative());
}

function platformDefaultsOf(native: UndraNativeModule): NativePlatformDefaults {
  try {
    return typeof native.platformDefaults === "function" ? native.platformDefaults() : { ports: [] };
  } catch (error) {
    return { ports: [], error: messageOf(error) };
  }
}

/**
 * Loads the app's native core, linked into the app by `@undra/react-native` (ADR-038), and
 * resolves with the running {@link UndraCore}: the first one becomes `UndraCore.shared`, so the
 * generated bindings and `@undra/runtime/react` (`useUndra`, `useSignal`) use it as on the web.
 *
 * ```ts
 * import { loadNative } from "@undra/react-native";   // first: it installs what Hermes lacks
 * import { Todos, UndraIds } from "@acme/core";      // the generated bindings
 *
 * await loadNative({ expectedSchemaHash: UndraIds.schemaHash });
 * const todos = await Todos.create();
 * ```
 *
 * Every standard port has a default (ADR-038 amendment B): `Kv`, `SecureStore`, `Fs` and the
 * `Connectivity` source are the module's, native and off the JS thread (files in the app's private
 * storage, the Keychain or the Android Keystore, `NWPathMonitor` or `ConnectivityManager`); `Http` is
 * React Native's `fetch` and `Lifecycle` its `AppState`; `Clock`, `Rng`, `Log` and `Timer` are native.
 * A value in `adapters` (or an implementation in `ports`) replaces a default, and `null` removes it.
 * Replace a native default here: `core.registerPort` after the load does not reach a port the module
 * answers itself.
 *
 * Rejects with `UndraSchemaMismatchError` when the core was built from another schema (checked
 * before the core starts) and with `UndraTransportError` when the module is not linked or the core
 * does not start. There is one native core per process; a second `loadNative` while the first is
 * open resolves with the same core.
 */
export function loadNative(options: NativeLoadOptions): Promise<UndraCore> {
  if (current !== null) {
    const pending = current;
    return pending.then(
      (core) => (core.closed ? start(options) : core),
      () => start(options),
    );
  }
  return start(options);
}

let current: Promise<UndraCore> | null = null;

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function start(options: NativeLoadOptions): Promise<UndraCore> {
  let native: UndraNativeModule;
  try {
    native = installNative();
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
  const transport = new NativeTransport({
    native,
    nativePorts: nativeDefaultPorts(offered, options),
    expectedSchemaHash: options.expectedSchemaHash,
    platform: options.platform ?? `react-native-${Platform.OS}`,
    ...(options.devtools !== undefined && { devtools: options.devtools }),
    ...(options.logLevel !== undefined && { logLevel: options.logLevel }),
    onError: report("the native module"),
  });
  const schedule = nativeFrameScheduler(native, {
    isActive: () => AppState.currentState === "active",
    onError: report("the frame scheduler"),
  });
  const attach: AttachOptions = {
    ...options,
    adapters,
    mirror: { schedule, ...options.mirror },
  };
  const loading = UndraCore.attach(transport, attach).then((attached) => {
    core = attached;
    return attached;
  });
  current = loading;
  loading.catch(() => {
    if (current === loading) current = null;
  });
  return loading;
}
