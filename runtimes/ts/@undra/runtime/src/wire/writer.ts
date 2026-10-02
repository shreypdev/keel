import { WireError } from "./errors.js";
import { type Uuid, encodeUuid } from "./types.js";

const ENCODER = new TextEncoder();
const U32_MAX = 0xffff_ffff;
const TWO_POW_32 = 0x1_0000_0000;
const MIN_GROWTH = 64;
/** The default initial capacity: V8 allocates a typed array of at most this many bytes on its heap (no backing store, no `ArrayBuffer`). */
const DEFAULT_CAPACITY = 64;

/**
 * Strings of at most this many UTF-16 code units are encoded by a hand-written
 * loop straight into the buffer. `TextEncoder.encodeInto` needs a fresh
 * `Uint8Array` view of the destination for every call, which would allocate
 * on each short string; the loop does not. Longer strings amortise the view
 * over the copy and use the native encoder.
 */
const SHORT_STRING_UNITS = 48;

const EMPTY_BUFFER = new Uint8Array(0);

function outOfRange(ty: string, value: unknown): RangeError {
  return new RangeError(`${ty} out of range: ${String(value)}`);
}

/**
 * Encoder for the Undra wire format (docs/SPEC.md section 3.1): little-endian,
 * no alignment, no padding, every length a `u32`.
 *
 * The writer owns a growable `Uint8Array`; capacity grows geometrically, so
 * writing `n` bytes costs amortised O(n) and no write allocates unless the
 * buffer must grow. Integer writers validate their argument and throw
 * `RangeError` instead of silently wrapping (`writeU8(300)` is a bug in the
 * caller, never a value to truncate).
 *
 * ```ts
 * const w = new UndraWriter();
 * w.writeU32(7);
 * w.writeStr("héllo");
 * const bytes = w.finish();
 * ```
 */
export class UndraWriter {
  #buf: Uint8Array;
  /** Over `#buf`, made on first use: only the 64-bit and floating-point writers need one (the integer writers store bytes). */
  #dv: DataView | null = null;
  #pos = 0;

  /**
   * @param initialCapacity Bytes to allocate up front; the buffer grows on demand beyond it. The default
   *   (64) is what V8 keeps on the JavaScript heap, so the writer of a small call allocates no backing store.
   */
  constructor(initialCapacity = DEFAULT_CAPACITY) {
    if (!Number.isInteger(initialCapacity) || initialCapacity < 0 || initialCapacity > U32_MAX) {
      throw outOfRange("initial capacity", initialCapacity);
    }
    this.#buf = initialCapacity === 0 ? EMPTY_BUFFER : new Uint8Array(initialCapacity);
  }

  /** A `DataView` over the buffer, for the writers that cannot store bytes one by one cheaply. */
  #view(): DataView {
    return (this.#dv ??= new DataView(this.#buf.buffer, this.#buf.byteOffset, this.#buf.byteLength));
  }

  /** Number of bytes written so far. */
  get position(): number {
    return this.#pos;
  }

  /** Size of the internal buffer; writes beyond it trigger a geometric grow. */
  get capacity(): number {
    return this.#buf.length;
  }

  /**
   * Ensures at least `n` more bytes can be written without reallocating.
   * Use it before a burst of writes whose total size is known.
   */
  reserve(n: number): void {
    if (!Number.isInteger(n) || n < 0) throw outOfRange("reserve", n);
    this.#ensure(n);
  }

  #ensure(n: number): void {
    if (this.#pos + n > this.#buf.length) this.#grow(n);
  }

  #grow(extra: number): void {
    const need = this.#pos + extra;
    if (need > U32_MAX) {
      throw new WireError({ code: "length_too_large", len: need, at: this.#pos });
    }
    let cap = this.#buf.length * 2;
    if (cap < MIN_GROWTH) cap = MIN_GROWTH;
    if (cap < need) cap = need;
    if (cap > U32_MAX) cap = U32_MAX;
    const next = new Uint8Array(cap);
    next.set(this.#buf);
    this.#buf = next;
    this.#dv = null;
  }

  /** Stores `v` (any 32-bit integer) as four little-endian bytes. */
  #put32(v: number): void {
    this.#ensure(4);
    const b = this.#buf;
    const p = this.#pos;
    b[p] = v;
    b[p + 1] = v >>> 8;
    b[p + 2] = v >>> 16;
    b[p + 3] = v >>> 24;
    this.#pos = p + 4;
  }

  /** Stores `v` as four little-endian bytes at `at` (inside what was written already). */
  #put32At(at: number, v: number): void {
    const b = this.#buf;
    b[at] = v;
    b[at + 1] = v >>> 8;
    b[at + 2] = v >>> 16;
    b[at + 3] = v >>> 24;
  }

  /** Writes an unsigned 8-bit integer. */
  writeU8(v: number): void {
    if ((v & 0xff) !== v) throw outOfRange("u8", v);
    this.#ensure(1);
    this.#buf[this.#pos++] = v;
  }

