/*
 * The inbound queue of one WebSocket connection, as an adapter keeps it: what the socket
 * delivered and the consumer has not taken, the end the socket reported, and the async iterator
 * `WebSocketConnection.messages()` returns. Internal to `@undra/runtime/realtime`.
 */

interface Waiter<T> {
  resolve(step: IteratorResult<T, undefined>): void;
  reject(error: unknown): void;
}

/** A queue with one consumer, an end (an error, after the queued items), and a close that drops everything. */
export class Inbox<T> {
  readonly #items: T[] = [];
  readonly #sizes: number[] = [];
  #bytes = 0;
  #waiter: Waiter<T> | null = null;
  #end: unknown = null;
  #ended = false;
  #endReported = false;
  #closed = false;
  #taken = false;

  /** Called when the consumer waits on an empty queue (a socket that was paused can read again). */
  onWait: (() => void) | null = null;
  /** Called after the consumer took an item. */
  onTake: (() => void) | null = null;

  /** How many items wait. */
  get length(): number {
    return this.#items.length;
  }

  /** The total size of the items that wait, as their producer counted it. */
  get bytes(): number {
    return this.#bytes;
  }

  /** Whether the inbox takes no more items (it ended, or the consumer closed it). */
  get done(): boolean {
    return this.#ended || this.#closed;
  }

  /** The end the producer reported, if any. */
  get end(): unknown {
    return this.#ended ? this.#end : null;
  }

  /** Queues `item` (ignored once the inbox is done). */
  push(item: T, size: number): void {
    if (this.done) return;
    const waiter = this.#waiter;
    if (waiter !== null && this.#items.length === 0) {
      this.#waiter = null;
      waiter.resolve({ value: item, done: false });
      this.onTake?.();
      return;
    }
    this.#items.push(item);
    this.#sizes.push(size);
    this.#bytes += size;
  }

  /** Ends the inbox with `error`, delivered after the queued items. The first end wins. */
  fail(error: unknown): void {
    if (this.done) return;
    this.#ended = true;
    this.#end = error;
    if (this.#items.length === 0) this.#settleWaiter();
  }

  /** The consumer is done: the queue is dropped and a waiting `next` finishes. */
  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#items.length = 0;
    this.#sizes.length = 0;
    this.#bytes = 0;
    this.#settleWaiter();
  }

  /** The one async iterable of the inbox; a second call gets an iterable that fails at once. */
  iterable(): AsyncIterable<T> {
    const taken = this.#taken;
    this.#taken = true;
    if (taken) {
      return {
        [Symbol.asyncIterator]: () => ({
          next: () => Promise.reject(new TypeError("the messages of this connection were already taken")),
        }),
      };
    }
    return {
      [Symbol.asyncIterator]: () => ({
        next: () => this.#next(),
        return: () => Promise.resolve({ value: undefined, done: true as const }),
      }),
    };
  }

  #next(): Promise<IteratorResult<T, undefined>> {
    if (this.#items.length > 0) {
      const value = this.#items.shift() as T;
      this.#bytes -= this.#sizes.shift() ?? 0;
      this.onTake?.();
      return Promise.resolve({ value, done: false });
    }
    if (this.#closed || (this.#ended && this.#endReported)) return Promise.resolve({ value: undefined, done: true });
    if (this.#ended) {
      this.#endReported = true;
      return Promise.reject(this.#end);
    }
    if (this.#waiter !== null) return Promise.reject(new TypeError("a next() is already pending"));
    return new Promise((resolve, reject) => {
      this.#waiter = { resolve, reject };
      this.onWait?.();
    });
  }

  #settleWaiter(): void {
    const waiter = this.#waiter;
    if (waiter === null) return;
    this.#waiter = null;
    if (this.#closed) {
      waiter.resolve({ value: undefined, done: true });
    } else {
      this.#endReported = true;
      waiter.reject(this.#end);
    }
  }
}
