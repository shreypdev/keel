import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { fnv1a32, fnv1a64 } from "../src/fnv.js";
import { type Codec, codecs, decodeValue, encodeValue } from "../src/wire/codec.js";
import { type Kind, decodeEnvelope, encodeEnvelope } from "../src/wire/envelope.js";
import {
  type CallPayload,
  type ChangeEntry,
  type PatchOp,
  type ReplyPayload,
  CallTarget,
  ChangeOp,
  ReplyStatus,
  decodeCall,
  decodeChangeSet,
  decodePatch,
  decodeReply,
  encodeCall,
  encodeChangeSet,
  encodePatch,
  encodeReply,
} from "../src/wire/payloads.js";
import { UndraReader } from "../src/wire/reader.js";
import { durationFromNanos, joinHandle, makeHandle, splitHandle, handleGeneration, handleIndex } from "../src/wire/types.js";
import { UndraWriter } from "../src/wire/writer.js";
import { filterCodec, shapeCodec, todoCodec } from "./fixtures.js";
import { embedded, fromHex, toHex } from "./helpers.js";

/*
 * Table-driven conformance test over contract-tests/wire-vectors.json, the
 * vectors every runtime must agree on. Each vector is checked three ways:
 *   encode(value) == hex, decode(hex) == value (also from a view with a byteOffset),
 *   and re-encoding what was decoded gives hex again.
 * A vector this file does not know how to express fails the suite, so adding a
 * vector to the JSON cannot silently go untested here.
 */

interface Vector {
  name: string;
  type: string;
  value: unknown;
  hex: string;
  note: string;
}

const file = JSON.parse(
  readFileSync(new URL("../../../../../contract-tests/wire-vectors.json", import.meta.url), "utf8"),
) as { spec: string; vectors: Vector[] };

/** How to check one vector: build the bytes from the JSON value, and decode bytes and compare. */
interface Case {
  encode(): Uint8Array;
  /** Decodes `bytes` completely and asserts the result equals the vector's value. */
  check(bytes: Uint8Array): void;
  /** Bytes produced by decoding then re-encoding `bytes`. */
  reencode(bytes: Uint8Array): Uint8Array;
}

// -- scalar and composite types, parsed from the vector's type string --------

interface TypeSpec {
  codec: Codec<unknown>;
  /** Converts the JSON representation of a value to the TypeScript one. */
  fromJson(json: unknown): unknown;
  /** `true` when JSON object keys of this type must be parsed as numbers. */
  numeric?: boolean;
}

function splitArgs(s: string): string[] {
  const out: string[] = [];
  let depth = 0;
  let start = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c === "<") depth++;
    else if (c === ">") depth--;
    else if (c === "," && depth === 0) {
      out.push(s.slice(start, i));
      start = i + 1;
    }
  }
  out.push(s.slice(start));
  return out;
}

function asCodec<T>(codec: Codec<T>): Codec<unknown> {
  return codec as Codec<unknown>;
}

function parseType(type: string): TypeSpec | undefined {
  const num = (codec: Codec<number>): TypeSpec => ({ codec: asCodec(codec), fromJson: (j) => j, numeric: true });
  const big = (codec: Codec<bigint>): TypeSpec => ({ codec: asCodec(codec), fromJson: (j) => BigInt(j as string) });
  switch (type) {
    case "bool":
      return { codec: asCodec(codecs.bool), fromJson: (j) => j };
    case "u8":
      return num(codecs.u8);
    case "i8":
      return num(codecs.i8);
    case "u16":
      return num(codecs.u16);
    case "i16":
      return num(codecs.i16);
    case "u32":
      return num(codecs.u32);
    case "i32":
      return num(codecs.i32);
    case "f32":
      return num(codecs.f32);
    case "f64":
      return num(codecs.f64);
    case "u64":
      return big(codecs.u64);
    case "i64":
      return big(codecs.i64);
    case "handle":
      return big(codecs.handle);
    case "string":
      return { codec: asCodec(codecs.string), fromJson: (j) => j };
    case "bytes":
      return { codec: asCodec(codecs.bytes), fromJson: (j) => Uint8Array.from(j as number[]) };
    case "uuid":
      return { codec: asCodec(codecs.uuid), fromJson: (j) => j };
    case "duration":
      // The vector states nanoseconds; the TypeScript value is milliseconds.
      return { codec: asCodec(codecs.duration), fromJson: (j) => durationFromNanos(BigInt(j as string)) };
    case "timestamp":
      return { codec: asCodec(codecs.timestamp), fromJson: (j) => Number(BigInt(j as string)) };
  }
  const generic = /^(option|vec|map|result)<(.*)>$/.exec(type);
  if (!generic) return undefined;
  const name = generic[1];
  const args = splitArgs(generic[2] as string).map((t) => parseType(t.trim()));
  if (args.some((a) => a === undefined)) return undefined;
  const [a, b] = args as [TypeSpec, TypeSpec | undefined];
  switch (name) {
    case "option":
      return { codec: asCodec(codecs.option(a.codec)), fromJson: (j) => (j === null ? null : a.fromJson(j)) };
    case "vec":
      return { codec: asCodec(codecs.vec(a.codec)), fromJson: (j) => (j as unknown[]).map((x) => a.fromJson(x)) };
    case "map":
      if (!b) return undefined;
      return {
        codec: asCodec(codecs.map(a.codec, b.codec)),
        fromJson: (j) =>
          new Map(
            Object.entries(j as Record<string, unknown>).map(([k, v]) => [
              a.fromJson(a.numeric ? Number(k) : k),
              b.fromJson(v),
            ]),
          ),
      };
    case "result":
      if (!b) return undefined;
      return {
        codec: asCodec(codecs.result(a.codec, b.codec)),
        fromJson: (j) => {
          const r = j as { ok?: unknown; err?: unknown };
          return "ok" in r ? { ok: a.fromJson(r.ok) } : { err: b.fromJson(r.err) };
        },
      };
  }
  return undefined;
}

