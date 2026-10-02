import { type Header, type HttpAdapter, HttpError, type HttpRequest, type HttpResponse } from "@undra/runtime";

/** The address the core is told the server is at (`configureServer`). Nothing listens there: the `Http` port below answers. */
export const SERVER_URL = "https://fieldbook.undra.test";

/** The team code the demo server accepts. */
export const TEAM_CODE = "fieldbook";

/** A note as the server holds it: the JSON the core's `PUT /notes/{id}` sends. */
export interface ServerNote {
  readonly id: number;
  readonly title: string;
  readonly body: string;
  readonly tag: string;
  readonly pinned: boolean;
  readonly created_ms: number;
  readonly photos: readonly string[];
}

/** What `DemoServer` can be told. */
export interface DemoServerOptions {
  /** How long every answer takes, in milliseconds. Default 250, long enough to see a note "waiting to sync". */
  readonly latencyMs?: number;
}

const JSON_HEADERS: readonly Header[] = [{ name: "Content-Type", value: "application/json" }];
const encoder = new TextEncoder();
const decoder = new TextDecoder();

function answer(status: number, value: unknown = {}): HttpResponse {
  return { status, headers: [...JSON_HEADERS], body: encoder.encode(JSON.stringify(value)) };
}

function readJson(request: HttpRequest): Record<string, unknown> | undefined {
  try {
    const value: unknown = JSON.parse(request.body === null ? "" : decoder.decode(request.body));
    return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : undefined;
  } catch {
    return undefined;
  }
}

/**
 * The team's backend, in memory, behind the core's `Http` port: sign-in with a team code, short-lived access
 * tokens that the refresh endpoint renews, notes, photos, and an offline switch. It is what the web app talks to,
 * so the demo needs no process besides the page; the iOS and Android apps answer the port the same way.
 *
 * `access` tokens expire after {@link DemoServer.expireAfter} requests, which is how the demo shows a `401`
 * followed by a refresh the user never sees. While {@link DemoServer.offline} is on, every request fails with
 * `HttpError.Network`, as it would with no connection.
 */
export class DemoServer implements HttpAdapter {
  /** Whether the network is down: every request fails. The app pairs it with a `Connectivity` event. */
  offline = false;
  /** How many authenticated requests an access token serves before the server answers `401` (0: never). */
  expireAfter = 0;

  readonly #latencyMs: number;
  readonly #notes = new Map<number, ServerNote>();
  readonly #photos = new Map<string, Uint8Array>();
  /** What each `Idempotency-Key` already did, so a replayed request answers like the first one. */
  readonly #seen = new Map<string, HttpResponse>();
  #generation = 1;
  #served = 0;

  constructor(options: DemoServerOptions = {}) {
    this.#latencyMs = options.latencyMs ?? 250;
  }

  /** The notes the server holds right now, oldest first. */
  notes(): ServerNote[] {
    return [...this.#notes.values()].sort((a, b) => a.id - b.id);
  }

  /** The photos the server holds, by `"<note>/<index>"`. */
  photos(): Map<string, Uint8Array> {
    return new Map(this.#photos);
  }

  async request(request: HttpRequest): Promise<HttpResponse> {
    this.#failWhenOffline();
    if (this.#latencyMs > 0) await new Promise<void>((resolve) => setTimeout(resolve, this.#latencyMs));
    this.#failWhenOffline();
    let url: URL;
    try {
      url = new URL(request.url);
    } catch {
      throw new HttpError.InvalidUrl(request.url);
    }
    if (url.origin !== SERVER_URL) return answer(404, { error: "no such server" });
    const key = request.headers.find((h) => h.name.toLowerCase() === "idempotency-key")?.value;
    const repeat = key === undefined ? undefined : this.#seen.get(key);
    if (repeat !== undefined) return repeat;
    const response = this.#route(request, url.pathname);
    if (key !== undefined && response.status < 300) this.#seen.set(key, response);
    return response;
  }

  #failWhenOffline(): void {
    if (this.offline) throw new HttpError.Network("offline");
  }

  #token(kind: "access" | "refresh"): string {
    return `${kind}-${String(this.#generation)}`;
  }

  #authorized(request: HttpRequest): boolean {
    const header = request.headers.find((h) => h.name.toLowerCase() === "authorization")?.value;
    if (header !== `Bearer ${this.#token("access")}`) return false;
    if (this.expireAfter > 0 && ++this.#served > this.expireAfter) {
      // The access token ran out: the next one is issued by the refresh endpoint.
      this.#served = 0;
      this.#generation++;
      return false;
    }
    return true;
  }

  #route(request: HttpRequest, path: string): HttpResponse {
    if (path === "/auth/login" && request.method === "post") {
      const body = readJson(request);
      if (typeof body?.["name"] !== "string" || body["code"] !== TEAM_CODE) return answer(401, { error: "wrong team code" });
      return answer(200, { access: this.#token("access"), refresh: "refresh", user: body["name"] });
    }
    if (path === "/auth/refresh" && request.method === "post") {
      if (readJson(request)?.["refresh"] !== "refresh") return answer(401, { error: "refresh refused" });
      return answer(200, { access: this.#token("access"), refresh: "refresh" });
    }
    if (!this.#authorized(request)) return answer(401, { error: "sign in" });
    if (path === "/me") return answer(200, { name: "demo" });
    const note = /^\/notes\/(\d+)$/.exec(path);
    if (note !== null) {
      const id = Number(note[1]);
      if (request.method === "put") {
        const body = readJson(request) as unknown as ServerNote | undefined;
        if (typeof body?.title !== "string") return answer(400, { error: "expected a note" });
        this.#notes.set(id, body);
        return answer(204);
      }
      if (request.method === "delete") return this.#notes.delete(id) ? answer(204) : answer(404);
    }
    const photo = /^\/notes\/(\d+)\/photos\/(\d+)$/.exec(path);
    if (photo !== null && request.method === "put") {
      this.#photos.set(`${photo[1] as string}/${photo[2] as string}`, request.body ?? new Uint8Array());
      return answer(204);
    }
    return answer(404, { error: "no such route" });
  }
}
