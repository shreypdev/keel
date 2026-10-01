import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { type Codec, UndraReader, UndraWriter, codecs } from "@undra/runtime";

/**
 * The recording S19 step 9 replays: 60,000 seeded operations over three derived views, checked
 * against `filter + stable sort` in Rust (`crates/undra-signals/tests/support/seeded_views.rs`), as
 * the change-sets a host received plus the FNV-1a 64 hash of each view after each of them. Written by
 * `cargo run -p undra-signals --example derived_vectors -- <path>` (run.sh does);
 * `UNDRA_DERIVED_VECTORS` overrides the path.
 */
export const DERIVED_VECTORS: string =
  process.env["UNDRA_DERIVED_VECTORS"] ??
  fileURLToPath(new URL("../../../examples/playground/build/derived-vectors.bin", import.meta.url));

/** The store handle the recording's change-sets carry. */
export const VECTORS_HANDLE = 0x0000_0001_0000_0019n;

/** One change-set of the recording and each view's hash after it. */
export interface VectorRecord {
  readonly payload: Uint8Array;
  readonly hashes: readonly bigint[];
}

export interface Row {
  readonly id: number;
  readonly title: string;
  readonly done: boolean;
  readonly rank: number;
}

export interface Label {
  readonly id: number;
  readonly text: string;
}

export const RowCodec: Codec<Row> = {
  encode(w, v) {
    w.writeU32(v.id);
    w.writeStr(v.title);
    w.writeBool(v.done);
    w.writeU32(v.rank);
  },
  decode(r) {
    return { id: r.readU32(), title: r.readStr(), done: r.readBool(), rank: r.readU32() };
  },
};

export const LabelCodec: Codec<Label> = {
  encode(w, v) {
    w.writeU32(v.id);
    w.writeStr(v.text);
  },
  decode(r) {
    return { id: r.readU32(), text: r.readStr() };
  },
};

/** The views' row codecs, by signal id: `open`, `ranked` (rows), `labels`. */
export const VIEW_CODECS: readonly Codec<unknown>[] = [RowCodec, RowCodec, LabelCodec] as Codec<unknown>[];

/** Reads the `UDV1` recording. */
export function readVectors(path: string = DERIVED_VECTORS): VectorRecord[] {
  let bytes: Uint8Array;
  try {
    bytes = readFileSync(path);
  } catch (cause) {
    throw new Error(
      `cannot read the derived-list recording at ${path}; write it with \`cargo run -p undra-signals --example derived_vectors -- ${path}\` (contract-tests/*/run.sh does)`,
      { cause },
    );
  }
  const r = new UndraReader(bytes);
  const magic = String.fromCharCode(r.readU8(), r.readU8(), r.readU8(), r.readU8());
  if (magic !== "UDV1") throw new Error(`not a UDV1 recording: ${magic}`);
  const views = r.readU32();
  const count = r.readU32();
  const records: VectorRecord[] = [];
  for (let i = 0; i < count; i++) {
    const payload = r.readRaw(r.readU32());
    const hashes: bigint[] = [];
    for (let v = 0; v < views; v++) hashes.push(r.readU64());
    records.push({ payload, hashes });
  }
  r.finish();
  return records;
}

/** FNV-1a 64 of `bytes`, on two 32-bit halves (the arithmetic of the runtime's `fnv1a64`). */
export function fnv1a64Bytes(bytes: Uint8Array): bigint {
  let hi = 0xcbf29ce4;
  let lo = 0x84222325;
  for (let i = 0; i < bytes.length; i++) {
    lo = (lo ^ (bytes[i] as number)) >>> 0;
    const low = lo * 0x1b3;
    const carry = Math.floor(low / 0x1_0000_0000);
    const shifted = (lo << 8) >>> 0;
    hi = (hi * 0x1b3 + carry + shifted) >>> 0;
    lo = low >>> 0;
  }
  return (BigInt(hi) << 32n) | BigInt(lo);
}

/** The hash of a view as the recording computes it: of its encoding as a `Vec`. */
export function hashOf(list: readonly unknown[], codec: Codec<unknown>): bigint {
  const w = new UndraWriter();
  codecs.vec(codec).encode(w, list as unknown[]);
  return fnv1a64Bytes(w.finish());
}

/** SplitMix64 drain points: deterministic, so a failure reproduces. */
export function splitmix(seed: bigint): () => number {
  let state = seed;
  return () => {
    state = (state + 0x9e3779b97f4a7c15n) & 0xffffffffffffffffn;
    let z = state;
    z = ((z ^ (z >> 30n)) * 0xbf58476d1ce4e5b9n) & 0xffffffffffffffffn;
    z = ((z ^ (z >> 27n)) * 0x94d049bb133111ebn) & 0xffffffffffffffffn;
    return Number((z ^ (z >> 31n)) & 0xffffn);
  };
}
