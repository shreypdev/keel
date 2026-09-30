import { derived, get } from "svelte/store";
import { describe, expect, it } from "vitest";
import { Signal } from "../src/signal.js";
import { signalStore } from "../src/svelte.js";

/** The Svelte adapter against Svelte's own store helpers (`get`, `derived`); no component is needed. */

describe("signalStore", () => {
  it("gives the current value to a subscriber at once, as the store contract wants", () => {
    const count = new Signal(3);
    const seen: number[] = [];
    const stop = signalStore(count).subscribe((value) => seen.push(value));
    expect(seen).toEqual([3]);
    stop();
  });

  it("calls the subscriber for each change, and not after it unsubscribed", () => {
    const count = new Signal(0);
    const seen: number[] = [];
    const stop = signalStore(count).subscribe((value) => seen.push(value));
    count._set(1);
    count._set(2);
    stop();
    count._set(3);
    expect(seen).toEqual([0, 1, 2]);
    expect(count.subscriberCount).toBe(0);
  });

  it("holds one subscription per subscriber, and none while nobody listens", () => {
    const count = new Signal(0);
    const store = signalStore(count);
    expect(count.subscriberCount).toBe(0);
    const stopA = store.subscribe(() => {});
    const stopB = store.subscribe(() => {});
    expect(count.subscriberCount).toBe(2);
    stopA();
    stopB();
    expect(count.subscriberCount).toBe(0);
  });

  it("works with get(), which is what $store in a script does", () => {
    const count = new Signal(7);
    expect(get(signalStore(count))).toBe(7);
    count._set(8);
    expect(get(signalStore(count))).toBe(8);
  });

  it("composes with derived()", () => {
    const count = new Signal(1);
    const doubled = derived(signalStore(count), (value) => value * 2);
    const seen: number[] = [];
    const stop = doubled.subscribe((value) => seen.push(value));
    count._set(5);
    stop();
    expect(seen).toEqual([2, 10]);
    expect(count.subscriberCount).toBe(0);
  });

  it("carries the very list the signal holds, not a copy", () => {
    const items = [1, 2, 3];
    const list = new Signal(items);
    expect(get(signalStore(list))).toBe(items);
  });
});
