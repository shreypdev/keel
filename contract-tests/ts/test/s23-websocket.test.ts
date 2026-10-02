import { expect, test } from "vitest";
import { WsError, type WsMessage } from "@undra/runtime";
import { OptInPortIds, nodeWebSocket, webSocketPort } from "@undra/runtime/realtime";
import { Live, wsEcho } from "@playground/core";
import { boot, bootWorker } from "../src/harness.js";
import { lastOn, useRealtimeServer } from "../src/realtime-server.js";
import { sleep, step, waitFor } from "../src/wait.js";

// S23 WebSocket (ADR-047): the opt-in WebSocket port through the platform's default adapter against
// the shared realtime server: echo, subprotocol and headers, the core's credit bounding the
// read-ahead, typed ends, and a connection nobody closes closed going away (1001).
//
// The adapter is `nodeWebSocket()`, the runtime's Node adapter (NOTES.md: Node's global WebSocket
// hides a refusal's status and refuses to send 1001 from script, which steps 4 and 5 need).

const server = useRealtimeServer();

const text = (value: string): WsMessage => ({ kind: "text", value });
const texts = (from: number, to: number): WsMessage[] => Array.from({ length: to - from }, (_, i) => text(String(from + i)));

/** What `run` rejected with; the step fails if it resolved. */
async function failure(run: () => Promise<unknown>): Promise<unknown> {
  try {
    await run();
  } catch (error) {
    return error;
  }
  throw new Error("expected the call to fail, but it succeeded");
}

/** Waits for the server to record the client's close frame on `path`, and returns `[code, reason]`. */
async function closeSeen(path: string, timeoutMs = 5000): Promise<[number | null, string | null]> {
  const seen = await waitFor(`the server to see the client close ${path}`, () => {
    const c = lastOn(server(), path);
    return c !== undefined && c.closeCode !== null && c;
  }, { timeoutMs });
  return [seen.closeCode, seen.closeReason];
}

test("S23 WebSocket", async () => {
  const WS = server().wsUrl;
  const { core } = await boot({ ports: { [OptInPortIds.WebSocket.portId]: webSocketPort(nodeWebSocket()) } });

  await step("1. echo: three messages come back in order; the client closed with (1000, done)", async () => {
    const sent: WsMessage[] = [text("a"), { kind: "binary", value: Uint8Array.of(1, 2, 3) }, text("é")];
    expect(await wsEcho(`${WS}/ws/echo`, sent, core)).toEqual(sent);
    expect(await closeSeen("/ws/echo")).toEqual([1000, "done"]);
  });

  const live = await Live.create(core);

  await step("2. subprotocol and headers; send and read; disconnect (4000, bye)", async () => {
    expect(await live.connect(`${WS}/ws/headers`, ["v2", "v1"], [{ name: "X-Token", value: "t" }])).toBe("v2");
    const [upgrade] = await live.read(1);
    expect(upgrade?.kind).toBe("text");
    expect(JSON.parse((upgrade as { value: string }).value)).toMatchObject({ "x-token": "t" });
    await live.send(text("ping"));
    expect(await live.read(1)).toEqual([text("ping")]);
    await live.disconnect(4000, "bye");
    expect(await closeSeen("/ws/headers")).toEqual([4000, "bye"]);
  });

  await step("3. credit: a core that stopped reading pulls no more; then everything, in order, and the server's close", async () => {
    await live.connect(`${WS}/ws/flood?n=1000`, [], []);
    expect(await live.read(5)).toEqual(texts(0, 5));
    await sleep(200);
    expect(await live.pulls(), "at most two pulls while the core read nothing").toBeLessThanOrEqual(2n);
    expect(await live.read(995)).toEqual(texts(5, 1000));
    const end = await failure(() => live.read(1));
    expect(end).toBeInstanceOf(WsError.Closed);
    expect(end).toMatchObject({ code: 1000, reason: "end" });
  });

  await step("4. typed ends: a refused upgrade (401), the peer's close (4001, kicked), a drop", async () => {
    const refused = await failure(() => wsEcho(`${WS}/ws/deny?status=401`, [text("x")], core));
    expect(refused).toBeInstanceOf(WsError.Refused);
    expect((refused as WsError.Refused).status, "Node reports the status").toBe(401);

    await live.connect(`${WS}/ws/close?code=4001&reason=kicked`, [], []);
    expect(await live.read(1)).toEqual([text("hello")]);
    const kicked = await failure(() => live.read(1));
    expect(kicked).toBeInstanceOf(WsError.Closed);
    expect(kicked).toMatchObject({ code: 4001, reason: "kicked" });

    await live.connect(`${WS}/ws/drop`, [], []);
    expect(await live.read(1)).toEqual([text("hello")]);
    expect(await failure(() => live.read(1))).toBeInstanceOf(WsError.Network);
  });

  await step("5. a connection nobody closes: within 1 s the server saw the client close with 1001", async () => {
    await live.connect(`${WS}/ws/stall`, [], []);
    await live.abandon();
    const [code] = await closeSeen("/ws/stall", 1000);
    expect(code).toBe(1001);
  });
});

test("wasm-worker mode: the WebSocket port is served on the main thread (ADR-049 §2)", async () => {
  const { core } = await bootWorker({ ports: { [OptInPortIds.WebSocket.portId]: webSocketPort(nodeWebSocket()) } });
  const sent: WsMessage[] = [text("from the worker"), { kind: "binary", value: Uint8Array.of(9) }];
  expect(await wsEcho(`${server().wsUrl}/ws/echo`, sent, core)).toEqual(sent);
});
