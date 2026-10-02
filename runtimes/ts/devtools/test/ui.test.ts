// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DevtoolsState } from "../src/state.js";
import { mount } from "../src/ui/shell.js";
import type { ServerMsg } from "../src/proto.js";
import { SCHEMA, bytes, changeSet, full, i32Bytes, listBytes, patch } from "./helpers.js";

const COUNTER = 5n;
const TODOS = 6n;

function page(): { state: DevtoolsState; root: HTMLElement; restored: number[] } {
  document.body.innerHTML = '<div id="app"></div>';
  const root = document.getElementById("app") as HTMLElement;
  const state = new DevtoolsState();
  const restored: number[] = [];
  mount(root, state, { restore: (s) => restored.push(s) });
  state.setConn("open");
  state.onMessage({
    t: "welcome",
    welcome: { protocol: 1, undraVersion: "1.2.0", schemaHash: 0xabcdef0123456789n, platform: "rust", mode: "dev", coreEpoch: 1n, startedUnixMs: 1_790_000_000_000, ringSteps: 200, ringBytes: 1 << 20, ringStepBytes: 1 << 20, schemaJson: JSON.stringify(SCHEMA) },
  });
  state.onMessage({ t: "stores", stores: [{ handle: COUNTER, typeId: 10 }, { handle: TODOS, typeId: 11 }] });
  return { state, root, restored };
}

const send = (s: DevtoolsState, msg: ServerMsg): void => s.onMessage(msg);
const commit = (seq: number, payload: Uint8Array, methodId = 101): ServerMsg => ({ t: "changeSet", seq, atMs: seq * 10, delivery: "commit", cause: { kind: "call", methodId }, payload });
const step = (n: number, through: number, restoredFrom = 0): ServerMsg => ({ t: "step", step: { step: n, throughSeq: through, txn: BigInt(n), atMs: n * 10, bytes: 20, stores: 2, restorable: true, restoredFrom } });

beforeEach(() => {
  vi.stubGlobal("requestAnimationFrame", (cb: () => void) => {
    cb();
    return 0;
  });
  vi.stubGlobal("matchMedia", () => ({ matches: false }));
});

