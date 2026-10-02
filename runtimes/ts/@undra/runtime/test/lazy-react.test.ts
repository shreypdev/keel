// @vitest-environment jsdom
import { StrictMode, act, createElement, memo, type ReactNode } from "react";
import { type Root, createRoot } from "react-dom/client";
import { renderToString } from "react-dom/server";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { LazyList } from "../src/lazy.js";
import { type LoadMoreOptions, type LoadMoreQuery, useLazyList, useLoadMore } from "../src/react.js";
import { Signal } from "../src/signal.js";
import { UndraReader, codecs, encodeLazyInvalidated } from "../src/wire/index.js";
import { LazyServer, numbersList, settle } from "./support/lazy-server.js";

/*
 * `useLazyList` and `useLoadMore` (ADR-043) against real React in jsdom: a list that pages over a fake core, and a
 * query handle with a fake `IntersectionObserver`.
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
  delete (globalThis as { IntersectionObserver?: unknown }).IntersectionObserver;
  FakeObserver.all.length = 0;
  vi.restoreAllMocks();
});

async function render(element: ReactNode): Promise<{ container: HTMLElement; root: Root; update(next: ReactNode): Promise<void> }> {
  const container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  mounted.push({ root, container });
  await act(async () => root.render(element));
  return { container, root, update: (next) => act(async () => root.render(next)) };
}

/** Lets the list's microtask run (and its in-process reply land) inside `act`. */
const pump = (): Promise<void> => act(async () => settle());

/** The first `rows` rows: `7` for the row `7 * 10 = 70`, `·` while its page loads. */
function Rows(props: { list: LazyList<number> | null | undefined; rows: number; texts?: string[] }): ReactNode {
  const { length, getItem } = useLazyList(props.list);
  const cells: string[] = [];
  for (let i = 0; i < Math.min(length, props.rows); i++) cells.push(String(getItem(i) ?? "·"));
  const text = `${length}: ${cells.join(" ")}`;
  props.texts?.push(text);
  return createElement("p", { "data-testid": "rows" }, text);
}

describe("useLazyList", () => {
  it("renders the length and the rows that are there, reads the rest as undefined, and renders again when the pages arrive", async () => {
    const { list } = await numbersList(500, { synchronous: true });
    const texts: string[] = [];
    const view = await render(createElement(Rows, { list, rows: 3, texts }));
    // The first render reads while the pages are not there; the pages arrive from a microtask and render again.
    expect(texts).toEqual(["500: · · ·", "500: 0 10 20"]);
    expect(view.container.textContent).toBe("500: 0 10 20");
  });

  it("renders again when the length changes, and shows the stale rows until the window is fetched", async () => {
    const { list, server } = await numbersList(500, { synchronous: true });
    const view = await render(createElement(Rows, { list, rows: 2 }));
    await pump();
    server.change((rows) => {
      rows.splice(0, 0, 999);
    });
    await act(async () => list.applyInvalidated(new UndraReader(server.invalidated())));
    expect(view.container.textContent).toMatch(/^501: /);
    await pump();
    expect(view.container.textContent).toBe("501: 999 0");
  });

  it("renders once for a batch of pages", async () => {
    const { list } = await numbersList(500, { synchronous: true });
    const texts: string[] = [];
    await render(createElement(Rows, { list, rows: 120, texts }));
    // Pages 0, 1, 2 (what reading rows 0..120 asks for) and 3 (the neighbour of 2) arrive in one batch: one more render.
    expect(texts.length).toBe(2);
  });

  it("gives a getItem that is new after rows arrive, so a memoised row renders again", async () => {
    const { list } = await numbersList(300, { synchronous: true });
    const seen: Array<(i: number) => number | undefined> = [];
    const Row = memo(function Row(props: { getItem: (i: number) => number | undefined }): ReactNode {
      seen.push(props.getItem);
      return createElement("i", null, String(props.getItem(0) ?? "·"));
    });
    function Host(): ReactNode {
      const { getItem } = useLazyList(list);
      return createElement(Row, { getItem });
    }
    const view = await render(createElement(Host));
    expect(view.container.textContent).toBe("0");
    expect(seen.length).toBe(2); // the row rendered again, with the new function
    expect(new Set(seen).size).toBe(2);
  });

  it("is an empty list for null or undefined, so it can be called before the store exists, and follows the list when it appears", async () => {
    const { list } = await numbersList(40, { synchronous: true });
    const view = await render(createElement(Rows, { list: undefined, rows: 2 }));
    expect(view.container.textContent).toBe("0: ");
    await view.update(createElement(Rows, { list, rows: 2 }));
    await pump();
    expect(view.container.textContent).toBe("40: 0 10");
    await view.update(createElement(Rows, { list: null, rows: 2 }));
    expect(view.container.textContent).toBe("0: ");
    expect(list.length.subscriberCount).toBe(0);
    expect(list.revision.subscriberCount).toBe(0);
  });

  it("holds one subscription to each signal and lets go of them on unmount", async () => {
    const { list } = await numbersList(100, { synchronous: true });
    const view = await render(createElement(StrictMode, null, createElement(Rows, { list, rows: 1 })));
    expect([list.length.subscriberCount, list.revision.subscriberCount]).toEqual([1, 1]);
    await act(async () => view.root.unmount());
    mounted.pop();
    view.container.remove();
    expect([list.length.subscriberCount, list.revision.subscriberCount]).toEqual([0, 0]);
  });

  it("renders on the server with what the list has: its length and no rows", async () => {
    const { list, server } = await numbersList(60, { synchronous: true });
    expect(renderToString(createElement(Rows, { list, rows: 2 }))).toContain("60: · ·");
    await settle();
    expect(server.calls.length).toBeGreaterThan(0); // the read requested the page, as it does anywhere
  });
});

