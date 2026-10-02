import { UndraTransportError } from "./errors.js";

/*
 * The panic report of a wasm core that trapped (ADR-046 decision 4.4, the part this runtime implements). Small and on
 * every core's path (`LoadOptions.onPanic` hears every trap); crash recovery (`crashRecovery`, ADR-049) is elsewhere.
 */

/**
 * What the TypeScript runtime knows about a panic that trapped a wasm core (ADR-046 decision 4.4, the part this
 * runtime implements): the panic's own record, which the core logs at FATAL level with target `undra::panic`
 * before it traps, and the stack of the trap. Delivered to `LoadOptions.onPanic` once per trap, before a
 * restart (ADR-049) and whether or not recovery is on.
 */
export interface UndraPanicReport {
  /** The panic message from the core's FATAL `undra::panic` record; the trap's own text when the core logged none (a stack overflow, out of memory). */
  readonly message: string;
  /** `file:line[:column]` of the panic when the record carried it (a panic before `undra_init`), else `""`. */
  readonly location: string;
  /** What the runtime was running when the core trapped, as far as it knows: `"wasm-main"` or `"wasm-worker"`, and the trap's text. */
  readonly operation: string;
  /** The frames of the trap's stack that are in the wasm module (`wasm-function[123]:0x4567`, or a name in a build with names), innermost first. */
  readonly frames: readonly string[];
  /** The schema hash of the core. */
  readonly schemaHash: bigint;
  /** The text of the trap (`RuntimeError: unreachable`, ...). */
  readonly trap: string;
}

/** The stack of a trap: the engine's, carried as the `cause` of the transport's error (or its own stack). */
export function trapStack(error: unknown): string {
  const e = error as { cause?: { stack?: unknown } | null; stack?: unknown } | null;
  const stack = e?.cause?.stack ?? e?.stack;
  return typeof stack === "string" ? stack : "";
}

/** The panic report of a trap, from the core's last FATAL `undra::panic` record (if any) and the trap. */
export function panicReport(record: string | null, trap: Error, schemaHash: bigint, mode: string): UndraPanicReport {
  const cause = trap.cause;
  const text = cause instanceof Error ? `${cause.name}: ${cause.message}` : trap.message;
  // The record of a panic before `undra_init` ends with " at file:line"; on wasm a runtime record is the message alone.
  const at = /^(.*) at (\S+:\d+(?::\d+)?)$/s.exec(record ?? "");
  return {
    message: at?.[1] ?? record ?? text,
    location: at?.[2] ?? "",
    operation: `${mode}: ${text}`,
    frames: trapStack(trap)
      .split("\n")
      .filter((line) => /wasm-function\[|\.wasm/.test(line))
      .map((line) => line.trim().replace(/^at /, "")),
    schemaHash,
    trap: text,
  };
}

/** Whether `error` is the trap of a wasm core. */
export function isTrap(error: unknown): error is UndraTransportError {
  return error instanceof UndraTransportError && error.reason === "trap";
}
