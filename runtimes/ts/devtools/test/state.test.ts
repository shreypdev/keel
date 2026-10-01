import { describe, expect, it } from "vitest";
import type { Cause, ServerMsg, StepInfo, Welcome } from "../src/proto.js";
import { DevtoolsState } from "../src/state.js";
import { SCHEMA, changeSet, full, i32Bytes, listBytes, patch, bytes } from "./helpers.js";

const COUNTER = 5n;
const TODOS = 6n;

const welcome = (epoch = 1n): Welcome => ({
  protocol: 1,
  undraVersion: "1.2.0",
  schemaHash: 1n,
  platform: "rust",
  mode: "dev",
  coreEpoch: epoch,
  startedUnixMs: 1_790_000_000_000,
  ringSteps: 200,
  ringBytes: 1 << 20,
  ringStepBytes: 1 << 20,
  schemaJson: JSON.stringify(SCHEMA),
});

const step = (n: number, through: number, extra: Partial<StepInfo> = {}): ServerMsg => ({
  t: "step",
  step: { step: n, throughSeq: through, txn: BigInt(n), atMs: n * 10, bytes: 10, stores: 2, restorable: true, restoredFrom: 0, ...extra },
});

const commit = (seq: number, payload: Uint8Array, cause: Cause = { kind: "other" }): ServerMsg => ({
  t: "changeSet",
  seq,
  atMs: seq * 10,
  delivery: "commit",
  cause,
  payload,
});

function started(): DevtoolsState {
  const s = new DevtoolsState();
  s.onMessage({ t: "welcome", welcome: welcome() });
  s.onMessage({ t: "stores", stores: [{ handle: COUNTER, typeId: 10 }, { handle: TODOS, typeId: 11 }] });
  return s;
}

describe("the timeline", () => {
  it("lists a commit with its cause named from the schema and its changes decoded", () => {
    const s = started();
    s.onMessage(commit(1, changeSet(1n, [full(COUNTER, 0, i32Bytes(4))]), { kind: "call", methodId: 101 }));
    const e = s.timeline[0];
    expect(e).toMatchObject({ kind: "commit", label: "Counter.add", txn: 1n, step: undefined, changes: [{ signal: "count", after: 4 }] });
    expect(s.dirty.has(`${COUNTER}:0`)).toBe(true);
  });

  it("names the other causes", () => {
    const s = started();
    s.onMessage(commit(1, changeSet(1n, [full(COUNTER, 0, i32Bytes(1))])));
    s.onMessage(commit(2, changeSet(2n, [full(COUNTER, 0, i32Bytes(2))]), { kind: "restore", step: 3 }));
    s.onMessage(commit(3, changeSet(3n, [full(COUNTER, 0, i32Bytes(3))]), { kind: "call", methodId: 0x7777 }));
    expect(s.timeline.map((e) => (e.kind === "commit" ? e.label : ""))).toEqual(["method 0x00007777", "restore to step 3", "task, timer or stream"]);
  });

  it("does not list initial values, but applies them", () => {
    const s = started();
    s.onMessage({ t: "changeSet", seq: 1, atMs: 1, delivery: "initial", cause: { kind: "other" }, payload: changeSet(1n, [full(COUNTER, 0, i32Bytes(9))]) });
    expect(s.timeline).toEqual([]);
    expect(s.mirror?.stores.get(COUNTER)?.signals.get(0)?.value).toBe(9);
    expect(s.dirty.size).toBe(0);
  });

  it("gives the commits since the last step to the step that records them", () => {
    const s = started();
    s.onMessage(commit(1, changeSet(1n, [full(COUNTER, 0, i32Bytes(1))])));
    s.onMessage(step(1, 1));
    s.onMessage(commit(2, changeSet(2n, [full(COUNTER, 0, i32Bytes(2))])));
    s.onMessage(commit(3, changeSet(3n, [full(COUNTER, 0, i32Bytes(3))])));
    s.onMessage(step(2, 3));
    const steps = s.timeline.map((e) => (e.kind === "commit" ? e.step : -1));
    expect(steps).toEqual([2, 2, 1]);
    expect(s.currentStep).toBe(2);
    // A step that arrives twice (a page that attaches is told the ones it may have) is applied once.
    s.onMessage(step(2, 3));
    expect(s.steps.length).toBe(2);
  });

  it("drops evicted steps from what can be travelled to", () => {
    const s = started();
    for (let i = 1; i <= 4; i++) s.onMessage(step(i, i));
    s.onMessage({ t: "evicted", belowStep: 3 });
    expect(s.restorableSteps.map((x) => x.step)).toEqual([3, 4]);
  });

  it("does not offer a step whose state was too big to keep", () => {
    const s = started();
    s.onMessage(step(1, 1, { restorable: false }));
    s.onMessage(step(2, 2));
    expect(s.restorableSteps.map((x) => x.step)).toEqual([2]);
  });

  it("reports a change-set it cannot read without stopping", () => {
    const s = started();
    s.onMessage(commit(1, Uint8Array.of(1, 2)));
    expect(s.notices[0]).toMatch(/could not be decoded/);
    s.onMessage(commit(2, changeSet(2n, [full(COUNTER, 0, i32Bytes(2))])));
    expect(s.timeline.length).toBe(1);
  });

  it("keeps a keyed list's rows through patches", () => {
    const s = started();
    s.onMessage(commit(1, changeSet(1n, [full(TODOS, 0, listBytes([[1, "a"], [2, "b"]]))])));
    s.onMessage(commit(2, changeSet(2n, [patch(TODOS, 0, [["insert", 2, 3, "c"]])])));
    expect((s.mirror?.stores.get(TODOS)?.signals.get(0)?.value as unknown[]).length).toBe(3);
    expect(s.timeline[0]).toMatchObject({ changes: [{ op: "patch" }] });
  });
});