  /** Writes a signed 8-bit integer. */
  writeI8(v: number): void {
    if (((v << 24) >> 24) !== v) throw outOfRange("i8", v);
    this.#ensure(1);
    this.#buf[this.#pos++] = v;
  }

  /** Writes an unsigned 16-bit integer. */
  writeU16(v: number): void {
    if ((v & 0xffff) !== v) throw outOfRange("u16", v);
    this.#ensure(2);
    const b = this.#buf;
    const p = this.#pos;
    b[p] = v;
    b[p + 1] = v >>> 8;
    this.#pos = p + 2;
  }

  /** Writes a signed 16-bit integer. */
  writeI16(v: number): void {
    if (((v << 16) >> 16) !== v) throw outOfRange("i16", v);
    this.#ensure(2);
    const b = this.#buf;
    const p = this.#pos;
    b[p] = v;
    b[p + 1] = v >>> 8;
    this.#pos = p + 2;
  }

  /** Writes an unsigned 32-bit integer. */
  writeU32(v: number): void {
    if (v >>> 0 !== v) throw outOfRange("u32", v);
    this.#put32(v);
  }

  /** Writes a signed 32-bit integer. */
  writeI32(v: number): void {
    if ((v | 0) !== v) throw outOfRange("i32", v);
    this.#put32(v);
  }

  /** Writes an unsigned 64-bit integer given as a `bigint`. */
  writeU64(v: bigint): void {
    if (BigInt.asUintN(64, v) !== v) throw outOfRange("u64", v);
    this.#ensure(8);
    this.#view().setBigUint64(this.#pos, v, true);
    this.#pos += 8;
  }

  /** Writes a signed 64-bit integer given as a `bigint`. */
  writeI64(v: bigint): void {
    if (BigInt.asIntN(64, v) !== v) throw outOfRange("i64", v);
    this.#ensure(8);
    this.#view().setBigInt64(this.#pos, v, true);
    this.#pos += 8;
  }

  /**
   * Writes an unsigned 64-bit integer given as a JS `number` (a
   * `#[undra(js_number)]` field). Throws `RangeError` unless `n` is a
   * non-negative safe integer. Does not allocate a `BigInt`.
   */
  writeU64Number(n: number): void {
    if (!Number.isSafeInteger(n) || n < 0) throw outOfRange("u64 (number)", n);
    const lo = n >>> 0;
    this.#put32(lo);
    this.#put32((n - lo) / TWO_POW_32);
  }

  /**
   * Writes a signed 64-bit integer given as a JS `number`. Throws `RangeError`
   * unless `n` is a safe integer. Does not allocate a `BigInt`.
   */
  writeI64Number(n: number): void {
    if (!Number.isSafeInteger(n)) throw outOfRange("i64 (number)", n);
    const lo = n >>> 0;
    this.#put32(lo);
    this.#put32((n - lo) / TWO_POW_32);
  }

  /** Writes an IEEE 754 binary32 (the value is rounded to `f32`). */
  writeF32(v: number): void {
    this.#ensure(4);
    this.#view().setFloat32(this.#pos, v, true);
    this.#pos += 4;
  }

  /** Writes an IEEE 754 binary64. */
  writeF64(v: number): void {
    this.#ensure(8);
    this.#view().setFloat64(this.#pos, v, true);
    this.#pos += 8;
  }

  /** Writes a boolean as one byte, `0` or `1`. */
  writeBool(v: boolean): void {
    this.#ensure(1);
    this.#buf[this.#pos++] = v ? 1 : 0;
  }

