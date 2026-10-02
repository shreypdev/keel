import { describe, expect, it } from "vitest";
import { type Codec, codecs, decodeValue, encodeValue } from "../src/wire/codec.js";
import { Kind, decodeEnvelope, encodeEnvelope } from "../src/wire/envelope.js";
import { WireError } from "../src/wire/errors.js";
import {
  CallTarget,
  ChangeOp,
  PortStatus,
  ReplyStatus,
  StreamFlag,
  decodeCall,
  decodeCancel,
  decodeChangeSet,
  decodeEvent,
  decodeHello,
  decodeLog,
  decodeObserve,
  decodePatch,
  decodePortCall,
  decodePortReply,
  decodeRelease,
  decodeReply,
  decodeSnapshot,
  decodeStreamCredit,
  decodeStreamFailure,
  decodeStreamItem,
  decodeTimerFired,
  encodeCall,
  encodeCancel,
  encodeChangeSet,
  encodeEvent,
  encodeHello,
  encodeLog,
  encodeObserve,
  encodePatch,
  encodePortCall,
  encodePortReply,
  encodeRelease,
  encodeReply,
  encodeSnapshot,
  encodeStreamCredit,
  encodeStreamFailure,
  encodeStreamItem,
  encodeTimerFired,
  iterateChangeSet,
  readChangeSetHeader,
  type StreamFailure,
} from "../src/wire/payloads.js";
import { UndraReader } from "../src/wire/reader.js";
import { UndraWriter } from "../src/wire/writer.js";
import { filterCodec, shapeCodec, todoCodec } from "./fixtures.js";
import { fromHex, prng, randInt, randomBytes, toHex } from "./helpers.js";

/** Arrays of random bytes each decoder must survive. */
const RANDOM_ARRAYS = 5_000;
/** Mutated valid encodings each decoder must survive. */
const MUTATED_ARRAYS = 2_000;
const SEED = 0x5eed;

interface Target {
  name: string;
  /** Decodes `bytes` completely. Anything but a `WireError` escaping is a bug. */
  decode(bytes: Uint8Array): unknown;
  /** Well-formed inputs the mutation fuzzer starts from. */
  valid: Uint8Array[];
  /** If set, a successful decode re-encodes to exactly the input. */
  reencode?: (value: never) => Uint8Array;
}

function codecTarget<T>(name: string, codec: Codec<T>, samples: T[], canonical = true): Target {
  return {
    name,
    decode: (bytes) => decodeValue(codec, bytes),
    valid: samples.map((s) => encodeValue(codec, s)),
    ...(canonical ? { reencode: ((v: T) => encodeValue(codec, v)) as (value: never) => Uint8Array } : {}),
  };
}

const HANDLE = 4294967297n;
const TODO = { id: "123e4567-e89b-12d3-a456-426614174000", title: "Milk \u{1f30a}", done: true };

function patchBytes<T>(item: Codec<T>, ops: Parameters<typeof encodePatch<T>>[1]): Uint8Array {
  const w = new UndraWriter();
  encodePatch(w, ops, item);
  return w.finish();
}

function patchTarget<T>(name: string, item: Codec<T>, sample: T): Target {
  return {
    name,
    decode: (bytes) => {
      const r = new UndraReader(bytes);
      const ops = decodePatch(r, item);
      r.finish();
      return ops;
    },
    valid: [
      patchBytes(item, []),
      patchBytes(item, [
        { op: "insert", index: 0, item: sample },
        { op: "remove", index: 1 },
        { op: "update", index: 2, item: sample },
        { op: "move", from: 3, to: 0 },
        { op: "clear" },
      ]),
    ],
  };
}

/** Drives the reader through a byte-selected sequence of operations, so every reader method sees hostile input. */
function readerScript(bytes: Uint8Array): unknown {
  const r = new UndraReader(bytes);
  const script = bytes.subarray(0, 8);
  const out: unknown[] = [];
  for (let i = 0; i < 64; i++) {
    const op = (script[i % Math.max(script.length, 1)] ?? 0) % 22;
    switch (op) {
      case 0: out.push(r.readU8()); break;
      case 1: out.push(r.readI8()); break;
      case 2: out.push(r.readU16()); break;
      case 3: out.push(r.readI16()); break;
      case 4: out.push(r.readU32()); break;
      case 5: out.push(r.readI32()); break;
      case 6: out.push(r.readU64()); break;
      case 7: out.push(r.readI64()); break;
      case 8: out.push(r.readU64Number()); break;
      case 9: out.push(r.readI64Number()); break;
      case 10: out.push(r.readF32()); break;
      case 11: out.push(r.readF64()); break;
      case 12: out.push(r.readBool()); break;
      case 13: out.push(r.readLen()); break;
      case 14: out.push(r.readLen(8)); break;
      case 15: out.push(r.readStr()); break;
      case 16: out.push(r.readBytes()); break;
      case 17: out.push(r.readUuid()); break;
      case 18: out.push(r.readRaw(r.remaining >> 1)); break;
      case 19: out.push(r.readRest()); break;
      default: out.push(r.readU8());
    }
    if (r.remaining === 0) break;
  }
  r.finish();
  return out;
}

