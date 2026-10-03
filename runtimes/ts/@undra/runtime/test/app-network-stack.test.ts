import { type IncomingMessage, type Server, type ServerResponse, createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { afterAll, beforeAll, beforeEach, describe, expect, it } from "vitest";
import { fetchHttp } from "../src/adapters/http.js";
import type { SseEvent } from "../src/adapters/types.js";
import { type FetchLike, type WebSocketConstructorLike, browserWebSocket, fetchSse } from "../src/realtime.js";
import { type RealtimeServer, lastOn, startRealtimeServer, stopRealtimeServer, until } from "./support/realtime-server.js";

/*
 * The app's own network stack inside the ports (ADR-060): `fetchHttp({ fetch })`, `fetchSse({ fetch })` and
 * `browserWebSocket({ WebSocket })` take the app's `fetch` and `WebSocket`, so whatever the app puts around them (a token
 * added to every request, a 401 answered with a refreshed token and the request sent again, a trace header) applies to every
 * request the core makes, with nothing Undra-specific configured. These tests are the proof that it does, over real sockets.
 */

interface Seen {
  readonly method: string;
  readonly path: string;
  readonly headers: IncomingMessage["headers"];
  readonly body: string;
}

let api: Server;
let base = "";
const seen: Seen[] = [];

/** A server whose `/secure` and `/events` need `Authorization: Bearer fresh`, and which records what it was asked. */
beforeAll(async () => {
  api = createServer((req: IncomingMessage, res: ServerResponse) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => {
      const body = Buffer.concat(chunks).toString("utf8");
      const path = (req.url ?? "/").split("?")[0] ?? "/";
      seen.push({ method: req.method ?? "", path, headers: req.headers, body });
      if ((path === "/secure" || path === "/events") && req.headers.authorization !== "Bearer fresh") {
        res.writeHead(401, { "www-authenticate": "Bearer" }).end();
        return;
      }
      if (path === "/events") {
        res.writeHead(200, { "content-type": "text/event-stream" }).end("id: 1\ndata: hello\n\n");
        return;
      }
      res.writeHead(200, { "content-type": "text/plain" }).end(`ok ${path} ${body.length}`);
    });
  });
  await new Promise<void>((resolve) => api.listen(0, "127.0.0.1", resolve));
  base = `http://127.0.0.1:${(api.address() as AddressInfo).port}`;
});

afterAll(async () => {
  await new Promise<void>((resolve) => api.close(() => resolve()));
});

beforeEach(() => {
  seen.length = 0;
});

const text = (body: Uint8Array): string => new TextDecoder().decode(body);

describe("the app's fetch behind the Http port", () => {
  it("sees every request the core makes and can change a header", async () => {
    const calls: string[] = [];
    const appFetch: typeof fetch = (input, init) => {
      calls.push(`${init?.method} ${new URL(String(input)).pathname}`);
      const headers = new Headers(init?.headers);
      headers.set("x-traced", "yes");
      return fetch(input, { ...init, headers });
    };
    const http = fetchHttp({ fetch: appFetch });
    const get = await http.request({ method: "get", url: `${base}/a`, headers: [{ name: "x-core", value: "1" }], body: null, timeoutMs: null });
    const post = await http.request({ method: "post", url: `${base}/b`, headers: [], body: new TextEncoder().encode("hello"), timeoutMs: 5000 });
    expect(text(get.body)).toBe("ok /a 0");
    expect(text(post.body)).toBe("ok /b 5");
    expect(calls).toEqual(["GET /a", "POST /b"]);
    expect(seen.map((s) => s.headers["x-traced"])).toEqual(["yes", "yes"]);
    expect(seen[0]?.headers["x-core"]).toBe("1");
  });

  it("can refresh a token: a 401 is answered with a new one and the request, body included, goes again", async () => {
    let token = "stale";
    let refreshed = 0;
    const appFetch: typeof fetch = async (input, init) => {
      const send = (): Promise<Response> => {
        const headers = new Headers(init?.headers);
        headers.set("authorization", `Bearer ${token}`);
        return fetch(input, { ...init, headers });
      };
      const first = await send();
      if (first.status !== 401) return first;
      token = "fresh";
      refreshed++;
      return send();
    };
    const http = fetchHttp({ fetch: appFetch });
    const answer = await http.request({ method: "post", url: `${base}/secure`, headers: [], body: new TextEncoder().encode("12345"), timeoutMs: null });
    expect(answer.status).toBe(200);
    expect(text(answer.body)).toBe("ok /secure 5");
    expect(refreshed).toBe(1);
    expect(seen.map((s) => s.headers.authorization)).toEqual(["Bearer stale", "Bearer fresh"]);
    expect(seen.map((s) => s.body)).toEqual(["12345", "12345"]);
    // The next request starts with the refreshed token: the refresh state is the app's.
    await http.request({ method: "get", url: `${base}/secure`, headers: [], body: null, timeoutMs: null });
    expect(seen.at(-1)?.headers.authorization).toBe("Bearer fresh");
    expect(refreshed).toBe(1);
  });

  it("answers what the app's fetch gives: an error status it leaves alone is a response, a failure it throws is Network", async () => {
    const http = fetchHttp({ fetch: (async () => new Response("nope", { status: 503 })) as typeof fetch });
    expect((await http.request({ method: "get", url: `${base}/a`, headers: [], body: null, timeoutMs: null })).status).toBe(503);
    const broken = fetchHttp({ fetch: (async () => Promise.reject(new TypeError("the app's proxy refused"))) as typeof fetch });
    await expect(broken.request({ method: "get", url: `${base}/a`, headers: [], body: null, timeoutMs: null })).rejects.toMatchObject({
      kind: "network",
      value: "the app's proxy refused",
    });
    expect(seen).toEqual([]);
  });
});

