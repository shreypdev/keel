const DIGITS = "0123456789abcdef";

/** `bytes` as lower-case hex, the encoding of every payload in a recording. */
export function toHex(bytes: Uint8Array): string {
  let out = "";
  for (const byte of bytes) out += DIGITS[byte >> 4]! + DIGITS[byte & 15]!;
  return out;
}

/** Hex (either case) back to bytes; `null` for an odd length or a character that is not a hex digit. */
export function fromHex(text: string): Uint8Array | null {
  if (text.length % 2 !== 0) return null;
  const out = new Uint8Array(text.length / 2);
  for (let i = 0; i < out.length; i++) {
    const high = Number.parseInt(text[2 * i]!, 16);
    const low = Number.parseInt(text[2 * i + 1]!, 16);
    if (Number.isNaN(high) || Number.isNaN(low)) return null;
    out[i] = high * 16 + low;
  }
  return out;
}

/** The UTF-8 bytes of `text`. */
export function utf8(text: string): Uint8Array {
  return new TextEncoder().encode(text);
}

/**
 * Orders strings by their UTF-8 bytes (which is code-point order), as the Rust fakes and the platform
 * adapters do. JavaScript's `<` compares UTF-16 code units, which disagrees for characters outside the
 * Basic Multilingual Plane.
 */
export function compareUtf8(a: string, b: string): number {
  const x = [...a];
  const y = [...b];
  for (let i = 0; i < Math.min(x.length, y.length); i++) {
    const p = x[i]!.codePointAt(0)!;
    const q = y[i]!.codePointAt(0)!;
    if (p !== q) return p < q ? -1 : 1;
  }
  return x.length - y.length;
}