const targets: Target[] = [
  // primitives
  codecTarget("bool", codecs.bool, [true, false]),
  codecTarget("u8", codecs.u8, [0, 255]),
  codecTarget("i8", codecs.i8, [-128, 127]),
  codecTarget("u16", codecs.u16, [0, 65535]),
  codecTarget("i16", codecs.i16, [-32768, 32767]),
  codecTarget("u32", codecs.u32, [0, 4294967295]),
  codecTarget("i32", codecs.i32, [-2147483648, 2147483647]),
  codecTarget("u64", codecs.u64, [0n, 2n ** 64n - 1n]),
  codecTarget("i64", codecs.i64, [-(2n ** 63n), 2n ** 63n - 1n]),
  codecTarget("u64Number", codecs.u64Number, [0, Number.MAX_SAFE_INTEGER]),
  codecTarget("i64Number", codecs.i64Number, [Number.MIN_SAFE_INTEGER, Number.MAX_SAFE_INTEGER]),
  codecTarget("f32", codecs.f32, [0, 1.5, Number.NaN], false),
  codecTarget("f64", codecs.f64, [0, Math.PI, Number.NaN], false),
  codecTarget("unit", codecs.unit, [undefined]),
  codecTarget("string", codecs.string, ["", "hello", "héllo \u{1f30a}", "﻿x"]),
  codecTarget("bytes", codecs.bytes, [new Uint8Array(0), new Uint8Array([1, 2, 3])]),
  codecTarget("duration", codecs.duration, [0, 1500, 0.5], false),
  codecTarget("timestamp", codecs.timestamp, [0, 1727654400000, -1]),
  codecTarget("uuid", codecs.uuid, ["123e4567-e89b-12d3-a456-426614174000", "00000000-0000-0000-0000-000000000000"]),
  codecTarget("handle", codecs.handle, [0n, HANDLE]),
  // combinators and generated shapes
  codecTarget("option<string>", codecs.option(codecs.string), [null, "x"]),
  codecTarget("vec<u16>", codecs.vec(codecs.u16), [[], [1, 2, 3]]),
  codecTarget("vec<string>", codecs.vec(codecs.string), [[], ["a", "日本"]]),
  codecTarget("vec<vec<i32>>", codecs.vec(codecs.vec(codecs.i32)), [[], [[1], [], [2, 3]]]),
  codecTarget("vec<option<bytes>>", codecs.vec(codecs.option(codecs.bytes)), [[null, new Uint8Array([9])]]),
  codecTarget("vec<unit>", codecs.vec(codecs.unit), [[]]),
  codecTarget("map<string, i32>", codecs.map(codecs.string, codecs.i32), [new Map(), new Map([["a", 1], ["b", 2]])], false),
  codecTarget("map<u8, vec<string>>", codecs.map(codecs.u8, codecs.vec(codecs.string)), [new Map([[1, ["x"]], [2, []]])], false),
  codecTarget("result<i32, string>", codecs.result(codecs.i32, codecs.string), [{ ok: 5 }, { err: "bad" }]),
  codecTarget("result<option<u8>, vec<bool>>", codecs.result(codecs.option(codecs.u8), codecs.vec(codecs.bool)), [
    { ok: null },
    { ok: 3 },
    { err: [true, false] },
  ]),
  codecTarget("record Todo", todoCodec, [TODO]),
  codecTarget("enum Filter", filterCodec, ["all", "done"]),
  codecTarget("enum Shape", shapeCodec, [{ kind: "circle", radius: 1 }, { kind: "rect", w: 2, h: 3 }], false),
  // envelope
  {
    name: "envelope",
    decode: (bytes) => decodeEnvelope(bytes),
    valid: [
      encodeEnvelope(Kind.Call, 7, 0x0102030405060708n, fromHex("aabbcc")),
      encodeEnvelope(Kind.Hello, 0, 0n, new Uint8Array(0)),
      encodeEnvelope(Kind.Restore, 0xffffffff, 2n ** 64n - 1n, new Uint8Array(40)),
    ],
  },
  {
    name: "envelope (expected schema)",
    decode: (bytes) => decodeEnvelope(bytes, 0x0102030405060708n),
    valid: [encodeEnvelope(Kind.Reply, 1, 0x0102030405060708n, fromHex("01"))],
  },
  // payloads
  {
    name: "call",
    decode: decodeCall,
    valid: [
      encodeCall({ target: CallTarget.FreeFunction, methodId: 1, callId: 2, args: fromHex("0102") }),
      encodeCall({ target: CallTarget.ObjectMethod, handle: HANDLE, methodId: 1, callId: 2, args: fromHex("03") }),
      encodeCall({ target: CallTarget.Constructor, typeId: 4, methodId: 5, callId: 6, args: new Uint8Array(0) }),
      encodeCall({ target: CallTarget.LazyListPage, handle: HANDLE, offset: 0, limit: 10, callId: 7 }),
    ],
  },
  {
    name: "reply",
    decode: decodeReply,
    valid: [
      encodeReply({ callId: 1, status: ReplyStatus.Ok, body: fromHex("05000000") }),
      encodeReply({ callId: 2, status: ReplyStatus.Error, body: fromHex("00") }),
      encodeReply({ callId: 3, status: ReplyStatus.Panic, message: "boom", backtrace: "at x" }),
      encodeReply({ callId: 4, status: ReplyStatus.Cancelled }),
      encodeReply({ callId: 5, status: ReplyStatus.StreamOpened }),
      encodeReply({ callId: 6, status: ReplyStatus.BadRequest, reason: "unknown method" }),
    ],
  },
  {
    name: "changeset",
    decode: decodeChangeSet,
    valid: [
      encodeChangeSet({ txnId: 0n, entries: [] }),
      encodeChangeSet({
        txnId: 42n,
        entries: [
          { handle: HANDLE, signalId: 0, op: ChangeOp.FullValue, value: fromHex("0102") },
          { handle: HANDLE, signalId: 1, op: ChangeOp.KeyedPatch, value: fromHex("00000000") },
          { handle: HANDLE, signalId: 2, op: ChangeOp.LazyInvalidated, value: new Uint8Array(0) },
        ],
      }),
    ],
  },
  {
    name: "changeset (iterated)",
    decode: (bytes) => [...iterateChangeSet(bytes)],
    valid: [
      encodeChangeSet({
        txnId: 1n,
        entries: [
          { handle: 1n, signalId: 0, op: ChangeOp.FullValue, value: fromHex("aa") },
          { handle: 2n, signalId: 1, op: ChangeOp.FullValue, value: fromHex("bbcc") },
        ],
      }),
    ],
  },
  {
    name: "changeset header",
    decode: (bytes) => readChangeSetHeader(bytes),
    valid: [encodeChangeSet({ txnId: 9n, entries: [] })],
  },
  {
    name: "port call",
    decode: decodePortCall,
    valid: [encodePortCall({ portId: 1, methodId: 2, portCallId: 3, args: fromHex("0a") })],
  },
  {
    name: "port reply",
    decode: decodePortReply,
    valid: [
      encodePortReply({ portCallId: 1, status: PortStatus.Ok, body: fromHex("01") }),
      encodePortReply({ portCallId: 1, status: PortStatus.Unavailable, body: new Uint8Array(0) }),
    ],
  },
  { name: "cancel", decode: decodeCancel, valid: [encodeCancel({ callId: 3 })] },
  { name: "stream credit", decode: decodeStreamCredit, valid: [encodeStreamCredit({ callId: 3, credit: 16 })] },
  {
    name: "stream item",
    decode: decodeStreamItem,
    valid: [
      encodeStreamItem({ callId: 1, flag: StreamFlag.Item, body: fromHex("07000000") }),
      encodeStreamItem({ callId: 1, flag: StreamFlag.End }),
      encodeStreamItem({ callId: 1, flag: StreamFlag.Error, body: fromHex("00") }),
      encodeStreamItem({ callId: 1, flag: StreamFlag.Failed, failure: { status: ReplyStatus.Cancelled, message: "the runtime shut down", detail: "" } }),
      encodeStreamItem({ callId: 1, flag: StreamFlag.Failed, failure: { status: ReplyStatus.Panic, message: "boom", detail: "at x" } }),
    ],
  },
  {
    name: "stream failure",
    decode: decodeStreamFailure,
    valid: [
      encodeStreamFailure({ status: ReplyStatus.Panic, message: "boom \u{1f30a}", detail: "at core.rs:1" }),
      encodeStreamFailure({ status: ReplyStatus.Cancelled, message: "", detail: "" }),
      encodeStreamFailure({ status: ReplyStatus.BadRequest, message: "stale handle", detail: "" }),
    ],
    reencode: ((v: StreamFailure) => encodeStreamFailure(v)) as (value: never) => Uint8Array,
  },
  {
    name: "observe",
    decode: decodeObserve,
    valid: [encodeObserve({ handle: HANDLE, signalId: 1, on: true }), encodeObserve({ handle: 1n, signalId: 0, on: false })],
  },
  { name: "release", decode: decodeRelease, valid: [encodeRelease({ handle: HANDLE })] },
  {
    name: "event",
    decode: decodeEvent,
    valid: [encodeEvent({ portId: 1, methodId: 2, payload: fromHex("01") })],
  },
  {
    name: "hello",
    decode: decodeHello,
    valid: [encodeHello({ undraVersion: "0.1.0", schemaHash: 5n, platform: "web", mode: "dev" })],
  },
  { name: "log", decode: decodeLog, valid: [encodeLog({ level: 2, target: "undra", message: "hi \u{1f30a}" })] },
  { name: "timer fired", decode: decodeTimerFired, valid: [encodeTimerFired({ timerId: 4 })] },
  {
    name: "snapshot",
    decode: decodeSnapshot,
    valid: [
      encodeSnapshot({ generationFloor: 0, schemaHash: 0n, types: [], description: "", stores: [] }),
      encodeSnapshot({
        generationFloor: 3,
        schemaHash: 0x0123_4567_89ab_cdefn,
        types: [
          { typeId: 7, fingerprint: 1n },
          { typeId: 8, fingerprint: 0xffff_ffff_ffff_ffffn },
        ],
        description: '{"stores":[{"name":"Todos"}]}',
        stores: [
          {
            handle: HANDLE,
            typeId: 7,
            signals: [
              { signalId: 0, value: fromHex("0102") },
              { signalId: 1, value: new Uint8Array(0) },
            ],
          },
          { handle: 2n, typeId: 8, signals: [] },
        ],
      }),
    ],
  },
  // keyed patches
  patchTarget("patch<i32>", codecs.i32, 5),
  patchTarget("patch<string>", codecs.string, "x"),
  patchTarget("patch<Todo>", todoCodec, TODO),
  // the reader itself
  { name: "reader script", decode: readerScript, valid: [fromHex("0004 0102 0304 05060708 090a0b0c 0d0e0f10")] },
];