// ----- useLoadMore -----------------------------------------------------------------------------

/** An `IntersectionObserver` that does nothing until a test says an entry intersects. */
class FakeObserver {
  static readonly all: FakeObserver[] = [];
  /** What a new observer reports as soon as it observes (the sentinel is, or is not, in view when it is created). */
  static initially = false;
  readonly observed: Element[] = [];
  disconnected = false;

  constructor(
    readonly callback: IntersectionObserverCallback,
    readonly options?: IntersectionObserverInit,
  ) {
    FakeObserver.all.push(this);
  }

  observe(target: Element): void {
    this.observed.push(target);
    if (FakeObserver.initially) this.report(true);
  }

  unobserve(): void {}
  takeRecords(): IntersectionObserverEntry[] {
    return [];
  }

  disconnect(): void {
    this.disconnected = true;
  }

  report(isIntersecting: boolean): void {
    const target = this.observed[0];
    if (target === undefined || this.disconnected) return;
    this.callback([{ isIntersecting, target } as IntersectionObserverEntry], this as unknown as IntersectionObserver);
  }

  static live(): FakeObserver[] {
    return FakeObserver.all.filter((o) => !o.disconnected);
  }
}

function install(): void {
  FakeObserver.initially = false;
  (globalThis as { IntersectionObserver?: unknown }).IntersectionObserver = FakeObserver;
}

interface FakeQuery extends LoadMoreQuery {
  readonly hasNextPage: Signal<boolean>;
  readonly fetchingNextPage: Signal<boolean>;
  readonly error: Signal<unknown>;
  readonly fetches: number[];
}

function fakeQuery(): FakeQuery {
  const fetches: number[] = [];
  return {
    hasNextPage: new Signal(true),
    fetchingNextPage: new Signal(false),
    error: new Signal<unknown>(null),
    fetches,
    fetchNextPage: () => {
      fetches.push(fetches.length + 1);
      return Promise.resolve();
    },
  };
}

function Feed(props: { query: LoadMoreQuery | null | undefined; options?: LoadMoreOptions; hidden?: boolean }): ReactNode {
  const sentinel = useLoadMore(props.query, props.options);
  return createElement("div", null, props.hidden === true ? null : createElement("div", { ref: sentinel, "data-testid": "sentinel" }));
}

