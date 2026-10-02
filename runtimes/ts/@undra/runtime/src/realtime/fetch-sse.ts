import { SseError, type SseEvent } from "../adapters/types.js";
import { errorMessage } from "../platform.js";
import { SseParser } from "./sse-parser.js";
import type { SseAdapter, SseStream } from "./sse.js";
import { msg } from "../messages.js";

/** The `fetch` {@link fetchSse} calls: the global one's shape, as far as it is used. */
export type FetchLike = (
  url: string,
  init: { readonly method: string; readonly headers: Array<[string, string]>; readonly signal: AbortSignal },
) => Promise<Response>;

/** Options of {@link fetchSse}. */
export interface FetchSseOptions {
  /** The `fetch` to use; default the global one. */
  readonly fetch?: FetchLike;
}

/**
 * `value` as the header value `fetch` sends as its UTF-8 bytes: one character per byte (a header value is bytes, and
 * `fetch` refuses a character above U+00FF). `Last-Event-ID` carries the id as UTF-8, as `EventSource` sends it.
 */
function utf8HeaderValue(value: string): string {
  let out = "";
  for (const byte of new TextEncoder().encode(value)) out += String.fromCharCode(byte);
  return out;
}

/** The media type of `contentType` (`text/event-stream; charset=utf-8` → `text/event-stream`). */
function essence(contentType: string): string {
  return (contentType.split(";")[0] ?? "").trim().toLowerCase();
}

/** One stream of {@link fetchSse}: reads the body only when the consumer asks for the next event. */
class FetchStream implements SseStream {
  readonly #reader: ReadableStreamDefaultReader<Uint8Array>;
  readonly #controller: AbortController;
  readonly #parser: SseParser;
  /** Keeps a byte order mark: the parser skips one at the start, and only one (the standard's UTF-8 decode strips one). */
  readonly #decoder = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
  readonly #ready: SseEvent[] = [];
  #end: SseError | null = null;
  #endReported = false;
  #closed = false;
  #taken = false;

  constructor(reader: ReadableStreamDefaultReader<Uint8Array>, controller: AbortController, lastEventId: string | null) {
    this.#reader = reader;
    this.#controller = controller;
    this.#parser = new SseParser(lastEventId);
  }

  events(): AsyncIterable<SseEvent> {
    const taken = this.#taken;
    this.#taken = true;
    return {
      [Symbol.asyncIterator]: () => ({
        next: () => (taken ? Promise.reject(new TypeError(msg(122))) : this.#next()),
        return: () => Promise.resolve({ value: undefined, done: true as const }),
      }),
    };
  }

  async #next(): Promise<IteratorResult<SseEvent, undefined>> {
    for (;;) {
      const event = this.#ready.shift();
      if (event !== undefined) return { value: event, done: false };
      if (this.#closed || this.#endReported) return { value: undefined, done: true };
      if (this.#end !== null) {
        this.#endReported = true;
        throw this.#end;
      }
      let chunk: ReadableStreamReadResult<Uint8Array>;
      try {
        chunk = await this.#reader.read();
      } catch (error) {
        if (this.#closed) return { value: undefined, done: true };
        this.#end = new SseError.Network(errorMessage(error));
        continue;
      }
      if (this.#closed) return { value: undefined, done: true };
      let text: string;
      try {
        text = chunk.done ? this.#decoder.decode() : this.#decoder.decode(chunk.value, { stream: true });
      } catch {
        this.#end = new SseError.Protocol("the event stream is not UTF-8");
        this.#abort();
        continue;
      }
      this.#ready.push(...this.#parser.push(text));
      if (chunk.done) {
        this.#parser.end();
        this.#end = new SseError.Ended();
      }
    }
  }

  #abort(): void {
    this.#controller.abort();
    this.#reader.cancel().catch(() => {});
  }

  close(): Promise<void> {
    if (!this.#closed) {
      this.#closed = true;
      this.#ready.length = 0;
      this.#abort();
    }
    return Promise.resolve();
  }
}

/**
 * The default Sse adapter (ADR-047 §9): `fetch` with a streamed body and {@link SseParser}, not
 * `EventSource`, which sends no headers, hides a refusal's status and reconnects on its own.
 *
 * The request sends `Accept: text/event-stream`, `Cache-Control: no-cache`, the headers and
 * `Last-Event-ID` when resuming (the id's UTF-8 bytes, as `EventSource` sends it). `open` resolves
 * on a 2xx answer (204 excepted: it means "stop") whose type is `text/event-stream`; any other
 * status is `Refused { status }`, another type `Protocol`, a request that got no answer `Network`.
 * The body is read only when the core pulls (`body.getReader()`), so a core that stops reading lets
 * TCP push back on the server. The body's end is `Ended`, a failure while reading `Network`, bytes
 * that are not UTF-8 `Protocol`.
 */
export function fetchSse(options: FetchSseOptions = {}): SseAdapter {
  return {
    async open(url, headers, lastEventId) {
      const doFetch = options.fetch ?? (globalThis as { fetch?: FetchLike }).fetch;
      if (doFetch === undefined) throw new SseError.Refused(null, "this platform has no fetch");
      const sent: Array<[string, string]> = [
        ["Accept", "text/event-stream"],
        ["Cache-Control", "no-cache"],
        ...headers.map(({ name, value }): [string, string] => [name, value]),
      ];
      if (lastEventId !== null) sent.push(["Last-Event-ID", utf8HeaderValue(lastEventId)]);
      const controller = new AbortController();
      let response: Response;
      try {
        response = await doFetch(url, { method: "GET", headers: sent, signal: controller.signal });
      } catch (error) {
        throw new SseError.Network(errorMessage(error));
      }
      const fail = (error: SseError): never => {
        controller.abort();
        response.body?.cancel().catch(() => {});
        throw error;
      };
      const status = response.status;
      if (status < 200 || status > 299 || status === 204) {
        fail(new SseError.Refused(status, `the server answered ${status}${response.statusText ? ` ${response.statusText}` : ""}`));
      }
      const type = response.headers.get("content-type") ?? "";
      if (essence(type) !== "text/event-stream") fail(new SseError.Protocol(`expected text/event-stream, got ${type === "" ? "no content type" : type}`));
      const body = response.body;
      if (body === null) return fail(new SseError.Protocol("the answer has no body"));
      return new FetchStream(body.getReader(), controller, lastEventId);
    },
  };
}
