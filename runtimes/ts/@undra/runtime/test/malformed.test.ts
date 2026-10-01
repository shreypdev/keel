import { describe, expect, it } from "vitest";
import { codecs, decodeValue, encodeValue } from "../src/wire/codec.js";
import { Kind, decodeEnvelope, encodeEnvelope } from "../src/wire/envelope.js";
import { WireError, type WireErrorCode } from "../src/wire/errors.js";
import { decodeCall, decodeChangeSet, decodeReply, decodeSnapshot, decodeStreamFailure, decodeStreamItem } from "../src/wire/payloads.js";
import { UndraReader } from "../src/wire/reader.js";
import { durationFromNanos } from "../src/wire/types.js";
import { UndraWriter } from "../src/wire/writer.js";
import { catchWireError, embedded, fromHex } from "./helpers.js";

/**
 * One way to provoke each WireError code from the public API. The record type
 * makes the compiler insist that a new code gets a case here.
 */
const triggers: Record<WireErrorCode, [string, () => unknown][]> = {
  unexpected_eof: [
    ["u32 from 3 bytes", () => decodeValue(codecs.u32, new Uint8Array(3))],
    ["empty input", () => decodeValue(codecs.string, new Uint8Array(0))],
    ["vec item cut short", () => decodeValue(codecs.vec(codecs.u32), fromHex("01000000 010000"))],
    ["truncated envelope header", () => decodeEnvelope(fromHex("554e44520100"))],
    ["truncated call", () => decodeCall(fromHex("01 0100000001000000"))],
    ["truncated uuid", () => decodeValue(codecs.uuid, new Uint8Array(15))],
    ["flag-3 stream item without its failure", () => decodeStreamItem(fromHex("01000000 03"))],
  ],
  invalid_utf8: [
    ["lone continuation byte", () => decodeValue(codecs.string, fromHex("01000000 80"))],
    ["overlong encoding", () => decodeValue(codecs.string, fromHex("02000000 c080"))],
    ["encoded surrogate", () => decodeValue(codecs.string, fromHex("03000000 eda080"))],
    ["truncated sequence at the end of the string", () => decodeValue(codecs.string, fromHex("02000000 e282"))],
    ["inside a panic reply", () => decodeReply(fromHex("01000000 02 01000000 ff 00000000"))],
    ["inside a stream failure", () => decodeStreamFailure(fromHex("03 01000000 ff 00000000"))],
  ],
  invalid_tag: [
    ["bool 2", () => decodeValue(codecs.bool, fromHex("02"))],
    ["option tag 2", () => decodeValue(codecs.option(codecs.u8), fromHex("02"))],
    ["result tag 2", () => decodeValue(codecs.result(codecs.u8, codecs.u8), fromHex("02"))],
    ["envelope kind 0", () => decodeEnvelope(corrupt(validEnvelope(), 14, 0))],
    ["envelope kind 17", () => decodeEnvelope(corrupt(validEnvelope(), 14, 17))],
    ["call target 9", () => decodeCall(fromHex("09"))],
    ["reply status 9", () => decodeReply(fromHex("0100000009"))],
    ["change-set op 9", () => decodeChangeSet(fromHex("0000000000000000 01000000 0000000000000000 00000000 09 00000000"))],
    ["stream flag 4", () => decodeStreamItem(fromHex("01000000 04"))],
    ["stream failure status 1 (a typed error is flag 2, not a failure)", () => decodeStreamItem(fromHex("01000000 03 01 00000000 00000000"))],
    ["stream failure status 4", () => decodeStreamFailure(fromHex("04 00000000 00000000"))],
  ],
  length_too_large: [
    ["string length beyond input", () => decodeValue(codecs.string, fromHex("ffffffff 61"))],
    ["bytes length beyond input", () => decodeValue(codecs.bytes, fromHex("05000000 0102"))],
    ["vec count beyond input", () => decodeValue(codecs.vec(codecs.u8), fromHex("0a000000 01"))],
    ["map count beyond input", () => decodeValue(codecs.map(codecs.u8, codecs.u8), fromHex("ffffffff"))],
    ["envelope payload shorter than declared", () => decodeEnvelope(validEnvelope().subarray(0, 24))],
    ["change-set count beyond input", () => decodeChangeSet(fromHex("0000000000000000 ffffffff"))],
    ["snapshot count beyond input", () => decodeSnapshot(fromHex("ffffffff"))],
    ["length above u32 when encoding", () => new UndraWriter().writeLen(2 ** 32)],
  ],
  trailing_bytes: [
    ["value followed by a byte", () => decodeValue(codecs.u8, fromHex("0102"))],
    ["reader not fully consumed", () => new UndraReader(fromHex("01")).finish()],
    ["bytes after an envelope payload", () => decodeEnvelope(new Uint8Array([...validEnvelope(), 0]))],
    ["body on a cancelled reply", () => decodeReply(fromHex("01000000 03 00"))],
    ["bytes after a stream failure", () => decodeStreamItem(fromHex("01000000 03 03 00000000 00000000 00"))],
  ],
  bad_magic: [
    ["wrong bytes", () => decodeEnvelope(fromHex("deadbeef"))],
    ["lower-case magic", () => decodeEnvelope(corrupt(validEnvelope(), 0, 0x75))],
    ["zeroes", () => decodeEnvelope(new Uint8Array(40))],
  ],
  unsupported_version: [
    ["version 0", () => decodeEnvelope(corrupt(validEnvelope(), 4, 0))],
    ["version 2", () => decodeEnvelope(corrupt(validEnvelope(), 4, 2))],
    ["version 65535", () => decodeEnvelope(corrupt(corrupt(validEnvelope(), 4, 255), 5, 255))],
  ],
  schema_mismatch: [
    ["expected differs", () => decodeEnvelope(validEnvelope(), 999n)],
    ["expected zero", () => decodeEnvelope(validEnvelope(), 0n)],
  ],
  duplicate_key: [
    ["duplicate on decode", () => decodeValue(codecs.map(codecs.u8, codecs.u8), fromHex("02000000 0101 0102"))],
    [
      "equal encoded keys on encode",
      () => {
        const key = () => new Uint8Array([1]);
        return encodeValue(codecs.map(codecs.bytes, codecs.u8), new Map([[key(), 1], [key(), 2]]));
      },
    ],
  ],
  negative_duration: [
    ["decode -1 ns", () => decodeValue(codecs.duration, fromHex("ffffffffffffffff"))],
    ["decode i64 min", () => decodeValue(codecs.duration, fromHex("0000000000000080"))],
    ["encode -5 ms", () => encodeValue(codecs.duration, -5)],
    ["convert negative nanoseconds", () => durationFromNanos(-1n)],
  ],
  unsafe_integer: [
    ["u64 as number above 2^53", () => decodeValue(codecs.u64Number, fromHex("0000000000002000"))],
    ["i64 as number below -2^53", () => decodeValue(codecs.i64Number, fromHex("000000000000e0ff"))],
    ["timestamp above 2^53", () => decodeValue(codecs.timestamp, fromHex("ffffffffffffff7f"))],
  ],
};

