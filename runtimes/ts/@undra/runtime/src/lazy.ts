import { UndraTransportError, UndraModeError } from "./errors.js";
import type { UndraCore } from "./core.js";
import { Signal, batch } from "./signal.js";
import {
  CallTarget,
  type Codec,
  type Handle,
  type LazyPage,
  decodeLazyPage,
  readLazyInvalidated,
  readLazyValue,
  type UndraReader,
} from "./wire/index.js";

/*
 * `LazyList<T>`: the host side of a core `Lazy<T>` signal (ADR-043 decision 3.5, docs/SPEC.md sections 3.3, 3.5
 * and 17). One class, not generated code, linked only by a store that has a `Lazy<T>` signal, so an app without
 * one does not carry a byte of it. It keeps a small cache of decoded pages and asks the core for the rest.
 */

/** Rows per page unless {@link LazyList.pageSize} says otherwise. */
const DEFAULT_PAGE_SIZE = 50;
/** Pages kept unless {@link LazyList.maxCachedPages} says otherwise. */
const DEFAULT_MAX_CACHED_PAGES = 24;
/** A page whose reply is older than the list's version is asked for again this often before it counts as a protocol error. */
const MAX_STALE_REPLIES = 2;
/** The largest `pageSize`: the limit of a page call is a `u32`, and a page is decoded in one go. */
const MAX_PAGE_SIZE = 0xffff;
const NO_ARGS = new Uint8Array(0);

/** One cached page: its decoded rows and what the cache needs to know about it. */
interface CachedPage<T> {
  readonly rows: readonly T[];
  /** The list version the rows were read at; older than the list's own means stale (shown until replaced). */
  readonly version: bigint;
  /** The invalidation epoch in which a read last wanted the page; the pages of the current epoch are the window. */
  touched: number;
}

function asPositiveInt(name: string, value: number, max: number): number {
  if (!Number.isInteger(value) || value < 1 || value > max) {
    throw new RangeError(`${name} must be an integer between 1 and ${max}, not ${String(value)}`);
  }
  return value;
}

/**
 * A list the core owns and the host pages through: a window of rows, never the whole list.
 *
 * A generated store holds one per `Lazy<T>` signal, creates it with the core and the row codec, and hands it the
 * change-set entries of its signal (`FullValue` to {@link LazyList.applyFull}, `LazyInvalidated` to
 * {@link LazyList.applyInvalidated}):
 *
 * ```ts
 * class Library extends UndraStore {
 *   readonly books = new LazyList<Book>(this.core, BookCodec);
 *   // in _apply, for the signal's id:
 *   //   if (op === ChangeOp.FullValue) this.books.applyFull(new UndraReader(value));
 *   //   else if (op === ChangeOp.LazyInvalidated) this.books.applyInvalidated(new UndraReader(value));
 * }
 *
 * // a row, in any framework: `length` is how many there are, `get(i)` what to draw
 * const count = useSignal(library.books.length);
 * const book = library.books.get(i);   // Book | undefined while its page loads
 * ```
 *
 * **Reading.** `length` and `revision` are signals. `get(index)` returns the cached row, or `undefined` while its page
 * loads, and in that case (and when the page is stale) it **requests** the page, plus the pages before and after it
 * if they are not cached, once: it never blocks and never calls into the core itself, so it can be called while
 * rendering. Requests made in one turn of the event loop go out together, from a microtask, one page call each; a
 * page already requested is not requested again. Where the transport can answer in process (`wasm-main`) the calls
 * are synchronous and the rows are there when the microtask has run, else the replies arrive later. Either way
 * `revision` changes when rows arrive, so a view that read `undefined` reads again.
 *
 * **Changing.** When the core changes the list it sends the new length and version; `length` follows at once and the
 * rows already cached stay visible, stale, while the **window** (the pages a read has wanted since the previous
 * change, at most {@link LazyList.maxCachedPages}) is fetched again: the cost of a change is the size of the window,
 * never of the list. A page the core answers at an older version than the list's is dropped and asked for again; one
 * at a newer version raises the list's version (the change that follows then says nothing new). When the core
 * restarts the list under a new handle the cache is dropped.
 *
 * **Failing.** A reply that cannot be decoded, a page call the core refuses, a transport that is down: each is
 * reported through the core's error channel (`UndraCore.report`, so `onError`), never thrown into the reader. The
 * page stays absent (`get` returns `undefined`) and is not requested again until the list changes or `prefetch`
 * asks for it.
 */
