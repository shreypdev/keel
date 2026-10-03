import type { Codec } from "./codec.js";
import { WireError } from "./errors.js";
import { decodeAll } from "./payloads.js";
import { UndraReader } from "./reader.js";
import type { Handle } from "./types.js";
import { UndraWriter } from "./writer.js";

/*
 * The lazy list payloads (SPEC 3.5 and 10.3c, ADR-043): the `LazyValue` and `LazyInvalidated` change-set entries and the
 * page a page call answers with. `LazyList` (`lazy.ts`) reads them; an app whose schema has no lazy list carries none of it.
 */


// ---------------------------------------------------------------------------
// Lazy lists (ADR-043): the value of a `Lazy<T>` signal, change-set op 2, the page reply
// ---------------------------------------------------------------------------

/** Size of a {@link LazyValue}: handle 8, len 4, version 8. */
const LAZY_VALUE_LEN = 20;
/** Size of a {@link LazyInvalidated}: len 4, version 8. */
const LAZY_INVALIDATED_LEN = 12;
/** Size of the header of a lazy page reply: version 8, total 4, count 4. */
const LAZY_PAGE_HEADER_LEN = 16;

/** The value of a `Lazy<T>` signal (change-set op 0, `FullValue`): `handle u64, len u32, version u64`. */
export interface LazyValue {
  /** The page server: the object a page call (`CallTarget.LazyListPage`) is addressed to. */
  readonly handle: Handle;
  /** The number of rows. */
  readonly len: number;
  /** The version of the list `len` was read at; it increases with every change. */
  readonly version: bigint;
}

/** The value of change-set op 2 (`LazyInvalidated`): `len u32, version u64`. */
export interface LazyInvalidated {
  /** The new number of rows. */
  readonly len: number;
  /** The new version. */
  readonly version: bigint;
}

/** The header of a page reply: `version u64, total u32, count u32`; `count` rows follow, each encoded as the item type. */
export interface LazyPageHeader {
  /** The version of the list the page was read at. */
  readonly version: bigint;
  /** The number of rows the list had at that version. */
  readonly total: number;
  /** How many rows follow the header. */
  readonly count: number;
}

/** One page of a lazy list: its header with the decoded rows. */
export interface LazyPage<T> {
  /** The version of the list the page was read at. */
  readonly version: bigint;
  /** The number of rows the list had at that version. */
  readonly total: number;
  /** The rows of the page. */
  readonly items: readonly T[];
}

/** Reads a {@link LazyValue} from `r` without requiring the reader to be exhausted. */
export function readLazyValue(r: UndraReader): LazyValue {
  const handle = r.readU64();
  const len = r.readU32();
  return { handle, len, version: r.readU64() };
}

/** Reads a {@link LazyInvalidated} from `r` without requiring the reader to be exhausted. */
export function readLazyInvalidated(r: UndraReader): LazyInvalidated {
  const len = r.readU32();
  return { len, version: r.readU64() };
}

/** Reads the 16-byte header of a page reply from `r`; the rows follow in `r`. */
export function readLazyPageHeader(r: UndraReader): LazyPageHeader {
  const version = r.readU64();
  const total = r.readU32();
  return { version, total, count: r.readU32() };
}

/** Encodes a {@link LazyValue}. */
export function encodeLazyValue(value: LazyValue): Uint8Array {
  const w = new UndraWriter(LAZY_VALUE_LEN);
  w.writeU64(value.handle);
  w.writeU32(value.len);
  w.writeU64(value.version);
  return w.finish();
}

/** Decodes a whole {@link LazyValue} payload. */
export function decodeLazyValue(bytes: Uint8Array): LazyValue {
  return decodeAll(bytes, readLazyValue);
}

/** Encodes a {@link LazyInvalidated}. */
export function encodeLazyInvalidated(value: LazyInvalidated): Uint8Array {
  const w = new UndraWriter(LAZY_INVALIDATED_LEN);
  w.writeU32(value.len);
  w.writeU64(value.version);
  return w.finish();
}

/** Decodes a whole {@link LazyInvalidated} payload. */
export function decodeLazyInvalidated(bytes: Uint8Array): LazyInvalidated {
  return decodeAll(bytes, readLazyInvalidated);
}

/** Encodes a page reply: the header (with `count` taken from `page.items`) and the rows, each with `item`. */
export function encodeLazyPage<T>(item: Codec<T>, page: LazyPage<T>): Uint8Array {
  const w = new UndraWriter(LAZY_PAGE_HEADER_LEN);
  w.writeU64(page.version);
  w.writeU32(page.total);
  w.writeLen(page.items.length);
  for (const row of page.items) item.encode(w, row);
  return w.finish();
}

/**
 * Decodes a whole page reply, rows with `item`. `maxCount` (default: whatever the input can hold, at one byte per
 * row) bounds the row count the header may claim: a hostile count never makes the decoder allocate or loop beyond it.
 *
 * @throws {WireError} For a truncated or oversized reply, a count above `maxCount`, trailing bytes, or a row `item` rejects.
 */
export function decodeLazyPage<T>(item: Codec<T>, bytes: Uint8Array, maxCount: number = 0xffff_ffff): LazyPage<T> {
  const r = new UndraReader(bytes);
  const at = r.position + LAZY_PAGE_HEADER_LEN - 4;
  const head = readLazyPageHeader(r);
  if (head.count > maxCount || head.count > r.remaining) throw new WireError({ code: "length_too_large", len: head.count, at });
  const items = new Array<T>(head.count);
  for (let i = 0; i < head.count; i++) items[i] = item.decode(r);
  r.finish();
  return { version: head.version, total: head.total, items };
}