function validEnvelope(): Uint8Array {
  return encodeEnvelope(Kind.Call, 1, 42n, fromHex("aabbcc"));
}

function corrupt(bytes: Uint8Array, at: number, value: number): Uint8Array {
  const copy = bytes.slice();
  copy[at] = value;
  return copy;
}

describe("WireError codes", () => {
  for (const [code, cases] of Object.entries(triggers) as [WireErrorCode, [string, () => unknown][]][]) {
    describe(code, () => {
      it.each(cases)("%s", (_name, fn) => {
        const err = catchWireError(fn);
        expect(err.code).toBe(code);
        expect(err.detail.code).toBe(code);
      });
    });
  }

  it("covers every code in the union", () => {
    expect(Object.keys(triggers).sort()).toEqual(
      [
        "bad_magic",
        "duplicate_key",
        "invalid_tag",
        "invalid_utf8",
        "length_too_large",
        "negative_duration",
        "schema_mismatch",
        "trailing_bytes",
        "unexpected_eof",
        "unsupported_version",
        "unsafe_integer",
      ].sort(),
    );
  });

  it("gives each failure the same code from an offset view as from a plain buffer", () => {
    for (const bytes of [
      fromHex("deadbeef"),
      corrupt(validEnvelope(), 4, 9),
      corrupt(validEnvelope(), 14, 99),
      validEnvelope().subarray(0, 25),
      new Uint8Array([...validEnvelope(), 1]),
    ]) {
      const plain = catchWireError(() => decodeEnvelope(bytes));
      const viewed = catchWireError(() => decodeEnvelope(embedded(bytes)));
      expect(viewed.code).toBe(plain.code);
      expect(viewed.detail).toEqual(plain.detail);
    }
  });
});

