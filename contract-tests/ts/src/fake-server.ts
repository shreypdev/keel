import { type Header, type HttpAdapter, HttpError, type HttpMethod, type HttpRequest, type HttpResponse } from "@undra/runtime";

/** One request the core made, as the server saw it. */
export interface RecordedRequest {
  /** The method in upper case: `GET`, `POST`, `PATCH`. */
  readonly method: string;
  /** The full URL, such as `https://playground.test/lists/s12/todos`. */
  readonly url: string;
  /** The headers, in the order they were sent. */
  readonly headers: readonly Header[];
  /** The body, or `null` for a request without one. */
  readonly body: Uint8Array | null;
  /** The value of the first header called `name` (case-insensitive), if there is one. */
  header(name: string): string | undefined;
  /** The body as UTF-8 text (`""` when there is none). */
  text(): string;
  /** The body parsed as JSON. */
  json(): unknown;
}

/** How the server answers a request: a response (any status), or a network failure. */
export type Reply =
  | {
      readonly kind: "response";
      readonly status: number;
      readonly body: Uint8Array;
      readonly headers: readonly Header[];
      /** Milliseconds to wait before answering. */
      readonly delayMs: number;
    }
  | {
      readonly kind: "network";
      /** The text of the `HttpError.Network` the core sees. */
      readonly message: string;
      /** Milliseconds to wait before failing. */
      readonly delayMs: number;
    };

/** What a route holds: a fixed reply, or a function that decides per request (so a reply can depend on what the server has seen). */
export type Route = Reply | ((request: RecordedRequest) => Reply);

/** Options shared by the reply builders. */
export interface ReplyOptions {
  /** Milliseconds to wait before answering. Default 0. */
  readonly delayMs?: number;
  /** Extra response headers. */
  readonly headers?: readonly Header[];
}

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** Builders of the replies a route can give. */
export const replies = {
  /** A response whose body is `value` as JSON. */
  json(status: number, value: unknown, options: ReplyOptions = {}): Reply {
    return {
      kind: "response",
      status,
      body: encoder.encode(JSON.stringify(value)),
      headers: [{ name: "Content-Type", value: "application/json" }, ...(options.headers ?? [])],
      delayMs: options.delayMs ?? 0,
    };
  },
  /** A response whose body is the text `body`. */
  text(status: number, body: string, options: ReplyOptions = {}): Reply {
    return { kind: "response", status, body: encoder.encode(body), headers: options.headers ?? [], delayMs: options.delayMs ?? 0 };
  },
  /** The connection fails: the core sees `HttpError.Network(message)`. */
  networkError(message: string, options: Pick<ReplyOptions, "delayMs"> = {}): Reply {
    return { kind: "network", message, delayMs: options.delayMs ?? 0 };
  },
} as const;

/** The answer for a route nobody scripted: status 404, no body. */
const NOT_FOUND: Reply = { kind: "response", status: 404, body: new Uint8Array(0), headers: [], delayMs: 0 };

const routeKey = (method: string, url: string): string => `${method.toUpperCase()} ${url}`;

function recorded(request: HttpRequest): RecordedRequest {
  const body = request.body === null ? null : request.body.slice();
  return {
    method: request.method.toUpperCase(),
    url: request.url,
    headers: request.headers.map((h) => ({ ...h })),
    body,
    header(name) {
      const wanted = name.toLowerCase();
      return request.headers.find((h) => h.name.toLowerCase() === wanted)?.value;
    },
    text: () => (body === null ? "" : decoder.decode(body)),
    json: () => JSON.parse(body === null ? "" : decoder.decode(body)) as unknown,
  };
}

/**
 * The `Http` port of the contract tests: an in-memory server.
 *
 * Routes are chosen by method and exact URL, every request is recorded, a reply can be delayed
 * by N milliseconds (a real timer) or be a network error, and a route nobody scripted answers 404.
 *
 * ```ts
 * const server = new FakeServer();
 * server.on("GET", "https://playground.test/lists/a/todos", replies.json(200, []));
 * server.count("GET", "https://playground.test/lists/a/todos"); // 0 until the core asks
 * ```
 */
export class FakeServer implements HttpAdapter {
  /** Every request received so far, oldest first (recorded when it arrives, before any delay). */
  readonly requests: RecordedRequest[] = [];
  readonly #routes = new Map<string, Route>();

  /** Scripts the answer to `method` `url`, replacing an earlier one. Returns the server for chaining. */
  on(method: HttpMethod | Uppercase<HttpMethod>, url: string, route: Route): this {
    this.#routes.set(routeKey(method, url), route);
    return this;
  }

  /** The requests received for `method` `url`, oldest first. */
  requestsTo(method: HttpMethod | Uppercase<HttpMethod>, url: string): RecordedRequest[] {
    const key = routeKey(method, url);
    return this.requests.filter((request) => routeKey(request.method, request.url) === key);
  }

  /** How many requests were received for `method` `url`. */
  count(method: HttpMethod | Uppercase<HttpMethod>, url: string): number {
    return this.requestsTo(method, url).length;
  }

  async request(req: HttpRequest): Promise<HttpResponse> {
    const seen = recorded(req);
    this.requests.push(seen);
    const route = this.#routes.get(routeKey(seen.method, seen.url)) ?? NOT_FOUND;
    const reply = typeof route === "function" ? route(seen) : route;
    if (reply.delayMs > 0) await new Promise<void>((resolve) => setTimeout(resolve, reply.delayMs));
    if (reply.kind === "network") throw new HttpError.Network(reply.message);
    return { status: reply.status, headers: [...reply.headers], body: reply.body.slice() };
  }
}