export class LazyList<T> {
  /** The number of rows, as of the last value or change the core sent. */
  readonly length: Signal<number> = new Signal<number>(0);
  /**
   * Changes whenever rows arrive, are replaced or are dropped: a view that reads rows with {@link LazyList.get}
   * reads them again on it (`length` alone does not change when a page arrives or a row is edited in place).
   */
  readonly revision: Signal<number> = new Signal<number>(0);

  private readonly _core: UndraCore;
  private readonly _codec: Codec<T>;
  private _handle: Handle | null = null;
  private _len = 0;
  private _version = 0n;
  private _pageSize = DEFAULT_PAGE_SIZE;
  private _maxPages = DEFAULT_MAX_CACHED_PAGES;
  /** Cached pages by index, least recently touched first. */
  private readonly _pages = new Map<number, CachedPage<T>>();
  /** Pages to ask for in the next microtask. */
  private readonly _queued = new Set<number>();
  /** Pages asked for and not yet answered. */
  private readonly _inflight = new Set<number>();
  /** Pages a read asked for (as opposed to prefetched) that have not arrived: they join the window when they do. */
  private readonly _wanted = new Set<number>();
  /** Pages whose last request failed: not requested again until the list changes. */
  private readonly _failed = new Set<number>();
  /** How often each page's reply was older than the list. */
  private readonly _stale = new Map<number, number>();
  private _scheduled = false;
  /** Bumped when the page server changes or the page size does: replies to older requests are dropped. */
  private _generation = 0;
  /** Bumped with every change of the list. */
  private _epoch = 0;
  /** The page `get` touched last, so that reading the rows of one page does not reorder the cache for each row. */
  private _last = -1;
  /** Whether the transport answers `callSync` (`undefined` until the first request finds out). */
  private _sync: boolean | undefined = undefined;

  /**
   * @param core The core the list's page server lives in.
   * @param codec The codec of one row.
   */
  constructor(core: UndraCore, codec: Codec<T>) {
    this._core = core;
    this._codec = codec;
  }

  /**
   * Rows per page request (default 50). A read also fetches the page before and after the one it needs.
   * Changing it drops the cache, since the pages' boundaries move.
   */
  get pageSize(): number {
    return this._pageSize;
  }

  set pageSize(rows: number) {
    asPositiveInt("pageSize", rows, MAX_PAGE_SIZE);
    if (rows === this._pageSize) return;
    this._pageSize = rows;
    batch(() => {
      this._reset();
    });
  }

  /**
   * How many pages are kept (default 24, which is 1,200 rows at the default page size). The pages of the window are
   * dropped last, the least recently read first; a window larger than this is trimmed to it.
   */
  get maxCachedPages(): number {
    return this._maxPages;
  }

  set maxCachedPages(pages: number) {
    this._maxPages = asPositiveInt("maxCachedPages", pages, Number.MAX_SAFE_INTEGER);
    batch(() => {
      if (this._evict()) this._bump();
    });
  }

  /**
   * The row at `index`, or `undefined` while its page loads or when `index` is not a row (negative, fractional,
   * or at or past `length`; those request nothing). Reading a row whose page is not cached or is stale requests it,
   * and the pages around it if they are not cached; see the class documentation.
   */
  get(index: number): T | undefined {
    if (!(index >= 0 && index < this._len) || !Number.isInteger(index)) return undefined;
    const size = this._pageSize;
    const at = Math.floor(index / size);
    const page = this._pages.get(at);
    if (page === undefined) {
      this._wanted.add(at);
      this._want(at);
    } else {
      page.touched = this._epoch;
      if (at !== this._last) {
        // Most recently touched last: eviction takes from the front.
        this._pages.delete(at);
        this._pages.set(at, page);
      }
      if (page.version !== this._version) this._want(at);
    }
    this._last = at;
    if (at > 0 && !this._pages.has(at - 1)) this._want(at - 1);
    if (!this._pages.has(at + 1) && (at + 1) * size < this._len) this._want(at + 1);
    return page?.rows[index - at * size];
  }