// -- input generation ---------------------------------------------------------

/** Byte values that steer a decoder towards valid tags, small lengths and boundary values. */
const INTERESTING = [0, 0, 0, 1, 1, 2, 3, 4, 5, 8, 16, 0x7f, 0x80, 0xc0, 0xe0, 0xf0, 0xff, 0xff];

function biasedBytes(rand: () => number, n: number): Uint8Array {
  const out = new Uint8Array(n);
  for (let i = 0; i < n; i++) {
    out[i] = rand() < 0.6 ? (INTERESTING[randInt(rand, INTERESTING.length)] as number) : randInt(rand, 256);
  }
  return out;
}

function randomInput(rand: () => number): Uint8Array {
  const length = rand() < 0.1 ? randInt(rand, 400) : randInt(rand, 64);
  return rand() < 0.5 ? randomBytes(rand, length) : biasedBytes(rand, length);
}

function mutate(rand: () => number, source: Uint8Array): Uint8Array {
  let bytes = Array.from(source);
  const mutations = 1 + randInt(rand, 3);
  for (let m = 0; m < mutations; m++) {
    const at = bytes.length === 0 ? 0 : randInt(rand, bytes.length);
    switch (randInt(rand, 6)) {
      case 0: // flip a bit
        if (bytes.length > 0) bytes[at] = (bytes[at] as number) ^ (1 << randInt(rand, 8));
        break;
      case 1: // replace a byte
        if (bytes.length > 0) bytes[at] = rand() < 0.5 ? randInt(rand, 256) : (INTERESTING[randInt(rand, INTERESTING.length)] as number);
        break;
      case 2: // truncate
        bytes = bytes.slice(0, at);
        break;
      case 3: // insert a byte
        bytes.splice(at, 0, randInt(rand, 256));
        break;
      case 4: // append garbage
        bytes.push(...randomBytes(rand, 1 + randInt(rand, 4)));
        break;
      default: // delete a byte
        if (bytes.length > 0) bytes.splice(at, 1);
    }
  }
  return Uint8Array.from(bytes);
}

