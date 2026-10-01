import { type AttachOptions, UndraCore, UndraTransportError } from "@undra/runtime";
import { AppState, Platform, TurboModuleRegistry } from "react-native";
import { reactNativeAdapters } from "./adapters.js";
import { nativeFrameScheduler } from "./frame.js";
import type { UndraNativeModule } from "./native.js";
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
 * Loads the app's native core, linked into the app by `@undra/react-native` (ADR-038), and
 * resolves with the running {@link UndraCore}: the first one becomes `UndraCore.shared`, so the
 * generated bindings and `@undra/runtime/react` (`useUndra`, `useSignal`) use it as on the web.
 *
 * ```ts
 * import { loadNative } from "@undra/react-native";   // first: it installs what Hermes lacks
 * import { Todos, UndraIds } from "@acme/core";      // the generated bindings
 *
 * await loadNative({ expectedSchemaHash: UndraIds.schemaHash, adapters: { kv: myKv } });
 * const todos = await Todos.create();
 * ```
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

function start(options: NativeLoadOptions): Promise<UndraCore> {
  let native: UndraNativeModule;
  try {
    native = installNative();
  } catch (error) {
    return Promise.reject(error);
  }
  const transport = new NativeTransport({
    native,
    expectedSchemaHash: options.expectedSchemaHash,
    platform: options.platform ?? `react-native-${Platform.OS}`,
    ...(options.devtools !== undefined && { devtools: options.devtools }),
    ...(options.logLevel !== undefined && { logLevel: options.logLevel }),
    ...(options.onError !== undefined && { onError: options.onError }),
  });
  const schedule = nativeFrameScheduler(native, {
    isActive: () => AppState.currentState === "active",
    ...(options.onError !== undefined && { onError: options.onError }),
  });
  const attach: AttachOptions = {
    ...options,
    adapters: { ...reactNativeAdapters(), ...options.adapters },
    mirror: { schedule, ...options.mirror },
  };
  const loading = UndraCore.attach(transport, attach);
  current = loading;
  loading.catch(() => {
    if (current === loading) current = null;
  });
  return loading;
}