  /**
   * Writes a length or element count as a `u32`. Throws `WireError`
   * (`length_too_large`) above `2^32 - 1` and `RangeError` for values that are
   * not non-negative integers.
   */
  writeLen(n: number): void {
    if (n >>> 0 !== n) {
      if (Number.isInteger(n) && n > U32_MAX) {
        throw new WireError({ code: "length_too_large", len: n, at: this.#pos });
      }
      throw outOfRange("length", n);
    }
    this.#put32(n);
  }

  /**
   * Writes a string as a `u32` byte length followed by UTF-8. Lone surrogates
   * (which cannot occur in a Rust `String`) are replaced by U+FFFD, exactly as
   * `TextEncoder` does.
   */
  writeStr(s: string): void {
    const n = s.length;
    if (n <= SHORT_STRING_UNITS) {
      this.#writeShortStr(s, n);
      return;
    }
    // Optimistic: assume ASCII (one byte per unit) and grow only if the
    // native encoder runs out of room, so a 10 MB ASCII string does not
    // reserve 30 MB.
    this.#ensure(4 + n);
    const lenPos = this.#pos;
    let end = lenPos + 4;
    const first = ENCODER.encodeInto(s, this.#buf.subarray(end));
    end += first.written;
    if (first.read < n) {
      // Destination filled up. Room for the rest in the worst case (three
      // bytes per unit) always suffices, so one more call finishes the job.
      this.#pos = end;
      this.#ensure((n - first.read) * 3);
      end += ENCODER.encodeInto(s.slice(first.read), this.#buf.subarray(end)).written;
    }
    this.#put32At(lenPos, end - lenPos - 4);
    this.#pos = end;
  }

  #writeShortStr(s: string, n: number): void {
    // ASCII first: one byte a unit, so room for the length and the text is all it needs.
    this.#ensure(4 + n);
    let buf = this.#buf;
    const start = this.#pos + 4;
    let p = start;
    let i = 0;
    while (i < n) {
      const c = s.charCodeAt(i);
      if (c >= 0x80) break;
      buf[p++] = c;
      i++;
    }
    if (i < n) {
      // Something else: room for the rest at three bytes a unit (what was written stays: a grow copies the buffer).
      this.#ensure(p - this.#pos + (n - i) * 3);
      buf = this.#buf;
    }
    for (; i < n; i++) {
      const c = s.charCodeAt(i);
      if (c < 0x80) {
        buf[p++] = c;
      } else if (c < 0x800) {
        buf[p++] = 0xc0 | (c >> 6);
        buf[p++] = 0x80 | (c & 0x3f);
      } else if (c >= 0xd800 && c <= 0xdfff) {
        if (c <= 0xdbff && i + 1 < n) {
          const d = s.charCodeAt(i + 1);
          if (d >= 0xdc00 && d <= 0xdfff) {
            const cp = 0x10000 + ((c - 0xd800) << 10) + (d - 0xdc00);
            buf[p++] = 0xf0 | (cp >> 18);
            buf[p++] = 0x80 | ((cp >> 12) & 0x3f);
            buf[p++] = 0x80 | ((cp >> 6) & 0x3f);
            buf[p++] = 0x80 | (cp & 0x3f);
            i++;
            continue;
          }
        }
        // Lone surrogate: U+FFFD, as TextEncoder does.
        buf[p++] = 0xef;
        buf[p++] = 0xbf;
        buf[p++] = 0xbd;
      } else {
        buf[p++] = 0xe0 | (c >> 12);
        buf[p++] = 0x80 | ((c >> 6) & 0x3f);
        buf[p++] = 0x80 | (c & 0x3f);
      }
    }
    this.#put32At(this.#pos, p - start);
    this.#pos = p;
  }

  /**
   * Writes a UUID as 16 raw bytes, parsed straight into the buffer (see
   * {@link encodeUuid} for the accepted syntax; `RangeError` if invalid).
   */
  writeUuid(uuid: Uuid): void {
    this.#ensure(16);
    encodeUuid(uuid, this.#buf, this.#pos);
    this.#pos += 16;
  }

  /** Writes a `u32` length followed by the bytes of `b`. */
  writeBytes(b: Uint8Array): void {
    this.writeLen(b.length);
    this.writeRaw(b);
  }

  /**
   * Appends raw bytes with no length prefix (the tail of a payload, or bytes
   * that were encoded elsewhere). `start` and `end` select a sub-range of `b`
   * without creating a view.
   */
  writeRaw(b: Uint8Array, start = 0, end: number = b.length): void {
    if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end < start || end > b.length) {
      throw outOfRange("raw range", `${start}..${end} of ${b.length}`);
    }
    const n = end - start;
    this.#ensure(n);
    this.#buf.set(start === 0 && end === b.length ? b : b.subarray(start, end), this.#pos);
    this.#pos += n;
  }

  /**
   * A view of the bytes written in `[start, end)`. The view aliases the
   * writer's buffer: it is invalidated by the next write that grows the
   * buffer and by `finish()`, so use it immediately or copy it.
   */
  view(start: number, end: number = this.#pos): Uint8Array {
    if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end < start || end > this.#pos) {
      throw outOfRange("view range", `${start}..${end} of ${this.#pos}`);
    }
    return this.#buf.subarray(start, end);
  }

  /** Moves the write position back to `position`, discarding the bytes after it. */
  rewind(position: number): void {
    if (!Number.isInteger(position) || position < 0 || position > this.#pos) {
      throw outOfRange("rewind position", position);
    }
    this.#pos = position;
  }

  /**
   * Returns the written bytes as an exact-length view and hands ownership of
   * the buffer to the caller. The writer is reset to empty (a later write
   * allocates a fresh buffer), so the returned bytes are never overwritten.
   *
   * The view's `byteLength` is exact, but its underlying `ArrayBuffer` may be
   * larger: pass the view itself, not `view.buffer`, to anything that takes
   * bytes, and `slice()` it if it will be retained for long.
   */
  finish(): Uint8Array {
    // A small result is copied out exactly: `subarray` would give the 64-byte on-heap buffer a backing store of its own.
    const out = this.#pos <= DEFAULT_CAPACITY ? this.#buf.slice(0, this.#pos) : this.#buf.subarray(0, this.#pos);
    this.#buf = EMPTY_BUFFER;
    this.#dv = null;
    this.#pos = 0;
    return out;
  }
}