describe("stream item flags (ADR-036)", () => {
  it("accepts flag 3 with a well-formed failure; flag 4 and up are invalid", () => {
    expect(decodeStreamItem(fromHex("01000000 03 03 00000000 00000000"))).toMatchObject({ callId: 1, flag: 3 });
    for (const flag of [4, 5, 0x7f, 0xff]) {
      expect(catchWireError(() => decodeStreamItem(new Uint8Array([1, 0, 0, 0, flag]))).detail).toEqual({
        code: "invalid_tag",
        tag: flag,
        at: 4,
        ty: "StreamFlag",
      });
    }
  });

  it("names a bad failure status StreamFailure.status, at the status byte", () => {
    for (const status of [0, 1, 4, 6, 0xff]) {
      expect(catchWireError(() => decodeStreamItem(new Uint8Array([1, 0, 0, 0, 3, status, 0, 0, 0, 0, 0, 0, 0, 0]))).detail).toEqual({
        code: "invalid_tag",
        tag: status,
        at: 5,
        ty: "StreamFailure.status",
      });
      expect(catchWireError(() => decodeStreamFailure(new Uint8Array([status, 0, 0, 0, 0, 0, 0, 0, 0]))).message).toBe(
        `wire: invalid StreamFailure.status tag ${status} at offset 0`,
      );
    }
  });
});

describe("WireError", () => {
  it("is an Error with a name, a code, a structured detail and a readable message", () => {
    const err = catchWireError(() => decodeValue(codecs.bool, fromHex("07")));
    expect(err).toBeInstanceOf(Error);
    expect(err).toBeInstanceOf(WireError);
    expect(err.name).toBe("WireError");
    expect(err.code).toBe("invalid_tag");
    expect(err.detail).toEqual({ code: "invalid_tag", tag: 7, at: 0, ty: "bool" });
    expect(err.message).toBe("wire: invalid bool tag 7 at offset 0");
    expect(err.stack).toContain("WireError");
  });

  it("describes each variant with its fields", () => {
    const message = (fn: () => unknown): string => catchWireError(fn).message;
    expect(message(() => decodeValue(codecs.u32, new Uint8Array(1)))).toBe(
      "wire: unexpected end of input at offset 0: needed 3 more bytes",
    );
    expect(message(() => decodeValue(codecs.string, fromHex("01000000 80")))).toBe(
      "wire: invalid UTF-8 in string at offset 0",
    );
    expect(message(() => decodeValue(codecs.string, fromHex("09000000")))).toBe(
      "wire: length 9 at offset 0 exceeds the available input",
    );
    expect(message(() => decodeValue(codecs.u8, fromHex("0102")))).toBe(
      "wire: 1 trailing byte after the end of the message",
    );
    expect(message(() => decodeEnvelope(fromHex("00000000")))).toBe(
      'wire: bad magic: envelope does not start with 554e4452',
    );
    expect(message(() => decodeEnvelope(corrupt(validEnvelope(), 4, 2)))).toBe("wire: unsupported wire version 2");
    expect(message(() => decodeEnvelope(validEnvelope(), 7n))).toBe(
      "wire: schema mismatch: expected 0x0000000000000007, got 0x000000000000002a",
    );
    expect(message(() => decodeValue(codecs.map(codecs.u8, codecs.u8), fromHex("02000000 0101 0102")))).toBe(
      "wire: duplicate map key at offset 6",
    );
    expect(message(() => decodeValue(codecs.duration, fromHex("ffffffffffffffff")))).toBe(
      "wire: negative duration (-1 ns)",
    );
    expect(message(() => decodeValue(codecs.u64Number, fromHex("0000000000002000")))).toBe(
      "wire: integer 9007199254740992 at offset 0 is outside the JS safe integer range",
    );
  });

  it("carries the structured fields of each variant", () => {
    expect(catchWireError(() => decodeValue(codecs.u32, new Uint8Array(1))).detail).toEqual({
      code: "unexpected_eof",
      needed: 3,
      at: 0,
    });
    expect(catchWireError(() => decodeValue(codecs.string, fromHex("09000000"))).detail).toEqual({
      code: "length_too_large",
      len: 9,
      at: 0,
    });
    expect(catchWireError(() => decodeValue(codecs.u8, fromHex("0102"))).detail).toEqual({
      code: "trailing_bytes",
      count: 1,
    });
    expect(catchWireError(() => decodeEnvelope(fromHex("00000000"))).detail).toEqual({ code: "bad_magic" });
    expect(catchWireError(() => decodeEnvelope(corrupt(validEnvelope(), 4, 2))).detail).toEqual({
      code: "unsupported_version",
      version: 2,
    });
    expect(catchWireError(() => decodeEnvelope(validEnvelope(), 7n)).detail).toEqual({
      code: "schema_mismatch",
      expected: 7n,
      got: 42n,
    });
    expect(catchWireError(() => decodeValue(codecs.duration, fromHex("ffffffffffffffff"))).detail).toEqual({
      code: "negative_duration",
      nanos: -1n,
    });
  });

  it("narrows on detail.code", () => {
    const err = catchWireError(() => decodeEnvelope(corrupt(validEnvelope(), 4, 3)));
    switch (err.detail.code) {
      case "unsupported_version":
        expect(err.detail.version).toBe(3);
        break;
      default:
        expect.unreachable("expected unsupported_version");
    }
  });
});
