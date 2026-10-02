import type { Codec, WireResult } from "./codec.js";
import { WireError } from "./errors.js";
import { type Bytes, type Duration, type Timestamp, durationFromNanos, durationToNanos } from "./types.js";

/*
 * The other fourteen codecs: `i8 i16 i32 i64 u64Number i64Number f32 f64 bytes duration timestamp` and the combinators `option`,
 * `map` and `result`. Part of `codecs` (see `./codecs.js`); a module nothing in a hello page's first chunk names, so a bundle
 * carries it only when a schema has such a type (ADR-057: `map` alone is 320 bytes gzipped of what a hello page never ran).
 */

export const i8: Codec<number> = { encode: (w, v) => w.writeI8(v), decode: (r) => r.readI8() };
export const i16: Codec<number> = { encode: (w, v) => w.writeI16(v), decode: (r) => r.readI16() };
export const i32: Codec<number> = { encode: (w, v) => w.writeI32(v), decode: (r) => r.readI32() };
export const i64: Codec<bigint> = { encode: (w, v) => w.writeI64(v), decode: (r) => r.readI64() };
export const f32: Codec<number> = { encode: (w, v) => w.writeF32(v), decode: (r) => r.readF32() };
export const f64: Codec<number> = { encode: (w, v) => w.writeF64(v), decode: (r) => r.readF64() };

/** `u64` as a JS `number` (`#[undra(js_number)]`); decoding fails with `unsafe_integer` above 2^53 - 1. */
export const u64Number: Codec<number> = {
  encode: (w, v) => w.writeU64Number(v),
  decode: (r) => r.readU64Number(),
};

/** `i64` as a JS `number` (`#[undra(js_number)]`); decoding fails with `unsafe_integer` outside the safe range. */
export const i64Number: Codec<number> = {
  encode: (w, v) => w.writeI64Number(v),
  decode: (r) => r.readI64Number(),
};

/**
 * `Bytes`. Decoding returns an owned copy, so a decoded value stays valid when
 * the input buffer is reused; read with `UndraReader.readBytes` when a borrowed
 * view is enough.
 */
export const bytes: Codec<Bytes> = {
  encode: (w, v) => w.writeBytes(v),
  decode: (r) => r.readBytes().slice(),
};

/** `Duration` as milliseconds; see {@link Duration}. */
export const duration: Codec<Duration> = {
  encode: (w, v) => w.writeI64(durationToNanos(v)),
  decode: (r) => durationFromNanos(r.readI64()),
};

/** `Timestamp` as integer milliseconds since the Unix epoch. */
export const timestamp: Codec<Timestamp> = {
  encode: (w, v) => w.writeI64Number(v),
  decode: (r) => r.readI64Number(),
};

/**
 * `Option<T>` as `T | null`.
 *
 * Because `None` is `null`, `Option<Option<T>>` cannot be represented: `Some(None)`
 * and `None` are both `null` and both encode as a single `0` byte.
 */
export function option<T>(inner: Codec<T>): Codec<T | null> {
  return {
    encode(w, v) {
      if (v === null) {
        w.writeU8(0);
      } else {
        w.writeU8(1);
        inner.encode(w, v);
      }
    },
    decode(r) {
      const at = r.position;
      const tag = r.readU8();
      if (tag === 0) return null;
      if (tag === 1) return inner.decode(r);
      throw new WireError({ code: "invalid_tag", tag, at, ty: "Option" });
    },
  };
}

/** Lexicographic unsigned comparison of `buf[aStart, aEnd)` and `buf[bStart, bEnd)`. */
function compareRanges(buf: Uint8Array, aStart: number, aEnd: number, bStart: number, bEnd: number): number {
  const la = aEnd - aStart;
  const lb = bEnd - bStart;
  const n = la < lb ? la : lb;
  for (let i = 0; i < n; i++) {
    const d = (buf[aStart + i] as number) - (buf[bStart + i] as number);
    if (d !== 0) return d;
  }
  return la - lb;
}

