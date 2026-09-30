import { describe, expect, it } from "vitest";
import { HEADER_LEN, Kind, WIRE_VERSION, decodeEnvelope, encodeEnvelope } from "../src/wire/envelope.js";
import { embedded, expectWireError, fromHex, toHex } from "./helpers.js";

const SCHEMA = 0x0102030405060708n;

describe("Kind", () => {
  it("has the 16 wire values of SPEC 3.2", () => {
    expect(Kind.Call).toBe(1);
    expect(Kind.Reply).toBe(2);
    expect(Kind.ChangeSet).toBe(3);
    expect(Kind.PortCall).toBe(4);
    expect(Kind.PortReply).toBe(5);
    expect(Kind.Cancel).toBe(6);
    expect(Kind.StreamCredit).toBe(7);
    expect(Kind.StreamItem).toBe(8);
    expect(Kind.Observe).toBe(9);
    expect(Kind.Release).toBe(10);
    expect(Kind.Event).toBe(11);
    expect(Kind.Hello).toBe(12);
    expect(Kind.Log).toBe(13);
    expect(Kind.TimerFired).toBe(14);
    expect(Kind.Snapshot).toBe(15);
    expect(Kind.Restore).toBe(16);
  });

  it("has exactly 16 members", () => {
    const names = Object.keys(Kind).filter((k) => Number.isNaN(Number(k)));
    expect(names).toHaveLength(16);
  });
});

describe("encodeEnvelope", () => {
  it("lays out magic, version, schema, kind, seq, len, payload (contract vector)", () => {
    const bytes = encodeEnvelope(Kind.Call, 7, SCHEMA, fromHex("aabbcc"));
    expect(toHex(bytes)).toBe("4b45454c01000807060504030201010700000003000000aabbcc");
  });

  it("uses a 23-byte header", () => {
    expect(HEADER_LEN).toBe(23);
    expect(WIRE_VERSION).toBe(1);
    expect(encodeEnvelope(Kind.Hello, 0, 0n, new Uint8Array(0)).length).toBe(23);
    expect(encodeEnvelope(Kind.Hello, 0, 0n, new Uint8Array(10)).length).toBe(33);
  });

  it("starts with the ASCII magic UNDRA", () => {
    const bytes = encodeEnvelope(Kind.Log, 0, 0n, new Uint8Array(0));
    expect(new TextDecoder().decode(bytes.subarray(0, 4))).toBe("UNDRA");
  });

  it("returns a tight array (its buffer is exactly as long as the message)", () => {
    const bytes = encodeEnvelope(Kind.Log, 0, 0n, new Uint8Array(5));
    expect(bytes.byteOffset).toBe(0);
    expect(bytes.buffer.byteLength).toBe(bytes.length);
  });

  it("copies only the viewed range of a payload view", () => {
    const big = fromHex("00112233445566");
    const bytes = encodeEnvelope(Kind.Event, 1, 0n, big.subarray(2, 5));
    expect(toHex(bytes.subarray(HEADER_LEN))).toBe("223344");
    expect(decodeEnvelope(bytes).payload.length).toBe(3);
  });

  it("rejects a kind that is not one of the 16", () => {
    for (const kind of [0, 17, 255, -1, 1.5, Number.NaN]) {
      expect(() => encodeEnvelope(kind as Kind, 0, 0n, new Uint8Array(0)), String(kind)).toThrow(RangeError);
    }
  });

  it("rejects a seq outside u32 and a schema hash outside u64", () => {
    expect(() => encodeEnvelope(Kind.Call, -1, 0n, new Uint8Array(0))).toThrow(RangeError);
    expect(() => encodeEnvelope(Kind.Call, 2 ** 32, 0n, new Uint8Array(0))).toThrow(RangeError);
    expect(() => encodeEnvelope(Kind.Call, 1.5, 0n, new Uint8Array(0))).toThrow(RangeError);
    expect(() => encodeEnvelope(Kind.Call, 0, -1n, new Uint8Array(0))).toThrow(RangeError);
    expect(() => encodeEnvelope(Kind.Call, 0, 2n ** 64n, new Uint8Array(0))).toThrow(RangeError);
  });
});

