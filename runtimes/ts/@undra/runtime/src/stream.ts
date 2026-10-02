import { UndraError } from "./errors.js";
import { msg } from "./messages.js";

/*
 * Client side of a core stream (docs/SPEC.md section 3.7): a queue of received
 * items, drained by `for await`, that hands credit back to the core as items are
 * consumed. Credit measures consumption, not arrival, so a consumer that stops
 * pulling stops the core after at most `STREAM_WINDOW` more items: real
 * backpressure.
 */

/** Credit granted when a stream opens, and the level it is topped back up to. */
export const STREAM_WINDOW = 16;
/** Credit is topped up when the credit left falls below this. */
export const STREAM_LOW_WATER = 8;

/** What a stream needs from the core that owns it. */
export interface StreamHost {
  /** Grants `credit` more items (`StreamCredit`). May throw when the channel is closed. */
  sendCredit(callId: number, credit: number): void;
  /** The consumer stopped early: forget the stream and send `Cancel`. */
  cancel(callId: number): void;
}

type State = "opening" | "open" | "ended" | "failed" | "closed";

interface Waiter {
  resolve(result: IteratorResult<Uint8Array, undefined>): void;
  reject(error: unknown): void;
}

const DONE: IteratorResult<Uint8Array, undefined> = { value: undefined, done: true };

/**
 * One open stream, as an async iterator of the raw item bodies. The core
 * drives it with `opened`, `push`, `end` and `fail`; the consumer with
 * `next` and `return` (which `for await` calls on `break`).
 */
export class StreamCall implements AsyncIterableIterator<Uint8Array> {
  /** The call id of the stream. */
  readonly callId: number;
  private readonly _host: StreamHost;
  private readonly _items: Uint8Array[] = [];
  private readonly _waiters: Waiter[] = [];
  private _state: State = "opening";
  private _error: unknown = undefined;
  private _credit = 0;

  /** @param callId The call id the stream was opened with. @param host The core. */
  constructor(callId: number, host: StreamHost) {
    this.callId = callId;
    this._host = host;
  }

  /** Items received and not yet consumed. */
  get buffered(): number {
    return this._items.length;
  }

  /** The core accepted the stream (`Reply` status 4): grant the initial credit. */
  opened(): void {
    if (this._state !== "opening") return;
    this._state = "open";
    this._credit = STREAM_WINDOW;
    this._grant(STREAM_WINDOW);
  }

  /** An item arrived. */
  push(body: Uint8Array): void {
    if (this._state !== "open" && this._state !== "opening") return;
    const waiter = this._waiters.shift();
    if (waiter === undefined) {
      this._items.push(body);
      return;
    }
    waiter.resolve({ value: body, done: false });
    this._consumed();
  }

  /** The stream ended normally; buffered items are still delivered first. */
  end(): void {
    if (this._state !== "open" && this._state !== "opening") return;
    this._state = "ended";
    this._settle();
  }

  /** The stream failed (an error or failure item, a rejected open, a lost channel); buffered items are still delivered first. */
  fail(error: unknown): void {
    if (this._state === "failed" || this._state === "closed" || this._state === "ended") return;
    this._state = "failed";
    this._error = error;
    this._settle();
  }

  next(): Promise<IteratorResult<Uint8Array, undefined>> {
    const item = this._items.shift();
    if (item !== undefined) {
      this._consumed();
      return Promise.resolve({ value: item, done: false });
    }
    switch (this._state) {
      case "ended":
      case "closed":
        this._state = "closed";
        return Promise.resolve(DONE);
      case "failed": {
        this._state = "closed";
        const error = this._error;
        this._error = undefined;
        return Promise.reject(error);
      }
      default:
        return new Promise((resolve, reject) => {
          this._waiters.push({ resolve, reject });
        });
    }
  }

  /** The consumer is done (`break`, `return`, an exception in the loop body): cancel the stream if it is still running. */
  return(): Promise<IteratorResult<Uint8Array, undefined>> {
    const running = this._state === "opening" || this._state === "open";
    this._state = "closed";
    this._items.length = 0;
    this._error = undefined;
    for (const waiter of this._waiters.splice(0)) waiter.resolve(DONE);
    if (running) {
      try {
        this._host.cancel(this.callId);
      } catch {
        // The channel is gone; the core has nothing left to cancel.
      }
    }
    return Promise.resolve(DONE);
  }

  [Symbol.asyncIterator](): this {
    return this;
  }

  /** Answers the waiting `next()` calls once the stream is terminal. Nothing to do while items are buffered or nobody waits (the next `next()` sees the outcome). */
  private _settle(): void {
    if (this._items.length > 0 || this._waiters.length === 0) return;
    const failing = this._state === "failed";
    const error = this._error;
    this._state = "closed";
    this._error = undefined;
    this._waiters.splice(0).forEach((waiter, index) => {
      if (failing && index === 0) waiter.reject(error);
      else waiter.resolve(DONE);
    });
  }

  /** One item was consumed: return credit when the window runs low. */
  private _consumed(): void {
    if (this._state !== "open") return;
    this._credit--;
    if (this._credit >= STREAM_LOW_WATER) return;
    const grant = STREAM_WINDOW - this._credit;
    this._credit = STREAM_WINDOW;
    this._grant(grant);
  }

  private _grant(credit: number): void {
    try {
      this._host.sendCredit(this.callId, credit);
    } catch (error) {
      this.fail(error instanceof UndraError ? error : new UndraError("state", msg(172), { cause: error }));
    }
  }
}