describe("the app's fetch behind the Sse port", () => {
  it("sees the request, can add a header and answer a 401 with a refreshed token, and the stream opens", async () => {
    let token = "stale";
    const calls: string[] = [];
    const appFetch: FetchLike = async (url, init) => {
      calls.push(url);
      const send = (): Promise<Response> => fetch(url, { ...init, headers: [...init.headers, ["authorization", `Bearer ${token}`], ["x-traced", "yes"]] });
      const first = await send();
      if (first.status !== 401) return first;
      token = "fresh";
      return send();
    };
    const stream = await fetchSse({ fetch: appFetch }).open(`${base}/events`, [{ name: "x-core", value: "1" }], "7");
    const events: SseEvent[] = [];
    try {
      for await (const event of stream.events()) events.push(event);
    } catch {
      // the body ended: Ended
    }
    expect(events).toEqual([{ id: "1", event: "message", data: "hello", retryMs: null }]);
    expect(calls).toEqual([`${base}/events`]);
    expect(seen.map((s) => s.headers.authorization)).toEqual(["Bearer stale", "Bearer fresh"]);
    expect(seen.every((s) => s.headers["x-traced"] === "yes" && s.headers["x-core"] === "1" && s.headers["last-event-id"] === "7")).toBe(true);
    expect(seen.every((s) => s.headers.accept === "text/event-stream")).toBe(true);
  });
});

describe("the app's WebSocket behind the WebSocket port", () => {
  let realtime: RealtimeServer;

  beforeAll(async () => {
    realtime = await startRealtimeServer();
  });

  afterAll(async () => {
    await stopRealtimeServer(realtime);
  });

  it("is the constructor that opens the connection, so what it adds to the upgrade reaches the server", async () => {
    const opened: string[] = [];
    // An app's wrapper: opens the platform's WebSocket with its own header (here a token) on every connection.
    const Platform = (globalThis as unknown as { WebSocket: new (url: string, init: { protocols: string[]; headers: Record<string, string> }) => object }).WebSocket;
    const AppWebSocket = class {
      constructor(url: string, protocols?: string | string[] | { readonly protocols: string[]; readonly headers: Record<string, string> }) {
        opened.push(url);
        const offered = Array.isArray(protocols) ? protocols : typeof protocols === "string" ? [protocols] : (protocols?.protocols ?? []);
        const given = typeof protocols === "object" && !Array.isArray(protocols) ? protocols.headers : {};
        return new Platform(url, { protocols: offered, headers: { ...given, authorization: "Bearer fresh", "x-traced": "yes" } }) as never;
      }
    } as unknown as WebSocketConstructorLike;

    const connection = await browserWebSocket({ WebSocket: AppWebSocket, headers: "pass" }).connect(`${realtime.wsUrl}/ws/headers`, ["v2"], [{ name: "x-core", value: "1" }]);
    expect(opened).toEqual([`${realtime.wsUrl}/ws/headers`]);
    expect(connection.protocol).toBe("v2");
    await connection.close(1000, "done");
    const connectionSeen = await until("the server saw the upgrade", () => lastOn(realtime, "/ws/headers"));
    expect(connectionSeen.headers["x-traced"]).toBe("yes");
    expect(connectionSeen.headers.authorization).toBe("Bearer fresh");
    expect(connectionSeen.headers["x-core"]).toBe("1");
  });
});
