import { HttpError, type HttpRequest } from "@undra/runtime";
import { describe, expect, it } from "vitest";
import { DemoServer, SERVER_URL, TEAM_CODE } from "./demo-server";

const encoder = new TextEncoder();
const json = (value: unknown): Uint8Array => encoder.encode(JSON.stringify(value));
const call = (method: HttpRequest["method"], path: string, init: { body?: unknown; token?: string; key?: string } = {}): HttpRequest => ({
  method,
  url: SERVER_URL + path,
  headers: [
    ...(init.token === undefined ? [] : [{ name: "Authorization", value: `Bearer ${init.token}` }]),
    ...(init.key === undefined ? [] : [{ name: "Idempotency-Key", value: init.key }]),
  ],
  body: init.body === undefined ? null : json(init.body),
  timeoutMs: null,
});
const note = (id: number) => ({ id, title: `note ${String(id)}`, body: "", tag: "", pinned: false, created_ms: 1, photos: [] });

describe("DemoServer", () => {
  const login = async (server: DemoServer) => {
    const response = await server.request(call("post", "/auth/login", { body: { name: "Ada", code: TEAM_CODE } }));
    return JSON.parse(new TextDecoder().decode(response.body)) as { access: string; refresh: string; user: string };
  };

  it("signs in with the team code and refuses another", async () => {
    const server = new DemoServer({ latencyMs: 0 });
    expect(await login(server)).toMatchObject({ user: "Ada", access: "access-1" });
    const refused = await server.request(call("post", "/auth/login", { body: { name: "Ada", code: "nope" } }));
    expect(refused.status).toBe(401);
  });

  it("holds the notes it is sent, and answers a replayed request as it did the first time", async () => {
    const server = new DemoServer({ latencyMs: 0 });
    const { access } = await login(server);
    expect((await server.request(call("put", "/notes/1", { body: note(1), token: access, key: "k1" }))).status).toBe(204);
    expect((await server.request(call("put", "/notes/1", { body: note(1), token: access, key: "k1" }))).status).toBe(204);
    expect((await server.request(call("put", "/notes/2", { body: note(2), token: access }))).status).toBe(204);
    expect(server.notes().map((n) => n.id)).toEqual([1, 2]);
    expect((await server.request(call("delete", "/notes/1", { token: access }))).status).toBe(204);
    expect((await server.request(call("delete", "/notes/1", { token: access }))).status).toBe(404);
    expect((await server.request(call("put", "/notes/3/photos/0", { body: undefined, token: access }))).status).toBe(204);
    expect([...server.photos().keys()]).toEqual(["3/0"]);
  });

  it("answers 401 without a token and after the access token expires, and the refresh renews it", async () => {
    const server = new DemoServer({ latencyMs: 0 });
    server.expireAfter = 1;
    const { access } = await login(server);
    expect((await server.request(call("get", "/me"))).status).toBe(401);
    expect((await server.request(call("get", "/me", { token: access }))).status).toBe(200);
    expect((await server.request(call("get", "/me", { token: access }))).status).toBe(401); // expired
    const renewed = await server.request(call("post", "/auth/refresh", { body: { refresh: "refresh" } }));
    const { access: next } = JSON.parse(new TextDecoder().decode(renewed.body)) as { access: string };
    expect(next).not.toBe(access);
    expect((await server.request(call("get", "/me", { token: next }))).status).toBe(200);
    expect((await server.request(call("post", "/auth/refresh", { body: { refresh: "stolen" } }))).status).toBe(401);
  });

  it("fails every request while offline, like a network that is down", async () => {
    const server = new DemoServer({ latencyMs: 0 });
    server.offline = true;
    await expect(server.request(call("get", "/me"))).rejects.toBeInstanceOf(HttpError.Network);
  });
});
