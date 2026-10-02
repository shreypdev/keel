// PROTOTYPE (ADR-057 lever d2): one export per codec; `codecs` is this module's namespace.
import type { Codec } from "./codec.js";

import {


  type Handle,

  type Uuid,


} from "./types.js";

export const bool: Codec<boolean> = {
  encode: (w, v) => w.writeBool(v),
  decode: (r) => r.readBool(),
};

export const u8: Codec<number> = { encode: (w, v) => w.writeU8(v), decode: (r) => r.readU8() };
export const u16: Codec<number> = { encode: (w, v) => w.writeU16(v), decode: (r) => r.readU16() };
export const u32: Codec<number> = { encode: (w, v) => w.writeU32(v), decode: (r) => r.readU32() };
export const u64: Codec<bigint> = { encode: (w, v) => w.writeU64(v), decode: (r) => r.readU64() };

/** `Unit`: zero bytes. */
export const unit: Codec<void> = {
  encode: () => {},
  decode: () => undefined,
};

export const string: Codec<string> = {
  encode: (w, v) => w.writeStr(v),
  decode: (r) => r.readStr(),
};

export const uuid: Codec<Uuid> = {
  encode: (w, v) => w.writeUuid(v),
  decode: (r) => r.readUuid(),
};

/** An object handle (`u64`). */
export const handle: Codec<Handle> = u64;

/**
 * `Vec<T>` as `T[]`. The decoder preallocates the array and refuses counts
 * larger than the remaining input, so a hostile count cannot trigger a large
 * allocation. Elements that encode to zero bytes (`Vec<()>`) are not
 * supported for counts beyond the remaining input; see `UndraReader.readLen`.
 */
export function vec<T>(item: Codec<T>): Codec<T[]> {
  return {
    encode(w, v) {
      w.writeLen(v.length);
      for (let i = 0; i < v.length; i++) item.encode(w, v[i] as T);
    },
    decode(r) {
      const n = r.readLen();
      const out = new Array<T>(n);
      for (let i = 0; i < n; i++) out[i] = item.decode(r);
      return out;
    },
  };
}
