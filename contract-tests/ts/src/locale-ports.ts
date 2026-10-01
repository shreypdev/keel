import { UndraIds, localePortImpl } from "@playground/core";

/**
 * The module of `LoadOptions.worker.ports` that S20 loads (ADR-049): the playground's synchronous `Locale` port,
 * implemented in the worker, where the core can wait for it. `hello()` answers `"Hola"`.
 */
export default {
  [UndraIds.Ports.Locale.portId]: localePortImpl({ hello: () => "Hola" }),
};
