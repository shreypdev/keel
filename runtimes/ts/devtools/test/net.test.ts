import { describe, expect, it } from "vitest";
import { Connection, backoffMs, type SocketLike } from "../src/net.js";
import type { ConnState } from "../src/state.js";

class FakeSocket implements SocketLike {
  binaryType = "";
  onopen: ((e: unknown) => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  onclose: ((e: { code: number }) => void) | null = null;
  onerror: ((e: unknown) => void) | null = null;
  sent: Uint8Array[] = [];
  closed = false;
  send(data: Uint8Array): void {
    this.sent.push(data);
  }
  close(): void {
    this.closed = true;
  }
}

function rig() {
  const sockets: FakeSocket[] = [];
  const timers: { fn: () => void; ms: number }[] = [];
  const states: ConnState[] = [];
  const messages: Uint8Array[] = [];
  const gaveUp: string[] = [];
  const conn = new Connection({
    url: "ws://x/devtools/ws?token=t",
    make: () => {
      const s = new FakeSocket();
      sockets.push(s);
      return s;
    },
    onMessage: (b) => messages.push(b),
    onState: (s) => states.push(s),
    onGiveUp: (r) => gaveUp.push(r),
    setTimer: (fn, ms) => {
      timers.push({ fn, ms });
      return timers.length;
    },
    clearTimer: () => undefined,
  });
  return { conn, sockets, timers, states, messages, gaveUp };
}

describe("the reconnect", () => {
  it("backs off from 250 ms to 5 s", () => {
    expect([1, 2, 3, 4, 5, 6, 7].map(backoffMs)).toEqual([250, 500, 1000, 2000, 4000, 5000, 5000]);
  });

  it("opens, delivers binary messages and sends only while open", () => {
    const r = rig();
    r.conn.start();
    expect(r.states).toEqual(["connecting"]);
    expect(r.conn.send(Uint8Array.of(1))).toBe(false);
    const s = r.sockets[0] as FakeSocket;
    s.onopen?.({});
    expect(r.states.at(-1)).toBe("open");
    s.onmessage?.({ data: Uint8Array.of(7, 7).buffer });
    s.onmessage?.({ data: "text is ignored" });
    expect(r.messages).toEqual([Uint8Array.of(7, 7)]);
    expect(r.conn.send(Uint8Array.of(1))).toBe(true);
    expect(s.sent).toEqual([Uint8Array.of(1)]);
  });

  it("reconnects after a drop (a rebuild replaces the runner) and counts from one again once it is open", () => {
    const r = rig();
    r.conn.start();
    (r.sockets[0] as FakeSocket).onopen?.({});
    (r.sockets[0] as FakeSocket).onclose?.({ code: 1001 });
    expect(r.states.at(-1)).toBe("reconnecting");
    expect(r.timers[0]?.ms).toBe(250);
    r.timers[0]?.fn();
    (r.sockets[1] as FakeSocket).onclose?.({ code: 1006 });
    expect(r.timers[1]?.ms).toBe(500);
    r.timers[1]?.fn();
    (r.sockets[2] as FakeSocket).onopen?.({});
    (r.sockets[2] as FakeSocket).onclose?.({ code: 1006 });
    expect(r.timers[2]?.ms).toBe(250);
  });

  it("gives up when it never got in: the token is wrong or the server is not there", () => {
    const r = rig();
    r.conn.start();
    for (let i = 0; i < 4; i++) {
      (r.sockets[i] as FakeSocket).onclose?.({ code: 1006 });
      if (i < 3) r.timers[i]?.fn();
    }
    expect(r.states.at(-1)).toBe("closed");
    expect(r.gaveUp[0]).toMatch(/token/);
    expect(r.sockets.length).toBe(4);
  });

  it("does not retry when the server has no room for another page", () => {
    const r = rig();
    r.conn.start();
    (r.sockets[0] as FakeSocket).onopen?.({});
    (r.sockets[0] as FakeSocket).onclose?.({ code: 1013 });
    expect(r.states.at(-1)).toBe("closed");
    expect(r.timers.length).toBe(0);
  });

  it("stops for good when asked", () => {
    const r = rig();
    r.conn.start();
    r.conn.stop();
    expect((r.sockets[0] as FakeSocket).closed).toBe(true);
    (r.sockets[0] as FakeSocket).onclose?.({ code: 1000 });
    expect(r.timers.length).toBe(0);
  });
});
