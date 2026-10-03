import { expect, test } from "vitest";
import { SseError, type SseEvent } from "@undra/runtime";
import { OptInPortIds, fetchSse, ssePort } from "@undra/runtime/realtime";
import { sseFollow } from "@playground/core";
import { boot, bootWorker } from "../src/harness.js";
import { lastOn, useRealtimeServer } from "../src/realtime-server.js";
import { WAIT_TIMEOUT_MS, step, waitFor } from "../src/wait.js";

// S24 server-sent events (ADR-047): the opt-in Sse port through the platform's default adapter
// (`fetchSse()`: `fetch` with a body stream) against the shared realtime server: the feed parsed as
// the HTML standard says, a resume after an id, a reader that stops, typed refusals.

const server = useRealtimeServer();

/** The four events of /sse/feed: the comment and the event without data are not events; CRLF ends lines. */
const FEED: SseEvent[] = [
  { id: "1", event: "message", data: "one", retryMs: 1500 },
  { id: "2", event: "tick", data: "two\nlines", retryMs: null },
  { id: "2", event: "message", data: "three", retryMs: null },
  { id: "4", event: "message", data: "four", retryMs: null },
];

async function failure(run: () => Promise<unknown>): Promise<unknown> {
  try {
    await run();
  } catch (error) {
    return error;
  }
  throw new Error("expected the call to fail, but it succeeded");
}

test("S24 server-sent events", async () => {
  const HTTP = server().url;
  const { core } = await boot({ ports: { [OptInPortIds.Sse.portId]: ssePort(fetchSse()) } });

  await step("1. the feed: four events, then the end; no Last-Event-ID was sent", async () => {
    expect(await sseFollow(`${HTTP}/sse/feed`, null, 10, core)).toEqual({ events: FEED, ended: true });
    expect(lastOn(server(), "/sse/feed")?.headers["last-event-id"]).toBeUndefined();
  });

  await step("2. resume after id 2: three (id 2) and four (id 4); the server saw Last-Event-ID: 2", async () => {
    expect(await sseFollow(`${HTTP}/sse/feed`, "2", 10, core)).toEqual({ events: FEED.slice(2), ended: true });
    expect(lastOn(server(), "/sse/feed")?.headers["last-event-id"]).toBe("2");
  });

  await step("3. a reader that stops: two events and not ended; a hang read for none, and the server saw the client leave", async () => {
    expect(await sseFollow(`${HTTP}/sse/feed`, null, 2, core)).toEqual({ events: FEED.slice(0, 2), ended: false });
    expect(await sseFollow(`${HTTP}/sse/hang`, null, 0, core)).toEqual({ events: [], ended: false });
    // Bounded by WAIT_TIMEOUT_MS, a hang detector: the server never ends /sse/hang, and the port leaves it when the core closes
    // the subscription, on no timer, so a port that did not would stay; how soon the server sees it is the machine's.
    await waitFor("the server to see the client leave /sse/hang", () => lastOn(server(), "/sse/hang")?.clientClosed, { timeoutMs: WAIT_TIMEOUT_MS });
  });

  await step("4. typed failures: 204 and 500 are Refused with their status, text/html is Protocol", async () => {
    for (const code of [204, 500]) {
      const refused = await failure(() => sseFollow(`${HTTP}/sse/status?code=${code}`, null, 10, core));
      expect(refused, String(code)).toBeInstanceOf(SseError.Refused);
      expect((refused as SseError.Refused).status).toBe(code);
    }
    expect(await failure(() => sseFollow(`${HTTP}/sse/html`, null, 10, core))).toBeInstanceOf(SseError.Protocol);
  });
});

test("wasm-worker mode: the Sse port is served on the main thread (ADR-049 §2)", async () => {
  const { core } = await bootWorker({ ports: { [OptInPortIds.Sse.portId]: ssePort(fetchSse()) } });
  expect(await sseFollow(`${server().url}/sse/feed`, null, 10, core)).toEqual({ events: FEED, ended: true });
});