describe("a core that was replaced", () => {
  it("marks the break in the timeline and forgets steps that cannot be travelled to", () => {
    const s = started();
    s.onMessage(commit(1, changeSet(1n, [full(COUNTER, 0, i32Bytes(1))])));
    s.onMessage(step(1, 1));
    s.onMessage({ t: "welcome", welcome: welcome(2n) });
    expect(s.steps).toEqual([]);
    expect(s.timeline[0]).toMatchObject({ kind: "divider" });
    expect(s.timeline[1]).toMatchObject({ kind: "commit", epoch: 0 });
    expect(s.epoch).toBe(1);
    s.onMessage({ t: "stores", stores: [{ handle: COUNTER, typeId: 10 }] });
    s.onMessage(commit(1, changeSet(1n, [full(COUNTER, 0, i32Bytes(7))])));
    expect(s.timeline[0]).toMatchObject({ kind: "commit", epoch: 1 });
  });

  it("only notes a reconnection to the same core", () => {
    const s = started();
    s.onMessage(commit(1, changeSet(1n, [full(COUNTER, 0, i32Bytes(1))])));
    s.onMessage({ t: "welcome", welcome: welcome(1n) });
    expect(s.epoch).toBe(0);
    expect(s.timeline[0]).toMatchObject({ kind: "divider" });
  });
});

describe("time travel", () => {
  it("remembers the answer to a request", () => {
    const s = started();
    const id = s.nextTravel();
    expect(s.traveling).toBe(true);
    s.onMessage({ t: "traveled", result: { requestId: id, ok: true, step: 2, dropped: 0, message: "restored step 2" } });
    expect(s.traveling).toBe(false);
    expect(s.travel).toEqual({ ok: true, step: 2, message: "restored step 2" });
  });
});

describe("port calls", () => {
  it("joins the end of a call to its start and names the method", () => {
    const s = started();
    const args = bytes((w) => { w.writeStr("https://x"); w.writeU8(2); });
    s.onMessage({ t: "port", record: { phase: "start", id: 1, portId: 300, methodId: 301, atMs: 10, args } });
    expect(s.ports[0]).toMatchObject({ name: "Http.send", status: undefined });
    s.onMessage({ t: "port", record: { phase: "end", id: 1, portId: 300, methodId: 301, atMs: 52, status: 0, latencyUs: 42_000, reply: bytes((w) => w.writeU16(200)) } });
    expect(s.ports.length).toBe(1);
    expect(s.ports[0]).toMatchObject({ status: 0, latencyUs: 42_000 });
  });

  it("shows a call whose start it never saw", () => {
    const s = started();
    s.onMessage({ t: "port", record: { phase: "end", id: 9, portId: 300, methodId: 301, atMs: 5, status: 2, latencyUs: 1, reply: new Uint8Array(0) } });
    expect(s.ports[0]).toMatchObject({ name: "Http.send", status: 2 });
  });
});

describe("the query cache", () => {
  const doc = (entries: object[]): ServerMsg => ({ t: "queries", atMs: 1, json: JSON.stringify({ online: true, pending_mutations: 0, entries }) });
  const entry = (extra: object) => ({ query_id: 400, key: "todos/0", status: "fetching", fetching: true, observers: 1, invalidated: false, failed: false, layers: 0, updated_at: null, data: null, data_len: 0, error: null, error_len: 0, ...extra });

  it("decodes the cached value by the query's return type and logs what happened to an entry", () => {
    const s = started();
    s.onMessage(doc([entry({})]));
    const hex = [...listBytes([[1, "milk"]])].map((b) => b.toString(16).padStart(2, "0")).join("");
    s.onMessage(doc([entry({ status: "success", fetching: false, data: hex, data_len: hex.length / 2, updated_at: 5 })]));
    expect(s.queries.rows[0]).toMatchObject({ name: "todos_page", status: "success", data: [{ id: 1, title: "milk", done: false }] });
    s.onMessage(doc([entry({ status: "success", fetching: false, invalidated: true })]));
    s.onMessage(doc([]));
    expect(s.queryLog.map((e) => e.event).reverse()).toEqual(["new", "fetching", "success", "invalidated", "removed"]);
  });

  it("copes with a value it cannot decode", () => {
    const s = started();
    s.onMessage(doc([entry({ status: "success", fetching: false, data: "ff", data_len: 1 })]));
    expect(s.queries.rows[0]?.data).toBeUndefined();
  });
});

describe("counters and the app", () => {
  it("keeps the last samples", () => {
    const s = started();
    for (let i = 0; i < 40; i++) s.onMessage({ t: "stats", json: JSON.stringify({ at_ms: i * 1000, core: { tasks: i }, server: { commits: i } }) });
    expect(s.stats.length).toBe(30);
    expect(s.stats.at(-1)?.core["tasks"]).toBe(39);
    s.onMessage({ t: "stats", json: "not json" });
    expect(s.notices[0]).toMatch(/did not parse/);
  });
  it("knows whether an app is attached", () => {
    const s = started();
    s.onMessage({ t: "app", connected: true, platform: "web" });
    expect(s.app).toEqual({ connected: true, platform: "web" });
  });
  it("tells its listeners what changed, once per message", () => {
    const s = started();
    const seen: string[][] = [];
    s.onChange((what) => seen.push([...what].sort()));
    s.onMessage(commit(1, changeSet(1n, [full(COUNTER, 0, i32Bytes(1))])));
    expect(seen).toEqual([["stores", "timeline"]]);
    s.setConn("reconnecting");
    expect(seen.at(-1)).toEqual(["conn"]);
  });
});