  /**
   * Requests the pages that hold rows `start` up to (not including) `end`, as a read of each would (at most
   * {@link LazyList.maxCachedPages} of them). A page that failed is asked for again. Rows outside `0..length`
   * are ignored.
   */
  prefetch(start: number, end: number): void {
    const from = Math.max(0, Math.floor(start));
    const to = Math.min(this._len, Math.ceil(end));
    if (!(from < to)) return;
    const size = this._pageSize;
    const first = Math.floor(from / size);
    const last = Math.min(Math.floor((to - 1) / size), first + this._maxPages - 1);
    for (let at = first; at <= last; at++) {
      this._failed.delete(at);
      const page = this._pages.get(at);
      if (page === undefined) this._wanted.add(at);
      else page.touched = this._epoch;
      if (page === undefined || page.version !== this._version) this._want(at);
    }
  }

  /**
   * Applies the value of the signal (change-set op `FullValue`): the page server's handle, the length and the
   * version. A handle other than the one the list has (the first value, or the core restarted the list) drops
   * the cache; the same handle with a newer version is a change; the same value again retries the pages whose
   * request failed. Reads `reader` to its end.
   *
   * @throws {WireError} If the value is malformed; the list is unchanged.
   */
  applyFull(reader: UndraReader): void {
    const value = readLazyValue(reader);
    reader.finish();
    batch(() => {
      if (value.handle !== this._handle) {
        this._handle = value.handle;
        this._len = value.len;
        this._version = value.version;
        this.length._set(value.len);
        this._reset();
      } else if (!this._advance(value.len, value.version, -1)) {
        const retry = [...this._failed];
        this._failed.clear();
        for (const at of retry) this._want(at);
      }
    });
  }

  /**
   * Applies a `LazyInvalidated` entry (change-set op 2): the new length and version. A version the list has already
   * seen (a page reply that carried it) changes nothing. Reads `reader` to its end.
   *
   * @throws {WireError} If the value is malformed; the list is unchanged.
   */
  applyInvalidated(reader: UndraReader): void {
    const value = readLazyInvalidated(reader);
    reader.finish();
    if (this._handle === null) return;
    batch(() => {
      this._advance(value.len, value.version, -1);
    });
  }

  // ----- the list changed ------------------------------------------------------------

  /** Drops everything cached and everything in flight; the length and version stay. */
  private _reset(): void {
    this._generation++;
    this._epoch++;
    const had = this._pages.size > 0;
    this._pages.clear();
    this._queued.clear();
    this._inflight.clear();
    this._wanted.clear();
    this._failed.clear();
    this._stale.clear();
    this._last = -1;
    if (had) this._bump();
  }

  /**
   * Takes a newer length and version. The pages of the window (those a read wanted since the previous change) are
   * asked for again, except `skip` (the page that carried the news); stale rows stay visible meanwhile. Pages past
   * the new end are dropped. Returns whether `version` was newer.
   */
  private _advance(len: number, version: bigint, skip: number): boolean {
    if (version <= this._version) return false;
    this._version = version;
    this._len = len;
    this.length._set(len);
    const window = this._epoch++;
    this._failed.clear();
    this._stale.clear();
    const last = len === 0 ? -1 : Math.floor((len - 1) / this._pageSize);
    let dropped = false;
    for (const [at, page] of this._pages) {
      if (at > last) {
        this._pages.delete(at);
        dropped = true;
      } else if (page.touched === window && at !== skip) {
        this._want(at);
      }
    }
    for (const at of this._queued) if (at > last) this._queued.delete(at);
    if (dropped) this._bump();
    return true;
  }

  // ----- asking ----------------------------------------------------------------------

  /** Queues a request for page `at` unless it is queued, in flight or failed; the next microtask sends the queue. */
  private _want(at: number): void {
    if (this._queued.has(at) || this._inflight.has(at) || this._failed.has(at)) return;
    this._queued.add(at);
    if (!this._scheduled) {
      this._scheduled = true;
      queueMicrotask(() => {
        this._flush();
      });
    }
  }

  /** Sends the queued requests, lowest page first, in one batch. */
  private _flush(): void {
    this._scheduled = false;
    const handle = this._handle;
    if (handle === null || this._core.closed || this._queued.size === 0) {
      this._queued.clear();
      return;
    }
    const pages = [...this._queued].sort((a, b) => a - b);
    this._queued.clear();
    const generation = this._generation;
    batch(() => {
      for (const at of pages) {
        // A request made earlier in this loop may have moved the list (a synchronous call applies the core's change-sets).
        if (generation !== this._generation || this._handle !== handle) return;
        if (at * this._pageSize >= this._len) continue;
        if (this._inflight.has(at)) continue;
        this._request(handle, at);
      }
    });
  }

