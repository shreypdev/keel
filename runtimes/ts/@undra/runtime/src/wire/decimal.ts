import { WireError } from "./errors.js";
import type { Codec } from "./codec.js";

const MAX_SCALE = 38;
const I128_MIN = -(1n << 127n);
const I128_MAX = (1n << 127n) - 1n;

/**
 * An exact decimal number: `mantissa x 10^-scale`, the wire `Decimal` (ADR-042).
 *
 * Immutable. There is no arithmetic and no float round trip: convert through
 * {@link Decimal.toString} / {@link Decimal.parse} to the app's decimal library. It is not
 * normalised, so `1.0` and `1.00` are different values to {@link Decimal.equals} and the same
 * number to {@link Decimal.compare}.
 *
 * ```ts
 * const price = Decimal.parse("19.99");
 * price.mantissa; // 1999n
 * price.scale; // 2
 * price.toString(); // "19.99"
 * ```
 */
export class Decimal {
  /** The unscaled value, a signed 128-bit integer. */
  readonly mantissa: bigint;
  /** How many of the digits are after the point, 0 to 38. */
  readonly scale: number;

  /**
   * @param mantissa The unscaled value; must fit a signed 128-bit integer.
   * @param scale Digits after the point, an integer from 0 to 38.
   * @throws RangeError if either is out of range.
   */
  constructor(mantissa: bigint, scale = 0) {
    if (!Number.isInteger(scale) || scale < 0 || scale > MAX_SCALE) {
      throw new RangeError(`a decimal's scale is an integer from 0 to ${MAX_SCALE}, got ${scale}`);
    }
    if (mantissa < I128_MIN || mantissa > I128_MAX) {
      throw new RangeError("a decimal's mantissa must fit a signed 128-bit integer");
    }
    this.mantissa = mantissa;
    this.scale = scale;
  }

  /** Zero, with scale 0. */
  static readonly ZERO: Decimal = new Decimal(0n, 0);

  /**
   * Parses `[+-]digits[.digits]`; the scale is the number of digits after the point, kept
   * (`"1.50"` has scale 2).
   * @throws SyntaxError if the text is not a decimal, RangeError if it does not fit.
   */
  static parse(text: string): Decimal {
    const m = /^([+-]?)(\d*)(?:\.(\d*))?$/.exec(text);
    const whole = m?.[2] ?? "";
    const fraction = m?.[3];
    if (m === null || whole.length + (fraction?.length ?? 0) === 0) {
      throw new SyntaxError(`not a decimal: ${JSON.stringify(text)}`);
    }
    const digits = whole + (fraction ?? "");
    const magnitude = BigInt(digits);
    return new Decimal(m[1] === "-" ? -magnitude : magnitude, fraction?.length ?? 0);
  }

  /** The exact text, with every digit of the scale (`1.50`). */
  toString(): string {
    const negative = this.mantissa < 0n;
    let digits = (negative ? -this.mantissa : this.mantissa).toString();
    if (this.scale > 0) {
      digits = digits.padStart(this.scale + 1, "0");
      digits = `${digits.slice(0, digits.length - this.scale)}.${digits.slice(digits.length - this.scale)}`;
    }
    return negative ? `-${digits}` : digits;
  }

  /** The text, so `JSON.stringify` and template strings agree with {@link Decimal.toString}. */
  toJSON(): string {
    return this.toString();
  }

  /** Structural equality: the same mantissa and the same scale (`1.0` is not `1.00`). */
  equals(other: Decimal): boolean {
    return this.mantissa === other.mantissa && this.scale === other.scale;
  }

  /** Numeric comparison, ignoring scale: -1, 0 or 1. */
  compare(other: Decimal): -1 | 0 | 1 {
    const common = Math.max(this.scale, other.scale);
    const a = this.mantissa * 10n ** BigInt(common - this.scale);
    const b = other.mantissa * 10n ** BigInt(common - other.scale);
    return a < b ? -1 : a > b ? 1 : 0;
  }
}

/**
 * The codec of {@link Decimal}: 16 little-endian bytes of two's-complement mantissa, then the
 * scale as one byte (at most 38; a decoder rejects a larger one with `invalid_tag`).
 */
export const decimalCodec: Codec<Decimal> = {
  encode(w, v) {
    w.writeU64(BigInt.asUintN(64, v.mantissa));
    w.writeI64(BigInt.asIntN(64, v.mantissa >> 64n));
    w.writeU8(v.scale);
  },
  decode(r) {
    const low = r.readU64();
    const high = r.readI64();
    const at = r.position;
    const scale = r.readU8();
    if (scale > MAX_SCALE) throw new WireError({ code: "invalid_tag", tag: scale, at, ty: "decimal scale" });
    return new Decimal((high << 64n) | low, scale);
  },
};
