/*
 * What `@undra/runtime` needs from the platform that Hermes lacks: `TextDecoder` (UTF-8, with
 * `fatal` and `ignoreBOM`) and `TextEncoder`. `./polyfills.ts` installs them on `globalThis` when
 * they are missing. No dependency, no effect where they exist.
 */

type Source = ArrayBuffer | ArrayBufferView;

function bytesOf(input: Source | undefined): Uint8Array {
  if (input === undefined) return new Uint8Array(0);
  if (input instanceof Uint8Array) return input;
  if (ArrayBuffer.isView(input)) return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
  return new Uint8Array(input);
}

const CHUNK = 8192;

/** A UTF-8 `TextDecoder` (WHATWG semantics for UTF-8: `fatal`, `ignoreBOM`, U+FFFD replacement). */
export class Utf8TextDecoder {
  readonly encoding = "utf-8";
  readonly fatal: boolean;
  readonly ignoreBOM: boolean;

  constructor(label = "utf-8", options: { fatal?: boolean; ignoreBOM?: boolean } = {}) {
    const name = label.trim().toLowerCase();
    if (name !== "utf-8" && name !== "utf8" && name !== "unicode-1-1-utf-8") {
      throw new RangeError(`The encoding "${label}" is not supported by this TextDecoder`);
    }
    this.fatal = options.fatal === true;
    this.ignoreBOM = options.ignoreBOM === true;
  }

  decode(input?: Source): string {
    const bytes = bytesOf(input);
    const n = bytes.length;
    let i = 0;
    if (!this.ignoreBOM && n >= 3 && bytes[0] === 0xef && bytes[1] === 0xbb && bytes[2] === 0xbf) i = 3;
    let out = "";
    const units: number[] = [];
    const flush = (): void => {
      out += String.fromCharCode.apply(null, units);
      units.length = 0;
    };
    const bad = (): void => {
      if (this.fatal) throw new TypeError("The encoded data was not valid UTF-8");
      units.push(0xfffd);
    };
    while (i < n) {
      const b0 = bytes[i] as number;
      if (b0 < 0x80) {
        units.push(b0);
        i += 1;
      } else {
        let need = 0;
        let cp = 0;
        let lower = 0x80;
        let upper = 0xbf;
        if (b0 >= 0xc2 && b0 <= 0xdf) {
          need = 1;
          cp = b0 & 0x1f;
        } else if (b0 >= 0xe0 && b0 <= 0xef) {
          need = 2;
          cp = b0 & 0x0f;
          if (b0 === 0xe0) lower = 0xa0;
          if (b0 === 0xed) upper = 0x9f;
        } else if (b0 >= 0xf0 && b0 <= 0xf4) {
          need = 3;
          cp = b0 & 0x07;
          if (b0 === 0xf0) lower = 0x90;
          if (b0 === 0xf4) upper = 0x8f;
        } else {
          bad();
          i += 1;
          continue;
        }
        let j = 1;
        for (; j <= need; j++) {
          const b = bytes[i + j];
          if (b === undefined || b < lower || b > upper) break;
          cp = (cp << 6) | (b & 0x3f);
          lower = 0x80;
          upper = 0xbf;
        }
        if (j <= need) {
          // A maximal subpart of an ill-formed sequence becomes one U+FFFD.
          bad();
          i += j;
          continue;
        }
        i += need + 1;
        if (cp > 0xffff) {
          cp -= 0x10000;
          units.push(0xd800 | (cp >> 10), 0xdc00 | (cp & 0x3ff));
        } else {
          units.push(cp);
        }
      }
      if (units.length >= CHUNK) flush();
    }
    flush();
    return out;
  }
}

/** A `TextEncoder` (UTF-8; lone surrogates become U+FFFD). */
export class Utf8TextEncoder {
  readonly encoding = "utf-8";

  encode(input = ""): Uint8Array {
    const out = new Uint8Array(input.length * 3);
    const { written } = this.encodeInto(input, out);
    return out.slice(0, written);
  }

  encodeInto(input: string, dest: Uint8Array): { read: number; written: number } {
    let read = 0;
    let written = 0;
    const n = input.length;
    while (read < n) {
      let cp = input.charCodeAt(read);
      let units = 1;
      if (cp >= 0xd800 && cp <= 0xdbff) {
        const next = read + 1 < n ? input.charCodeAt(read + 1) : 0;
        if (next >= 0xdc00 && next <= 0xdfff) {
          cp = 0x10000 + ((cp - 0xd800) << 10) + (next - 0xdc00);
          units = 2;
        } else {
          cp = 0xfffd;
        }
      } else if (cp >= 0xdc00 && cp <= 0xdfff) {
        cp = 0xfffd;
      }
      const size = cp < 0x80 ? 1 : cp < 0x800 ? 2 : cp < 0x10000 ? 3 : 4;
      if (written + size > dest.length) break;
      if (size === 1) {
        dest[written++] = cp;
      } else if (size === 2) {
        dest[written++] = 0xc0 | (cp >> 6);
        dest[written++] = 0x80 | (cp & 0x3f);
      } else if (size === 3) {
        dest[written++] = 0xe0 | (cp >> 12);
        dest[written++] = 0x80 | ((cp >> 6) & 0x3f);
        dest[written++] = 0x80 | (cp & 0x3f);
      } else {
        dest[written++] = 0xf0 | (cp >> 18);
        dest[written++] = 0x80 | ((cp >> 12) & 0x3f);
        dest[written++] = 0x80 | ((cp >> 6) & 0x3f);
        dest[written++] = 0x80 | (cp & 0x3f);
      }
      read += units;
    }
    return { read, written };
  }
}

/** Installs the polyfills that are missing; returns which ones it installed. */
export function installPolyfills(target: Record<string, unknown> = globalThis as unknown as Record<string, unknown>): string[] {
  const installed: string[] = [];
  if (typeof target["TextDecoder"] !== "function") {
    target["TextDecoder"] = Utf8TextDecoder;
    installed.push("TextDecoder");
  }
  if (typeof target["TextEncoder"] !== "function") {
    target["TextEncoder"] = Utf8TextEncoder;
    installed.push("TextEncoder");
  }
  return installed;
}

