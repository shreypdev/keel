import { HttpError, type HttpRequest } from "@keel/runtime";
import { describe, expect, test } from "vitest";
import { FakeServer, replies } from "../src/fake-server.js";
import { CLOCK_START_MS, ManualClock } from "../src/manual-clock.js";
import { MemoryKv } from "../src/memory-kv.js";
import { step, waitFor } from "../src/wait.js";

// The harness's own tests: the fakes the scenarios lean on do what the harness section of
// scenarios.md says they do. (Not scenarios: the reporter ignores them.)

const request = (method: HttpRequest["method"], url: string, body: string | null = null, headers: HttpRequest["headers"] = []): HttpRequest => ({
  method,
  url,
  headers,
  body: body === null ? null : new TextEncoder().encode(body),
  timeoutMs: null,
});

describe("ManualClock", () => {
  test("starts at the documented time and only moves when told", () => {
    const clock = new ManualClock();
    expect(clock.nowMs()).toBe(1_700_000_000_000);
    expect(clock.nowMs()).toBe(CLOCK_START_MS);
    clock.advance(31_000);
    expect(clock.nowMs()).toBe(CLOCK_START_MS + 31_000);
    clock.set(5);
    expect(clock.nowMs()).toBe(5);
    expect(clock.monotonicNs()).toBe(5_000_000n);
  });
});

describe("FakeServer", () => {
  test("routes by method and exact URL, and answers 404 for anything else", async () => {
    const server = new FakeServer().on("GET", "https://x.test/a", replies.json(200, [1]));
    const ok = await server.request(request("get", "https://x.test/a"));
    expect([ok.status, new TextDecoder().decode(ok.body)]).toEqual([200, "[1]"]);
    expect((await server.request(request("post", "https://x.test/a"))).status).toBe(404);
    expect((await server.request(request("get", "https://x.test/a?q=1"))).status).toBe(404);
  });

  test("records every request with its headers and body", async () => {
    const server = new FakeServer().on("POST", "https://x.test/a", replies.json(201, { id: 1 }));
    await server.request(request("post", "https://x.test/a", '{"title":"t"}', [{ name: "Idempotency-Key", value: "k1" }]));
    await server.request(request("get", "https://x.test/a"));
    expect(server.requests.map((r) => `${r.method} ${r.url}`)).toEqual(["POST https://x.test/a", "GET https://x.test/a"]);
    const [post] = server.requestsTo("POST", "https://x.test/a");
    expect(post?.header("idempotency-key")).toBe("k1");
    expect(post?.json()).toEqual({ title: "t" });
    expect(server.count("GET", "https://x.test/a")).toBe(1);
    expect(server.requests[1]?.body).toBeNull();
  });

  test("a route can be a function of the request, and a later route replaces an earlier one", async () => {
    const server = new FakeServer();
    server.on("GET", "https://x.test/a", (seen) => replies.text(200, `call ${server.requests.length} of ${seen.method}`));
    const first = await server.request(request("get", "https://x.test/a"));
    expect(new TextDecoder().decode(first.body)).toBe("call 1 of GET");
    server.on("GET", "https://x.test/a", replies.text(503, "down"));
    expect((await server.request(request("get", "https://x.test/a"))).status).toBe(503);
  });

  test("a reply can be delayed, and a network error is an HttpError.Network", async () => {
    const server = new FakeServer()
      .on("GET", "https://x.test/slow", replies.text(200, "late", { delayMs: 50 }))
      .on("GET", "https://x.test/down", replies.networkError("offline"));
    const started = performance.now();
    await server.request(request("get", "https://x.test/slow"));
    expect(performance.now() - started).toBeGreaterThanOrEqual(45);
    const failure = await server.request(request("get", "https://x.test/down")).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(HttpError.Network);
    expect(failure).toMatchObject({ value: "offline" });
    expect(server.count("GET", "https://x.test/down"), "a failed request is still recorded").toBe(1);
  });
});

describe("MemoryKv", () => {
  test("stores values, lists by prefix in order, and remembers every call", async () => {
    const kv = new MemoryKv();
    await kv.set("b.2", Uint8Array.of(2));
    await kv.set("b.1", Uint8Array.of(1));
    await kv.set("a", Uint8Array.of(9));
    expect(await kv.list("b.")).toEqual(["b.1", "b.2"]);
    expect(await kv.get("a")).toEqual(Uint8Array.of(9));
    expect(await kv.get("missing")).toBeNull();
    await kv.delete("a");
    expect(kv.peek("a")).toBeUndefined();
    expect(kv.writesTo("a").map((w) => w.op)).toEqual(["set", "delete"]);
    expect(kv.operations.map((o) => o.op)).toEqual(["set", "set", "set", "list", "get", "get", "delete"]);
  });
});

describe("waiting", () => {
  test("waitFor returns the first truthy value, and fails with what it waited for", async () => {
    let n = 0;
    expect(await waitFor("a third look", () => (++n === 3 ? "done" : undefined))).toBe("done");
    await expect(waitFor("never", () => false, { timeoutMs: 40 })).rejects.toThrow(/timed out after 40 ms waiting for never/);
    await expect(
      waitFor(
        "a thrower",
        () => {
          throw new Error("boom");
        },
        { timeoutMs: 30 },
      ),
    ).rejects.toThrow(/last error: boom/);
  });

  test("step labels a failure with its number", async () => {
    await expect(
      step("2. area", () => {
        throw new Error("expected 8");
      }),
    ).rejects.toThrow("[step 2. area] expected 8");
    expect(await step("1. fine", () => 7)).toBe(7);
  });
});