  /** One page call: synchronous where the transport answers in process, else asynchronous. */
  private _request(handle: Handle, at: number): void {
    const size = this._pageSize;
    const generation = this._generation;
    const target = { target: CallTarget.LazyListPage, handle, offset: at * size, limit: size } as const;
    this._inflight.add(at);
    if (this._sync !== false) {
      let body: Uint8Array | undefined;
      try {
        body = this._core.callSync(target, 0, NO_ARGS);
      } catch (error) {
        if (!(error instanceof UndraModeError)) {
          this._sync = true;
          this._failedPage(generation, at, error);
          return;
        }
        this._sync = false; // the transport cannot answer in process: ask asynchronously, now and from here on
      }
      if (body !== undefined) {
        this._sync = true;
        this._arrived(generation, at, size, body);
        return;
      }
    }
    this._core.call(target, 0, NO_ARGS).then(
      (body) => {
        batch(() => {
          this._arrived(generation, at, size, body);
        });
      },
      (error: unknown) => {
        this._failedPage(generation, at, error);
      },
    );
  }

  // ----- the answer ------------------------------------------------------------------

  private _failedPage(generation: number, at: number, error: unknown): void {
    if (generation !== this._generation) return;
    this._inflight.delete(at);
    this._failed.add(at);
    this._report(error);
  }

  /** A page reply: checked, then installed, dropped (stale) or the reason for a newer version. Never throws. */
  private _arrived(generation: number, at: number, size: number, body: Uint8Array): void {
    if (generation !== this._generation) return;
    this._inflight.delete(at);
    let page: LazyPage<T>;
    try {
      page = decodeLazyPage(this._codec, body, size);
      const expected = Math.min(size, Math.max(0, page.total - at * size));
      if (page.items.length !== expected) {
        throw new UndraTransportError(
          "protocol",
          `the core sent ${page.items.length} rows for page ${at} of a list of ${page.total}, which holds ${expected}`,
        );
      }
      if (page.version === this._version && page.total !== this._len) {
        throw new UndraTransportError(
          "protocol",
          `the core sent a list length of ${page.total} at version ${page.version}, which it told us had ${this._len}`,
        );
      }
    } catch (error) {
      this._failed.add(at);
      this._report(error);
      return;
    }
    if (page.version < this._version) {
      // Read before a change we already know of: ask again, a bounded number of times.
      const times = (this._stale.get(at) ?? 0) + 1;
      if (times > MAX_STALE_REPLIES) {
        this._stale.delete(at);
        this._failed.add(at);
        this._report(
          new UndraTransportError("protocol", `the core keeps answering page ${at} at version ${page.version}, older than the list's ${this._version}`),
        );
        return;
      }
      this._stale.set(at, times);
      this._want(at);
      return;
    }
    this._stale.delete(at);
    if (page.version > this._version) this._advance(page.total, page.version, at);
    if (at * size >= this._len) return; // the list shrank below this page
    this._install(at, page);
  }

  private _install(at: number, page: LazyPage<T>): void {
    const previous = this._pages.get(at);
    const touched = this._wanted.delete(at) ? this._epoch : (previous?.touched ?? -1);
    this._pages.delete(at);
    this._pages.set(at, { rows: page.items, version: page.version, touched });
    this._evict();
    this._bump();
  }

  /** Drops pages beyond `maxCachedPages`: the least recently touched first, those outside the window before those inside. */
  private _evict(): boolean {
    let excess = this._pages.size - this._maxPages;
    if (excess <= 0) return false;
    for (const [at, page] of this._pages) {
      if (excess === 0) break;
      if (page.touched !== this._epoch) {
        this._pages.delete(at);
        excess--;
      }
    }
    for (const at of this._pages.keys()) {
      if (excess === 0) break;
      this._pages.delete(at);
      excess--;
    }
    return true;
  }

  private _bump(): void {
    this.revision._set(this.revision.peek() + 1);
  }

  private _report(error: unknown): void {
    try {
      this._core.report(error, "LazyList.page");
    } catch {
      // `report` hands the error to the app's handler and logs it; nothing a reader of a list can do about the rest.
    }
  }
}
