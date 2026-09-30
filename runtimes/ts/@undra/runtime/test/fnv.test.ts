import { describe, expect, it } from "vitest";
import { fnv1a32, fnv1a64 } from "../src/fnv.js";
import { prng, randInt } from "./helpers.js";

/** Straightforward reference implementations over an explicit UTF-8 byte array. */
function reference32(s: string): number {
  let h = 0x811c9dc5n;
  for (const b of new TextEncoder().encode(s)) {
    h = ((h ^ BigInt(b)) * 0x01000193n) & 0xffffffffn;
  }
  return Number(h);
}

function reference64(s: string): bigint {
  let h = 0xcbf29ce484222325n;
  for (const b of new TextEncoder().encode(s)) {
    h = ((h ^ BigInt(b)) * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return h;
}

describe("fnv1a32", () => {
  it("matches the spec vector", () => {
    expect(fnv1a32("Calculator.add")).toBe(2353348832);
  });

  it("matches the published FNV test vectors", () => {
    expect(fnv1a32("")).toBe(0x811c9dc5);
    expect(fnv1a32("a")).toBe(0xe40c292c);
    expect(fnv1a32("foobar")).toBe(0xbf9cf968);
  });

  it("returns an unsigned 32-bit integer", () => {
    for (const s of ["a", "b", "Calculator.add", "port.Http", "query.todos", "x".repeat(100)]) {
      const h = fnv1a32(s);
      expect(Number.isInteger(h)).toBe(true);
      expect(h).toBeGreaterThanOrEqual(0);
      expect(h).toBeLessThanOrEqual(0xffffffff);
    }
  });

  it("hashes UTF-8 bytes, not UTF-16 units", () => {
    expect(fnv1a32("héllo \u{1f30a}")).toBe(reference32("héllo \u{1f30a}"));
    expect(fnv1a32("é")).not.toBe(fnv1a32("e"));
  });

  it("hashes a lone surrogate as U+FFFD, like TextEncoder", () => {
    expect(fnv1a32("a\ud800b")).toBe(fnv1a32("a�b"));
    expect(fnv1a32("\udc00")).toBe(fnv1a32("�"));
  });

  it("agrees with the reference on random strings, including ones beyond the scratch buffer", () => {
    const rand = prng(17);
    const units = ["a", "Z", "0", ".", "_", "é", "日", "\u{1f30a}", "\ud800", "\udc00", "\u0000"];
    for (let i = 0; i < 1500; i++) {
      const length = i % 50 === 0 ? 1000 + randInt(rand, 3000) : randInt(rand, 40);
      let s = "";
      for (let j = 0; j < length; j++) s += units[randInt(rand, units.length)];
      expect(fnv1a32(s)).toBe(reference32(s));
    }
  });
});

describe("fnv1a64", () => {
  it("matches the spec vector", () => {
    expect(fnv1a64("undra")).toBe(6367360722358687308n);
  });

  it("matches the published FNV test vectors", () => {
    expect(fnv1a64("")).toBe(0xcbf29ce484222325n);
    expect(fnv1a64("a")).toBe(0xaf63dc4c8601ec8cn);
    expect(fnv1a64("foobar")).toBe(0x85944171f73967e8n);
  });

  it("returns a value in the u64 range", () => {
    for (const s of ["a", "undra", "schema", "x".repeat(5000)]) {
      const h = fnv1a64(s);
      expect(h).toBeGreaterThanOrEqual(0n);
      expect(h).toBeLessThan(2n ** 64n);
    }
  });

  it("agrees with the BigInt reference on random strings, including ones beyond the scratch buffer", () => {
    const rand = prng(19);
    const units = ["a", "Z", "0", "{", "\"", "é", "日", "\u{1f30a}", "\ud800", "\u0000", "ÿ"];
    for (let i = 0; i < 1500; i++) {
      const length = i % 50 === 0 ? 1000 + randInt(rand, 3000) : randInt(rand, 40);
      let s = "";
      for (let j = 0; j < length; j++) s += units[randInt(rand, units.length)];
      expect(fnv1a64(s)).toBe(reference64(s));
    }
  });

  it("hashes a large schema-sized string", () => {
    const json = JSON.stringify({ types: Array.from({ length: 2000 }, (_, i) => ({ name: `T${i}`, id: i })) });
    expect(json.length).toBeGreaterThan(3072);
    expect(fnv1a64(json)).toBe(reference64(json));
    expect(fnv1a32(json)).toBe(reference32(json));
  });
});
