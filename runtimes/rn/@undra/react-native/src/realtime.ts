import { type Header, SseError, type SseEvent } from "@undra/runtime";
import {
  type FetchLike,
  type PlatformWebSocket,
  type SseAdapter,
  SseParser,
  type SseStream,
  type WebSocketAdapter,
  type WebSocketConstructorLike,
  type WebSocketHeaderInit,
  browserWebSocket,
  fetchSse,
} from "@undra/runtime/realtime";

/*
 * The `WebSocket` and `Sse` defaults of @undra/react-native (ADR-047): the TypeScript runtime's adapters and
 * bindings (`@undra/runtime/realtime`) over what React Native has, its `WebSocket` global and its `fetch` or
 * `XMLHttpRequest`. `loadNative` registers both ports; an app replaces one with `ports` (as on the web).
 */

/** React Native's `WebSocket` constructor: `new WebSocket(url, protocols, { headers })`. */
export type ReactNativeWebSocketConstructor = new (
  url: string,
  protocols?: string | string[],
  options?: { readonly headers: WebSocketHeaderInit },
) => PlatformWebSocket;

/** Options of {@link reactNativeWebSocket}. */
export interface ReactNativeWebSocketOptions {
  /** The `WebSocket` constructor; default React Native's global one. */
  readonly WebSocket?: ReactNativeWebSocketConstructor;
  /** How many received messages wait for the core before the connection is given up (1008). Default 4,096. */
  readonly maxBufferedMessages?: number;
  /** How many bytes of received messages wait before the connection is given up (1008). Default 16 MiB. */
  readonly maxBufferedBytes?: number;
}