// -- the property ---------------------------------------------------------------

interface Tally {
  decoded: number;
  rejected: number;
}

/** Runs the decoder on `bytes`; only a `WireError` may escape it. */
function attempt(target: Target, bytes: Uint8Array, tally: Tally): void {
  let value: unknown;
  try {
    value = target.decode(bytes);
  } catch (e) {
    if (e instanceof WireError) {
      tally.rejected++;
      return;
    }
    throw new Error(
      `${target.name} threw ${e instanceof Error ? `${e.name}: ${e.message}` : String(e)} instead of a WireError for input ${toHex(bytes)}`,
      { cause: e },
    );
  }
  tally.decoded++;
  if (target.reencode) {
    const again = (target.reencode as (v: unknown) => Uint8Array)(value);
    expect(toHex(again), `${target.name} re-encoding of ${toHex(bytes)}`).toBe(toHex(bytes));
  }
}

describe("fuzz: decoders throw WireError or decode, never anything else", () => {
  it(`survives ${RANDOM_ARRAYS} random byte arrays in every decoder`, () => {
    const rand = prng(SEED);
    const tallies = new Map<string, Tally>(targets.map((t) => [t.name, { decoded: 0, rejected: 0 }]));
    for (let i = 0; i < RANDOM_ARRAYS; i++) {
      const bytes = randomInput(rand);
      for (const target of targets) attempt(target, bytes, tallies.get(target.name) as Tally);
    }
    // Decoders whose valid inputs are short and fixed-size must see both outcomes from random bytes, or the
    // generator is not reaching them. (Structured decoders are reached by the mutation fuzz below.)
    for (const name of ["bool", "u8", "i16", "u32", "i64", "u64Number", "timestamp", "option<string>", "cancel", "timer fired"]) {
      const tally = tallies.get(name) as Tally;
      expect(tally.decoded, `${name} decoded`).toBeGreaterThan(0);
      expect(tally.rejected, `${name} rejected`).toBeGreaterThan(0);
    }
  });

  it(`survives ${MUTATED_ARRAYS} mutations of valid encodings in each decoder`, () => {
    const rand = prng(SEED + 1);
    for (const target of targets) {
      const tally = { decoded: 0, rejected: 0 };
      for (const valid of target.valid) {
        // The untouched sample must decode: the mutations below start from something real.
        expect(() => target.decode(valid), `${target.name} valid sample ${toHex(valid)}`).not.toThrow();
      }
      for (let i = 0; i < MUTATED_ARRAYS; i++) {
        const source = target.valid[randInt(rand, target.valid.length)] as Uint8Array;
        attempt(target, mutate(rand, source), tally);
      }
      expect(tally.rejected, `${target.name} rejected some mutations`).toBeGreaterThan(0);
    }
  });

  it("survives every strict prefix and every single-byte corruption of each valid sample", () => {
    const tally = { decoded: 0, rejected: 0 };
    for (const target of targets) {
      for (const valid of target.valid) {
        for (let n = 0; n < valid.length; n++) attempt(target, valid.subarray(0, n), tally);
        for (let at = 0; at < valid.length; at++) {
          for (const value of [0, 1, 2, 0x7f, 0x80, 0xff]) {
            const copy = valid.slice();
            copy[at] = value;
            attempt(target, copy, tally);
          }
        }
      }
    }
    expect(tally.rejected).toBeGreaterThan(0);
    expect(tally.decoded).toBeGreaterThan(0);
  });

  it("decodes correctly from views with a non-zero byteOffset", () => {
    const rand = prng(SEED + 2);
    const tally = { decoded: 0, rejected: 0 };
    for (let i = 0; i < 1000; i++) {
      const bytes = randomInput(rand);
      const padded = new Uint8Array(bytes.length + 9).fill(0xff);
      padded.set(bytes, 4);
      const view = padded.subarray(4, 4 + bytes.length);
      for (const target of targets) {
        let plain: unknown;
        let offset: unknown;
        let plainError: string | undefined;
        let offsetError: string | undefined;
        try {
          plain = target.decode(bytes);
        } catch (e) {
          plainError = e instanceof WireError ? e.message : `non-WireError ${String(e)}`;
        }
        try {
          offset = target.decode(view);
        } catch (e) {
          offsetError = e instanceof WireError ? e.message : `non-WireError ${String(e)}`;
        }
        expect(offsetError, `${target.name} ${toHex(bytes)}`).toBe(plainError);
        if (plainError === undefined) {
          expect(offset, `${target.name} ${toHex(bytes)}`).toEqual(plain);
          tally.decoded++;
        } else {
          tally.rejected++;
        }
      }
    }
    expect(tally.decoded).toBeGreaterThan(0);
    expect(tally.rejected).toBeGreaterThan(0);
  });
});
