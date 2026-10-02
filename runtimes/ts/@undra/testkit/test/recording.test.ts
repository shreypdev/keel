import { describe, expect, it } from "vitest";
import { RecordingError, parseRecording, standardName, writeRecording, portId, methodId } from "../src/index.js";
import { fixture } from "./support/fixtures.js";

describe("the recording format", () => {
  it.each(["recording-all-kinds.json", "session-todos.json", "ports-remote-todos.json"])("reads %s and writes it back byte for byte", (name) => {
    const text = fixture(`fixtures/${name}`);
    expect(writeRecording(parseRecording(text))).toBe(text);
  });

  it("reads every kind with its typed fields", () => {
    const r = parseRecording(fixture("fixtures/recording-all-kinds.json"));
    expect(r.schemaHash).toBe(0xdeadbeefn);
    expect(r.platform).toBe("ios");
    const kinds = new Set(r.events.map((e) => e.kind));
    expect([...kinds].sort()).toEqual(
      ["call", "cancel", "change_set", "event", "observe", "port_call", "port_reply", "release", "reply", "stream_item", "timer_fired"].sort(),
    );
    const page = r.events.find((e) => e.kind === "call" && e.target.kind === "page");
    expect(page).toMatchObject({ target: { kind: "page", handle: 0x2_0000_0003n, offset: 0, limit: 50 } });
    const observe = r.events.find((e) => e.kind === "observe");
    expect(observe).toMatchObject({ signal: 0xffff_ffff, on: true });
  });

  it("names the standard ports and only them", () => {
    expect(standardName(portId("Http"), methodId("Http", "request"))).toBe("Http.request");
    expect(standardName(portId("SecureStore"), methodId("SecureStore", "get"))).toBe("SecureStore.get");
    expect(standardName(1, 2)).toBeUndefined();
  });

  it("escapes strings only where JSON requires, like the other writers", () => {
    const text = writeRecording({ schemaHash: 1n, source: 'a"b\\c\nd\u0001é', events: [] });
    expect(text).toContain('"source": "a\\"b\\\\c\\nd\\u0001é"');
    expect(parseRecording(text).source).toBe('a"b\\c\nd\u0001é');
    expect(text.endsWith('"events": []\n}\n')).toBe(true);
  });

  it("refuses what it cannot read with an error that names the field", () => {
    const head = '{"format":"undra.recording","version":1,"schema_hash":"0x1","source":"t","events":';
    const fails = (text: string): RecordingError => {
      try {
        parseRecording(text);
      } catch (error) {
        if (error instanceof RecordingError) return error;
        throw error;
      }
      throw new Error("expected a RecordingError");
    };
    expect(fails("nope").message).toContain("not valid JSON");
    expect(fails('{"format":"other","version":1}').message).toContain("not a recording");
    expect(fails('{"format":"undra.recording","version":2}').message).toContain("version 2");
    const bad = fails(`${head}[{"t":0,"kind":"cancel","call":"x"}]}`);
    expect([bad.event, bad.field]).toEqual([0, "call"]);
    expect(fails(`${head}[{"t":0,"kind":"warp"}]}`).field).toBe("kind");
    expect(fails(`${head}[{"t":0,"kind":"reply","call":1,"status":"ok","body":"zz"}]}`).field).toBe("body");
    expect(fails(`${head}[{"t":0,"kind":"release","handle":"7"}]}`).field).toBe("handle");
  });
});
