import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { WireError } from "@undra/runtime/wire";
import { decodeServerMsg, encodeRestore, encodeResync, type ServerMsg } from "../src/proto.js";
import { hexOf, unhex } from "./helpers.js";

// The bytes the Rust server writes (crates/undra-transport/src/devtools/proto.rs writes this file).
const vectors = JSON.parse(readFileSync(new URL("./vectors.json", import.meta.url), "utf8")) as {
  server: { name: string; hex: string }[];
  client: { name: string; hex: string }[];
};

const get = (name: string): ServerMsg => {
  const v = vectors.server.find((x) => x.name === name);
  if (v === undefined) throw new Error(`no vector ${name}`);
  return decodeServerMsg(unhex(v.hex));
};

describe("what the server writes, the page reads", () => {
  it("decodes the welcome", () => {
    const m = get("welcome");
    expect(m).toMatchObject({
      t: "welcome",
      welcome: {
        protocol: 1,
        undraVersion: "1.2.0",
        schemaHash: 0x0123_4567_89ab_cdefn,
        platform: "rust",
        mode: "dev",
        coreEpoch: 77n,
        startedUnixMs: 1_790_000_000_000,
        ringSteps: 200,
        ringBytes: 32 << 20,
        ringStepBytes: 4 << 20,
        schemaJson: '{"records":[]}',
      },
    });
  });

  it("decodes the stores, with 64-bit handles as bigints", () => {
    expect(get("stores")).toEqual({
      t: "stores",
      stores: [
        { handle: 0x0000_0002_0000_0001n, typeId: 7 },
        { handle: 5n, typeId: 9 },
      ],
    });
  });

  it("decodes change-sets with their cause and delivery", () => {
    expect(get("change_set_commit_call")).toEqual({
      t: "changeSet",
      seq: 12,
      atMs: 3456,
      delivery: "commit",
      cause: { kind: "call", methodId: 0xdead_beef },
      payload: Uint8Array.of(1, 2, 3, 4),
    });
    expect(get("change_set_initial_restore")).toMatchObject({ delivery: "initial", cause: { kind: "restore", step: 3 }, payload: new Uint8Array(0) });
  });

  it("decodes steps, evictions, ports, counters, queries, travel results and the app", () => {
    expect(get("step")).toEqual({
      t: "step",
      step: { step: 4, throughSeq: 13, txn: 99n, atMs: 4001, bytes: 2048, stores: 3, restorable: true, restoredFrom: 2 },
    });
    expect(get("evicted")).toEqual({ t: "evicted", belowStep: 3 });
    expect(get("port_start")).toEqual({
      t: "port",
      record: { phase: "start", id: 8, portId: 0xaabb_ccdd, methodId: 0x1122_3344, atMs: 5000, args: Uint8Array.of(9, 8, 7) },
    });
    expect(get("port_end")).toEqual({
      t: "port",
      record: { phase: "end", id: 8, portId: 0xaabb_ccdd, methodId: 0x1122_3344, atMs: 5042, status: 1, latencyUs: 42_000, reply: Uint8Array.of(5) },
    });
    expect(get("stats")).toEqual({ t: "stats", json: '{"at_ms":1}' });
    expect(get("queries")).toEqual({ t: "queries", atMs: 6000, json: '{"entries":[]}' });
    expect(get("traveled")).toEqual({ t: "traveled", result: { requestId: 7, ok: true, step: 3, dropped: 1, message: "restored step 3" } });
    expect(get("app")).toEqual({ t: "app", connected: true, platform: "web" });
  });

  it("decodes every vector (none is left untested)", () => {
    for (const v of vectors.server) expect(() => decodeServerMsg(unhex(v.hex))).not.toThrow();
  });
});

describe("what the page writes, the server reads", () => {
  it("writes the bytes the server's decoder is tested with", () => {
    const hex = (name: string): string | undefined => vectors.client.find((x) => x.name === name)?.hex;
    expect(hexOf(encodeRestore(7, 3))).toBe(hex("restore"));
    expect(hexOf(encodeResync())).toBe(hex("resync"));
  });
});

describe("malformed messages are errors, never crashes", () => {
  it("rejects an empty message, an unknown tag, a truncated message and trailing bytes", () => {
    expect(() => decodeServerMsg(new Uint8Array(0))).toThrow(WireError);
    expect(() => decodeServerMsg(Uint8Array.of(0xee))).toThrow(WireError);
    const step = unhex(vectors.server.find((x) => x.name === "step")?.hex ?? "");
    expect(() => decodeServerMsg(step.slice(0, step.length - 2))).toThrow(WireError);
    expect(() => decodeServerMsg(Uint8Array.of(...step, 0))).toThrow(WireError);
  });

  it("survives random bytes", () => {
    let state = 0x9e3779b9;
    for (let len = 0; len < 150; len++) {
      const b = Uint8Array.from({ length: len }, () => {
        state ^= state << 13;
        state ^= state >>> 17;
        state ^= state << 5;
        return state & 0xff;
      });
      try {
        decodeServerMsg(b);
      } catch (e) {
        expect(e).toBeInstanceOf(WireError);
      }
    }
  });
});