describe("decodeEnvelope", () => {
  it("decodes the contract vector", () => {
    const env = decodeEnvelope(fromHex("4b45454c01000807060504030201010700000003000000aabbcc"));
    expect(env.kind).toBe(Kind.Call);
    expect(env.seq).toBe(7);
    expect(env.schemaHash).toBe(72623859790382856n);
    expect(toHex(env.payload)).toBe("aabbcc");
  });

  it("round-trips every kind", () => {
    for (let kind = 1; kind <= 16; kind++) {
      const payload = new Uint8Array([kind, kind + 1]);
      const env = decodeEnvelope(encodeEnvelope(kind as Kind, 1000 + kind, SCHEMA + BigInt(kind), payload));
      expect(env.kind).toBe(kind);
      expect(env.seq).toBe(1000 + kind);
      expect(env.schemaHash).toBe(SCHEMA + BigInt(kind));
      expect(toHex(env.payload)).toBe(toHex(payload));
    }
  });

  it("round-trips the extremes of seq and schema hash", () => {
    for (const [seq, schema] of [
      [0, 0n],
      [2 ** 32 - 1, 2n ** 64n - 1n],
      [1, 2n ** 63n],
    ] as const) {
      const env = decodeEnvelope(encodeEnvelope(Kind.Reply, seq, schema, new Uint8Array(0)));
      expect(env.seq).toBe(seq);
      expect(env.schemaHash).toBe(schema);
      expect(env.payload.length).toBe(0);
    }
  });

  it("round-trips a large payload", () => {
    const payload = new Uint8Array(1 << 20).map((_, i) => i & 0xff);
    const env = decodeEnvelope(encodeEnvelope(Kind.Snapshot, 1, 1n, payload));
    expect(env.payload.length).toBe(payload.length);
    expect(env.payload[123457]).toBe(123457 & 0xff);
  });

  it("decodes from a view with a byteOffset and returns a borrowed payload", () => {
    const bytes = embedded(encodeEnvelope(Kind.Call, 3, SCHEMA, fromHex("010203")));
    expect(bytes.byteOffset).toBeGreaterThan(0);
    const env = decodeEnvelope(bytes);
    expect(toHex(env.payload)).toBe("010203");
    expect(env.payload.buffer).toBe(bytes.buffer);
    expect(env.payload.byteOffset).toBe(bytes.byteOffset + HEADER_LEN);
  });

  it("reports a truncated header as unexpected_eof", () => {
    const valid = encodeEnvelope(Kind.Call, 7, SCHEMA, new Uint8Array(0));
    for (let n = 0; n < HEADER_LEN; n++) {
      const err = expectWireError(() => decodeEnvelope(valid.subarray(0, n)), "unexpected_eof");
      expect(err.at).toBeLessThanOrEqual(n);
    }
  });

  it("reports a payload shorter than declared as length_too_large", () => {
    const valid = encodeEnvelope(Kind.Call, 7, SCHEMA, fromHex("aabbcc"));
    for (let n = HEADER_LEN; n < valid.length; n++) {
      expectWireError(() => decodeEnvelope(valid.subarray(0, n)), "length_too_large", { len: 3, at: 19 });
    }
  });

  it("reports bytes after the payload as trailing_bytes", () => {
    const valid = encodeEnvelope(Kind.Call, 7, SCHEMA, fromHex("aabbcc"));
    const longer = new Uint8Array(valid.length + 2);
    longer.set(valid);
    expectWireError(() => decodeEnvelope(longer), "trailing_bytes", { count: 2 });
  });

  it("rejects a wrong magic", () => {
    const valid = encodeEnvelope(Kind.Call, 7, SCHEMA, new Uint8Array(0));
    for (const magic of ["KEEK", "undra", "\u0000\u0000\u0000\u0000", "LEEK", "KEE\u0000"]) {
      const bytes = valid.slice();
      bytes.set(new TextEncoder().encode(magic), 0);
      expectWireError(() => decodeEnvelope(bytes), "bad_magic");
    }
    // Four wrong bytes are enough to know; the rest need not exist.
    expectWireError(() => decodeEnvelope(fromHex("deadbeef")), "bad_magic");
  });

  it("checks the magic before the version", () => {
    const bytes = encodeEnvelope(Kind.Call, 7, SCHEMA, new Uint8Array(0));
    bytes.set([0, 0, 0, 0, 9, 9], 0);
    expectWireError(() => decodeEnvelope(bytes), "bad_magic");
  });

  it("rejects an unsupported version", () => {
    const valid = encodeEnvelope(Kind.Call, 7, SCHEMA, new Uint8Array(0));
    for (const version of [0, 2, 256, 0xffff]) {
      const bytes = valid.slice();
      new DataView(bytes.buffer).setUint16(4, version, true);
      expectWireError(() => decodeEnvelope(bytes), "unsupported_version", { version });
    }
  });

  it("rejects an unknown kind, reporting its offset", () => {
    const valid = encodeEnvelope(Kind.Call, 7, SCHEMA, new Uint8Array(0));
    for (const kind of [0, 17, 18, 255]) {
      const bytes = valid.slice();
      bytes[14] = kind;
      expectWireError(() => decodeEnvelope(bytes), "invalid_tag", { tag: kind, at: 14, ty: "Kind" });
    }
  });

  it("returns a differing schema hash unless one is expected", () => {
    const bytes = encodeEnvelope(Kind.Hello, 1, 111n, new Uint8Array(0));
    expect(decodeEnvelope(bytes).schemaHash).toBe(111n);
    expect(decodeEnvelope(bytes, 111n).schemaHash).toBe(111n);
    expectWireError(() => decodeEnvelope(bytes, 222n), "schema_mismatch", { expected: 222n, got: 111n });
  });

  it("checks structure before the schema", () => {
    const bytes = encodeEnvelope(Kind.Call, 1, 111n, new Uint8Array(1));
    expectWireError(() => decodeEnvelope(bytes.subarray(0, 23), 222n), "length_too_large");
  });

  it("a schema mismatch message names both hashes", () => {
    const bytes = encodeEnvelope(Kind.Call, 1, 0xabcn, new Uint8Array(0));
    try {
      decodeEnvelope(bytes, 0xdefn);
      expect.unreachable();
    } catch (e) {
      expect((e as Error).message).toContain("0000000000000def");
      expect((e as Error).message).toContain("0000000000000abc");
    }
  });
});
