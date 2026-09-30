import { HttpError, type HttpMethod, type HttpRequest } from "@keel/runtime";
import { describe, expect, test } from "vitest";
import { INBOX, PlaygroundServer, REMOTE_BASE_URL } from "./playground-server";

const url = (path: string): string => `${REMOTE_BASE_URL}${path}`;

function request(method: HttpMethod, path: string, body?: unknown, headers: HttpRequest["headers"] = []): HttpRequest {
  return {
    method,
    url: url(path),
    headers,
    body: body === undefined ? null : new TextEncoder().encode(JSON.stringify(body)),
    timeoutMs: null,
  };
}

async function json(server: PlaygroundServer, req: HttpRequest): Promise<{ status: number; body: unknown }> {
  const response = await server.request(req);
  return { status: response.status, body: JSON.parse(new TextDecoder().decode(response.body)) as unknown };
}

describe("PlaygroundServer", () => {
  test("the inbox starts with three items; any other list is empty", async () => {
    const server = new PlaygroundServer({ latencyMs: 0 });
    const inbox = await json(server, request("get", `/lists/${INBOX}/todos`));
    expect(inbox.status).toBe(200);
    expect(inbox.body).toEqual([
      { id: 1, title: "Buy milk", done: false },
      { id: 2, title: "Walk the dog", done: false },
      { id: 3, title: "Write Keel", done: false },
    ]);
    expect((await json(server, request("get", "/lists/other/todos"))).body).toEqual([]);
  });

  test("POST creates an item with the next id; PATCH changes it", async () => {
    const server = new PlaygroundServer({ latencyMs: 0 });
    const created = await json(server, request("post", `/lists/${INBOX}/todos`, { title: "Ship it" }));
    expect(created).toEqual({ status: 201, body: { id: 4, title: "Ship it", done: false } });
    const patched = await json(server, request("patch", `/lists/${INBOX}/todos/4`, { done: true }));
    expect(patched).toEqual({ status: 200, body: { id: 4, title: "Ship it", done: true } });
    expect(server.todos(INBOX).map((todo) => todo.done)).toEqual([false, false, false, true]);
  });

  test("a POST that repeats an Idempotency-Key creates one item, not two", async () => {
    const server = new PlaygroundServer({ latencyMs: 0 });
    const key = [{ name: "Idempotency-Key", value: "3f2a" }];
    const first = await json(server, request("post", `/lists/${INBOX}/todos`, { title: "Once" }, key));
    const again = await json(server, request("post", `/lists/${INBOX}/todos`, { title: "Once" }, key));
    expect(again).toEqual(first);
    expect(server.todos(INBOX)).toHaveLength(4);
  });

  test("bad requests are answered, not thrown", async () => {
    const server = new PlaygroundServer({ latencyMs: 0 });
    expect((await json(server, request("post", `/lists/${INBOX}/todos`, { name: "x" }))).status).toBe(400);
    expect((await json(server, request("patch", `/lists/${INBOX}/todos/1`, { done: "yes" }))).status).toBe(400);
    expect((await json(server, request("patch", `/lists/${INBOX}/todos/99`, { done: true }))).status).toBe(404);
    expect((await json(server, request("delete", `/lists/${INBOX}/todos/1`))).status).toBe(405);
    expect((await json(server, request("get", "/elsewhere"))).status).toBe(404);
  });

  test("offline, every request fails with a network error and changes nothing", async () => {
    const server = new PlaygroundServer({ latencyMs: 0 });
    server.offline = true;
    const failure = await server.request(request("post", `/lists/${INBOX}/todos`, { title: "Lost" })).catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(HttpError.Network);
    expect(server.todos(INBOX)).toHaveLength(3);
    server.offline = false;
    expect((await json(server, request("get", `/lists/${INBOX}/todos`))).status).toBe(200);
  });

  test("a request in flight when the network drops fails too, after its latency", async () => {
    const server = new PlaygroundServer({ latencyMs: 30 });
    const started = performance.now();
    const pending = server.request(request("post", `/lists/${INBOX}/todos`, { title: "Lost" })).catch((error: unknown) => error);
    server.offline = true;
    expect(await pending).toBeInstanceOf(HttpError.Network);
    expect(performance.now() - started).toBeGreaterThanOrEqual(25);
    expect(server.todos(INBOX)).toHaveLength(3);
  });
});
