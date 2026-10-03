import { UndraReader } from "./reader.js";
import { UndraWriter } from "./writer.js";

/**
 * Encoder and decoder of one wire type. Generated code implements one codec
 * per record, enum and error by composing the built-ins below; hand-written
 * code can do the same:
 *
 * ```ts
 * const todo: Codec<Todo> = {
 *   encode(w, v) { codecs.uuid.encode(w, v.id); codecs.string.encode(w, v.title); codecs.bool.encode(w, v.done); },
 *   decode(r) { return { id: codecs.uuid.decode(r), title: codecs.string.decode(r), done: codecs.bool.decode(r) }; },
 * };
 * ```
 *
 * `decode` throws {@link WireError} for malformed input and never reads past
 * the end of the reader. It does not check for trailing bytes; see
 * {@link decodeValue}.
 */
export interface Codec<T> {
  /** Appends the encoding of `v` to `w`. */
  encode(w: UndraWriter, v: T): void;
  /** Reads one value from `r`. */
  decode(r: UndraReader): T;
}

/** Encodes `value` with `codec` into a fresh, exact-length byte array. */
export function encodeValue<T>(codec: Codec<T>, value: T): Uint8Array {
  const w = new UndraWriter();
  codec.encode(w, value);
  return w.finish();
}

/**
 * Decodes one value from `bytes` with `codec`, requiring that the input is
 * consumed exactly (`trailing_bytes` otherwise).
 */
export function decodeValue<T>(codec: Codec<T>, bytes: Uint8Array): T {
  const r = new UndraReader(bytes);
  const value = codec.decode(r);
  r.finish();
  return value;
}

/**
 * Result of a `Result<T, E>` value. Discriminate with `"ok" in result`; the
 * key is present even when its value is `undefined` (a `Result<(), E>`).
 */
export type WireResult<T, E> = { readonly ok: T } | { readonly err: E };

/**
 * All built-in codecs and combinators, by name (`codecs.vec(codecs.u32)`).
 *
 * `codecs` is a module namespace, one export per codec, so a bundle keeps the codecs it names and drops the rest (ADR-057). It
 * is not a frozen plain object: its members cannot be reassigned or added to, but `Object.isFrozen(codecs)` is `false`.
 *
 * | schema type | codec | JS type |
 * |---|---|---|
 * | `bool` | `bool` | `boolean` |
 * | `u8 u16 u32 i8 i16 i32` | `u8 u16 u32 i8 i16 i32` | `number` |
 * | `u64 i64` | `u64 i64` | `bigint` |
 * | `u64 i64` with `#[undra(js_number)]` | `u64Number i64Number` | `number` |
 * | `f32 f64` | `f32 f64` | `number` |
 * | `Unit` | `unit` | `void` |
 * | `String` | `string` | `string` |
 * | `Bytes` | `bytes` | `Uint8Array` (owned copy) |
 * | `Duration` | `duration` | `number`, milliseconds |
 * | `Timestamp` | `timestamp` | `number`, milliseconds since the epoch |
 * | `Uuid` | `uuid` | `string`, canonical lowercase |
 * | object handle | `handle` | `bigint` |
 * | `Option<T>` | `option(t)` | `T \| null` |
 * | `Vec<T>` | `vec(t)` | `T[]` |
 * | `Map<K, V>` | `map(k, v)` | `Map<K, V>` |
 * | `Result<T, E>` | `result(t, e)` | `{ ok: T } \| { err: E }` |
 */
export * as codecs from "./codecs.js";
