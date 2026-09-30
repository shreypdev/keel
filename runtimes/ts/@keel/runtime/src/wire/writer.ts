import { WireError } from "./errors.js";
import { type Uuid, encodeUuid } from "./types.js";

const ENCODER = new TextEncoder();
const U32_MAX = 0xffff_ffff;
const TWO_POW_32 = 0x1_0000_0000;
const MIN_GROWTH = 64;

/**
 * Strings of at most this many UTF-16 code units are encoded by a hand-written
 * loop straight into the buffer. `TextEncoder.encodeInto` needs a fresh
 * `Uint8Array` view of the destination for every call, which would allocate
 * on each short string; the loop does not. Longer strings amortise the view
 * over the copy and use the native encoder.
 */
const SHORT_STRING_UNITS = 48;

const EMPTY_BUFFER = new Uint8Array(0);
const EMPTY_VIEW = new DataView(EMPTY_BUFFER.buffer);

function outOfRange(ty: string, value: unknown): RangeError {
  return new RangeError(`${ty} out of range: ${String(value)}`);
}

/**
 * Encoder for the Keel wire format (docs/SPEC.md section 3.1): little-endian,
 * no alignment, no padding, every length a `u32`.
 *
 * The writer owns a growable `Uint8Array`; capacity grows geometrically, so
 * writing `n` bytes costs amortised O(n) and no write allocates unless the
 * buffer must grow. Integer writers validate their argument and throw
 * `RangeError` instead of silently wrapping (`writeU8(300)` is a bug in the
 * caller, never a value to truncate).
 *
 * ```ts
 * const w = new KeelWriter();
 * w.writeU32(7);
 * w.writeStr("héllo");
 * const bytes = w.finish();
 * ```
 */
export class KeelWriter {
  #buf: Uint8Array;
  #view: DataView;
  #pos = 0;

  /** @param initialCapacity Bytes to allocate up front; the buffer grows on demand beyond it. */
  constructor(initialCapacity = 256) {
    if (!Number.isInteger(initialCapacity) || initialCapacity < 0 || initialCapacity > U32_MAX) {
      throw outOfRange("initial capacity", initialCapacity);
    }
    if (initialCapacity === 0) {
      this.#buf = EMPTY_BUFFER;
      this.#view = EMPTY_VIEW;
    } else {
      this.#buf = new Uint8Array(initialCapacity);
      this.#view = new DataView(this.#buf.buffer);
    }
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
    next.set(this.#buf.subarray(0, this.#pos));
    this.#buf = next;
    this.#view = new DataView(next.buffer);
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
    this.#view.setInt8(this.#pos, v);
    this.#pos += 1;
  }

  /** Writes an unsigned 16-bit integer. */
  writeU16(v: number): void {
    if ((v & 0xffff) !== v) throw outOfRange("u16", v);
    this.#ensure(2);
    this.#view.setUint16(this.#pos, v, true);
    this.#pos += 2;
  }

  /** Writes a signed 16-bit integer. */
  writeI16(v: number): void {
    if (((v << 16) >> 16) !== v) throw outOfRange("i16", v);
    this.#ensure(2);
    this.#view.setInt16(this.#pos, v, true);
    this.#pos += 2;
  }

  /** Writes an unsigned 32-bit integer. */
  writeU32(v: number): void {
    if (v >>> 0 !== v) throw outOfRange("u32", v);
    this.#ensure(4);
    this.#view.setUint32(this.#pos, v, true);
    this.#pos += 4;
  }

  /** Writes a signed 32-bit integer. */
  writeI32(v: number): void {
    if ((v | 0) !== v) throw outOfRange("i32", v);
    this.#ensure(4);
    this.#view.setInt32(this.#pos, v, true);
    this.#pos += 4;
  }

  /** Writes an unsigned 64-bit integer given as a `bigint`. */
  writeU64(v: bigint): void {
    if (BigInt.asUintN(64, v) !== v) throw outOfRange("u64", v);
    this.#ensure(8);
    this.#view.setBigUint64(this.#pos, v, true);
    this.#pos += 8;
  }

  /** Writes a signed 64-bit integer given as a `bigint`. */
  writeI64(v: bigint): void {
    if (BigInt.asIntN(64, v) !== v) throw outOfRange("i64", v);
    this.#ensure(8);
    this.#view.setBigInt64(this.#pos, v, true);
    this.#pos += 8;
  }

  /**
   * Writes an unsigned 64-bit integer given as a JS `number` (a
   * `#[keel(js_number)]` field). Throws `RangeError` unless `n` is a
   * non-negative safe integer. Does not allocate a `BigInt`.
   */
  writeU64Number(n: number): void {
    if (!Number.isSafeInteger(n) || n < 0) throw outOfRange("u64 (number)", n);
    this.#ensure(8);
    this.#view.setUint32(this.#pos, n >>> 0, true);
    this.#view.setUint32(this.#pos + 4, (n - (n >>> 0)) / TWO_POW_32, true);
    this.#pos += 8;
  }

  /**
   * Writes a signed 64-bit integer given as a JS `number`. Throws `RangeError`
   * unless `n` is a safe integer. Does not allocate a `BigInt`.
   */
  writeI64Number(n: number): void {
    if (!Number.isSafeInteger(n)) throw outOfRange("i64 (number)", n);
    const lo = n >>> 0;
    this.#ensure(8);
    this.#view.setUint32(this.#pos, lo, true);
    this.#view.setInt32(this.#pos + 4, (n - lo) / TWO_POW_32, true);
    this.#pos += 8;
  }

  /** Writes an IEEE 754 binary32 (the value is rounded to `f32`). */
  writeF32(v: number): void {
    this.#ensure(4);
    this.#view.setFloat32(this.#pos, v, true);
    this.#pos += 4;
  }

  /** Writes an IEEE 754 binary64. */
  writeF64(v: number): void {
    this.#ensure(8);
    this.#view.setFloat64(this.#pos, v, true);
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
    this.#ensure(4);
    this.#view.setUint32(this.#pos, n, true);
    this.#pos += 4;
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
    this.#view.setUint32(lenPos, end - lenPos - 4, true);
    this.#pos = end;
  }

  #writeShortStr(s: string, n: number): void {
    this.#ensure(4 + n * 3);
    const buf = this.#buf;
    const start = this.#pos + 4;
    let p = start;
    for (let i = 0; i < n; i++) {
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
    this.#view.setUint32(this.#pos, p - start, true);
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
    const out = this.#buf.subarray(0, this.#pos);
    this.#buf = EMPTY_BUFFER;
    this.#view = EMPTY_VIEW;
    this.#pos = 0;
    return out;
  }
}