function codecCase(codec: Codec<unknown>, expected: unknown): Case {
  return {
    encode: () => encodeValue(codec, expected),
    check: (bytes) => expect(decodeValue(codec, bytes)).toEqual(expected),
    reencode: (bytes) => encodeValue(codec, decodeValue(codec, bytes)),
  };
}

// -- records and enums: the vectors name a schema, the fixtures implement it ------

const SCHEMA_CODECS: [RegExp, Codec<unknown>][] = [
  [/^record Todo\{id:uuid,title:string,done:bool\}$/, asCodec(todoCodec)],
  [/^enum Filter\{All,Active,Done\}$/, asCodec(filterCodec)],
  [/^enum Shape\{Circle\{radius:f64\},Rect\{w:f64,h:f64\}\}$/, asCodec(shapeCodec)],
];

// -- payload vectors --------------------------------------------------------

interface JsonCall {
  target: number;
  handle: string;
  method_id: string;
  call_id: number;
  args: number[];
}

/** The call and reply vectors give their argument and body as i32 values. */
function i32s(values: number[]): Uint8Array {
  const w = new UndraWriter();
  for (const v of values) w.writeI32(v);
  return w.finish();
}

function callCase(v: Vector): Case {
  const json = v.value as JsonCall;
  expect(json.target).toBe(CallTarget.ObjectMethod);
  const call: CallPayload = {
    target: CallTarget.ObjectMethod,
    handle: BigInt(json.handle),
    methodId: Number(json.method_id),
    callId: json.call_id,
    args: i32s(json.args),
  };
  return {
    encode: () => encodeCall(call),
    check: (bytes) => expect(decodeCall(bytes)).toEqual(call),
    reencode: (bytes) => encodeCall(decodeCall(bytes)),
  };
}

function replyCase(v: Vector): Case {
  const json = v.value as { call_id: number; status: number; body: number };
  expect(json.status).toBe(ReplyStatus.Ok);
  const reply: ReplyPayload = { callId: json.call_id, status: ReplyStatus.Ok, body: i32s([json.body]) };
  return {
    encode: () => encodeReply(reply),
    check: (bytes) => expect(decodeReply(bytes)).toEqual(reply),
    reencode: (bytes) => encodeReply(decodeReply(bytes)),
  };
}

function changeSetCase(v: Vector): Case {
  const json = v.value as {
    txn_id: string;
    entries: { handle: string; signal_id: number; op: number; value: number[] }[];
  };
  // Each entry's value is a Vec<i32> in the vector.
  const entries: ChangeEntry[] = json.entries.map((e) => ({
    handle: BigInt(e.handle),
    signalId: e.signal_id,
    op: e.op as ChangeOp,
    value: encodeValue(codecs.vec(codecs.i32), e.value),
  }));
  expect(entries.every((e) => e.op === ChangeOp.FullValue)).toBe(true);
  const changeSet = { txnId: BigInt(json.txn_id), entries };
  return {
    encode: () => encodeChangeSet(changeSet),
    check: (bytes) => expect(decodeChangeSet(bytes)).toEqual(changeSet),
    reencode: (bytes) => encodeChangeSet(decodeChangeSet(bytes)),
  };
}

