import { Kind, type WasmMainTransport } from "@keel/runtime";

/** The wasm exports of a Keel core that `KeelCore` does not wrap (SPEC 7). */
interface CoreExports {
  readonly memory: WebAssembly.Memory;
  keel_schema_hash(): bigint;
  keel_schema_json(): number;
  keel_snapshot(): number;
  keel_buf_free(bufPtr: number): void;
}

function exportsOf(transport: WasmMainTransport): CoreExports {
  const instance = transport.instance;
  if (instance === null) throw new Error("the transport has not been started");
  return instance.exports as unknown as CoreExports;
}

/** Copies the bytes of a `KeelBuf { ptr, len, cap }` out of wasm memory and frees it. */
function takeBuf(e: CoreExports, bufPtr: number): Uint8Array {
  try {
    const view = new DataView(e.memory.buffer);
    const ptr = view.getUint32(bufPtr, true);
    const len = view.getUint32(bufPtr + 4, true);
    return new Uint8Array(e.memory.buffer, ptr, len).slice();
  } finally {
    e.keel_buf_free(bufPtr);
  }
}

/**
 * The core's snapshot (`keel_snapshot`): an opaque byte string that `restore` accepts. `KeelCore`
 * has no `snapshot()` (SPEC 17.1 lists none), so a scenario goes through the wasm export.
 */
export function snapshot(transport: WasmMainTransport): Uint8Array {
  const e = exportsOf(transport);
  return takeBuf(e, e.keel_snapshot());
}

/** The schema hash the module exports (`keel_schema_hash`), as an unsigned 64-bit number. */
export function exportedSchemaHash(transport: WasmMainTransport): bigint {
  return BigInt.asUintN(64, exportsOf(transport).keel_schema_hash());
}

/** The schema the core exports (`keel_schema_json`), parsed. */
export function exportedSchema(transport: WasmMainTransport): unknown {
  const e = exportsOf(transport);
  return JSON.parse(new TextDecoder().decode(takeBuf(e, e.keel_schema_json()))) as unknown;
}

/**
 * Restores the core from a snapshot (`Kind.Restore`, which drives `keel_restore`). Throws
 * `KeelTransportError` when the core refuses the bytes.
 */
export function restore(transport: WasmMainTransport, bytes: Uint8Array): void {
  transport.send(Kind.Restore, bytes);
}
