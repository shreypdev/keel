/** Schema-driven decoding of wire values (SPEC 3.1) into plain JavaScript values the page can show and compare. */

import { UndraReader, WireError } from "@undra/runtime/wire";
import type { SchemaIndex, TypeRef } from "./schema.js";

/**
 * What a decoded value is: `null` (a unit, a `None`), booleans, numbers (a 64-bit integer that is not a safe integer is a
 * bigint), strings (a uuid too), bytes, arrays (a `Vec`), and objects. An object is a record (its fields), or one of the
 * tagged shapes: `{ $: "Variant", ...fields }` for an enum variant (tuple fields are named `0`, `1`) or a `Result`
 * (`Ok`/`Err`, field `value`), `{ $map: [[k, v], ..] }`, `{ $ts: ms }`, `{ $dur: ns }`, `{ $handle: bigint }` for an object
 * and `{ $lazy: bigint }` for a lazy list.
 */
export type Value = null | boolean | number | bigint | string | Uint8Array | Value[] | { [field: string]: Value };

const MAX_DEPTH = 48;

/** A value the schema cannot describe, or one nested deeper than the page reads. */
export class ValueError extends Error {}

function safe(n: bigint): number | bigint {
  return n >= BigInt(Number.MIN_SAFE_INTEGER) && n <= BigInt(Number.MAX_SAFE_INTEGER) ? Number(n) : n;
}

/** Reads a value of type `ty` from `r`. Throws a `WireError` for anything malformed. */
export function readValue(index: SchemaIndex, ty: TypeRef, r: UndraReader, depth = 0): Value {
  if (depth > MAX_DEPTH) throw new ValueError(`nested deeper than ${MAX_DEPTH} levels at offset ${r.position}`);
  switch (ty.kind) {
    case "bool":
      return r.readBool();
    case "i8":
      return r.readI8();
    case "i16":
      return r.readI16();
    case "i32":
      return r.readI32();
    case "u8":
      return r.readU8();
    case "u16":
      return r.readU16();
    case "u32":
      return r.readU32();
    case "i64":
      return safe(r.readI64());
    case "u64":
      return safe(r.readU64());
    case "f32":
      return r.readF32();
    case "f64":
      return r.readF64();
    case "unit":
      return null;
    case "string":
      return r.readStr();
    case "bytes":
      return r.readBytes().slice();
    case "uuid":
      return r.readUuid();
    case "duration":
      return { $dur: safe(r.readI64()) };
    case "timestamp":
      return { $ts: safe(r.readI64()) };
    case "option": {
      const at = r.position;
      const tag = r.readU8();
      if (tag === 0) return null;
      if (tag !== 1) throw new WireError({ code: "invalid_tag", tag, at, ty: "Option" });
      return readValue(index, ty.of, r, depth + 1);
    }
    case "vec": {
      const count = r.readLen(1);
      const out: Value[] = new Array<Value>(count);
      for (let i = 0; i < count; i++) out[i] = readValue(index, ty.of, r, depth + 1);
      return out;
    }
    case "map": {
      const count = r.readLen(1);
      const pairs: Value[] = new Array<Value>(count);
      for (let i = 0; i < count; i++) {
        pairs[i] = [readValue(index, ty.of[0], r, depth + 1), readValue(index, ty.of[1], r, depth + 1)];
      }
      return { $map: pairs };
    }
    case "result": {
      const at = r.position;
      const tag = r.readU8();
      if (tag === 0) return { $: "Ok", value: readValue(index, ty.of[0], r, depth + 1) };
      if (tag === 1) return { $: "Err", value: readValue(index, ty.of[1], r, depth + 1) };
      throw new WireError({ code: "invalid_tag", tag, at, ty: "Result" });
    }
    case "lazy":
      return { $lazy: r.readU64() };
    case "stream":
      throw new WireError({ code: "invalid_tag", tag: 0, at: r.position, ty: "Stream" });
    case "named":
      return readNamed(index, ty.of, r, depth);
  }
}

function readNamed(index: SchemaIndex, name: string, r: UndraReader, depth: number): Value {
  const record = index.records.get(name);
  if (record !== undefined) {
    const out: { [field: string]: Value } = {};
    for (const f of record.fields) out[f.name] = readValue(index, f.ty, r, depth + 1);
    return out;
  }
  const en = index.enums.get(name);
  if (en !== undefined) {
    const at = r.position;
    const tag = r.readU16();
    const variant = en.variants.find((v) => v.index === tag);
    if (variant === undefined) throw new WireError({ code: "invalid_tag", tag, at, ty: name });
    const out: { [field: string]: Value } = { $: variant.name };
    variant.fields.forEach((f, i) => {
      out[variant.tuple ? String(i) : f.name] = readValue(index, f.ty, r, depth + 1);
    });
    return out;
  }
  if (index.objectsByName.has(name)) return { $handle: r.readU64() };
  throw new WireError({ code: "invalid_tag", tag: 0, at: r.position, ty: `unknown type ${name}` });
}

