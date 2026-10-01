import type { TurboModule } from "react-native";
import { TurboModuleRegistry } from "react-native";

/**
 * The codegen spec of the `UndraNative` TurboModule (ADR-038, decision 1). Its one method,
 * `install(namespace)`, loads the native core of that namespace (ADR-044) and installs
 * `globalThis.__undraNative[namespace]`, the JSI object that `NativeTransport` talks to; nothing
 * else crosses through TurboModule dispatch.
 */
export interface Spec extends TurboModule {
  /**
   * Installs `globalThis.__undraNative[coreNamespace]` in this runtime (idempotent per namespace).
   * Throws when the app has no core of that namespace, or its table is refused (another ABI
   * version). (Not `namespace`: codegen writes the parameter into Objective-C++, where that is a
   * keyword.)
   */
  install(coreNamespace: string): boolean;
}

export default TurboModuleRegistry.get<Spec>("UndraNative");
