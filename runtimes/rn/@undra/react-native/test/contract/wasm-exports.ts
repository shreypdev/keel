import { Kind } from "@undra/runtime";
import type { NativeTransport } from "../../src/transport.js";
import { nativeOf } from "./harness.js";

/*
 * contract-tests/ts/src/wasm-exports.ts for the React Native column: the same four functions, over
 * `NativeTransport` and its stand-in (vitest.contract.config.ts swaps this file in).
 */

function native(transport: NativeTransport) {
  const n = nativeOf.get(transport);
  if (n === undefined) throw new Error("this transport was not booted by the React Native harness");
  return n;
}

/** The core's snapshot (`undra_snapshot`), through the transport. */
export function snapshot(transport: NativeTransport): Promise<Uint8Array> {
  return transport.snapshot();
}

/** The schema hash the core reports (`undra_schema_hash`). */
export function exportedSchemaHash(transport: NativeTransport): bigint {
  return native(transport).schemaHash();
}

/** The schema the core exports (`undra_schema_json`), parsed. */
export function exportedSchema(transport: NativeTransport): unknown {
  return JSON.parse(native(transport).schemaJson()) as unknown;
}

/** Restores the core from a snapshot (`Kind.Restore`, `undra_restore`). */
export function restore(transport: NativeTransport, bytes: Uint8Array): void {
  transport.send(Kind.Restore, bytes);
}
