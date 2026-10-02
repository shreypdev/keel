import { CallTarget, Kind, type Transport, UndraReader, decodeCall, readLazyPageHeader, type UndraCore } from "@undra/runtime";

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
 * Counts the page calls a core makes: the `Kind.Call` payloads whose first byte is 3 (`CallTarget.LazyListPage`), seen at the core's
 * transport (`callSync` for an in-process core, `send` for a worker or a socket). The transport is the core's private field, which a
 * test harness may reach and an app does not.
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
    return new PageCallLog(Reflect.get(core, "_transport") as Transport);
  }

  /** Every page call since `watch` or the last `take()`, oldest first. */
  #calls: PageCall[] = [];
  #total = 0;
  readonly #stop: () => void;

  private constructor(transport: Transport) {
    const callSync = transport.callSync?.bind(transport);
    const send = transport.send.bind(transport);
    const record = (payload: Uint8Array, reply: Uint8Array | undefined): void => {
      if (payload[0] !== CallTarget.LazyListPage) return;
      const call = decodeCall(payload);
      if (call.target !== CallTarget.LazyListPage) return;
      let version: bigint | undefined;
      // A reply is `call_id u32, status u8, body`; an `Ok` body of a page starts with the page's version.
      if (reply !== undefined && reply.length >= 5 + 16 && reply[4] === 0) version = readLazyPageHeader(new UndraReader(reply.subarray(5))).version;
      this.#calls.push({ handle: call.handle, offset: call.offset, limit: call.limit, version });
      this.#total++;
    };
    if (callSync !== undefined) {
      transport.callSync = (payload) => {
        const reply = callSync(payload);
        record(payload, reply);
        return reply;
      };
    }
    transport.send = (kind, payload) => {
      if (kind === Kind.Call) record(payload, undefined);
      send(kind, payload);
    };
    this.#stop = () => {
      if (callSync !== undefined) transport.callSync = callSync;
      transport.send = send;
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
