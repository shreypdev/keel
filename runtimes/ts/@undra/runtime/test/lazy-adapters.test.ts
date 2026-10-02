import { derived, get } from "svelte/store";
import { createMemo, createRoot } from "solid-js";
import { computed, effectScope } from "vue";
import { describe, expect, it } from "vitest";
import { signalStore } from "../src/svelte.js";
import { useSignal as useSolidSignal } from "../src/solid.js";
import { useSignal as useVueSignal } from "../src/vue.js";
import { UndraReader } from "../src/wire/index.js";
import { numbersList, settle } from "./support/lazy-server.js";

/*
 * The Vue, Svelte and Solid adapters need nothing for a `LazyList` (ADR-043): its `length` and `revision` are signals,
 * and a view that reads `list.get(i)` next to them follows the rows.
 */

describe("a LazyList through the framework adapters", () => {
  it("vue: a computed over length and revision follows a change of the length and the arrival of rows", async () => {
    const { list, server } = await numbersList(300, { synchronous: true });
    const scope = effectScope();
    const view = scope.run(() => {
      const length = useVueSignal(list.length);
      const revision = useVueSignal(list.revision);
      return computed(() => `${length.value}: ${revision.value >= 0 ? (list.get(0) ?? "·") : ""}`);
    });
    expect(view?.value).toBe("300: ·");
    await settle();
    expect(view?.value).toBe("300: 0");
    server.change((rows) => {
      rows.unshift(77);
    });
    list.applyInvalidated(new UndraReader(server.invalidated()));
    expect(view?.value).toMatch(/^301: /);
    await settle();
    expect(view?.value).toBe("301: 77");
    scope.stop();
    expect(list.length.subscriberCount + list.revision.subscriberCount).toBe(0);
  });

  it("svelte: derived stores over length and revision", async () => {
    const { list, server } = await numbersList(300, { synchronous: true });
    const view = derived([signalStore(list.length), signalStore(list.revision)], ([length]) => `${length}: ${list.get(0) ?? "·"}`);
    const seen: string[] = [];
    const stop = view.subscribe((text) => seen.push(text));
    await settle();
    server.change((rows) => {
      rows[0] = 5;
    });
    list.applyInvalidated(new UndraReader(server.invalidated()));
    await settle();
    expect(get(view)).toBe("300: 5");
    expect(seen[0]).toBe("300: ·");
    stop();
    expect(list.length.subscriberCount + list.revision.subscriberCount).toBe(0);
  });

  it("solid: a memo over length and revision", async () => {
    const { list, server } = await numbersList(300, { synchronous: true });
    await createRoot(async (dispose) => {
      const length = useSolidSignal(list.length);
      const revision = useSolidSignal(list.revision);
      const text = createMemo(() => `${length()}: ${revision() >= 0 ? (list.get(0) ?? "·") : ""}`);
      expect(text()).toBe("300: ·");
      await settle();
      expect(text()).toBe("300: 0");
      server.change((rows) => {
        rows[0] = 5;
      });
      list.applyInvalidated(new UndraReader(server.invalidated()));
      await settle();
      expect(text()).toBe("300: 5");
      dispose();
    });
  });
});
