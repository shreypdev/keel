// PROTOTYPE (ADR-057 lever d7): snapshot, restore and the twin of the in-process host, as functions: a page that never
// snapshots ships none of it. `UndraCore.snapshot` / `restore` and crash recovery load this module.
import { UndraRestoreError, UndraTransportError } from "../errors.js";
import type { WasmHost, WasmMainOptions } from "./wasm-main.js";

/** `undra_snapshot`, copied out of wasm memory, at once; throws `UndraTransportError` when the core cannot be asked. */
export function takeSnapshot(host: WasmHost): Uint8Array {
  return host._run((e) => {
    if (e.undra_snapshot === undefined) throw new UndraTransportError("unsupported", "the core does not export undra_snapshot");
    return host._takeBuf(e, e.undra_snapshot());
  });
}

/** `undra_restore`; throws `UndraRestoreError` for a non-zero code. */
export function restoreInto(host: WasmHost, bytes: Uint8Array): void {
  const code = host._invoke(bytes, (e, ptr, len) => {
    if (e.undra_restore === undefined) throw new UndraTransportError("unsupported", "the core does not export undra_restore");
    return e.undra_restore(ptr, len);
  });
  if (code !== 0) throw new UndraRestoreError(code);
}

/** A new host over the same compiled module (no recompile) and options, not started (ADR-049). */
export function twin<T extends WasmHost>(host: T): T {
  return new (host.constructor as new (options: WasmMainOptions) => T)({ ...host._options, wasm: host._module ?? host._options.wasm });
}
