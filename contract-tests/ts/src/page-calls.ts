import { CallTarget, type CallTargetArg, UndraModeError, UndraReader, readLazyPageHeader, type UndraCore } from "@undra/runtime";

/** One page call (target 3) the host made. */
export interface PageCall {
  /** The page server the call was addressed to. */
  readonly handle: bigint;
  /** The first row asked for. */
  readonly offset: number;
  /** How many rows. */
  readonly limit: number;
  /** The version in the reply, when the call was synchronous (`wasm-main`, native); `undefined` when the reply came later. */
  readonly version: bigint | undefined;
}

/**
 * Counts the page calls a core makes: the calls of `core.callSync` and `core.call` whose target is `CallTarget.LazyListPage`, seen at the
 * core's own public methods (what `LazyList` calls, whatever transport the core runs over: in process, a worker, a socket). Nothing here
 * reaches a private member: the same code counts the calls of the production build of the runtime, whose private names are renamed.
 *
 * ```ts
 * const pages = PageCallLog.watch(core);
 * library.books.get(120);
 * await waitFor("the pages", () => pages.count === 3);
 * ```
 */
export class PageCallLog {
  /** Starts counting the page calls of `core`. */
  static watch(core: UndraCore): PageCallLog {
    return new PageCallLog(core);
  }

  /** Every page call since `watch` or the last `take()`, oldest first. */
  #calls: PageCall[] = [];
  #total = 0;
  readonly #stop: () => void;

  private constructor(core: UndraCore) {
    const callSync = core.callSync;
    const call = core.call;
    const record = (target: CallTargetArg, body: Uint8Array | undefined): void => {
      if (typeof target === "number" || target.target !== CallTarget.LazyListPage) return;
      // An `Ok` body of a page starts with the page's version (16 bytes of header).
      const version = body !== undefined && body.length >= 16 ? readLazyPageHeader(new UndraReader(body)).version : undefined;
      this.#calls.push({ handle: target.handle, offset: target.offset, limit: target.limit, version });
      this.#total++;
    };
    core.callSync = (target, methodId, args) => {
      let body: Uint8Array | undefined;
      let reached = true;
      try {
        body = callSync.call(core, target, methodId, args);
        return body;
      } catch (error) {
        // A transport that cannot answer synchronously refuses before anything is sent: `LazyList` then makes the call it counts.
        reached = !(error instanceof UndraModeError);
        throw error;
      } finally {
        if (reached) record(target, body);
      }
    };
    core.call = (target, methodId, args, signal, orphan) => {
      record(target, undefined);
      return call.call(core, target, methodId, args, signal, orphan);
    };
    this.#stop = () => {
      Reflect.deleteProperty(core, "callSync");
      Reflect.deleteProperty(core, "call");
    };
  }

  /** How many page calls have been made since `watch`; `take()` does not reset it. */
  get count(): number {
    return this.#total;
  }

  /** The page calls since the last `take()` (or `watch`), oldest first; forgets them (the count keeps growing). */
  take(): PageCall[] {
    const calls = this.#calls;
    this.#calls = [];
    return calls;
  }

  /** Stops counting and puts the transport back as it was. */
  stop(): void {
    this.#stop();
  }
}
