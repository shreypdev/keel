// @vitest-environment jsdom
import { Component, type ErrorInfo, type ReactNode, StrictMode, act, createElement } from "react";
import { type Root, createRoot, hydrateRoot } from "react-dom/client";
import { renderToString } from "react-dom/server";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { UndraCore } from "../src/core.js";
import type { UndraClass } from "../src/lifetime.js";
import { useUndra, useSignal } from "../src/react.js";
import { Signal } from "../src/signal.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { deferred, track } from "./support/harness.js";
import { CounterStore, str, u32, vecU32 } from "./support/store.js";

/*
 * The React adapter against real React (react-dom in jsdom): `useSignal` with signals and with a
 * core that pushes changes, `useUndra` with its life cycle, server rendering and hydration.
 */

beforeAll(() => {
  (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
});

const mounted: Array<{ root: Root; container: HTMLElement }> = [];

afterEach(async () => {
  for (const { root, container } of mounted.splice(0)) {
    await act(async () => root.unmount());
    container.remove();
  }
  vi.restoreAllMocks();
});

/** Renders `element` into a fresh container, inside `act`. */
async function render(element: ReactNode): Promise<{ container: HTMLElement; root: Root; update(next: ReactNode): Promise<void> }> {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  mounted.push({ root, container });
  await act(async () => root.render(element));
  return {
    container,
    root,
    update: (next) => act(async () => root.render(next)),
  };
}

/** Shows the value of `signal`, and counts its renders in `renders`. */
function Value(props: { signal: Signal<number> | null | undefined; renders?: number[] }): ReactNode {
  const value = useSignal(props.signal);
  props.renders?.push(value ?? -1);
  return createElement("span", { "data-testid": "value" }, value === undefined ? "none" : String(value));
}

describe("useSignal", () => {
  it("renders the current value and follows the signal", async () => {
    const count = new Signal(3);
    const { container } = await render(createElement(Value, { signal: count }));
    expect(container.textContent).toBe("3");
    await act(async () => count._set(4));
    expect(container.textContent).toBe("4");
  });

  it("renders again only when the value changed", async () => {
    const count = new Signal(1);
    const renders: number[] = [];
    await render(createElement(Value, { signal: count, renders }));
    await act(async () => count._set(1)); // the same value: the signal does not even notify
    await act(async () => count._set(2));
    expect(renders).toEqual([1, 2]);
  });

  it("holds one subscription per signal, however often the component renders", async () => {
    const count = new Signal(0);
    const view = await render(createElement(Value, { signal: count }));
    expect(count.subscriberCount).toBe(1);
    for (let i = 0; i < 3; i++) await view.update(createElement(Value, { signal: count }));
    expect(count.subscriberCount).toBe(1);
  });

  it("unsubscribes when the component unmounts", async () => {
    const count = new Signal(0);
    const view = await render(createElement(Value, { signal: count }));
    expect(count.subscriberCount).toBe(1);
    await act(async () => view.root.unmount());
    expect(count.subscriberCount).toBe(0);
    mounted.pop();
    view.container.remove();
  });

  it("follows a different signal when the prop changes, and lets go of the old one", async () => {
    const a = new Signal(1);
    const b = new Signal(2);
    const view = await render(createElement(Value, { signal: a }));
    await view.update(createElement(Value, { signal: b }));
    expect(view.container.textContent).toBe("2");
    expect([a.subscriberCount, b.subscriberCount]).toEqual([0, 1]);
    await act(async () => a._set(10));
    expect(view.container.textContent).toBe("2");
    await act(async () => b._set(20));
    expect(view.container.textContent).toBe("20");
  });

  it("is undefined without a signal, so it can be called before a store exists", async () => {
    const count = new Signal(5);
    const view = await render(createElement(Value, { signal: undefined }));
    expect(view.container.textContent).toBe("none");
    await view.update(createElement(Value, { signal: count }));
    expect(view.container.textContent).toBe("5");
    await view.update(createElement(Value, { signal: null }));
    expect(view.container.textContent).toBe("none");
    expect(count.subscriberCount).toBe(0);
  });

  it("two components on one signal agree after every change", async () => {
    const count = new Signal(0);
    const view = await render(createElement("div", null, createElement(Value, { signal: count }), createElement(Value, { signal: count })));
    await act(async () => count._set(7));
    expect(view.container.textContent).toBe("77");
  });

  it("works under StrictMode, which subscribes twice and unsubscribes once", async () => {
    const count = new Signal(0);
    const view = await render(createElement(StrictMode, null, createElement(Value, { signal: count })));
    expect(count.subscriberCount).toBe(1);
    await act(async () => count._set(9));
    expect(view.container.textContent).toBe("9");
  });

  it("renders on the server, where there is no subscription, with the value the signal has", () => {
    const count = new Signal(11);
    expect(renderToString(createElement(Value, { signal: count }))).toContain(">11<");
    expect(count.subscriberCount).toBe(0);
    expect(renderToString(createElement(Value, { signal: undefined }))).toContain(">none<");
  });

  it("hydrates server markup without a mismatch and then follows the signal", async () => {
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    const count = new Signal(21);
    const container = document.createElement("div");
    document.body.append(container);
    container.innerHTML = renderToString(createElement(Value, { signal: count }));
    let root!: Root;
    await act(async () => {
      root = hydrateRoot(container, createElement(Value, { signal: count }));
    });
    mounted.push({ root, container });
    expect(errors).not.toHaveBeenCalled();
    expect(container.textContent).toBe("21");
    expect(count.subscriberCount).toBe(1);
    await act(async () => count._set(22));
    expect(container.textContent).toBe("22");
  });
});

/** A core with the fake transport, and a class that creates a `CounterStore` (a fresh handle each time) in it. */
async function setup() {
  const fake = new FakeCoreTransport({ synchronous: true });
  const core = track(
    await UndraCore.attach(fake, { expectedSchemaHash: SCHEMA, shared: false, adapters: { log: { log() {} }, http: null, timer: null } }),
  );
  let nextHandle = 0x1_0000_0001n;
  const created: CounterStore[] = [];
  const makeStore = async (target: UndraCore, label = "one"): Promise<CounterStore> => {
    const handle = nextHandle++;
    fake.store(
      handle,
      new Map([
        [0, u32(1)],
        [1, str(label)],
        [2, vecU32([1, 2, 3])],
      ]),
    );
    const store = await CounterStore.create(target, handle);
    created.push(store);
    return store;
  };
  const Counter: UndraClass<CounterStore> = { create: (target = core) => makeStore(target) };
  return { fake, core, Counter, makeStore, created };
}

/** Shows the count of the store `useUndra` returns, and what it returned on each render. */
function Counting(props: { type: UndraClass<CounterStore>; core?: UndraCore; seen?: Array<CounterStore | undefined> }): ReactNode {
  const store = useUndra(props.type, props.core);
  const count = useSignal(store?.count);
  props.seen?.push(store);
  return createElement("span", null, store === undefined ? "loading" : `count ${count}`);
}

class Boundary extends Component<{ children: ReactNode }, { error: unknown }> {
  override state = { error: undefined as unknown };
  static getDerivedStateFromError(error: unknown) {
    return { error };
  }
  override componentDidCatch(_error: Error, _info: ErrorInfo) {}
  override render() {
    return this.state.error === undefined ? this.props.children : createElement("p", null, `failed: ${String(this.state.error)}`);
  }
}

describe("useUndra", () => {
  it("is undefined until the store exists, then holds it, and its signals render", async () => {
    const { Counter } = await setup();
    const seen: Array<CounterStore | undefined> = [];
    const view = await render(createElement(Counting, { type: Counter, seen }));
    expect(seen[0]).toBeUndefined();
    expect(view.container.textContent).toBe("count 1");
    expect(seen.at(-1)).toBeInstanceOf(CounterStore);
  });

  it("shows what the core pushes into the store", async () => {
    const { fake, Counter, created } = await setup();
    const view = await render(createElement(Counting, { type: Counter }));
    const handle = created[0]?.handle as bigint;
    await act(async () => {
      fake.setSignal(handle, 0, u32(42));
      await fake.settle();
      // A change the core makes on its own is applied at the next animation frame (ADR-031).
      await new Promise((resolve) => requestAnimationFrame(resolve));
    });
    expect(view.container.textContent).toBe("count 42");
  });

  it("creates the store in the core it is given", async () => {
    const { core, makeStore } = await setup();
    const create = vi.fn(async (target?: UndraCore) => makeStore(target ?? core));
    const type: UndraClass<CounterStore> = { create };
    await render(createElement(Counting, { type, core }));
    expect(create).toHaveBeenCalledWith(core);
  });

  it("closes the store when the component unmounts", async () => {
    const { fake, Counter, created } = await setup();
    const view = await render(createElement(Counting, { type: Counter }));
    const store = created[0] as CounterStore;
    expect(store.closed).toBe(false);
    await act(async () => view.root.unmount());
    mounted.pop();
    view.container.remove();
    expect(store.closed).toBe(true);
    expect(fake.released).toEqual([store.handle]);
  });

  it("closes a store that arrives after the component unmounted", async () => {
    const { makeStore, core, created } = await setup();
    const gate = deferred();
    const type: UndraClass<CounterStore> = {
      create: async () => {
        await gate.promise;
        return makeStore(core);
      },
    };
    const view = await render(createElement(Counting, { type }));
    await act(async () => view.root.unmount());
    mounted.pop();
    view.container.remove();
    gate.resolve();
    await vi.waitFor(() => expect(created).toHaveLength(1));
    await vi.waitFor(() => expect(created[0]?.closed).toBe(true));
  });

  it("under StrictMode keeps the second store and closes the first", async () => {
    const { Counter, created, fake } = await setup();
    const view = await render(createElement(StrictMode, null, createElement(Counting, { type: Counter })));
    expect(created.length).toBeGreaterThanOrEqual(2);
    expect(view.container.textContent).toBe("count 1");
    const open = created.filter((store) => !store.closed);
    expect(open).toHaveLength(1);
    expect(fake.released).toHaveLength(created.length - 1);
  });

  it("throws a failed creation during render, where an error boundary catches it", async () => {
    vi.spyOn(console, "error").mockImplementation(() => {});
    const type: UndraClass<CounterStore> = {
      create: () => Promise.reject(new Error("the core said no")),
    };
    const view = await render(createElement(Boundary, null, createElement(Counting, { type })));
    await vi.waitFor(() => expect(view.container.textContent).toBe("failed: Error: the core said no"));
  });

  it("renders on the server without creating anything", async () => {
    const { Counter, created } = await setup();
    expect(renderToString(createElement(Counting, { type: Counter }))).toContain("loading");
    expect(created).toHaveLength(0);
  });

  describe("with a factory and dependencies", () => {
    function Labelled(props: { label: string; make: (label: string) => Promise<CounterStore>; seen: Array<CounterStore | undefined> }): ReactNode {
      const store = useUndra(() => props.make(props.label), [props.label]);
      const label = useSignal(store?.label);
      props.seen.push(store);
      return createElement("span", null, label ?? "loading");
    }

    it("creates again and closes the old store when a dependency changes, never returning a closed one", async () => {
      const { makeStore, core, created } = await setup();
      const seen: Array<CounterStore | undefined> = [];
      const make = (label: string) => makeStore(core, label);
      const view = await render(createElement(Labelled, { label: "a", make, seen }));
      expect(view.container.textContent).toBe("a");
      const first = created[0] as CounterStore;
      await view.update(createElement(Labelled, { label: "b", make, seen }));
      await vi.waitFor(() => expect(view.container.textContent).toBe("b"));
      expect(first.closed).toBe(true);
      expect(created).toHaveLength(2);
      expect(created[1]?.closed).toBe(false);
      // Every store a render got was open at the time: a store is never handed out after its close.
      for (const store of seen) if (store !== undefined) expect(store === first || store === created[1]).toBe(true);
      const afterChange = seen.slice(seen.indexOf(first) + 1);
      expect(afterChange.filter((store) => store === first)).toEqual([]);
    });

    it("does not create again when the dependencies are the same", async () => {
      const { makeStore, core, created } = await setup();
      const seen: Array<CounterStore | undefined> = [];
      const make = (label: string) => makeStore(core, label);
      const view = await render(createElement(Labelled, { label: "a", make, seen }));
      await view.update(createElement(Labelled, { label: "a", make, seen }));
      await view.update(createElement(Labelled, { label: "a", make, seen }));
      expect(created).toHaveLength(1);
    });
  });
});
