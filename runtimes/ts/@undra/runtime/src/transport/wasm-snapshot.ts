import { UndraRestoreError, UndraTransportError } from "../errors.js";
import type { WasmHost, WasmMainOptions } from "./wasm-main.js";

/*
 * Snapshot, restore and the twin of the in-process host (ADR-049, ADR-057), as functions over it: a page that never takes a
 * snapshot does not carry them. `UndraCore.snapshot` and `restore` load this module (through `core-extras.ts`), and so does crash
 * recovery; `WasmMainTransport` keeps the same operations as methods that call these.
 */

/** `undra_snapshot`, copied out of wasm memory, at once; throws `UndraTransportError` when the core cannot be asked (closed, trapped, no such export). */
export function takeSnapshot(host: WasmHost): Uint8Array {
  return host._run((e) => {
    if (e.undra_snapshot === undefined) throw new UndraTransportError("unsupported", "the core does not export undra_snapshot");
    return host._takeBuf(e, e.undra_snapshot());
  });
}

/**
 * Rebuilds the stores from `bytes` (`undra_restore`). The change-sets of the observed signals the core re-delivers during the
 * restore (ADR-023) have reached the handler when this returns. Throws `UndraRestoreError` when the core refuses the bytes (it is
 * unchanged).
 */
export function restoreInto(host: WasmHost, bytes: Uint8Array): void {
  const code = host._invoke(bytes, (e, ptr, len) => {
    if (e.undra_restore === undefined) throw new UndraTransportError("unsupported", "the core does not export undra_restore");
    return e.undra_restore(ptr, len);
  });
  if (code !== 0) throw new UndraRestoreError(code);
}

/**
 * A new host of the same class over the same compiled module (no recompile) and options, not started: what a restart after a trap
 * runs on (ADR-049, `crashRecovery`). `host` stays dead.
 */
export function twin<T extends WasmHost>(host: T): T {
  return new (host.constructor as new (options: WasmMainOptions) => T)({ ...host._options, wasm: host._module ?? host._options.wasm });
}