describe("useLoadMore", () => {
  it("fetches the next page when the sentinel comes into view, and not before", async () => {
    install();
    const query = fakeQuery();
    await render(createElement(Feed, { query }));
    const [observer] = FakeObserver.live();
    expect(observer?.observed.length).toBe(1);
    observer?.report(false);
    expect(query.fetches).toEqual([]);
    observer?.report(true);
    expect(query.fetches).toEqual([1]);
  });

  it("fetches at most once for an observer, however often it reports", async () => {
    install();
    const query = fakeQuery();
    await render(createElement(Feed, { query }));
    const observer = FakeObserver.live()[0];
    observer?.report(true);
    observer?.report(true);
    expect(query.fetches).toEqual([1]);
  });

  it("passes rootMargin to the observer, 0px by default", async () => {
    install();
    await render(createElement(Feed, { query: fakeQuery(), options: { rootMargin: "400px" } }));
    expect(FakeObserver.live()[0]?.options?.rootMargin).toBe("400px");
    FakeObserver.all.length = 0;
    await render(createElement(Feed, { query: fakeQuery() }));
    expect(FakeObserver.live()[0]?.options?.rootMargin).toBe("0px");
  });

  it("does not observe while there is no next page or one is being fetched, and observes when that changes", async () => {
    install();
    const query = fakeQuery();
    query.hasNextPage._set(false);
    await render(createElement(Feed, { query }));
    expect(FakeObserver.live()).toEqual([]);
    await act(async () => query.hasNextPage._set(true));
    expect(FakeObserver.live().length).toBe(1);
    await act(async () => query.fetchingNextPage._set(true));
    expect(FakeObserver.live()).toEqual([]); // disconnected: nothing to ask while a page is on its way
    expect(FakeObserver.all.every((o) => o.disconnected)).toBe(true);
  });

  it("asks again when the page has arrived and the sentinel is still in view (a new observer reports it at once)", async () => {
    install();
    const query = fakeQuery();
    await render(createElement(Feed, { query }));
    FakeObserver.live()[0]?.report(true);
    expect(query.fetches).toEqual([1]);
    await act(async () => query.fetchingNextPage._set(true));
    FakeObserver.initially = true; // after the page the sentinel is still on screen
    await act(async () => query.fetchingNextPage._set(false));
    expect(query.fetches).toEqual([1, 2]);
    await act(async () => query.fetchingNextPage._set(true));
    await act(async () => {
      query.hasNextPage._set(false); // the last page
      query.fetchingNextPage._set(false);
    });
    expect(query.fetches).toEqual([1, 2]);
    expect(FakeObserver.live()).toEqual([]);
  });

  it("does not fetch while the last fetch's error is set, and fetches again once it is cleared", async () => {
    install();
    const query = fakeQuery();
    query.error._set(new Error("offline"));
    await render(createElement(Feed, { query }));
    expect(FakeObserver.live()).toEqual([]);
    FakeObserver.initially = true;
    await act(async () => query.error._set(null));
    expect(query.fetches).toEqual([1]);
  });

  it("disconnects when the sentinel is detached, when the component unmounts and when the query changes", async () => {
    install();
    const first = fakeQuery();
    const second = fakeQuery();
    const view = await render(createElement(Feed, { query: first }));
    const initial = FakeObserver.live()[0];
    await view.update(createElement(Feed, { query: first, hidden: true }));
    expect(initial?.disconnected).toBe(true);
    await view.update(createElement(Feed, { query: first }));
    const reattached = FakeObserver.live()[0];
    expect(reattached).not.toBe(initial);
    await view.update(createElement(Feed, { query: second }));
    expect(reattached?.disconnected).toBe(true);
    FakeObserver.live()[0]?.report(true);
    expect(second.fetches).toEqual([1]);
    expect(first.fetches).toEqual([]);
    await act(async () => view.root.unmount());
    mounted.pop();
    view.container.remove();
    expect(FakeObserver.live()).toEqual([]);
    expect(first.hasNextPage.subscriberCount + second.hasNextPage.subscriberCount).toBe(0);
  });

  it("does nothing for a null or undefined query, and follows the query when it appears", async () => {
    install();
    const view = await render(createElement(Feed, { query: undefined }));
    expect(FakeObserver.all).toEqual([]);
    const query = fakeQuery();
    await view.update(createElement(Feed, { query }));
    FakeObserver.live()[0]?.report(true);
    expect(query.fetches).toEqual([1]);
    await view.update(createElement(Feed, { query: null }));
    expect(FakeObserver.live()).toEqual([]);
  });

  it("does nothing where there is no IntersectionObserver, and on the server", async () => {
    const query = fakeQuery();
    const view = await render(createElement(Feed, { query }));
    expect(query.fetches).toEqual([]);
    expect(view.container.querySelector("[data-testid=sentinel]")).not.toBeNull();
    expect(renderToString(createElement(Feed, { query }))).toContain("data-testid");
    expect(query.fetches).toEqual([]);
  });

  it("works under StrictMode", async () => {
    install();
    const query = fakeQuery();
    await render(createElement(StrictMode, null, createElement(Feed, { query })));
    expect(FakeObserver.live().length).toBe(1);
    FakeObserver.live()[0]?.report(true);
    expect(query.fetches).toEqual([1]);
  });
});

describe("useLazyList across a restart of the page server", () => {
  it("shows the new list and keeps rendering", async () => {
    const { list, fake } = await numbersList(200, { synchronous: true });
    const view = await render(createElement(Rows, { list, rows: 2 }));
    await pump();
    const second = new LazyServer(codecs.i32, [5, 6, 7], 0x0002_0000_0002n, 4n).install(fake);
    await act(async () => list.applyFull(new UndraReader(second.value())));
    expect(view.container.textContent).toMatch(/^3: /);
    await pump();
    expect(view.container.textContent).toBe("3: 5 6");
    await act(async () => list.applyInvalidated(new UndraReader(encodeLazyInvalidated({ len: 3, version: 4n }))));
    expect(view.container.textContent).toBe("3: 5 6");
  });
});
