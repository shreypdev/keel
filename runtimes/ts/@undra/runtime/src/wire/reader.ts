import { WireError } from "./errors.js";
import { type Uuid, decodeUuid } from "./types.js";
import { msg } from "../messages.js";

/**
 * `fatal` makes invalid UTF-8 throw instead of decoding to U+FFFD. Despite its
 * name, `ignoreBOM: true` is what *keeps* a leading U+FEFF in the output;
 * without it the decoder would silently strip it from strings that start
 * with one.
 */
const DECODER = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

/** Strings of at most this many bytes are decoded by a loop when they are ASCII; longer ones amortise `TextDecoder`'s fixed cost. */
const SHORT_ASCII = 24;

/**
 * Eight bytes shared by every reader: a 64-bit or floating-point value is copied here to be read by a `DataView`, which
 * costs a copy of a few bytes where a `DataView` over each input would cost an object (and, for a small input V8 keeps on
 * its heap, an `ArrayBuffer`) per reader. Nothing yields between the copy and the read.
 */
const SCRATCH = new DataView(new ArrayBuffer(8));
const SCRATCH_BYTES = new Uint8Array(SCRATCH.buffer);

const TWO_POW_32 = 0x1_0000_0000;
/** Largest `hi` word of a non-negative 64-bit value that is still a safe integer. */
const MAX_SAFE_HI = 0x1f_ffff;
/** `hi` word of `-2^53`, the first negative integer that is not safe. */
const MIN_UNSAFE_HI = -0x20_0000;

/**
 * Decoder for the Undra wire format (docs/SPEC.md section 3.1).
 *
 * The reader works over a `Uint8Array` and respects its `byteOffset` and
 * `byteLength`, so a view into a larger buffer (a subarray of a WebSocket
 * message, a slice of wasm memory) reads exactly the bytes of the view.
 * Every failure is a {@link WireError}; nothing else is ever thrown for
 * malformed input, and the reader never reads outside its view.
 *
 * ```ts
 * const r = new UndraReader(bytes);
 * const id = r.readU32();
 * const name = r.readStr();
 * r.finish(); // throws WireError("trailing_bytes") if anything is left over
 * ```
 */
export class UndraReader {
  private readonly _bytes: Uint8Array;
  private _pos = 0;

  /** @param bytes The message to read. The reader keeps a reference; it never copies or modifies the bytes. */
  constructor(bytes: Uint8Array) {
    this._bytes = bytes;
  }

  /** Copies the `n` bytes at `p` into the shared scratch and returns its `DataView`: for the 64-bit and floating-point reads. */
  private _scratch(p: number, n: number): DataView {
    const b = this._bytes;
    for (let i = 0; i < n; i++) SCRATCH_BYTES[i] = b[p + i] as number;
    return SCRATCH;
  }

  /** Offset of the next unread byte, relative to the start of the input. */
  get position(): number {
    return this._pos;
  }

  /** Number of unread bytes. */
  get remaining(): number {
    return this._bytes.length - this._pos;
  }

  /** Throws `unexpected_eof` for a read of `n` bytes at the current position. */
  private _eof(n: number): never {
    throw new WireError({
      code: "unexpected_eof",
      needed: n - (this._bytes.length - this._pos),
      at: this._pos,
    });
  }

  /** The little-endian `u32` at `p` (in range: the caller checked). */
  private _u32(p: number): number {
    const b = this._bytes;
    return ((b[p] as number) | ((b[p + 1] as number) << 8) | ((b[p + 2] as number) << 16) | ((b[p + 3] as number) << 24)) >>> 0;
  }

  /** Reads an unsigned 8-bit integer. */
  readU8(): number {
    const p = this._pos;
    if (p + 1 > this._bytes.length) this._eof(1);
    this._pos = p + 1;
    return this._bytes[p] as number;
  }

  /** Reads a signed 8-bit integer. */
  readI8(): number {
    const p = this._pos;
    if (p + 1 > this._bytes.length) this._eof(1);
    this._pos = p + 1;
    return ((this._bytes[p] as number) << 24) >> 24;
  }

  /** Reads an unsigned 16-bit integer. */
  readU16(): number {
    const p = this._pos;
    if (p + 2 > this._bytes.length) this._eof(2);
    this._pos = p + 2;
    const b = this._bytes;
    return (b[p] as number) | ((b[p + 1] as number) << 8);
  }

  /** Reads a signed 16-bit integer. */
  readI16(): number {
    const p = this._pos;
    if (p + 2 > this._bytes.length) this._eof(2);
    this._pos = p + 2;
    const b = this._bytes;
    return (((b[p] as number) | ((b[p + 1] as number) << 8)) << 16) >> 16;
  }

  /** Reads an unsigned 32-bit integer. */
  readU32(): number {
    const p = this._pos;
    if (p + 4 > this._bytes.length) this._eof(4);
    this._pos = p + 4;
    return this._u32(p);
  }

  /** Reads a signed 32-bit integer. */
  readI32(): number {
    const p = this._pos;
    if (p + 4 > this._bytes.length) this._eof(4);
    this._pos = p + 4;
    return this._u32(p) | 0;
  }

  /** Reads an unsigned 64-bit integer as a `bigint`. */
  readU64(): bigint {
    const p = this._pos;
    if (p + 8 > this._bytes.length) this._eof(8);
    this._pos = p + 8;
    return this._scratch(p, 8).getBigUint64(0, true);
  }

  /** Reads a signed 64-bit integer as a `bigint`. */
  readI64(): bigint {
    const p = this._pos;
    if (p + 8 > this._bytes.length) this._eof(8);
    this._pos = p + 8;
    return this._scratch(p, 8).getBigInt64(0, true);
  }

