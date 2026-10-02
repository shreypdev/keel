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

// ---------------------------------------------------------------------------
// Primitives
// ---------------------------------------------------------------------------

/** The primitive and composite codecs, by name (`codecs.vec(codecs.u32)`): a module namespace, so a bundle keeps the ones it names. */
export * as codecs from "./codecs.js";
