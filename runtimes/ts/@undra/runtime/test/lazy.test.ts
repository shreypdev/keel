import { describe, expect, it } from "vitest";
import { UndraCallError } from "../src/call-error.js";
import { UndraTransportError } from "../src/errors.js";
import { LazyList } from "../src/lazy.js";
import { UndraReader, WireError, codecs, encodeLazyInvalidated, encodeLazyPage, encodeLazyValue } from "../src/wire/index.js";
import { LazyServer, drain, lazyCore, numbersList, settle } from "./support/lazy-server.js";

/*
 * LazyList (ADR-043 decision 3.5) against a fake core that serves pages: reading, requesting, coalescing, prefetch,
 * eviction, the version rules, invalidation, restarts and hostile replies, on a core that answers in process
 * (`callSync`) and on one that answers later.
 */

const reader = (bytes: Uint8Array): UndraReader => new UndraReader(bytes);
/** The row `i` of a server built by `numbersList`. */
const row = (i: number): number => i * 10;
/** The first offsets of the page calls a server received. */
const offsets = (server: LazyServer<number>): number[] => server.calls.map((c) => c.offset);

describe.each([
  ["in process (callSync)", true],
  ["asynchronous", false],
] as const)("LazyList over a core that answers %s", (_name, synchronous) => {
  /** Waits until a request has been made and answered. */
  const answered = async (): Promise<void> => {
    await settle();
    if (!synchronous) await drain();
  };

  it("starts empty, and requests nothing before the core has sent its value", async () => {
    const { core, server } = await lazyCore({ synchronous }).then(async (base) => ({
      ...base,
      server: new LazyServer(codecs.i32, [1, 2, 3]),
    }));
    const list = new LazyList<number>(core, codecs.i32);
    expect(list.length.peek()).toBe(0);
    expect(list.revision.peek()).toBe(0);
    expect(list.get(0)).toBeUndefined();
    await answered();
    expect(server.calls).toEqual([]);
  });

  it("takes its length from the value and shows undefined for a row that has not arrived", async () => {
    const { list, server } = await numbersList(120, { synchronous });
    expect(list.length.peek()).toBe(120);
    expect(list.get(0)).toBeUndefined();
    expect(server.calls).toEqual([]); // nothing crosses the boundary inside `get`
    await answered();
    expect(list.get(0)).toBe(row(0));
    expect(list.get(49)).toBe(row(49));
  });

  it("requests the page of a row and one page on each side, once, as page calls", async () => {
    const { list, server } = await numbersList(1000, { synchronous });
    list.get(75);
    await answered();
    expect(server.calls.map((c) => [c.offset, c.limit])).toEqual([
      [0, 50],
      [50, 50],
      [100, 50],
    ]);
    expect(server.calls.every((c) => c.handle === server.handle)).toBe(true);
    // Every row of the three pages is there; nothing is asked twice.
    expect(list.get(0)).toBe(row(0));
    expect(list.get(75)).toBe(row(75));
    expect(list.get(99)).toBe(row(99));
    list.get(75);
    list.get(60);
    await answered();
    expect(server.calls.length).toBe(3);
    expect(list.get(120)).toBe(row(120)); // the page after the prefetched ones: its read asks for one more, and only that
    await answered();
    expect(offsets(server)).toEqual([0, 50, 100, 150]);
  });

  it("has no page before the first and none after the last", async () => {
    const { list, server } = await numbersList(120, { synchronous });
    list.get(0);
    await answered();
    expect(offsets(server)).toEqual([0, 50]);
    server.calls.length = 0;
    list.get(119); // the last page, 100..120
    await answered();
    expect(offsets(server)).toEqual([100]); // page 1 is cached, there is no page 3
    expect(list.get(119)).toBe(row(119));
    expect(list.get(100)).toBe(row(100));
  });

  it("coalesces the reads of one turn into one batch of page calls, lowest page first, each page once", async () => {
    const { list, server, fake } = await numbersList(10_000, { synchronous });
    for (const i of [260, 5, 2, 130, 261, 6]) list.get(i);
    expect(server.calls.length).toBe(0);
    expect(fake.calls.length).toBe(0);
    await settle(1); // one microtask: the batch is sent
    // Pages 5, 0 and 2 were read; 4, 6, 1 and 3 are their neighbours: seven pages, each once, in order.
    expect(offsets(server)).toEqual([0, 50, 100, 150, 200, 250, 300]);
  });

  it("answers a read of a page that is loading with undefined and does not ask again while it is in flight", async () => {
    const { list, server } = await numbersList(500, { synchronous: false });
    server.hold = true;
    list.get(10);
    await settle();
    expect(offsets(server)).toEqual([0, 50]);
    list.get(10);
    list.get(60);
    await settle();
    expect(offsets(server)).toEqual([0, 50, 100]); // page 2 is page 1's neighbour; pages 0 and 1 are not asked again
    expect(list.get(10)).toBeUndefined();
    server.releaseAll();
    await drain();
    expect(list.get(10)).toBe(row(10));
    expect(list.get(60)).toBe(row(60));
    expect(list.get(110)).toBe(row(110));
  });

  it("ignores an index that is not a row: it returns undefined and requests nothing", async () => {
    const { list, server } = await numbersList(75, { synchronous });
    for (const i of [-1, 75, 76, 1e9, 1.5, Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY, -0.5]) {
      expect(list.get(i)).toBeUndefined();
    }
    await answered();
    expect(server.calls).toEqual([]);
    expect(list.get(74)).toBeUndefined(); // a real row: requested, not there yet
    await answered();
    expect(list.get(74)).toBe(row(74));
    expect(list.get(75)).toBeUndefined();
  });

  it("prefetch asks for the pages of a range, clamps it to the list, and caps it at maxCachedPages", async () => {
    const { list, server } = await numbersList(1000, { synchronous });
    list.prefetch(120, 260);
    await answered();
    expect(offsets(server)).toEqual([100, 150, 200, 250]);
    server.calls.length = 0;
    list.prefetch(-100, 5);
    list.prefetch(990, 5000);
    list.prefetch(5, 5);
    list.prefetch(10, 3);
    list.prefetch(Number.NaN, 20);
    await answered();
    expect(offsets(server)).toEqual([0, 950]);
    server.calls.length = 0;
    list.maxCachedPages = 3;
    list.prefetch(300, 900);
    await answered();
    expect(offsets(server)).toEqual([300, 350, 400]);
  });

  it("raises revision when pages arrive, once per batch, and length when the length changes", async () => {
    const { list } = await numbersList(500, { synchronous });
    const lengths: number[] = [];
    const revisions: number[] = [];
    list.length.subscribe((n) => lengths.push(n));
    list.revision.subscribe((n) => revisions.push(n));
    list.get(0);
    await answered();
    expect(lengths).toEqual([]);
    if (synchronous) expect(revisions).toEqual([2]); // two pages, one batch, one notification (the counter moved by two)
    else expect(revisions.length).toBeGreaterThanOrEqual(1);
    expect(list.revision.peek()).toBe(2);
  });

  describe("a change of the list", () => {
    it("takes the new length at once and keeps the stale rows visible while the window is fetched again", async () => {
      const { list, server } = await numbersList(1000, { synchronous });
      list.get(75);
      await answered();
      server.calls.length = 0;
      server.change((rows) => {
        rows.splice(80, 0, 999);
      });
      list.applyInvalidated(reader(server.invalidated()));
      expect(list.length.peek()).toBe(1001); // at once, with no round trip
      expect(list.get(75)).toBe(row(75)); // stale rows stay
      expect(server.calls.length).toBe(0); // nothing is sent inside apply or get
      if (!synchronous) server.hold = true;
      await settle();
      // Only the window is fetched again: the page a read wanted (1), not its prefetched neighbours (0 and 2).
      expect(offsets(server)).toEqual([50]);
      if (!synchronous) {
        expect(list.get(79)).toBe(row(79)); // still the old rows
        server.releaseAll();
        await drain();
      }
      expect(list.get(80)).toBe(999);
      expect(list.get(81)).toBe(row(80));
      expect(list.get(75)).toBe(row(75));
    });

    it("costs the window, never the list: one page call per page touched since the last change, however long the list", async () => {
      const { list, server } = await numbersList(100_000, { synchronous });
      list.get(25_000);
      await answered();
      server.calls.length = 0;
      for (let change = 0; change < 5; change++) {
        server.change((rows) => {
          rows[25_000] = -change - 1;
        });
        list.applyInvalidated(reader(server.invalidated()));
        // The view renders again on `length`/`revision` and reads what it shows.
        list.get(25_001);
        await answered();
        expect(list.get(25_000)).toBe(-change - 1);
      }
      expect(offsets(server)).toEqual([25_000, 25_000, 25_000, 25_000, 25_000]); // 1 page of 50 per change
    });

    it("re-pages every page a read touched since the previous change, and none that was only prefetched", async () => {
      const { list, server } = await numbersList(1000, { synchronous });
      list.get(5); // page 0 wanted, 1 prefetched
      await answered();
      list.get(600); // page 12 wanted, 11 and 13 prefetched
      await answered();
      server.calls.length = 0;
      server.change((rows) => {
        rows[0] = -1;
        rows[600] = -2;
      });
      list.applyInvalidated(reader(server.invalidated()));
      await answered();
      expect(offsets(server)).toEqual([0, 600]);
      expect(list.get(0)).toBe(-1);
      expect(list.get(600)).toBe(-2);
      // The window of the next change is what was read since this one: the same two pages.
      server.calls.length = 0;
      server.change((rows) => {
        rows[600] = -3;
      });
      list.applyInvalidated(reader(server.invalidated()));
      await answered();
      expect(offsets(server)).toEqual([0, 600]);
      // A change after which nothing was read has an empty window: nothing is fetched until a read wants a page.
      server.calls.length = 0;
      server.change((rows) => {
        rows[600] = -4;
      });
      list.applyInvalidated(reader(server.invalidated()));
      await answered();
      expect(server.calls).toEqual([]);
      expect(list.get(600)).toBe(-3); // stale, and read now: asked for again
      await answered();
      expect(offsets(server)).toEqual([600]);
      expect(list.get(600)).toBe(-4);
    });

    it("answers a read of a stale page that was not in the window with the stale row and fetches it", async () => {
      const { list, server } = await numbersList(500, { synchronous });
      list.get(0);
      await answered();
      server.calls.length = 0;
      server.change((rows) => {
        rows[60] = -60; // in page 1, which was only prefetched
      });
      list.applyInvalidated(reader(server.invalidated()));
      await answered();
      expect(offsets(server)).toEqual([0]);
      expect(list.get(60)).toBe(row(60)); // stale
      await answered();
      expect(offsets(server)).toEqual([0, 50, 100]); // the page, and its neighbour that had never been asked for
      expect(list.get(60)).toBe(-60);
    });

    it("drops the pages past a shorter list and shows nothing for rows that are gone", async () => {
      const { list, server } = await numbersList(300, { synchronous });
      list.prefetch(0, 300);
      await answered();
      expect(list.get(250)).toBe(row(250));
      server.calls.length = 0;
      server.change((rows) => {
        rows.length = 120;
      });
      list.applyInvalidated(reader(server.invalidated()));
      expect(list.length.peek()).toBe(120);
      expect(list.get(250)).toBeUndefined();
      expect(list.get(119)).toBe(row(119));
      await answered();
      expect(offsets(server)).not.toContain(250);
      expect(offsets(server).every((o) => o < 120)).toBe(true);
    });

    it("an empty list asks for nothing", async () => {
      const { list, server } = await numbersList(100, { synchronous });
      list.get(0);
      await answered();
      server.calls.length = 0;
      server.change((rows) => {
        rows.length = 0;
      });
      list.applyInvalidated(reader(server.invalidated()));
      await answered();
      expect(list.length.peek()).toBe(0);
      expect(list.get(0)).toBeUndefined();
      expect(server.calls).toEqual([]);
    });

    it("ignores a change that is not newer than the list's version", async () => {
      const { list, server } = await numbersList(200, { synchronous });
      list.get(0);
      await answered();
      server.calls.length = 0;
      list.applyInvalidated(reader(encodeLazyInvalidated({ len: 5, version: server.version })));
      list.applyInvalidated(reader(encodeLazyInvalidated({ len: 5, version: server.version - 1n })));
      expect(list.length.peek()).toBe(200);
      await answered();
      expect(server.calls).toEqual([]);
    });
  });

  describe("a restart of the page server", () => {
    it("drops the cache when the value carries another handle, and reads again from the new one", async () => {
      const { list, server, fake } = await numbersList(500, { synchronous });
      list.get(10);
      await answered();
      expect(list.get(10)).toBe(row(10));
      const revision = list.revision.peek();
      const second = new LazyServer(codecs.i32, Array.from({ length: 80 }, (_, i) => i + 1000), 0x0002_0000_0002n, 3n).install(fake);
      // The fake routes page calls to one handler: the new server answers for its handle, the old one refuses its own no more.
      list.applyFull(reader(second.value()));
      expect(list.length.peek()).toBe(80);
      expect(list.revision.peek()).toBeGreaterThan(revision);
      expect(list.get(10)).toBeUndefined();
      server.calls.length = 0;
      await answered();
      expect(second.calls.length + server.calls.length).toBeGreaterThan(0);
      expect(server.calls.every((c) => c.handle === second.handle)).toBe(true);
    });

    it("drops the reply of a request made before the restart", async () => {
      const { list, server, fake } = await numbersList(500, { synchronous: false });
      server.hold = true;
      list.get(10);
      await settle();
      expect(server.held.length).toBe(2);
      const second = new LazyServer(codecs.i32, [7, 8, 9], 0x0002_0000_0002n, 5n).install(fake);
      list.applyFull(reader(second.value()));
      server.releaseAll(); // the old server's replies, for the old generation
      await drain();
      expect(list.length.peek()).toBe(3);
      expect(list.get(0)).toBeUndefined(); // not the old page 0
      await drain();
      expect(list.get(0)).toBe(7);
    });

    it("the same value again changes nothing", async () => {
      const { list, server } = await numbersList(200, { synchronous });
      list.get(0);
      await answered();
      const revision = list.revision.peek();
      server.calls.length = 0;
      list.applyFull(reader(server.value()));
      await answered();
      expect(server.calls).toEqual([]);
      expect(list.revision.peek()).toBe(revision);
      expect(list.get(0)).toBe(row(0));
    });

    it("the same handle with a newer version is a change, not a restart", async () => {
      const { list, server } = await numbersList(200, { synchronous });
      list.get(0);
      await answered();
      server.calls.length = 0;
      server.change((rows) => {
        rows[0] = -1;
      });
      list.applyFull(reader(server.value()));
      expect(list.get(0)).toBe(row(0)); // stale rows stay
      await answered();
      expect(offsets(server)).toEqual([0]);
      expect(list.get(0)).toBe(-1);
    });
  });

  describe("cache size", () => {
    it("pageSize is 50 and maxCachedPages is 24, and both are settable", async () => {
      const { list } = await numbersList(10, { synchronous });
      expect(list.pageSize).toBe(50);
      expect(list.maxCachedPages).toBe(24);
      list.pageSize = 20;
      list.maxCachedPages = 6;
      expect(list.pageSize).toBe(20);
      expect(list.maxCachedPages).toBe(6);
      for (const bad of [0, -1, 1.5, Number.NaN, 0x10000]) expect(() => (list.pageSize = bad)).toThrow(RangeError);
      for (const bad of [0, -3, 2.5, Number.NaN]) expect(() => (list.maxCachedPages = bad)).toThrow(RangeError);
      expect(list.pageSize).toBe(20);
    });

    it("a page size of its own sets the limit of the page calls and drops the cache", async () => {
      const { list, server } = await numbersList(100, { synchronous });
      list.get(0);
      await answered();
      expect(list.get(0)).toBe(row(0));
      list.pageSize = 10;
      expect(list.get(0)).toBeUndefined();
      server.calls.length = 0;
      await answered();
      expect(server.calls.map((c) => [c.offset, c.limit])).toEqual([
        [0, 10],
        [10, 10],
      ]);
      expect(list.get(15)).toBe(row(15));
    });

    it("evicts the least recently touched pages beyond maxCachedPages, never a page of the window before one outside it", async () => {
      const { list, server } = await numbersList(10 * 50, { synchronous });
      list.maxCachedPages = 4;
      for (const page of [0, 1, 2, 3, 4, 5]) {
        list.get(page * 50);
        await answered();
      }
      // The window (pages 0..5 were each read) does not fit: the oldest reads are gone, the newest are there.
      expect(list.get(0)).toBeUndefined();
      expect(list.get(5 * 50)).toBe(row(250));
      expect(list.get(4 * 50)).toBe(row(200));
      expect(list.get(3 * 50)).toBe(row(150));
      server.calls.length = 0;
      await answered();
      expect(offsets(server)).toContain(0); // the evicted page is asked for again when read
    });

    it("reading a cached page again makes it the most recently touched", async () => {
      const { list, server } = await numbersList(10 * 50, { synchronous });
      list.maxCachedPages = 4;
      list.prefetch(0, 200); // pages 0..3, read in this order
      await answered();
      expect(list.get(0)).toBe(row(0)); // page 0 is the youngest read now
      list.prefetch(200, 250); // page 4: one page too many
      await answered();
      server.calls.length = 0;
      expect(list.get(0)).toBe(row(0)); // kept
      expect(list.get(1 * 50)).toBeUndefined(); // the oldest read went
      expect(list.get(4 * 50)).toBe(row(200));
      await answered();
      expect(offsets(server)).toEqual([50, 250]); // the evicted page, and the neighbour of page 4
    });

    it("lowering maxCachedPages evicts at once", async () => {
      const { list } = await numbersList(10 * 50, { synchronous });
      list.prefetch(0, 500);
      await answered();
      list.maxCachedPages = 1;
      expect(list.get(0)).toBeUndefined();
      expect(list.get(9 * 50)).toBe(row(450)); // the youngest read stays
    });
  });

  describe("failures", () => {
    const reported = (errors: unknown[]): unknown[] => errors.map((e) => (e as { cause?: unknown }).cause);

    it("a core that refuses the page call (a stale handle) is reported, get still answers, and the page is not asked again", async () => {
      const { list, server, errors, fake } = await numbersList(200, { synchronous });
      list.applyFull(reader(encodeLazyValue({ handle: 0x0003_0000_0003n, len: 200, version: 1n }))); // a handle the fake does not know
      expect(list.get(0)).toBeUndefined();
      await answered();
      expect(errors.length).toBeGreaterThanOrEqual(1);
      expect(errors[0]?.operation).toBe("LazyList.page");
      expect(errors[0]?.error).toBeInstanceOf(UndraCallError.Refused);
      const asked = fake.calls.length;
      expect(list.get(0)).toBeUndefined();
      await answered();
      expect(fake.calls.length).toBe(asked);
      // A change of the list lets it try again.
      list.applyFull(reader(server.value()));
      list.get(0);
      await answered();
      expect(list.get(0)).toBe(row(0));
    });

    it("prefetch asks again for a page that failed", async () => {
      const { list, server, errors } = await numbersList(200, { synchronous });
      server.override = () => new Uint8Array([1, 2, 3]);
      list.get(0);
      await answered();
      expect(errors.length).toBeGreaterThan(0);
      server.override = null;
      list.prefetch(0, 50);
      await answered();
      expect(list.get(0)).toBe(row(0));
    });

    it("a truncated reply is a WireError reported through the core, never thrown into get", async () => {
      const { list, server, errors } = await numbersList(200, { synchronous });
      const whole = server.page(0, 50);
      server.override = () => whole.subarray(0, whole.length - 3);
      expect(() => list.get(0)).not.toThrow();
      await answered();
      expect(reported(errors)[0]).toBeInstanceOf(WireError);
      expect(list.get(0)).toBeUndefined();
    });

    it("a reply with more rows than the limit, with trailing bytes, or with a row that does not decode is a WireError", async () => {
      const { list, server, errors } = await numbersList(500, { synchronous });
      const tooMany = encodeLazyPage(codecs.i32, { version: 1n, total: 500, items: Array.from({ length: 51 }, (_, i) => i) });
      const trailing = new Uint8Array([...server.page(0, 50), 0]);
      const badRow = encodeLazyPage(codecs.string, { version: 1n, total: 500, items: ["x"] });
      for (const body of [tooMany, trailing, badRow]) {
        errors.length = 0;
        server.override = () => body;
        list.prefetch(0, 50);
        await answered();
        expect(errors.length).toBeGreaterThanOrEqual(1);
        expect(reported(errors)[0]).toBeInstanceOf(WireError);
      }
      expect(list.get(0)).toBeUndefined();
    });

    it("a count that is not what the list's length says is a protocol error", async () => {
      const { list, server, errors } = await numbersList(500, { synchronous });
      server.override = (call) =>
        encodeLazyPage(codecs.i32, { version: server.version, total: 500, items: Array.from({ length: 7 }, (_, i) => i + call.offset) });
      list.prefetch(0, 50);
      await answered();
      const cause = reported(errors)[0];
      expect(cause).toBeInstanceOf(UndraTransportError);
      expect((cause as UndraTransportError).reason).toBe("protocol");
      expect(list.get(0)).toBeUndefined();
    });

    it("a length that contradicts the one the core announced for the same version is a protocol error", async () => {
      const { list, server, errors } = await numbersList(500, { synchronous });
      server.override = (call) =>
        encodeLazyPage(codecs.i32, { version: server.version, total: 400, items: server.rows.slice(call.offset, Math.min(400, call.offset + call.limit)) });
      list.prefetch(0, 50);
      await answered();
      expect((reported(errors)[0] as UndraTransportError).reason).toBe("protocol");
      expect(list.length.peek()).toBe(500);
    });

    it("a malformed value or invalidation throws a WireError and leaves the list as it was", async () => {
      const { list, server } = await numbersList(100, { synchronous });
      const value = server.value();
      expect(() => list.applyFull(reader(value.subarray(0, 12)))).toThrow(WireError);
      expect(() => list.applyFull(reader(new Uint8Array([...value, 9])))).toThrow(WireError);
      const inv = encodeLazyInvalidated({ len: 5, version: 9n });
      expect(() => list.applyInvalidated(reader(inv.subarray(0, 4)))).toThrow(WireError);
      expect(() => list.applyInvalidated(reader(new Uint8Array([...inv, 1])))).toThrow(WireError);
      expect(list.length.peek()).toBe(100);
    });

    it("an invalidation before any value is ignored", async () => {
      const base = await lazyCore({ synchronous });
      const list = new LazyList<number>(base.core, codecs.i32);
      list.applyInvalidated(reader(encodeLazyInvalidated({ len: 5, version: 2n })));
      expect(list.length.peek()).toBe(0);
      expect(list.get(0)).toBeUndefined();
    });

    it("a core that is closed reports instead of throwing", async () => {
      const { list, core, errors } = await numbersList(100, { synchronous });
      list.get(0);
      core.close();
      await answered();
      expect(list.get(0)).toBeUndefined();
      expect(errors).toEqual([]); // nothing is requested of a closed core
    });
  });
});

