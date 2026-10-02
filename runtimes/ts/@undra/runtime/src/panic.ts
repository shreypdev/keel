import { UndraTransportError } from "./errors.js";

/*
 * What every core's path needs of a trap (ADR-046 decision 4.4, ADR-049): its stack, and whether an error is one. Building the
 * report that `LoadOptions.onPanic` receives is `panic-report.ts`, which a page loads only when it asked for reports.
 */

/** The stack of a trap: the engine's, carried as the `cause` of the transport's error (or its own stack). */
export function trapStack(error: unknown): string {
  const e = error as { cause?: { stack?: unknown } | null; stack?: unknown } | null;
  const stack = e?.cause?.stack ?? e?.stack;
  return typeof stack === "string" ? stack : "";
}

/** Whether `error` is the trap of a wasm core. */
export function isTrap(error: unknown): error is UndraTransportError {
  return error instanceof UndraTransportError && error.reason === "trap";
}
