import { describe, expect, it } from "vitest";
import { type DocumentLike, browserLifecycle } from "../src/adapters/browser.js";
import type { AppState } from "../src/adapters/types.js";
import { macrotask } from "./support/harness.js";

/*
 * The Lifecycle source of browsers (ADR-046 decision 3.4): a page that is hidden, left (`pagehide`) or frozen (`freeze`) is
 * `background`, a visible one `active`.
 */

type Listener = (event?: Event) => void;

/** An event target that remembers its listeners and fires them. */
class Target {
  readonly listeners = new Map<string, Set<Listener>>();
  addEventListener(type: string, listener: Listener): void {
    let set = this.listeners.get(type);
    if (set === undefined) this.listeners.set(type, (set = new Set()));
    set.add(listener);
  }
  removeEventListener(type: string, listener: Listener): void {
    this.listeners.get(type)?.delete(listener);
  }
  fire(type: string): void {
    for (const listener of [...(this.listeners.get(type) ?? [])]) listener({ type } as Event);
  }
  get count(): number {
    return [...this.listeners.values()].reduce((n, set) => n + set.size, 0);
  }
}

class FakeDocument extends Target implements DocumentLike {
  visibilityState = "visible";
}

function watch(doc: FakeDocument, win: Target): { states: AppState[]; stop: () => void } {
  const states: AppState[] = [];
  const stop = browserLifecycle(doc as unknown as DocumentLike, win as unknown as EventTarget).subscribe((state) => states.push(state));
  return { states, stop };
}

describe("browserLifecycle", () => {
  it("reports the current state from a microtask, then every visibility change", async () => {
    const doc = new FakeDocument();
    const { states } = watch(doc, new Target());
    expect(states).toEqual([]);
    await macrotask();
    expect(states).toEqual(["active"]);
    doc.visibilityState = "hidden";
    doc.fire("visibilitychange");
    doc.visibilityState = "visible";
    doc.fire("visibilitychange");
    expect(states).toEqual(["active", "background", "active"]);
  });

  it("a page that is being left (pagehide on the window) or frozen (freeze on the document) is in the background even while it still says visible", async () => {
    const doc = new FakeDocument();
    const win = new Target();
    const { states } = watch(doc, win);
    await macrotask();
    win.fire("pagehide");
    doc.fire("freeze");
    expect(doc.visibilityState).toBe("visible");
    expect(states).toEqual(["active", "background", "background"]);
  });

  it("stops listening when unsubscribed, on the document and on the window", async () => {
    const doc = new FakeDocument();
    const win = new Target();
    const { states, stop } = watch(doc, win);
    expect(doc.count).toBe(2);
    expect(win.count).toBe(1);
    stop();
    expect(doc.count + win.count).toBe(0);
    doc.fire("visibilitychange");
    win.fire("pagehide");
    await macrotask();
    expect(states).toEqual([]);
  });

  it("has nothing to watch without a document", () => {
    const states: AppState[] = [];
    const stop = browserLifecycle().subscribe((state) => states.push(state));
    expect(typeof stop).toBe("function");
    stop();
    expect(states).toEqual([]);
  });
});