  /**
   * Reads an unsigned 64-bit integer as a JS `number`. Throws `WireError`
   * (`unsafe_integer`) if the value exceeds `Number.MAX_SAFE_INTEGER`. Does not
   * allocate a `BigInt` on the success path.
   */
  readU64Number(): number {
    const p = this._pos;
    if (p + 8 > this._bytes.length) this._eof(8);
    const lo = this._u32(p);
    const hi = this._u32(p + 4);
    if (hi > MAX_SAFE_HI) {
      throw new WireError({ code: "unsafe_integer", value: this._scratch(p, 8).getBigUint64(0, true), at: p });
    }
    this._pos = p + 8;
    return hi * TWO_POW_32 + lo;
  }

  /**
   * Reads a signed 64-bit integer as a JS `number`. Throws `WireError`
   * (`unsafe_integer`) outside the safe integer range. Does not allocate a
   * `BigInt` on the success path.
   */
  readI64Number(): number {
    const p = this._pos;
    if (p + 8 > this._bytes.length) this._eof(8);
    const lo = this._u32(p);
    const hi = this._u32(p + 4) | 0;
    if (hi > MAX_SAFE_HI || hi < MIN_UNSAFE_HI || (hi === MIN_UNSAFE_HI && lo === 0)) {
      throw new WireError({ code: "unsafe_integer", value: this._scratch(p, 8).getBigInt64(0, true), at: p });
    }
    this._pos = p + 8;
    return hi * TWO_POW_32 + lo;
  }

  /** Reads an IEEE 754 binary32. */
  readF32(): number {
    const p = this._pos;
    if (p + 4 > this._bytes.length) this._eof(4);
    this._pos = p + 4;
    return this._scratch(p, 4).getFloat32(0, true);
  }

  /** Reads an IEEE 754 binary64. */
  readF64(): number {
    const p = this._pos;
    if (p + 8 > this._bytes.length) this._eof(8);
    this._pos = p + 8;
    return this._scratch(p, 8).getFloat64(0, true);
  }

  /** Reads a boolean. Any byte other than 0 or 1 is `invalid_tag`. */
  readBool(): boolean {
    const p = this._pos;
    if (p + 1 > this._bytes.length) this._eof(1);
    const v = this._bytes[p] as number;
    if (v > 1) throw new WireError({ code: "invalid_tag", tag: v, at: p, ty: "bool" });
    this._pos = p + 1;
    return v === 1;
  }

  /** Reads 16 raw bytes as a canonical lowercase UUID string, without an intermediate copy. */
  readUuid(): Uuid {
    const p = this._pos;
    if (p + 16 > this._bytes.length) this._eof(16);
    this._pos = p + 16;
    return decodeUuid(this._bytes, p);
  }

  /**
   * Reads a `u32` length or element count and checks that the input can
   * plausibly hold it: `count * minItemSize` must not exceed the unread bytes,
   * otherwise `length_too_large`. This bounds every allocation a hostile
   * count could trigger by the size of the input itself.
   *
   * @param minItemSize Smallest possible encoded size of one item; `1` for
   *   byte lengths and for items of unknown size. Items that encode to zero
   *   bytes (`Unit`, a record without fields) cannot be counted this way: a
   *   sequence of them longer than the remaining input is rejected.
   */
  readLen(minItemSize = 1): number {
    const at = this._pos;
    if (at + 4 > this._bytes.length) this._eof(4);
    const n = this._u32(at);
    if (n * minItemSize > this._bytes.length - at - 4) {
      throw new WireError({ code: "length_too_large", len: n, at });
    }
    this._pos = at + 4;
    return n;
  }

  /** Reads a `u32`-length-prefixed string. Invalid UTF-8 is `invalid_utf8`. */
  readStr(): string {
    const at = this._pos;
    const n = this.readLen();
    const start = this._pos;
    const end = start + n;
    let s = "";
    if (n <= SHORT_ASCII) {
      // ASCII is the common case for the short strings of names and keys: build it here, not through `TextDecoder`.
      const b = this._bytes;
      let i = start;
      while (i < end) {
        const c = b[i] as number;
        if (c >= 0x80) break;
        s += String.fromCharCode(c);
        i++;
      }
      if (i === end) {
        this._pos = end;
        return s;
      }
    }
    try {
      s = DECODER.decode(this._bytes.subarray(start, end));
    } catch {
      throw new WireError({ code: "invalid_utf8", at });
    }
    this._pos = end;
    return s;
  }

  /**
   * Reads a `u32`-length-prefixed byte string.
   *
   * The result is a **borrowed view** into the input buffer, not a copy: it is
   * only valid while the input is unchanged. If the input is reused (a pooled
   * transport buffer) or can be detached (wasm linear memory after growth),
   * `slice()` the result before keeping it.
   */
  readBytes(): Uint8Array {
    return this.readRaw(this.readLen());
  }

  /** Reads exactly `n` raw bytes (no length prefix) as a borrowed view; see {@link readBytes}. */
  readRaw(n: number): Uint8Array {
    if (!Number.isInteger(n) || n < 0) throw new RangeError(msg(223, String(n)));
    const p = this._pos;
    if (n > this._bytes.length - p) this._eof(n);
    this._pos = p + n;
    return this._bytes.subarray(p, p + n);
  }

  /** Reads all unread bytes as a borrowed view (the opaque tail of a payload); see {@link readBytes}. */
  readRest(): Uint8Array {
    const p = this._pos;
    this._pos = this._bytes.length;
    return this._bytes.subarray(p);
  }

  /** Asserts that the whole input was consumed; throws `trailing_bytes` otherwise. */
  finish(): void {
    const left = this._bytes.length - this._pos;
    if (left !== 0) throw new WireError({ code: "trailing_bytes", count: left });
  }
}