describe("LazyList version rules (asynchronous replies)", () => {
  it("drops a reply read before a change it knows of, asks again, and installs the fresh one", async () => {
    const { list, server } = await numbersList(200, { synchronous: false });
    server.hold = true;
    list.get(0);
    await settle();
    expect(server.held.length).toBe(2);
    const oldRows = [...server.rows];
    // A change commits; the op 2 reaches the host before the reply to the earlier request.
    server.change((rows) => {
      rows[0] = -1;
    });
    list.applyInvalidated(reader(server.invalidated()));
    // The earlier request is answered with what it read: the old version.
    server.release(0, server.pageAt(1n, oldRows, 0, 50));
    await drain();
    expect(list.get(0)).toBeUndefined(); // the old page was dropped, not shown
    expect(server.calls.length).toBe(3); // and page 0 was asked for again
    server.hold = false;
    server.release(0, server.pageAt(1n, oldRows, 50, 50)); // the old page 1, also stale: dropped
    await drain();
    // The held re-asks are answered now.
    server.releaseAll();
    await drain();
    expect(list.get(0)).toBe(-1);
  });

  it("a reply at a newer version raises the length and version, re-pages the rest of the window, and the op 2 that follows changes nothing", async () => {
    const { list, server } = await numbersList(500, { synchronous: false });
    list.get(0);
    list.get(120);
    await drain();
    expect(list.get(120)).toBe(row(120));
    expect(list.get(0)).toBe(row(0));
    server.calls.length = 0;
    // The core changes (an insert at 0) and answers a page before its op 2 arrives.
    server.change((rows) => {
      rows.unshift(-7);
    });
    list.get(300);
    await drain();
    expect(list.length.peek()).toBe(501); // raised by the page's `total`
    // The window of before (pages 0 and 2, which were read) is fetched again, once each; 5, 6 and 7 are the new reads.
    expect(offsets(server).sort((a, b) => a - b)).toEqual([0, 100, 250, 300, 350]);
    expect(list.get(0)).toBe(-7);
    expect(list.get(120)).toBe(row(119));
    server.calls.length = 0;
    list.applyInvalidated(reader(server.invalidated())); // the op 2 for the same version
    await drain();
    expect(server.calls).toEqual([]);
  });

  it("a core that keeps answering below the list's version is reported after a bounded number of tries", async () => {
    const { list, server, errors } = await numbersList(200, { synchronous: false });
    list.applyInvalidated(reader(encodeLazyInvalidated({ len: 200, version: 10n })));
    server.override = (call) => server.pageAt(1n, server.rows, call.offset, call.limit);
    list.get(0);
    await drain();
    await drain();
    await drain();
    await drain();
    expect(server.calls.filter((c) => c.offset === 0).length).toBe(3); // the first ask and two more
    expect(errors.length).toBeGreaterThan(0);
    expect(((errors[0] as { cause?: UndraTransportError }).cause as UndraTransportError).reason).toBe("protocol");
  });

  it("a reply for a page past the end of a list that shrank in the meantime is dropped", async () => {
    const { list, server } = await numbersList(500, { synchronous: false });
    server.hold = true;
    list.get(400);
    await settle();
    expect(server.calls.length).toBe(3);
    server.hold = false;
    server.change((rows) => {
      rows.length = 100;
    });
    list.applyInvalidated(reader(server.invalidated()));
    server.releaseAll(); // answered now, at the new version: page 8 is past the end
    await drain();
    expect(list.length.peek()).toBe(100);
    expect(list.get(400)).toBeUndefined();
  });

  it("a transport that cannot send the call is reported, get still answers, and a re-sent value retries the page", async () => {
    const { list, server, errors, fake } = await numbersList(100, { synchronous: false });
    fake.close(); // `send` now throws: the page call rejects
    expect(() => list.get(0)).not.toThrow();
    await drain();
    expect(errors.length).toBe(2); // pages 0 and 1
    expect(errors[0]?.operation).toBe("LazyList.page");
    expect(errors[0]?.error).toBeInstanceOf(UndraCallError.Unavailable);
    expect(list.get(0)).toBeUndefined();
    await drain();
    expect(errors.length).toBe(2); // not asked again
    fake.closed = false;
    list.applyFull(reader(server.value())); // what a reconnect does: the value again, same handle and version
    await drain();
    expect(list.get(0)).toBe(row(0));
  });
});

