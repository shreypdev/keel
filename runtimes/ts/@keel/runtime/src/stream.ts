import { KeelError } from "./errors.js";

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
  readonly #host: StreamHost;
  readonly #items: Uint8Array[] = [];
  readonly #waiters: Waiter[] = [];
  #state: State = "opening";
  #error: unknown = undefined;
  #credit = 0;

  /** @param callId The call id the stream was opened with. @param host The core. */
  constructor(callId: number, host: StreamHost) {
    this.callId = callId;
    this.#host = host;
  }

  /** Items received and not yet consumed. */
  get buffered(): number {
    return this.#items.length;
  }

  /** The core accepted the stream (`Reply` status 4): grant the initial credit. */
  opened(): void {
    if (this.#state !== "opening") return;
    this.#state = "open";
    this.#credit = STREAM_WINDOW;
    this.#grant(STREAM_WINDOW);
  }

  /** An item arrived. */
  push(body: Uint8Array): void {
    if (this.#state !== "open" && this.#state !== "opening") return;
    const waiter = this.#waiters.shift();
    if (waiter === undefined) {
      this.#items.push(body);
      return;
    }
    waiter.resolve({ value: body, done: false });
    this.#consumed();
  }

  /** The stream ended normally; buffered items are still delivered first. */
  end(): void {
    if (this.#state !== "open" && this.#state !== "opening") return;
    this.#state = "ended";
    this.#settle();
  }

  /** The stream failed (an error item, a rejected open, a lost channel); buffered items are still delivered first. */
  fail(error: unknown): void {
    if (this.#state === "failed" || this.#state === "closed" || this.#state === "ended") return;
    this.#state = "failed";
    this.#error = error;
    this.#settle();
  }

  next(): Promise<IteratorResult<Uint8Array, undefined>> {
    const item = this.#items.shift();
    if (item !== undefined) {
      this.#consumed();
      return Promise.resolve({ value: item, done: false });
    }
    switch (this.#state) {
      case "ended":
      case "closed":
        this.#state = "closed";
        return Promise.resolve(DONE);
      case "failed": {
        this.#state = "closed";
        const error = this.#error;
        this.#error = undefined;
        return Promise.reject(error);
      }
      default:
        return new Promise((resolve, reject) => {
          this.#waiters.push({ resolve, reject });
        });
    }
  }

  /** The consumer is done (`break`, `return`, an exception in the loop body): cancel the stream if it is still running. */
  return(): Promise<IteratorResult<Uint8Array, undefined>> {
    const running = this.#state === "opening" || this.#state === "open";
    this.#state = "closed";
    this.#items.length = 0;
    this.#error = undefined;
    for (const waiter of this.#waiters.splice(0)) waiter.resolve(DONE);
    if (running) {
      try {
        this.#host.cancel(this.callId);
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
  #settle(): void {
    if (this.#items.length > 0 || this.#waiters.length === 0) return;
    const failing = this.#state === "failed";
    const error = this.#error;
    this.#state = "closed";
    this.#error = undefined;
    this.#waiters.splice(0).forEach((waiter, index) => {
      if (failing && index === 0) waiter.reject(error);
      else waiter.resolve(DONE);
    });
  }

  /** One item was consumed: return credit when the window runs low. */
  #consumed(): void {
    if (this.#state !== "open") return;
    this.#credit--;
    if (this.#credit >= STREAM_LOW_WATER) return;
    const grant = STREAM_WINDOW - this.#credit;
    this.#credit = STREAM_WINDOW;
    this.#grant(grant);
  }

  #grant(credit: number): void {
    try {
      this.#host.sendCredit(this.callId, credit);
    } catch (error) {
      this.fail(error instanceof KeelError ? error : new KeelError("state", "could not grant stream credit", { cause: error }));
    }
  }
}
