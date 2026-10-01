import type { TurboModule } from "react-native";
import { TurboModuleRegistry } from "react-native";

/**
 * The codegen spec of the `UndraNative` TurboModule (ADR-038, decision 1). Its one method,
 * `install`, loads the native core and installs `globalThis.__undraNative`, the JSI object that
 * `NativeTransport` talks to; nothing else crosses through TurboModule dispatch.
 */
export interface Spec extends TurboModule {
  /** Installs `globalThis.__undraNative` in this runtime (idempotent). Throws when the core cannot be loaded. */
  install(): boolean;
}

export default TurboModuleRegistry.get<Spec>("UndraNative");
