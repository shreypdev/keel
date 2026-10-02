import type { UndraCore } from "./core.js";
import { Signal } from "./signal.js";
import type { Codec, UndraReader } from "./wire/index.js";

/*
 * `LazyList<T>`: the host side of a core `Lazy<T>` signal (ADR-043 decision 3.5, docs/SPEC.md
 * section 17). API skeleton: the behaviour lands in the next commits.
 */

/**
 * A list the core owns and the host pages through: a window of rows, never the whole list.
 *
 * A generated store holds one per `Lazy<T>` signal, creates it with the core and the item codec, and
 * hands it the change-set entries of its signal:
 *
 * ```ts
 * readonly books = new LazyList<Book>(core, BookCodec);
 * // in _apply: case 3: if (op === ChangeOp.FullValue) this.books.applyFull(r); else if (op === ChangeOp.LazyInvalidated) this.books.applyInvalidated(r);
 * ```
 *
 * `length` and `revision` are signals (`useSignal(list.length)`), and `get(index)` is what a row renders
 * from: the cached row, or `undefined` while its page loads (reading it requests the page).
 */
export class LazyList<T> {
  /** The number of rows, as of the last `LazyValue` or `LazyInvalidated` the core sent. */
  readonly length: Signal<number> = new Signal<number>(0);
  /** Changes whenever a page arrives or is replaced: observers of rows re-read on it, and `length` is not enough for that. */
  readonly revision: Signal<number> = new Signal<number>(0);

  private readonly _core: UndraCore;
  private readonly _codec: Codec<T>;

  /**
   * @param core The core the list's page server lives in.
   * @param codec The codec of one row.
   */
  constructor(core: UndraCore, codec: Codec<T>) {
    this._core = core;
    this._codec = codec;
  }

  /** Rows per page request (default 50); one page is also prefetched on each side of the page a read touches. */
  get pageSize(): number {
    return 50;
  }

  /**
   * The row at `index`, or `undefined` while its page loads. Reading an unloaded row requests its page and one
   * page of prefetch on each side, once; an index outside `0..length` returns `undefined` and requests nothing.
   */
  get(index: number): T | undefined {
    void index;
    void this._core;
    void this._codec;
    return undefined;
  }

  /** Requests the pages that hold rows `start` up to (not including) `end`. */
  prefetch(start: number, end: number): void {
    void start;
    void end;
  }

  /** Applies the value of the signal (change-set op `FullValue`): the page server's handle, the length and the version. Consumes `reader`. */
  applyFull(reader: UndraReader): void {
    reader.finish();
  }

  /** Applies a `LazyInvalidated` entry (change-set op 2): the new length and version. The window is re-paged. Consumes `reader`. */
  applyInvalidated(reader: UndraReader): void {
    reader.finish();
  }
}
