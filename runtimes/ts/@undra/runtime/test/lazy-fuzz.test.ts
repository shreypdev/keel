import { describe, expect, it } from "vitest";
import {
  UndraReader,
  WireError,
  codecs,
  decodeLazyInvalidated,
  decodeLazyPage,
  decodeLazyValue,
  encodeLazyInvalidated,
  encodeLazyPage,
  encodeLazyValue,
} from "../src/wire/index.js";
import { prng, randInt, randomBytes } from "./helpers.js";
import { drain, numbersList, settle } from "./support/lazy-server.js";

/*
 * Hostile input to everything a lazy list reads (ADR-043): the three payload decoders only ever throw WireError, and a
 * list fed random page replies and random notices keeps its invariants and never throws into a reader.
 */

/** Runs `fn`; the only thing it may throw is a `WireError`. */
function wireOnly(fn: () => unknown): void {
  try {
    fn();
  } catch (error) {
    expect(error).toBeInstanceOf(WireError);
  }
}

describe("the lazy payload decoders on hostile bytes", () => {
  it("random bytes either decode or are a WireError", () => {
    const rand = prng(0x1a2b);
    for (let i = 0; i < 4000; i++) {
      const bytes = randomBytes(rand, randInt(rand, 48));
      wireOnly(() => decodeLazyValue(bytes));
      wireOnly(() => decodeLazyInvalidated(bytes));
      wireOnly(() => decodeLazyPage(codecs.i32, bytes));
      wireOnly(() => decodeLazyPage(codecs.string, bytes, 50));
    }
  });

  it("every truncation and extension of a valid payload is a WireError, and a bit flip a WireError or another value", () => {
    const page = encodeLazyPage(codecs.string, { version: 9n, total: 7, items: ["a", "bb", "ccc"] });
    const value = encodeLazyValue({ handle: 0x1_0000_0001n, len: 5, version: 3n });
    const inv = encodeLazyInvalidated({ len: 5, version: 3n });
    for (const [bytes, decode] of [
      [page, (b: Uint8Array) => decodeLazyPage(codecs.string, b)],
      [value, decodeLazyValue],
      [inv, decodeLazyInvalidated],
    ] as const) {
      for (let cut = 0; cut < bytes.length; cut++) expect(() => decode(bytes.subarray(0, cut))).toThrow(WireError);
      expect(() => decode(new Uint8Array([...bytes, 0]))).toThrow(WireError);
      for (let bit = 0; bit < bytes.length * 8; bit++) {
        const flipped = bytes.slice();
        flipped[bit >> 3] = (flipped[bit >> 3] as number) ^ (1 << (bit & 7));
        wireOnly(() => decode(flipped));
      }
    }
  });

  it("a count the input cannot hold, or above the stated maximum, is rejected before any row is decoded", () => {
    const huge = encodeLazyPage(codecs.i32, { version: 1n, total: 5, items: [] });
    new DataView(huge.buffer).setUint32(12, 0xffff_fff0, true);
    expect(() => decodeLazyPage(codecs.i32, huge)).toThrow(WireError);
    const some = encodeLazyPage(codecs.i32, { version: 1n, total: 5, items: [1, 2, 3, 4] });
    expect(() => decodeLazyPage(codecs.i32, some, 3)).toThrow(WireError);
    expect(decodeLazyPage(codecs.i32, some, 4).items).toEqual([1, 2, 3, 4]);
  });

  it("a reader that was given more than the value is a WireError from the list's apply, and nothing changes", async () => {
    const { list, server } = await numbersList(100, { synchronous: true });
    expect(() => list.applyFull(new UndraReader(new Uint8Array([...server.value(), 1])))).toThrow(WireError);
    expect(list.length.peek()).toBe(100);
  });
});

describe("a LazyList fed hostile pages and notices", () => {
  for (const synchronous of [true, false]) {
    it(`keeps its invariants and never throws into a reader (${synchronous ? "in process" : "asynchronous"})`, async () => {
      const rand = prng(synchronous ? 0xbeef : 0xcafe);
      const { list, server, errors } = await numbersList(400, { synchronous });
      let reads = 0;
      server.override = (call) => {
        const kind = randInt(rand, 6);
        if (kind === 0) return randomBytes(rand, randInt(rand, 64));
        if (kind === 1) {
          const good = server.page(call.offset, call.limit);
          const bad = good.slice();
          bad[randInt(rand, bad.length)] = randInt(rand, 256);
          return bad;
        }
        if (kind === 2) return encodeLazyPage(codecs.i32, { version: BigInt(randInt(rand, 6)), total: randInt(rand, 500), items: [randInt(rand, 9)] });
        return undefined; // an honest answer
      };
      for (let step = 0; step < 300; step++) {
        const action = randInt(rand, 7);
        if (action <= 2) {
          const index = randInt(rand, 600) - 50;
          const row = list.get(index);
          reads++;
          if (index < 0 || index >= list.length.peek()) expect(row).toBeUndefined();
          else expect(row === undefined || Number.isInteger(row)).toBe(true);
        } else if (action === 3) {
          list.prefetch(randInt(rand, 500), randInt(rand, 700));
        } else if (action === 4) {
          const len = randInt(rand, 600);
          const version = BigInt(randInt(rand, 8));
          list.applyInvalidated(new UndraReader(encodeLazyInvalidated({ len, version })));
          // Only a newer version is taken.
          expect(list.length.peek()).toBeLessThanOrEqual(1000);
        } else if (action === 5) {
          if (randInt(rand, 4) === 0) list.applyFull(new UndraReader(encodeLazyValue({ handle: server.handle, len: randInt(rand, 300), version: BigInt(randInt(rand, 20)) })));
        } else {
          await settle(1);
          if (!synchronous) await drain();
        }
      }
      await settle();
      if (!synchronous) await drain();
      expect(reads).toBeGreaterThan(50);
      // Whatever was reported was reported through the core's error channel; nothing escaped.
      for (const error of errors) expect(error.operation === "LazyList.page" || typeof error.operation === "string").toBe(true);
      // The list still works against an honest server.
      server.override = null;
      list.applyFull(new UndraReader(encodeLazyValue({ handle: server.handle, len: server.rows.length, version: server.version + 100n })));
      list.prefetch(0, 50);
      await settle();
      if (!synchronous) await drain();
      await settle();
      expect(list.length.peek()).toBe(server.rows.length);
    });
  }
});