describe("the page", () => {
  it("shows the core, the connection and the stores with their signals", () => {
    const { state, root } = page();
    send(state, commit(1, changeSet(1n, [full(COUNTER, 0, i32Bytes(5)), full(TODOS, 1, bytes((w) => w.writeU16(0)))])));
    const text = root.textContent ?? "";
    expect(text).toContain("rust · dev · schema 0xabcdef01");
    expect(text).toContain("connected");
    expect(text).toContain("Counter");
    expect(text).toContain("count");
    expect(root.querySelector('[data-signal="5:0"]')?.textContent).toContain("5");
    expect(root.querySelector('[data-signal="5:0"]')?.className).toContain("flash");
    expect(root.querySelector('[data-signal="5:1"]')?.textContent).toContain("computed");
  });

  it("shows a keyed list as a table and cuts it at a page of rows", () => {
    const { state, root } = page();
    const rows: [number, string][] = Array.from({ length: 100 }, (_, i) => [i, `todo ${i}`]);
    send(state, commit(1, changeSet(1n, [full(TODOS, 0, listBytes(rows))])));
    const table = root.querySelector('[data-signal="6:0"] table');
    expect(table?.querySelectorAll("tbody tr").length).toBe(40);
    expect([...(table?.querySelectorAll("th") ?? [])].map((th) => th.textContent)).toEqual(["#", "id", "title", "done"]);
    expect(root.querySelector('[data-signal="6:0"] .more')?.textContent).toBe("show 60 more of 60");
    send(state, commit(2, changeSet(2n, [patch(TODOS, 0, [["insert", 0, 1000, "first"]])])));
    expect(root.querySelector('[data-signal="6:0"] tbody tr td:nth-child(2)')?.textContent).toBe("1000");
  });

  it("lists commits newest first and expands one into its diff", () => {
    const { state, root } = page();
    send(state, commit(1, changeSet(1n, [full(COUNTER, 0, i32Bytes(1))])));
    send(state, commit(2, changeSet(2n, [full(COUNTER, 0, i32Bytes(2))])));
    const heads = root.querySelectorAll(".timeline .row-head");
    expect(heads.length).toBe(2);
    expect(heads[0]?.textContent).toContain("Counter.add");
    expect(heads[0]?.textContent).toContain("count 2");
    (heads[0] as HTMLElement).click();
    expect(root.querySelector(".entry.open pre")?.textContent).toBe("1  →  2");
  });

  it("restores through the scrubber and through an entry, and Live goes back to the head", () => {
    const { state, root, restored } = page();
    send(state, commit(1, changeSet(1n, [full(COUNTER, 0, i32Bytes(1))])));
    send(state, step(1, 1));
    send(state, commit(2, changeSet(2n, [full(COUNTER, 0, i32Bytes(2))])));
    send(state, step(2, 2));
    const range = root.querySelector<HTMLInputElement>(".range");
    expect(range?.disabled).toBe(false);
    expect(range?.max).toBe("1");
    expect(root.querySelector(".scrub-label")?.textContent).toBe("step 2");
    // Releasing the scrubber on the first step asks the server to restore it.
    if (range === null) throw new Error("no range");
    range.value = "0";
    range.dispatchEvent(new Event("input"));
    expect(root.querySelector(".scrub-label")?.textContent).toBe("step 1");
    range.dispatchEvent(new Event("change"));
    expect(restored).toEqual([1]);
    // An entry offers its own step.
    (root.querySelector(".row-head") as HTMLElement).click();
    const button = [...root.querySelectorAll<HTMLButtonElement>(".entry-foot .btn")].find((b) => b.textContent?.startsWith("Restore"));
    button?.click();
    expect(restored).toEqual([1, 2]);
    // After a restore the live position is the step that was restored, and Live returns to the head.
    send(state, step(3, 3, 1));
    expect(root.querySelector(".scrub-label")?.textContent).toBe("step 1");
    const live = [...root.querySelectorAll<HTMLButtonElement>(".scrubber .btn")].find((b) => b.textContent === "Live");
    expect(live?.disabled).toBe(false);
    live?.click();
    expect(restored.at(-1)).toBe(2);
  });

  it("asks for changes when there is nothing to travel to", () => {
    const { root } = page();
    expect(root.querySelector<HTMLInputElement>(".range")?.disabled).toBe(true);
    expect(root.querySelector(".scrubber .hint")?.textContent).toMatch(/Change something/);
  });

  it("shows the port log with its arguments and latency, and the counters with what they count", () => {
    const { state, root } = page();
    send(state, { t: "port", record: { phase: "start", id: 1, portId: 300, methodId: 301, atMs: 10, args: bytes((w) => { w.writeStr("https://x"); w.writeU8(1); }) } });
    send(state, { t: "port", record: { phase: "end", id: 1, portId: 300, methodId: 301, atMs: 60, status: 0, latencyUs: 42_000, reply: bytes((w) => w.writeU16(200)) } });
    const tabs = [...root.querySelectorAll<HTMLButtonElement>(".tab")];
    tabs.find((t) => t.textContent?.startsWith("Ports"))?.click();
    const row = root.querySelector(".ports .row-head");
    expect(row?.textContent).toContain("Http.send");
    expect(row?.textContent).toContain('url: "https://x"');
    expect(row?.textContent).toContain("42 ms");
    expect(row?.textContent).toContain("ok");
    send(state, { t: "stats", json: JSON.stringify({ at_ms: 1000, core: { live_stores: 2, crossings: { change_sets: 4 } }, server: { commits: 4, steps: 2, app_connected: true, app_backlog_bytes: 0 } }) });
    [...root.querySelectorAll<HTMLButtonElement>(".tab")].find((t) => t.textContent?.startsWith("Counters"))?.click();
    const text = root.querySelector(".counters")?.textContent ?? "";
    expect(text).toContain("Dev server");
    expect(text).toContain("drains, merges and backlog");
    expect(text).toContain("commits merged per step2.0");
  });

  it("says so when the dev server is gone", () => {
    const { state, root } = page();
    state.setConn("reconnecting");
    expect(root.querySelector(".pill")?.textContent).toBe("reconnecting");
    state.notify("could not connect");
    expect(root.querySelector(".notice")?.textContent).toBe("could not connect");
  });
});