describe("LazyList on its own transport", () => {
  it("switches to asynchronous page calls once the transport turns out not to have callSync", async () => {
    const { list, server, fake } = await numbersList(300, { synchronous: false });
    list.get(0);
    await drain();
    expect(list.get(0)).toBe(row(0));
    expect(fake.calls.length).toBe(2);
    expect(server.calls.length).toBe(2);
  });

  it("answers inside the microtask when the transport is synchronous", async () => {
    const { list } = await numbersList(300, { synchronous: true });
    list.get(0);
    await settle(1);
    expect(list.get(0)).toBe(row(0));
  });

  it("a change delivered while a page call is being answered (callSync flushes the mirror) leaves the list consistent", async () => {
    const { list, server, fake } = await numbersList(500, { synchronous: true });
    list.get(0);
    await settle();
    server.calls.length = 0;
    // While the next page is read, the core commits a change whose op 2 reaches the mirror in the same call.
    server.onCall = () => {
      server.onCall = null;
      server.change((rows) => {
        rows[0] = -1;
      });
      fake.emitChangeSet([]);
      list.applyInvalidated(reader(server.invalidated()));
    };
    list.get(120); // wants pages 1, 2, 3 (2 is the read, 1 is cached)
    await settle();
    await settle();
    expect(list.length.peek()).toBe(500);
    expect(list.get(0)).toBe(-1);
    expect(list.get(120)).toBe(row(120));
  });
});
