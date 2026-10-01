import { UndraIds, Todos } from "@playground/core";
import { CallTarget, UndraSchemaMismatchError } from "@undra/runtime";
import { afterEach, describe, expect, it } from "vitest";
import { RecordedCore, type Recording } from "../src/index.js";
import { fixture } from "./support/fixtures.js";

const cores: RecordedCore[] = [];
afterEach(() => {
  for (const c of cores.splice(0)) c.close();
});
const load = async (recording: Recording | string, options: Partial<Parameters<typeof RecordedCore.load>[1]> = {}): Promise<RecordedCore> => {
  const core = await RecordedCore.load(recording, { expectedSchemaHash: UndraIds.schemaHash, shared: false, ...options });
  cores.push(core);
  return core;
};
const titles = (todos: Todos): string[] => todos.todos.peek().map((t) => t.title);

describe("RecordedCore replays a recorded session of the playground's Todos store", () => {
  it("shows the state the recording first saw, then follows the playhead", async () => {
    const recorded = await load(fixture("fixtures/session-todos.json"));
    expect(recorded.playhead).toBe(100);
    expect(recorded.durationMs).toBe(500);
    const todos = await Todos.create(recorded.core);
    expect(titles(todos)).toEqual([]);
    expect(todos.remaining.peek()).toBe(0);

    await recorded.advance(100);
    expect(titles(todos)).toEqual(["Buy milk"]);
    await recorded.advance(200);
    expect(titles(todos)).toEqual(["Buy milk", "Walk the dog", "Write the docs"]);
    expect(todos.remaining.peek()).toBe(3);

    await recorded.playAll();
    expect(todos.todos.peek().map((t) => t.done)).toEqual([true, false, false]);
    expect(todos.remaining.peek()).toBe(2);
    expect(recorded.playhead).toBe(500);
  });

  it("starts anywhere: a store observed late is brought up to the playhead", async () => {
    const recorded = await load(fixture("fixtures/session-todos.json"), { startAtMs: 400 });
    const todos = await Todos.create(recorded.core);
    expect(titles(todos)).toEqual(["Buy milk", "Walk the dog", "Write the docs"]);
    expect(todos.remaining.peek()).toBe(3);
  });

  it("refuses a call it has no recording for, with a typed failure that names it", async () => {
    const recorded = await load(fixture("fixtures/session-todos.json"));
    await Todos.create(recorded.core);
    // The recording holds one constructor reply; a second one has none left.
    await expect(Todos.create(recorded.core)).rejects.toMatchObject({ kind: "refused" });
    await expect(Todos.create(recorded.core)).rejects.toThrow(/no reply for constructor/);
  });

  it("can repeat the last reply instead of refusing", async () => {
    const recorded = await load(fixture("fixtures/session-todos.json"), { exhausted: "repeat_last" });
    const todos = await Todos.create(recorded.core);
    const titlesAdded = [];
    for (let i = 0; i < 5; i++) titlesAdded.push((await todos.add("x")).title);
    expect(titlesAdded).toEqual(["Buy milk", "Walk the dog", "Write the docs", "Write the docs", "Write the docs"]);
  });

  it("answers a method call from the recorded reply, whatever the arguments, in order", async () => {
    const recorded = await load(fixture("fixtures/session-todos.json"));
    const todos = await Todos.create(recorded.core);
    const first = await todos.add("anything at all");
    expect(first.title).toBe("Buy milk");
    expect((await todos.add("x")).title).toBe("Walk the dog");
  });

  it("refuses a recording of another schema", async () => {
    await expect(RecordedCore.load(fixture("fixtures/session-todos.json"), { expectedSchemaHash: 1n })).rejects.toBeInstanceOf(UndraSchemaMismatchError);
  });

  it("replays a stream's items with the call id of the live call", async () => {
    const hash = UndraIds.schemaHash;
    const recording: Recording = {
      schemaHash: hash,
      source: "hand",
      events: [
        { t: 0, kind: "call", target: { kind: "function", method: 5 }, call: 9, args: new Uint8Array(0) },
        { t: 0, kind: "reply", call: 9, status: "stream_opened", body: new Uint8Array(0) },
        { t: 1, kind: "stream_item", call: 9, flag: "item", body: Uint8Array.of(1, 0, 0, 0) },
        { t: 2, kind: "stream_item", call: 9, flag: "item", body: Uint8Array.of(2, 0, 0, 0) },
        { t: 3, kind: "stream_item", call: 9, flag: "end", body: new Uint8Array(0) },
      ],
    };
    const recorded = await load(recording);
    const seen: number[] = [];
    for await (const item of recorded.core.stream({ target: CallTarget.FreeFunction }, 5, new Uint8Array(0))) seen.push(new DataView(item.buffer, item.byteOffset).getInt32(0, true));
    expect(seen).toEqual([1, 2]);
  });
});
