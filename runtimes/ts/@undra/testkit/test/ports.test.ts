import { PortIds, UndraPortError, UndraWriter, kvPort, type PortImpl } from "@undra/runtime";
import { describe, expect, it } from "vitest";
import { MemKv, PortRecorder, Replayer, ReplayFailure, parseRecording, toHex } from "../src/index.js";
import { fixture } from "./support/fixtures.js";

const kv = PortIds.Kv;
const keyArgs = (key: string): Uint8Array => {
  const w = new UndraWriter();
  w.writeStr(key);
  return w.finish();
};
const setArgs = (key: string, value: Uint8Array): Uint8Array => {
  const w = new UndraWriter();
  w.writeStr(key);
  w.writeBytes(value);
  return w.finish();
};

describe("PortRecorder and Replayer", () => {
  it("record a session's port traffic and replay it in order", async () => {
    let now = 1_000;
    const recorder = new PortRecorder({ schemaHash: 0x2an, now: () => now, platform: "web" });
    const real = new MemKv();
    const ports = recorder.wrapAll({ [kv.portId]: kvPort(real) });
    const methods = ports[kv.portId]!.methods;
    await methods[kv.set]!(setArgs("a", Uint8Array.of(1, 2)));
    now = 1_040;
    const got = await methods[kv.get]!(keyArgs("a"));
    expect(toHex(got)).toBe("01020000000102"); // Some(Bytes [1, 2])

    const text = recorder.toJson();
    const recording = parseRecording(text);
    expect(recording.platform).toBe("web");
    expect(recording.source).toBe("adapters");
    expect(recording.events.map((e) => [e.t, e.kind])).toEqual([
      [0, "port_call"],
      [0, "port_reply"],
      [40, "port_call"],
      [40, "port_reply"],
    ]);
    expect(text).toContain('"name":"Kv.set"');

    // A fresh run answers from the recording, with no store behind it.
    const replayer = new Replayer(recording);
    const replayed = replayer.ports()[kv.portId]!.methods;
    expect(await replayed[kv.set]!(setArgs("a", Uint8Array.of(1, 2)))).toEqual(new Uint8Array(0));
    expect(toHex(await replayed[kv.get]!(keyArgs("a")))).toBe(toHex(got));
    expect(() => replayer.finish()).not.toThrow();
  });

  it("report a call that deviates as a typed error and consume nothing", async () => {
    const recorder = new PortRecorder({ schemaHash: 1n, now: () => 0 });
    const ports = recorder.wrapAll({ [kv.portId]: kvPort(new MemKv()) });
    await ports[kv.portId]!.methods[kv.get]!(keyArgs("a"));
    const replayer = new Replayer(recorder.record());
    const methods = replayer.ports()[kv.portId]!.methods;
    expect(() => methods[kv.get]!(keyArgs("other"))).toThrow(/other arguments/);
    expect(() => methods[kv.set]!(keyArgs("a"))).toThrow(/recording has Kv.get next/);
    expect(replayer.errors().map((e) => e.kind)).toEqual(["mismatch", "mismatch"]);
    expect(replayer.errors()[0]).toMatchObject({ argsDiffer: true, called: "Kv.get", nth: 0 });
    // Nothing was consumed: the right call still matches, and then the port is used up.
    expect(await methods[kv.get]!(keyArgs("a"))).toBeInstanceOf(Uint8Array);
    expect(() => methods[kv.get]!(keyArgs("a"))).toThrow(/used up/);
    expect(replayer.errors().map((e) => e.kind)).toEqual(["mismatch", "mismatch", "exhausted"]);
    expect(() => replayer.finish()).toThrow(ReplayFailure);
  });

  it("leave recorded calls that were never made as unconsumed", () => {
    const replayer = new Replayer(parseRecording(fixture("fixtures/ports-remote-todos.json")));
    expect(replayer.remaining).toBe(3);
    const problems = replayer.problems();
    expect(problems.every((p) => p.kind === "unconsumed")).toBe(true);
    expect(problems.map((p) => (p.kind === "unconsumed" ? p.next : ""))).toEqual(expect.arrayContaining(["Clock.now_ms", "Rng.fill", "Http.request"]));
    try {
      replayer.finish();
    } catch (error) {
      expect(error).toBeInstanceOf(ReplayFailure);
      expect((error as ReplayFailure).message).toContain("never made");
    }
  });

  it("can ignore arguments that change from run to run", async () => {
    const recorder = new PortRecorder({ schemaHash: 1n, now: () => 0 });
    await recorder.wrapAll({ [kv.portId]: kvPort(new MemKv()) })[kv.portId]!.methods[kv.get]!(keyArgs("a"));
    const replayer = new Replayer(recorder.record(), { args: "ignore" });
    await replayer.ports()[kv.portId]!.methods[kv.get]!(keyArgs("generated-id-7"));
    expect(replayer.errors()).toEqual([]);
  });

  it("replay a recorded typed error and an unavailable answer", async () => {
    const failing: PortImpl = {
      sync: false,
      methods: {
        1: () => {
          throw new UndraPortError(Uint8Array.of(7));
        },
        2: () => {
          throw new Error("no such port");
        },
      },
    };
    const recorder = new PortRecorder({ schemaHash: 1n, now: () => 0 });
    const wrapped = recorder.wrap(99, failing);
    await expect(Promise.resolve().then(() => wrapped.methods[1]!(new Uint8Array(0)))).rejects.toBeInstanceOf(UndraPortError);
    await expect(Promise.resolve().then(() => wrapped.methods[2]!(new Uint8Array(0)))).rejects.toThrow("no such port");
    const replayer = new Replayer(parseRecording(recorder.toJson()));
    const methods = replayer.ports()[99]!.methods;
    try {
      methods[1]!(new Uint8Array(0));
      expect.unreachable();
    } catch (error) {
      expect(error).toBeInstanceOf(UndraPortError);
      expect((error as UndraPortError).body).toEqual(Uint8Array.of(7));
    }
    expect(() => methods[2]!(new Uint8Array(0))).toThrow(/unavailable/);
    expect(() => replayer.finish()).not.toThrow();
  });
});
