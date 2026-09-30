import { type Header, type HttpAdapter, HttpError, type HttpRequest, type HttpResponse } from "@keel/runtime";

/** The address the core is told the server is at (`configureRemote`). Nothing listens there: the `Http` port below answers. */
export const REMOTE_BASE_URL = "https://playground.keel.test";

/** The list the Remote tab shows. Any other list name works too and starts empty. */
export const INBOX = "inbox";

/** What a list holds on the server; the same shape the core's `RemoteTodo` decodes from JSON. */
export interface ServerTodo {
  /** Identity on the server, counting up from 1 within a list. */
  id: number;
  /** What has to be done. */
  title: string;
  /** Whether it is finished. */
  done: boolean;
}

/** What `PlaygroundServer` can be told. */
export interface PlaygroundServerOptions {
  /** How long every answer takes, in milliseconds. Default 300, long enough to see the optimistic update before the server's word. */
  readonly latencyMs?: number;
}

const SEED = ["Buy milk", "Walk the dog", "Write Keel"];
const JSON_HEADERS: readonly Header[] = [{ name: "Content-Type", value: "application/json" }];
const ROUTE = /^\/lists\/([^/]+)\/todos(?:\/(\d+))?$/;
const encoder = new TextEncoder();
const decoder = new TextDecoder();

function answer(status: number, value: unknown): HttpResponse {
  return { status, headers: [...JSON_HEADERS], body: encoder.encode(JSON.stringify(value)) };
}

function readJson(request: HttpRequest): unknown {
  try {
    return JSON.parse(request.body === null ? "" : decoder.decode(request.body)) as unknown;
  } catch {
    return undefined;
  }
}

/**
 * The playground's backend, in memory, behind the core's `Http` port: a to-do server with the
 * routes the core speaks (`GET /lists/L/todos`, `POST /lists/L/todos`, `PATCH /lists/L/todos/ID`),
 * a fixed latency, and an offline switch.
 *
 * The `inbox` list starts with three items. `POST` honours `Idempotency-Key`, so a request the core
 * replays after an offline spell creates one item, not two. While {@link PlaygroundServer.offline}
 * is on, every request fails with `HttpError.Network`, as it would with no connection; a request
 * already in flight when the switch is turned on fails too.
 *
 * ```ts
 * const server = new PlaygroundServer({ latencyMs: 0 });
 * await KeelCore.load({ mode: "wasm-main", wasm, expectedSchemaHash, adapters: { http: server } });
 * ```
 */
export class PlaygroundServer implements HttpAdapter {
  /** Whether the network is down: every request fails. The app pairs it with a `Connectivity` event. */
  offline = false;

  readonly #latencyMs: number;
  readonly #lists = new Map<string, ServerTodo[]>([[INBOX, SEED.map((title, i) => ({ id: i + 1, title, done: false }))]]);
  /** What each `Idempotency-Key` created, so that a repeat of the same request answers with the same item. */
  readonly #created = new Map<string, ServerTodo>();

  /** @param options See {@link PlaygroundServerOptions}. */
  constructor(options: PlaygroundServerOptions = {}) {
    this.#latencyMs = options.latencyMs ?? 300;
  }

  /** The items of `list` right now (a copy), for tests and for looking at what the core did. */
  todos(list: string): ServerTodo[] {
    return (this.#lists.get(list) ?? []).map((todo) => ({ ...todo }));
  }

  async request(request: HttpRequest): Promise<HttpResponse> {
    this.#failWhenOffline();
    if (this.#latencyMs > 0) await new Promise<void>((resolve) => setTimeout(resolve, this.#latencyMs));
    this.#failWhenOffline();
    return this.#route(request);
  }

  #failWhenOffline(): void {
    if (this.offline) throw new HttpError.Network("offline");
  }

  #route(request: HttpRequest): HttpResponse {
    let url: URL;
    try {
      url = new URL(request.url);
    } catch {
      throw new HttpError.InvalidUrl(request.url);
    }
    const route = url.origin === REMOTE_BASE_URL ? ROUTE.exec(url.pathname) : null;
    if (route === null) return answer(404, { error: "no such route" });
    const list = decodeURIComponent(route[1] as string);
    const id = route[2] === undefined ? null : Number(route[2]);
    const todos = this.#lists.get(list) ?? [];
    this.#lists.set(list, todos);

    if (id === null && request.method === "get") return answer(200, todos);

    if (id === null && request.method === "post") {
      const body = readJson(request) as { title?: unknown } | undefined;
      if (typeof body?.title !== "string") return answer(400, { error: "expected {\"title\": string}" });
      const key = request.headers.find((header) => header.name.toLowerCase() === "idempotency-key")?.value;
      const repeat = key === undefined ? undefined : this.#created.get(key);
      if (repeat !== undefined) return answer(201, repeat);
      const created: ServerTodo = { id: todos.reduce((highest, todo) => Math.max(highest, todo.id), 0) + 1, title: body.title, done: false };
      todos.push(created);
      if (key !== undefined) this.#created.set(key, created);
      return answer(201, created);
    }

    if (id !== null && request.method === "patch") {
      const body = readJson(request) as { done?: unknown } | undefined;
      if (typeof body?.done !== "boolean") return answer(400, { error: "expected {\"done\": boolean}" });
      const todo = todos.find((candidate) => candidate.id === id);
      if (todo === undefined) return answer(404, { error: "no such item" });
      todo.done = body.done;
      return answer(200, todo);
    }

    return answer(405, { error: "method not allowed" });
  }
}