function patchCase(v: Vector): Case {
  const ops = (v.value as { ops: PatchOp<number>[] }).ops;
  const decode = (bytes: Uint8Array): PatchOp<number>[] => {
    const r = new UndraReader(bytes);
    const decoded = decodePatch(r, codecs.i32);
    r.finish();
    return decoded;
  };
  const encode = (list: PatchOp<number>[]): Uint8Array => {
    const w = new UndraWriter();
    encodePatch(w, list, codecs.i32);
    return w.finish();
  };
  return {
    encode: () => encode(ops),
    check: (bytes) => expect(decode(bytes)).toEqual(ops),
    reencode: (bytes) => encode(decode(bytes)),
  };
}

function envelopeCase(v: Vector): Case {
  const json = v.value as { kind: number; seq: number; schema: string; payload_hex: string };
  const payload = fromHex(json.payload_hex);
  return {
    encode: () => encodeEnvelope(json.kind as Kind, json.seq, BigInt(json.schema), payload),
    check: (bytes) => {
      const env = decodeEnvelope(bytes);
      expect(env.kind).toBe(json.kind);
      expect(env.seq).toBe(json.seq);
      expect(env.schemaHash).toBe(BigInt(json.schema));
      expect(toHex(env.payload)).toBe(json.payload_hex);
    },
    reencode: (bytes) => {
      const env = decodeEnvelope(bytes);
      return encodeEnvelope(env.kind, env.seq, env.schemaHash, env.payload);
    },
  };
}

function fnvCase(bits: 32 | 64, input: string, v: Vector): Case {
  const expected = BigInt(v.value as string);
  const write = (w: UndraWriter, n: bigint): void => (bits === 32 ? w.writeU32(Number(n)) : w.writeU64(n));
  const hash = (): bigint => (bits === 32 ? BigInt(fnv1a32(input)) : fnv1a64(input));
  return {
    // The vector's hex is the hash in little-endian order.
    encode: () => {
      const w = new UndraWriter();
      write(w, hash());
      return w.finish();
    },
    check: (bytes) => {
      const r = new UndraReader(bytes);
      expect(bits === 32 ? BigInt(r.readU32()) : r.readU64()).toBe(expected);
      r.finish();
      expect(hash()).toBe(expected);
    },
    reencode: (bytes) => bytes,
  };
}

function handleCase(v: Vector): Case {
  const base = codecCase(asCodec(codecs.handle), BigInt(v.value as string));
  return {
    ...base,
    check: (bytes) => {
      base.check(bytes);
      // "index 1, generation 1"
      const h = BigInt(v.value as string);
      expect(handleIndex(h)).toBe(1);
      expect(handleGeneration(h)).toBe(1);
      expect(makeHandle(1, 1)).toBe(h);
      const { lo, hi } = splitHandle(h);
      expect(joinHandle(lo, hi)).toBe(h);
    },
  };
}

function caseFor(v: Vector): Case | undefined {
  for (const [pattern, codec] of SCHEMA_CODECS) {
    if (pattern.test(v.type)) return codecCase(codec, v.value);
  }
  if (v.type === "handle") return handleCase(v);
  if (v.type === "envelope") return envelopeCase(v);
  if (v.type === "call payload") return callCase(v);
  if (v.type === "reply payload") return replyCase(v);
  if (v.type === "changeset payload") return changeSetCase(v);
  if (v.type === "keyed patch (item i32)") return patchCase(v);
  const fnv = /^fnv1a(32|64)\("(.*)"\)$/.exec(v.type);
  if (fnv) return fnvCase(fnv[1] === "32" ? 32 : 64, fnv[2] as string, v);
  const spec = parseType(v.type);
  if (spec) return codecCase(spec.codec, spec.fromJson(v.value));
  return undefined;
}

describe("contract-tests/wire-vectors.json", () => {
  it("is the SPEC section 3 vector file and is not empty", () => {
    expect(file.spec).toContain("SPEC.md");
    expect(file.vectors.length).toBeGreaterThanOrEqual(30);
  });

  it("has unique vector names", () => {
    const names = file.vectors.map((v) => v.name);
    expect(new Set(names).size).toBe(names.length);
  });

  it("is fully expressed by this runtime: no vector is skipped", () => {
    const unhandled = file.vectors.filter((v) => caseFor(v) === undefined).map((v) => `${v.name} (${v.type})`);
    expect(unhandled).toEqual([]);
  });

  describe.each(file.vectors.map((v) => [v.name, v] as const))("%s", (_name, vector) => {
    const c = caseFor(vector) as Case;
    const hex = vector.hex;

    it("encodes to the vector bytes", () => {
      expect(toHex(c.encode())).toBe(hex);
    });

    it("decodes the vector bytes to the vector value", () => {
      c.check(fromHex(hex));
    });

    it("decodes from a view with a non-zero byteOffset", () => {
      c.check(embedded(fromHex(hex)));
    });

    it("re-encodes what it decoded to the same bytes", () => {
      expect(toHex(c.reencode(fromHex(hex)))).toBe(hex);
    });
  });
});