function messageOf(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/**
 * The `WebSocket` default of React Native (ADR-047 §9): `browserWebSocket` of `@undra/runtime/realtime` over React
 * Native's `WebSocket` global, with the upgrade's headers passed as its third constructor argument, `{ headers }`
 * (React Native, unlike a browser, sends them: iOS's `NSURLRequest`, Android's OkHttp).
 *
 * React Native's `WebSocket` cannot stop reading, so a connection queues what arrives until the core pulls it, up to
 * 4,096 messages or 16 MiB, then closes with 1008 and ends with `WsError.Closed { 1008, "the core did not keep up" }`.
 * A refused upgrade is `Refused` without its status (React Native does not report it).
 */
export function reactNativeWebSocket(options: ReactNativeWebSocketOptions = {}): WebSocketAdapter {
  // `browserWebSocket` picks the constructor's form by the platform it sees (Node's options object when it finds
  // `process.versions.node`, which a polyfill or a debugger can give an app); React Native's is always
  // `(url, protocols, { headers })`, so the form is fixed here.
  const Socket = function (
    url: string,
    protocols?: string | string[] | { readonly protocols: string[]; readonly headers: WebSocketHeaderInit },
    init?: { readonly headers: WebSocketHeaderInit },
  ): PlatformWebSocket {
    const Native = options.WebSocket ?? (globalThis as { WebSocket?: ReactNativeWebSocketConstructor }).WebSocket;
    if (Native === undefined) throw new TypeError("this platform has no WebSocket");
    if (protocols !== undefined && typeof protocols === "object" && !Array.isArray(protocols)) {
      return new Native(url, [...protocols.protocols], { headers: protocols.headers });
    }
    return init === undefined ? new Native(url, protocols) : new Native(url, protocols, init);
  } as unknown as WebSocketConstructorLike;
  return browserWebSocket({
    WebSocket: Socket,
    headers: "pass",
    ...(options.maxBufferedMessages !== undefined && { maxBufferedMessages: options.maxBufferedMessages }),
    ...(options.maxBufferedBytes !== undefined && { maxBufferedBytes: options.maxBufferedBytes }),
  });
}

/** What {@link reactNativeSse} needs of an `XMLHttpRequest` (React Native's: text arrives in pieces, with `progress`). */
export interface XMLHttpRequestLike {
  /** 0 unsent, 1 opened, 2 headers received, 3 loading, 4 done. */
  readonly readyState: number;
  readonly status: number;
  readonly statusText: string;
  /** The text received so far (React Native appends each piece). */
  readonly responseText: string;
  open(method: string, url: string): void;
  setRequestHeader(name: string, value: string): void;
  getResponseHeader(name: string): string | null;
  send(body?: null): void;
  abort(): void;
  addEventListener(type: string, listener: () => void): void;
}

/** An `XMLHttpRequest` constructor. */
export type XMLHttpRequestConstructorLike = new () => XMLHttpRequestLike;

/** Options of {@link reactNativeSse}. */
export interface ReactNativeSseOptions {
  /**
   * A `fetch` whose responses stream their body (`response.body.getReader()`, as `expo/fetch` does): the stream is
   * then read through it, pulled. React Native's own `fetch` resolves only once the whole body arrived, so it is
   * never used for an event stream.
   */
  readonly fetch?: FetchLike;
  /** The `XMLHttpRequest` of the fallback; default React Native's global one. */
  readonly XMLHttpRequest?: XMLHttpRequestConstructorLike;
  /**
   * `"auto"` (default): `fetch` when one is given or the global `Response` has a body stream, else `XMLHttpRequest`.
   * `"fetch"` and `"xhr"` force one.
   */
  readonly transport?: "auto" | "fetch" | "xhr";
  /** On the `XMLHttpRequest` path, how many events wait for the core before the stream is given up. Default 4,096. */
  readonly maxBufferedEvents?: number;
  /** On the `XMLHttpRequest` path, how many characters of events wait before the stream is given up. Default 16 Mi. */
  readonly maxBufferedChars?: number;
}

/** Whether the global `fetch` streams response bodies (a `ReadableStream` behind `Response.prototype.body`). */
function fetchStreams(): boolean {
  const g = globalThis as { fetch?: unknown; ReadableStream?: unknown; Response?: { prototype?: object } };
  return typeof g.fetch === "function" && typeof g.ReadableStream === "function" && g.Response?.prototype !== undefined && "body" in g.Response.prototype;
}

/** The media type of `contentType` (`text/event-stream; charset=utf-8` is `text/event-stream`). */
function essence(contentType: string): string {
  return (contentType.split(";")[0] ?? "").trim().toLowerCase();
}

interface Waiter {
  resolve(step: IteratorResult<SseEvent, undefined>): void;
  reject(error: unknown): void;
}

/** One stream of the `XMLHttpRequest` path: the parsed events wait here until the binding pulls them. */
class XhrStream implements SseStream {
  readonly #xhr: XMLHttpRequestLike;
  readonly #maxEvents: number;
  readonly #maxChars: number;
  readonly #items: SseEvent[] = [];
  #chars = 0;
  #end: SseError | null = null;
  #endReported = false;
  #closed = false;
  #taken = false;
  #waiter: Waiter | null = null;

  constructor(xhr: XMLHttpRequestLike, maxEvents: number, maxChars: number) {
    this.#xhr = xhr;
    this.#maxEvents = maxEvents;
    this.#maxChars = maxChars;
  }

  /** Events the parser completed. Past the limits the request is given up: an `XMLHttpRequest` cannot pause. */
  deliver(events: readonly SseEvent[]): void {
    for (const event of events) {
      if (this.#closed || this.#end !== null) return;
      const waiter = this.#waiter;
      if (waiter !== null && this.#items.length === 0) {
        this.#waiter = null;
        waiter.resolve({ value: event, done: false });
        continue;
      }
      const size = event.data.length + event.event.length + (event.id?.length ?? 0);
      if (this.#items.length + 1 > this.#maxEvents || this.#chars + size > this.#maxChars) {
        this.finish(new SseError.Network("the core did not keep up: the event stream was given up"));
        this.#abort();
        return;
      }
      this.#items.push(event);
      this.#chars += size;
    }
  }

  /** The request ended: `Ended` for the body's end, `Network` for a failure; after the queued events. */
  finish(error: SseError): void {
    if (this.#closed || this.#end !== null) return;
    this.#end = error;
    if (this.#items.length === 0) this.#settle();
  }

  events(): AsyncIterable<SseEvent> {
    const taken = this.#taken;
    this.#taken = true;
    return {
      [Symbol.asyncIterator]: () => ({
        next: () => (taken ? Promise.reject(new TypeError("the events of this stream were already taken")) : this.#next()),
        return: () => Promise.resolve({ value: undefined, done: true as const }),
      }),
    };
  }

  #next(): Promise<IteratorResult<SseEvent, undefined>> {
    const event = this.#items.shift();
    if (event !== undefined) {
      this.#chars -= event.data.length + event.event.length + (event.id?.length ?? 0);
      return Promise.resolve({ value: event, done: false });
    }
    if (this.#closed || this.#endReported) return Promise.resolve({ value: undefined, done: true });
    if (this.#end !== null) {
      this.#endReported = true;
      return Promise.reject(this.#end);
    }
    if (this.#waiter !== null) return Promise.reject(new TypeError("a next() is already pending"));
    return new Promise((resolve, reject) => {
      this.#waiter = { resolve, reject };
    });
  }

  #settle(): void {
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

  #abort(): void {
    try {
      this.#xhr.abort();
    } catch {
      // Already done.
    }
  }

  close(): Promise<void> {
    if (!this.#closed) {
      this.#closed = true;
      this.#items.length = 0;
      this.#chars = 0;
      this.#settle();
      this.#abort();
    }
    return Promise.resolve();
  }
}

/** Opens an event stream with `XMLHttpRequest`: its `progress` events carry the text as it arrives. */
function openWithXhr(
  Xhr: XMLHttpRequestConstructorLike,
  url: string,
  headers: readonly Header[],
  lastEventId: string | null,
  maxEvents: number,
  maxChars: number,
): Promise<SseStream> {
  return new Promise<SseStream>((resolve, reject) => {
    let xhr: XMLHttpRequestLike;
    try {
      xhr = new Xhr();
      xhr.open("GET", url);
      xhr.setRequestHeader("Accept", "text/event-stream");
      xhr.setRequestHeader("Cache-Control", "no-cache");
      for (const { name, value } of headers) xhr.setRequestHeader(name, value);
      if (lastEventId !== null) xhr.setRequestHeader("Last-Event-ID", lastEventId);
    } catch (error) {
      reject(new SseError.Network(messageOf(error)));
      return;
    }
    const parser = new SseParser(lastEventId);
    let stream: XhrStream | null = null;
    let settled = false;
    let seen = 0;
    let failure: string | null = null;
    // Only the new part of the text: React Native keeps the whole response in `responseText`.
    const read = (): void => {
      if (stream === null) return;
      const text = xhr.responseText;
      if (text.length <= seen) return;
      const piece = text.slice(seen);
      seen = text.length;
      stream.deliver(parser.push(piece));
    };
    const answered = (): void => {
      settled = true;
      const status = xhr.status;
      if (status < 200 || status > 299 || status === 204) {
        xhr.abort();
        reject(new SseError.Refused(status, `the server answered ${status}${xhr.statusText ? ` ${xhr.statusText}` : ""}`));
        return;
      }
      const type = xhr.getResponseHeader("content-type") ?? "";
      if (essence(type) !== "text/event-stream") {
        xhr.abort();
        reject(new SseError.Protocol(`expected text/event-stream, got ${type === "" ? "no content type" : type}`));
        return;
      }
      stream = new XhrStream(xhr, maxEvents, maxChars);
      resolve(stream);
    };
    xhr.addEventListener("readystatechange", () => {
      if (!settled && xhr.readyState >= 2 && xhr.status !== 0) answered();
      // At 4 (done) the text is read in `loadend`, once it is known whether the request failed: React Native then
      // replaces the text with the error's.
      if (xhr.readyState === 3) read();
    });
    xhr.addEventListener("progress", () => {
      if (xhr.readyState === 3) read();
    });
    xhr.addEventListener("error", () => {
      failure = "the event stream's connection failed";
    });
    xhr.addEventListener("timeout", () => {
      failure = "the event stream timed out";
    });
    xhr.addEventListener("loadend", () => {
      if (!settled) {
        settled = true;
        const detail = xhr.responseText;
        reject(new SseError.Network(`${failure ?? "the event stream's request failed"}${detail ? `: ${detail}` : ""}`));
        return;
      }
      if (stream === null) return; // refused, already answered
      if (failure === null) read();
      parser.end();
      stream.finish(failure === null ? new SseError.Ended() : new SseError.Network(failure));
    });
    try {
      xhr.send(null);
    } catch (error) {
      settled = true;
      reject(new SseError.Network(messageOf(error)));
    }
  });
}

/**
 * The `Sse` default of React Native (ADR-047 §9): a `fetch` that streams the body when there is one (given in
 * `options.fetch`, or a global whose `Response` has a body stream; then `fetchSse` of `@undra/runtime/realtime`, read
 * only as the core pulls), else `XMLHttpRequest` (React Native's: `NSURLSession` on iOS, OkHttp on Android), whose
 * `progress` events carry the text as it arrives, parsed by the runtime's one `SseParser`.
 *
 * The request sends `Accept: text/event-stream`, `Cache-Control: no-cache`, the headers and `Last-Event-ID` when
 * resuming; `open` resolves on a 2xx answer (204 excepted) of type `text/event-stream`, other statuses are
 * `Refused { status }`, another type `Protocol`, no answer `Network`; the body's end is `Ended`. On the
 * `XMLHttpRequest` path the platform decodes the text (so bytes that are not UTF-8 are replaced, not `Protocol`), it
 * cannot pause (events wait for the core up to 4,096 or 16 Mi characters, then the stream ends with `Network`), and
 * React Native keeps the whole response text for the life of the request: reconnect a stream that runs for days.
 */
export function reactNativeSse(options: ReactNativeSseOptions = {}): SseAdapter {
  const maxEvents = Math.max(1, options.maxBufferedEvents ?? 4096);
  const maxChars = Math.max(1, options.maxBufferedChars ?? 16 * 1024 * 1024);
  const viaFetch = fetchSse(options.fetch === undefined ? {} : { fetch: options.fetch });
  return {
    open(url, headers, lastEventId) {
      const transport = options.transport ?? "auto";
      if (transport === "fetch" || (transport === "auto" && (options.fetch !== undefined || fetchStreams()))) {
        return viaFetch.open(url, headers, lastEventId);
      }
      const Xhr = options.XMLHttpRequest ?? (globalThis as { XMLHttpRequest?: XMLHttpRequestConstructorLike }).XMLHttpRequest;
      if (Xhr === undefined) return Promise.reject(new SseError.Refused(null, "this platform has no XMLHttpRequest"));
      return openWithXhr(Xhr, url, headers, lastEventId, maxEvents, maxChars);
    },
  };
}