/** Decodes `bytes` as a whole value of type `ty`. */
export function decodeValue(index: SchemaIndex, ty: TypeRef, bytes: Uint8Array): Value {
  const r = new UndraReader(bytes);
  const v = readValue(index, ty, r);
  r.finish();
  return v;
}

/** The keyed-patch operations of SPEC 3.8, with their items decoded. */
export type PatchOp =
  | { readonly op: "insert"; readonly index: number; readonly item: Value }
  | { readonly op: "remove"; readonly index: number }
  | { readonly op: "update"; readonly index: number; readonly item: Value }
  | { readonly op: "move"; readonly from: number; readonly to: number }
  | { readonly op: "clear" };

/** Decodes a keyed patch whose items are of type `item`. */
export function decodePatchOps(index: SchemaIndex, item: TypeRef, bytes: Uint8Array): PatchOp[] {
  const r = new UndraReader(bytes);
  const count = r.readLen(1);
  const ops: PatchOp[] = [];
  for (let i = 0; i < count; i++) {
    const at = r.position;
    const tag = r.readU8();
    switch (tag) {
      case 0:
        ops.push({ op: "insert", index: r.readU32(), item: readValue(index, item, r) });
        break;
      case 1:
        ops.push({ op: "remove", index: r.readU32() });
        break;
      case 2:
        ops.push({ op: "update", index: r.readU32(), item: readValue(index, item, r) });
        break;
      case 3:
        ops.push({ op: "move", from: r.readU32(), to: r.readU32() });
        break;
      case 4:
        ops.push({ op: "clear" });
        break;
      default:
        throw new WireError({ code: "invalid_tag", tag, at, ty: "PatchOp" });
    }
  }
  r.finish();
  return ops;
}

/** Thrown by {@link applyPatch} when an operation does not fit the list: the list is out of step with the core. */
export class PatchMismatch extends Error {}

/** Applies `ops` in order to a copy of `list` (SPEC 3.8: each index refers to the list the previous operation left). */
export function applyPatch(list: readonly Value[], ops: readonly PatchOp[]): Value[] {
  const out = list.slice();
  for (const op of ops) {
    switch (op.op) {
      case "insert":
        if (op.index > out.length) throw new PatchMismatch(`insert at ${op.index} of ${out.length}`);
        out.splice(op.index, 0, op.item);
        break;
      case "remove":
        if (op.index >= out.length) throw new PatchMismatch(`remove ${op.index} of ${out.length}`);
        out.splice(op.index, 1);
        break;
      case "update":
        if (op.index >= out.length) throw new PatchMismatch(`update ${op.index} of ${out.length}`);
        out[op.index] = op.item;
        break;
      case "move": {
        if (op.from >= out.length || op.to >= out.length) throw new PatchMismatch(`move ${op.from} to ${op.to} of ${out.length}`);
        const [item] = out.splice(op.from, 1);
        out.splice(op.to, 0, item as Value);
        break;
      }
      case "clear":
        out.length = 0;
        break;
    }
  }
  return out;
}

/** Decodes the arguments of a call: the parameters of `method`, in order, as `{ name: value }`. */
export function decodeArgs(index: SchemaIndex, params: readonly { name: string; ty: TypeRef }[], bytes: Uint8Array): { [name: string]: Value } {
  const r = new UndraReader(bytes);
  const out: { [name: string]: Value } = {};
  for (const p of params) out[p.name] = readValue(index, p.ty, r);
  r.finish();
  return out;
}

/**
 * Decodes the body of a port reply (SPEC 3.6): status 0 carries the method's `T`, status 1 its `E`; a method that returns a
 * `Result<T, E>` collapses into those two. `undefined` when there is nothing to decode (unavailable, or a unit).
 */
export function decodeReplyBody(index: SchemaIndex, returns: TypeRef, status: number, bytes: Uint8Array): Value | undefined {
  if (status === 2) return undefined;
  const ty = returns.kind === "result" ? (status === 0 ? returns.of[0] : returns.of[1]) : status === 0 ? returns : undefined;
  if (ty === undefined) return bytes;
  return decodeValue(index, ty, bytes);
}
