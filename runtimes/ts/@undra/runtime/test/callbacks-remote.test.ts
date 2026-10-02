import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { type CallbackInterface, callbacks, lend } from "../src/callbacks.js";
import { UndraCore } from "../src/core.js";
import { Kind, PortStatus, UndraWriter, codecs, decodePortReply, decodeValue, encodePortCall, encodeValue } from "../src/wire/index.js";
import { SCHEMA } from "./support/fake-core.js";
import { FakeServer } from "./support/fake-websocket.js";
import { track } from "./support/harness.js";

// ADR-041 over the remote transport (`undra dev`): the core's calls into a host callback arrive as `PortCall`
// envelopes on the callback's port, the host answers an async one with a `PortReply` envelope and never answers a
// fire-and-forget one (port call id 0), and a dropped connection drops the registry's entries.

const PORT = 0x7100_0001;
const NOTE = 0x21;
const ASK = 0x22;

interface Listener {
  note(line: string): void;
  ask(question: string, signal: AbortSignal): Promise<boolean>;
}

const ListenerCallback: CallbackInterface<Listener> = {
  name: "Listener",
  portId: PORT,
  releaseInstance: 0x2e,
  cancelCall: 0x2f,
  methods: {
    [NOTE]: {
      name: "note",
      notify: (r) => {
        const line = r.readStr();
        return (impl) => impl.note(line);
      },
    },
    [ASK]: {
      name: "ask",
      call: (r) => {
        const question = r.readStr();
        return async (impl, signal) => encodeValue(codecs.bool, await impl.ask(question, signal));
      },
    },
  },
};

function args(instance: bigint, text: string): Uint8Array {
  const w = new UndraWriter();
  w.writeU64(instance);
  w.writeStr(text);
  return w.finish();
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("host callbacks over a remote core", () => {
  it("answer PortCall envelopes, never answer a fire-and-forget call, and are dropped with the connection", async () => {
    const server = new FakeServer();
    const loading = UndraCore.load({
      mode: "remote",
      url: "ws://127.0.0.1:7443",
      expectedSchemaHash: SCHEMA,
      webSocket: server.Socket,
      reconnect: { random: () => 0 },
      shared: false,
      adapters: { log: { log() {} }, http: null, timer: null },
    });
    await vi.advanceTimersByTimeAsync(0);
    const core = track(await loading);
    const heard: string[] = [];
    const listener: Listener = {
      note: (line) => heard.push(line),
      ask: (question) => Promise.resolve(question === "yes?"),
    };
    const instance = lend(core, listener, ListenerCallback);

    server.current.deliver(Kind.PortCall, encodePortCall({ portId: PORT, methodId: NOTE, portCallId: 0, args: args(instance, "hello") }));
    server.current.deliver(Kind.PortCall, encodePortCall({ portId: PORT, methodId: ASK, portCallId: 7, args: args(instance, "yes?") }));
    await vi.advanceTimersByTimeAsync(0);
    expect(heard).toEqual(["hello"]);
    const replies = server.current.sent(Kind.PortReply).map((e) => decodePortReply(e.payload));
    expect(replies, "one reply, for the async call only").toHaveLength(1);
    expect(replies[0]?.portCallId).toBe(7);
    expect(replies[0]?.status).toBe(PortStatus.Ok);
    expect(decodeValue(codecs.bool, replies[0]?.body ?? new Uint8Array(0))).toBe(true);

    server.current.serverClose(1006);
    expect(core.connection.peek().kind).toBe("reconnecting");
    expect(callbacks(core).liveCount, "the core that held them is out of reach").toBe(0);
  });
});
