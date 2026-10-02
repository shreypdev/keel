import { UndraReader } from "./reader.js";
import { UndraWriter } from "./writer.js";

/*
 * The payloads only a framed transport (`remote`, the worker's) handles: the handshake, the core's log
 * records and the core's calls into a port. In a module of their own so that an app which runs its core
 * in-process, and so loads neither transport, does not carry them (ADR-052). `payloads.ts` re-exports them,
 * so the wire barrel and every import path keep working.
 */

/** Decodes a whole payload with `read` and asserts nothing is left over. */
function decodeAll<T>(bytes: Uint8Array, read: (r: UndraReader) => T): T {
  const r = new UndraReader(bytes);
  const value = read(r);
  r.finish();
  return value;
}

/** Decodes a payload whose last field is an opaque tail consumed by `read` via `readRest`. */
function decodeWithTail<T>(bytes: Uint8Array, read: (r: UndraReader) => T): T {
  return read(new UndraReader(bytes));
}

/** Payload of a `PortCall`: the core asks the platform to run a port method. */
export interface PortCallPayload {
  /** Port being called (`fnv1a32("port.<Trait>")`). */
  readonly portId: number;
  /** Method being called (`fnv1a32("<Trait>.<method>")`). */
  readonly methodId: number;
  /** Chosen by the core, unique among in-flight port calls. */
  readonly portCallId: number;
  /** Encoded parameters in declaration order. */
  readonly args: Uint8Array;
}

/** Encodes a `PortCall` payload. */
export function encodePortCall(call: PortCallPayload): Uint8Array {
  const w = new UndraWriter(12 + call.args.length);
  w.writeU32(call.portId);
  w.writeU32(call.methodId);
  w.writeU32(call.portCallId);
  w.writeRaw(call.args);
  return w.finish();
}

/** Decodes a `PortCall` payload. `args` is a borrowed view into `bytes`. */
export function decodePortCall(bytes: Uint8Array): PortCallPayload {
  return decodeWithTail(bytes, (r) => {
    const portId = r.readU32();
    const methodId = r.readU32();
    const portCallId = r.readU32();
    return { portId, methodId, portCallId, args: r.readRest() };
  });
}

/** Payload of a `Hello`, the handshake each side sends first. */
export interface HelloPayload {
  /** Version of the Undra crate or runtime that sent it. */
  readonly undraVersion: string;
  /** Schema hash of the sender; a mismatch is `UndraSchemaMismatch`. */
  readonly schemaHash: bigint;
  /** Platform name, for example `"web"`, `"ios"`, `"android"`, `"node"`. */
  readonly platform: string;
  /** Operating mode, for example `"prod"` or `"dev"`. */
  readonly mode: string;
}

/** Encodes a `Hello` payload. */
export function encodeHello(hello: HelloPayload): Uint8Array {
  const w = new UndraWriter(32);
  w.writeStr(hello.undraVersion);
  w.writeU64(hello.schemaHash);
  w.writeStr(hello.platform);
  w.writeStr(hello.mode);
  return w.finish();
}

/** Decodes a `Hello` payload. */
export function decodeHello(bytes: Uint8Array): HelloPayload {
  return decodeAll(bytes, (r) => {
    const undraVersion = r.readStr();
    const schemaHash = r.readU64();
    const platform = r.readStr();
    const mode = r.readStr();
    return { undraVersion, schemaHash, platform, mode };
  });
}

/** Payload of a `Log`: a record emitted by the core. */
export interface LogPayload {
  /** Severity as defined by the `Log` port (`u8`). */
  readonly level: number;
  /** Origin of the record. */
  readonly target: string;
  /** The message. */
  readonly message: string;
}

/** Encodes a `Log` payload. */
export function encodeLog(log: LogPayload): Uint8Array {
  const w = new UndraWriter(16 + log.target.length + log.message.length);
  w.writeU8(log.level);
  w.writeStr(log.target);
  w.writeStr(log.message);
  return w.finish();
}

/** Decodes a `Log` payload. */
export function decodeLog(bytes: Uint8Array): LogPayload {
  return decodeAll(bytes, (r) => {
    const level = r.readU8();
    const target = r.readStr();
    return { level, target, message: r.readStr() };
  });
}

