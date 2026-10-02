import { describe, expect, it } from "vitest";
import { SeedError, applySeed, createFakes, parseSeed, toHex } from "../src/index.js";
import { fixture } from "./support/fixtures.js";

describe("seeds", () => {
  it("the example seed reads and seeds the fakes", async () => {
    const seed = parseSeed(fixture("fixtures/seed.json"));
    expect(seed.nowMs).toBe(1_700_000_000_000);
    expect(seed.rngSeed).toBe(42n);
    const fakes = createFakes();
    applySeed(fakes, seed);
    expect(fakes.kv.value("greeting")).toEqual(new TextEncoder().encode("hello"));
    expect(toHex(fakes.kv.value("blob")!)).toBe("00ff");
    expect(new TextDecoder().decode(fakes.secureStore.value("token")!)).toBe("t-123");
    expect(new TextDecoder().decode(fakes.fs.contents("notes/a.txt")!)).toBe("hello");
    expect(fakes.connectivity.current).toEqual({ online: true, kind: "wifi" });
    const resp = await fakes.http.request({ method: "get", url: "https://api.test/lists/inbox/todos", headers: [], body: null, timeoutMs: null });
    expect([resp.status, resp.headers[0]?.value]).toEqual([200, "application/json"]);
    await expect(fakes.http.request({ method: "get", url: "https://api.test/slow/x", headers: [], body: null, timeoutMs: null })).rejects.toMatchObject({ kind: "timeout" });
    expect(toHex(fakes.rng.bytes(8))).toBe("a0a39b71b74ace56");
  });

  it("names the path of the first bad value", () => {
    const path = (text: string): string => {
      try {
        parseSeed(text);
      } catch (error) {
        if (error instanceof SeedError) return error.path;
        throw error;
      }
      throw new Error("expected a SeedError");
    };
    expect(path('{"kv": {"a": 1}}')).toBe("kv.a");
    expect(path('{"http": [{"status": "x"}]}')).toBe("http[0].status");
    expect(path('{"http": [{"method": "fetch"}]}')).toBe("http[0].method");
    expect(path('{"connectivity": {"online": true, "kind": "5g"}}')).toBe("connectivity.kind");
    expect(path('{"lifecycle": 3}')).toBe("lifecycle");
    expect(path("[]")).toBe("$");
    expect(path('{"version": 2}')).toBe("version");
    expect(path("{")).toBe("$");
    expect(() => applySeed(createFakes(), parseSeed('{"fs": {"../x": "y"}}'))).toThrow(/fs\.\.\.\/x/);
  });
});
