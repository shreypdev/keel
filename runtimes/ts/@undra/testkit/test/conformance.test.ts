import { FsError, HttpError, type HttpMethod } from "@undra/runtime";
import { describe, expect, it } from "vitest";
import { FakeClock, MemFs, MemKv, SeededRng, applySeed, createFakes, fromHex, parseSeed, toHex } from "../src/index.js";
import { fixture } from "./support/fixtures.js";

// testkit/conformance/fakes.json is what the Rust fakes answer (it is generated from them and checked in CI). Every kit replays it
// against its own fakes, so the four implementations cannot drift apart.

type Json = Record<string, unknown>;
const doc = JSON.parse(fixture("conformance/fakes.json")) as Record<string, Json[]>;

function errorOf(error: unknown): Json {
  if (error instanceof FsError.NotFound) return { error: "not_found" };
  if (error instanceof FsError.Denied) return { error: "denied" };
  if (error instanceof FsError.Io) return { error: "io", message: error.value };
  if (error instanceof HttpError.Network) return { error: "network", message: error.value };
  if (error instanceof HttpError.Timeout) return { error: "timeout" };
  if (error instanceof HttpError.Cancelled) return { error: "cancelled" };
  if (error instanceof HttpError.InvalidUrl) return { error: "invalid_url", message: error.value };
  throw error;
}

async function outcome<T>(run: () => Promise<T>, ok: (value: T) => unknown): Promise<unknown> {
  try {
    return ok(await run());
  } catch (error) {
    return errorOf(error);
  }
}

describe("the fakes conform to the Rust fakes", () => {
  it("SeededRng gives the same bytes for the same seed", () => {
    for (const c of doc["rng"]!) {
      const seed = typeof c["seed"] === "string" ? BigInt(c["seed"]) : BigInt(c["seed"] as number);
      const rng = new SeededRng(seed);
      const got = (c["fills"] as number[]).map((n) => toHex(rng.bytes(n)));
      expect(got).toEqual(c["results"]);
    }
  });

  it("FakeClock fires timers in deadline order with the clock at each deadline", () => {
    for (const c of doc["clock"]!) {
      const clock = new FakeClock(c["now_ms"] as number);
      const state = (): Json => ({ now_ms: clock.nowMs(), monotonic_ns: Number(clock.monotonicNs()) });
      for (const step of c["steps"] as Json[]) {
        if (step["op"] === "timer") clock.set(step["id"] as number, step["delay_ms"] as number, () => undefined);
        else if (step["op"] === "advance") {
          expect(clock.advance(step["ms"] as number)).toEqual(step["fired"]);
          expect(state()).toEqual(step["state"]);
        } else if (step["op"] === "set_now") {
          clock.setNowMs(step["ms"] as number);
          expect(state()).toEqual(step["state"]);
        }
      }
    }
  });

  it("MemKv stores bytes ordered by UTF-8, not UTF-16", async () => {
    for (const c of doc["store"]!) {
      const kv = new MemKv();
      for (const step of c["steps"] as Json[]) {
        if (step["op"] === "set") await kv.set(step["key"] as string, fromHex(step["value"] as string)!);
        else if (step["op"] === "delete") await kv.delete(step["key"] as string);
        else if (step["op"] === "list") expect(await kv.list(step["prefix"] as string)).toEqual(step["result"]);
        else if (step["op"] === "get") {
          const value = await kv.get(step["key"] as string);
          expect(value === null ? null : toHex(value)).toEqual(step["result"]);
        }
      }
    }
  });

  it("MemFs has the semantics the platform adapters share", async () => {
    for (const c of doc["fs"]!) {
      const fs = new MemFs();
      for (const step of c["steps"] as Json[]) {
        const path = (step["path"] ?? step["dir"]) as string;
        let got: unknown;
        if (step["op"] === "write") got = await outcome(() => fs.write(path, fromHex(step["data"] as string)!), () => null);
        else if (step["op"] === "read") got = await outcome(() => fs.read(path), toHex);
        else if (step["op"] === "delete") got = await outcome(() => fs.delete(path), () => null);
        else got = await outcome(() => fs.list(path), (names) => names);
        expect(got, `${String(step["op"])} ${path}`).toEqual(step["result"]);
      }
    }
  });

  it("FakeHttp answers the first matching rule and fails the rest", async () => {
    for (const c of doc["http"]!) {
      const fakes = createFakes();
      applySeed(fakes, parseSeed(JSON.stringify({ http: c["rules"] })));
      for (const r of c["requests"] as Json[]) {
        const request = { method: r["method"] as HttpMethod, url: r["url"] as string, headers: [], body: null, timeoutMs: null };
        const got = await outcome(
          () => fakes.http.request(request),
          (resp) => ({ status: resp.status, headers: resp.headers.map((h) => [h.name, h.value]), body: toHex(resp.body) }),
        );
        expect(got, `${String(r["method"])} ${String(r["url"])}`).toEqual(r["result"]);
      }
    }
  });
});
