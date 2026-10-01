import { describe, expect, test } from "vitest";
import { Utf8TextDecoder, Utf8TextEncoder, installPolyfills } from "../src/text-codec.js";

/** Byte sequences with known WHATWG decodings, valid and not. */
const SAMPLES: number[][] = [
  [],
  [0x68, 0x69],
  [0xc3, 0xa9],
  [0xe2, 0x9c, 0x93],
  [0xf0, 0x9f, 0x98, 0x80],
  [0xef, 0xbb, 0xbf, 0x41], // BOM
  [0xc0, 0x80], // overlong
  [0xe0, 0x80, 0x80], // overlong
  [0xed, 0xa0, 0x80], // surrogate
  [0xf4, 0x90, 0x80, 0x80], // above U+10FFFF
  [0xe2, 0x9c], // truncated
  [0xf0, 0x9f, 0x41], // truncated, then ASCII
  [0x80, 0xbf, 0x41],
  [0xff, 0xfe],
];

describe("Utf8TextDecoder", () => {
  test("decodes like the platform's TextDecoder, replacing or failing like it", () => {
    for (const sample of SAMPLES) {
      const bytes = new Uint8Array(sample);
      for (const ignoreBOM of [false, true]) {
        const native = new TextDecoder("utf-8", { ignoreBOM });
        const ours = new Utf8TextDecoder("utf-8", { ignoreBOM });
        expect(ours.decode(bytes), JSON.stringify({ sample, ignoreBOM })).toBe(native.decode(bytes));
        const nativeFatal = new TextDecoder("utf-8", { fatal: true, ignoreBOM });
        const oursFatal = new Utf8TextDecoder("utf-8", { fatal: true, ignoreBOM });
        let expected: string | "throws";
        try {
          expected = nativeFatal.decode(bytes);
        } catch {
          expected = "throws";
        }
        if (expected === "throws") expect(() => oursFatal.decode(bytes)).toThrow(TypeError);
        else expect(oursFatal.decode(bytes)).toBe(expected);
      }
    }
  });

  test("decodes long text and views into larger buffers", () => {
    const text = "ü✓😀a".repeat(5000);
    const encoded = new TextEncoder().encode(text);
    const padded = new Uint8Array(encoded.length + 4);
    padded.set(encoded, 2);
    expect(new Utf8TextDecoder().decode(padded.subarray(2, 2 + encoded.length))).toBe(text);
    expect(new Utf8TextDecoder().decode(encoded.buffer)).toBe(text);
  });

  test("refuses other encodings", () => {
    expect(() => new Utf8TextDecoder("latin1")).toThrow(RangeError);
  });
});

describe("Utf8TextEncoder", () => {
  test("encodes like the platform's TextEncoder, lone surrogates included", () => {
    for (const text of ["", "hi", "héllo, wörld ✓", "😀", "\ud800", "a\udc00b", "😀\ud83d"]) {
      expect(new Utf8TextEncoder().encode(text)).toEqual(new TextEncoder().encode(text));
    }
  });

  test("encodeInto stops at a whole character", () => {
    const out = new Uint8Array(4);
    expect(new Utf8TextEncoder().encodeInto("ab✓", out)).toEqual({ read: 2, written: 2 });
  });
});

test("installPolyfills fills only what is missing", () => {
  const target: Record<string, unknown> = { TextEncoder: TextEncoder };
  expect(installPolyfills(target)).toEqual(["TextDecoder"]);
  expect(target["TextDecoder"]).toBe(Utf8TextDecoder);
  expect(target["TextEncoder"]).toBe(TextEncoder);
  expect(installPolyfills(target)).toEqual([]);
});
