/**
 * A lazy async iterable the test scripts: `push` items, `fail` or `finish` it, and read how many
 * times the consumer asked for the next item (`requested`), which is how a binding's read-ahead
 * shows. `returned` says whether the consumer called `return()`.
 */
export class ScriptedSource<T> implements AsyncIterable<T> {
  readonly #items: T[] = [];
  #end: { readonly error: unknown } | "finished" | null = null;
  #waiter: { resolve(step: IteratorResult<T, undefined>): void; reject(error: unknown): void } | null = null;
  /** How many times `next()` was called. */
  requested = 0;
  /** Whether `return()` was called. */
  returned = false;

  push(...items: T[]): void {
    for (const item of items) {
      const waiter = this.#waiter;
      if (waiter !== null) {
        this.#waiter = null;
        waiter.resolve({ value: item, done: false });
      } else {
        this.#items.push(item);
      }
    }
  }

  fail(error: unknown): void {
    this.#end = { error };
    this.#settle();
  }

  finish(): void {
    this.#end = "finished";
    this.#settle();
  }

  /** Items pushed and not yet read. */
  get waiting(): number {
    return this.#items.length;
  }

  #settle(): void {
    const waiter = this.#waiter;
    if (waiter === null || this.#items.length > 0) return;
    this.#waiter = null;
    if (this.#end === "finished") waiter.resolve({ value: undefined, done: true });
    else if (this.#end !== null) waiter.reject(this.#end.error);
  }

  [Symbol.asyncIterator](): AsyncIterator<T, undefined> {
    return {
      next: () => {
        this.requested++;
        if (this.#items.length > 0) return Promise.resolve({ value: this.#items.shift() as T, done: false });
        if (this.#end === "finished" || this.returned) return Promise.resolve({ value: undefined, done: true });
        if (this.#end !== null) return Promise.reject(this.#end.error);
        return new Promise((resolve, reject) => {
          this.#waiter = { resolve, reject };
        });
      },
      return: () => {
        this.returned = true;
        const waiter = this.#waiter;
        this.#waiter = null;
        waiter?.resolve({ value: undefined, done: true });
        return Promise.resolve({ value: undefined, done: true });
      },
    };
  }
}

/** Lets every pending promise reaction and timer of 0 ms run. */
export function settle(): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, 0));
}
