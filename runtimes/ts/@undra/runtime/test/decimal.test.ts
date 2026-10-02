import { describe, expect, it } from "vitest";
import { Decimal, decimalCodec } from "../src/wire/decimal.js";
import { decodeValue, encodeValue } from "../src/wire/codec.js";
import { WireError } from "../src/wire/errors.js";
import { toHex } from "./helpers.js";

describe("Decimal (ADR-042)", () => {
  it("parses and prints exactly, keeping the scale", () => {
    for (const [text, mantissa, scale] of [
      ["0", 0n, 0],
      ["19.99", 1999n, 2],
      ["-1.50", -150n, 2],
      ["0.05", 5n, 2],
      ["-0.001", -1n, 3],
      ["100", 100n, 0],
    ] as const) {
      const d = Decimal.parse(text);
      expect([d.mantissa, d.scale]).toEqual([mantissa, scale]);
      expect(d.toString()).toBe(text);
      expect(JSON.stringify({ d })).toBe(JSON.stringify({ d: text }));
    }
    expect(Decimal.parse("+7.0").toString()).toBe("7.0");
    expect(Decimal.parse(".5").toString()).toBe("0.5");
  });

  it("rejects what it cannot hold", () => {
    for (const bad of ["", "-", ".", "1,5", "1e5", "1.2.3", " 1"]) expect(() => Decimal.parse(bad)).toThrow(SyntaxError);
    expect(() => new Decimal(0n, 39)).toThrow(RangeError);
    expect(() => new Decimal(0n, -1)).toThrow(RangeError);
    expect(() => new Decimal(0n, 1.5)).toThrow(RangeError);
    expect(() => new Decimal(1n << 127n)).toThrow(RangeError);
    expect(() => new Decimal(-(1n << 127n) - 1n)).toThrow(RangeError);
    expect(() => Decimal.parse(`0.${"1".repeat(39)}`)).toThrow(RangeError);
    expect(new Decimal(-(1n << 127n)).toString()).toBe("-170141183460469231731687303715884105728");
  });

  it("is equal structurally and ordered numerically", () => {
    const a = Decimal.parse("1.0");
    const b = Decimal.parse("1.00");
    expect(a.equals(b)).toBe(false);
    expect(a.compare(b)).toBe(0);
    expect(Decimal.parse("-2").compare(Decimal.parse("-1.5"))).toBe(-1);
    expect(Decimal.parse("0.5").compare(Decimal.parse("0.49"))).toBe(1);
    expect(Decimal.ZERO.compare(Decimal.parse("0.000"))).toBe(0);
    const big = new Decimal((1n << 127n) - 1n, 38);
    expect(big.compare(new Decimal((1n << 127n) - 1n, 0))).toBe(-1);
  });

  it("encodes as 16 little-endian bytes of mantissa and a scale byte", () => {
    expect(toHex(encodeValue(decimalCodec, Decimal.parse("19.99")))).toBe("cf07000000000000000000000000000002");
    expect(toHex(encodeValue(decimalCodec, Decimal.parse("-1.50")))).toBe("6affffffffffffffffffffffffffffff02");
  });

  it("round-trips a spread of values and rejects a scale above 38", () => {
    let seed = 12345n;
    const next = (): bigint => (seed = (seed * 6364136223846793005n + 1442695040888963407n) & ((1n << 64n) - 1n));
    for (let i = 0; i < 500; i++) {
      const raw = (next() << 64n) | next();
      const mantissa = BigInt.asIntN(128, raw);
      const d = new Decimal(mantissa, Number(next() % 39n));
      const back = decodeValue(decimalCodec, encodeValue(decimalCodec, d));
      expect(back.equals(d)).toBe(true);
      expect(Decimal.parse(d.toString()).equals(d)).toBe(true);
    }
    const bytes = encodeValue(decimalCodec, Decimal.ZERO);
    bytes[16] = 39;
    expect(() => decodeValue(decimalCodec, bytes)).toThrow(WireError);
    expect(() => decodeValue(decimalCodec, bytes.slice(0, 16))).toThrow(WireError);
  });
});