/**
 * `Map<K, V>` as a JS `Map`.
 *
 * The encoder writes entries sorted by the bytes of their *encoded* keys
 * (unsigned lexicographic order), so equal maps encode identically whatever
 * their insertion order; entries already in that order are written as they
 * are, without a second pass. Two keys that encode to the same bytes are
 * `duplicate_key`. The decoder rejects a key that appears twice with
 * `duplicate_key`; it does not require sorted input.
 *
 * Keys are compared with `Map` semantics, so use primitive key codecs
 * (`string`, integers, `bigint`, `uuid`, `bool`): a `bytes` key is a
 * different `Map` key on every decode and duplicates would go undetected.
 */
export function map<K, V>(key: Codec<K>, value: Codec<V>): Codec<Map<K, V>> {
  return {
    encode(w, m) {
      const n = m.size;
      w.writeLen(n);
      if (n < 2) {
        for (const [k, v] of m) {
          key.encode(w, k);
          value.encode(w, v);
        }
        return;
      }
      const start = w.position;
      // Entry i spans [ends[2i], ends[2i + 2]) with its key ending at ends[2i + 1].
      const ends = new Uint32Array(2 * n + 1);
      ends[0] = start;
      let i = 0;
      for (const [k, v] of m) {
        key.encode(w, k);
        ends[2 * i + 1] = w.position;
        value.encode(w, v);
        ends[2 * i + 2] = w.position;
        i++;
      }
      const buf = w.view(0);
      // In range by construction: ends has 2n + 1 entries and e < n.
      const keyStart = (e: number): number => ends[2 * e] as number;
      const keyEnd = (e: number): number => ends[2 * e + 1] as number;
      const entryEnd = (e: number): number => ends[2 * e + 2] as number;
      const compareKeys = (a: number, b: number): number =>
        compareRanges(buf, keyStart(a), keyEnd(a), keyStart(b), keyEnd(b));

      let ascending = true;
      for (let e = 1; e < n; e++) {
        if (compareKeys(e - 1, e) >= 0) {
          ascending = false;
          break;
        }
      }
      if (ascending) return;

      const order = new Uint32Array(n);
      for (let e = 0; e < n; e++) order[e] = e;
      order.sort(compareKeys);
      for (let j = 1; j < n; j++) {
        const prev = order[j - 1] as number;
        const cur = order[j] as number;
        if (compareKeys(prev, cur) === 0) {
          throw new WireError({ code: "duplicate_key", at: keyStart(cur) });
        }
      }
      // Rewrite the entries in sorted order from a copy of the region.
      const copy = buf.slice(start, entryEnd(n - 1));
      w.rewind(start);
      for (let j = 0; j < n; j++) {
        const e = order[j] as number;
        w.writeRaw(copy, keyStart(e) - start, entryEnd(e) - start);
      }
    },
    decode(r) {
      const n = r.readLen();
      const out = new Map<K, V>();
      for (let i = 0; i < n; i++) {
        const at = r.position;
        const k = key.decode(r);
        if (out.has(k)) throw new WireError({ code: "duplicate_key", at });
        out.set(k, value.decode(r));
      }
      return out;
    },
  };
}

/** `Result<T, E>` as `{ ok: T } | { err: E }`. */
export function result<T, E>(ok: Codec<T>, err: Codec<E>): Codec<WireResult<T, E>> {
  return {
    encode(w, v) {
      if ("ok" in v) {
        w.writeU8(0);
        ok.encode(w, v.ok);
      } else {
        w.writeU8(1);
        err.encode(w, v.err);
      }
    },
    decode(r) {
      const at = r.position;
      const tag = r.readU8();
      if (tag === 0) return { ok: ok.decode(r) };
      if (tag === 1) return { err: err.decode(r) };
      throw new WireError({ code: "invalid_tag", tag, at, ty: "Result" });
    },
  };
}
