import { expect, test } from "vitest";
import { ALL_SIGNALS, CallTarget, UndraCallError, UndraReader, codecs, decodeLazyValue, decodeValue, readLazyPageHeader } from "@undra/runtime";
import {
  Counter,
  FeedQueryHandle,
  Library,
  Probe,
  QueryStatusCodec,
  RemoteTodoCodec,
  RemoteTodosQueryHandle,
  RosterQueryHandle,
  TickerQueryHandle,
  UndraIds,
  configureRemote,
} from "@playground/core";
import { BASE_URL, boot } from "../src/harness.js";
import { replies } from "../src/fake-server.js";
import { type SignalUpdate, tapEntries } from "../src/raw-store.js";
import { counters } from "../src/stats.js";
import { sleep, step, waitFor } from "../src/wait.js";

// S35 query handles across a restore (ADR-059). A query handle is a view of the query cache, so a snapshot keeps what it is made of
// (the query's parameters and its observer's own polling interval) and a restore re-issues the handle under the same value: the app's
// wrapper keeps working with no code of its own. Steps 1 to 9 restore into the core that holds the handles, through the generated
// bindings; step 10 restores into a fresh core of build B (scenarios.md, "Two builds": the query `roster` changes the type of its
// parameter, `remote_todos` and `ticker` do not) through the raw API with the ids it knows.

const LIST = "s35";
const URL = `${BASE_URL}/lists/${LIST}/todos`;
const milk = { id: 1, title: "Buy milk", done: false };
const dog = { id: 2, title: "Walk the dog", done: false };
const todos = codecs.option(codecs.vec(RemoteTodoCodec));

/** The entries the mirror hands to `handle`, from now on (a handle a scenario did not construct itself: raw, as S14 and S15 do). */
function watch(core: Awaited<ReturnType<typeof boot>>["core"], handle: bigint): SignalUpdate[] {
  const entries: SignalUpdate[] = [];
  core.mirror.register(handle, (signalId, op, value) => {
    entries.push({ signalId, op, value: value.slice() });
  });
  return entries;
}

test("S35 query handles across a restore", { timeout: 90_000 }, async () => {
  const { core, server } = await boot();
  await configureRemote({ baseUrl: BASE_URL }, core);
  server.on("GET", URL, replies.json(200, [milk]));
  const gets = (): number => server.count("GET", URL);
  const liveBefore = (await counters(core)).liveHandles;

  const remote = await RemoteTodosQueryHandle.create(LIST, core);
  await waitFor("the first fetch", () => remote.status.peek() === "success");
  const ticker = await TickerQueryHandle.create(core);
  await waitFor("the first tick", () => (ticker.data.peek() ?? 0) >= 1);
  const feed = await FeedQueryHandle.create(false, core);
  await waitFor("the first page", () => feed.data.peek().length === 50);
  await feed.fetchNextPage();
  await waitFor("the second page", () => feed.data.peek().length === 100);
  const library = await Library.create(core);
  await waitFor("books[0]", () => library.books.get(0)?.id === 1);
  const roster = await RosterQueryHandle.create(7, core);
  await waitFor("the roster", () => roster.data.peek()?.[0] === "team 7");
  const counter = await Counter.create(core);
  await counter.add(5);
  const probe = await Probe.create(core);
  const handles = { remote: remote.handle, ticker: ticker.handle, feed: feed.handle, library: library.handle, roster: roster.handle };

  let snapshot: Uint8Array = new Uint8Array(0);

  await step("1. the handles and stores are open: the remote shows milk, the feed has 100 rows, books[0] is read", async () => {
    expect(remote.data.peek()).toEqual([milk]);
    expect(feed.data.peek().length).toBe(100);
    expect(counter.count.peek()).toBe(5);
    expect(gets()).toBe(1);
  });

  await step("2. snapshot, a change to the store and to the server, restore", async () => {
    snapshot = await core.snapshot();
    expect(snapshot.length).toBeGreaterThan(0);
    await counter.add(10);
    server.on("GET", URL, replies.json(200, [milk, dog]));
  });

  // What the wrappers' mirror registrations are applied from here on: the restore must send nothing for the remote and the feed.
  const remoteEntries = tapEntries(remote);
  const feedEntries = tapEntries(feed);
  const liveBeforeRestore = (await counters(core)).liveHandles;
  const getsBefore = gets();
  const wasAbsent: unknown[] = [];
  remote.data.subscribe((value) => {
    if (value === null) wasAbsent.push(value);
  });

  await step("3. same wrappers, nothing blinked: no entry for the remote or the feed, no request, the same live handles", async () => {
    await core.restore(snapshot);
    expect(counter.count.peek(), "the store went back to the snapshot").toBe(5);
    expect(remote.handle, "the same handle").toBe(handles.remote);
    expect(feed.handle).toBe(handles.feed);
    expect(remoteEntries, "the mirror applied nothing to the remote wrapper").toEqual([]);
    expect(feedEntries, "nor to the feed wrapper").toEqual([]);
    expect(wasAbsent, "the remote's data never became absent").toEqual([]);
    expect(remote.data.peek()).toEqual([milk]);
    expect(feed.data.peek().length).toBe(100);
    expect(gets(), "a restore fetches nothing").toBe(getsBefore);
    expect((await counters(core)).liveHandles, "no query handle was dropped and the restore made no handle of its own: only the probe went stale").toBe(
      liveBeforeRestore - 1,
    );
  });

  await step("4. refetch is accepted on the same remote wrapper", async () => {
    await remote.refetch();
    await waitFor("the refetch", () => remote.data.peek()?.length === 2 && remote.fetching.peek() === false);
    expect(gets()).toBe(getsBefore + 1);
    expect(remote.data.peek()).toEqual([milk, dog]);
  });

  await step("5. polling continues with no call from the runner", async () => {
    const shown = ticker.data.peek() as number;
    await waitFor("the ticker to advance", () => (ticker.data.peek() as number) > shown, { timeoutMs: 2_500 });
  });

  await step("6. the next page loads, and the library's rows are readable again", async () => {
    await feed.fetchNextPage();
    await waitFor("the third page", () => feed.data.peek().length === 150);
    await waitFor("books[0] through the new page server", () => library.books.get(0)?.id === 1);
  });

  await step("7. the probe, which is not re-creatable, is refused", async () => {
    const refused = await probe.counters().then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(refused).toBeInstanceOf(UndraCallError.Refused);
  });

  await step("8. restoring again changes nothing more", async () => {
    const live = (await counters(core)).liveHandles;
    remoteEntries.length = 0;
    feedEntries.length = 0;
    const requests = gets();
    await core.restore(snapshot);
    expect(counter.count.peek()).toBe(5);
    expect(remoteEntries).toEqual([]);
    expect(feedEntries).toEqual([]);
    expect(remote.data.peek()).toEqual([milk, dog]);
    expect(feed.data.peek().length).toBe(150);
    expect(gets()).toBe(requests);
    expect((await counters(core)).liveHandles).toBe(live);
  });

  await step("9. closing the wrappers gives every handle back", async () => {
    for (const wrapper of [remote, ticker, feed, library, roster, counter, probe]) wrapper.close();
    await waitFor("live_handles to return to where it was before step 1", async () => (await counters(core)).liveHandles === liveBefore);
  });

  await step("10. a fresh core of build B: the handles come back dormant and are built on first use; the changed query's is refused", async () => {
    const b = await boot({ build: "B" });
    const baseB = (await counters(b.core)).liveHandles;
    await b.core.restore(snapshot);
    // The stores of the snapshot (Counter, Library and its two page servers) and the three query handles build B honours.
    expect((await counters(b.core)).liveHandles - baseB, "re-issued: 2 stores, 2 page servers, 3 query handles").toBe(7);
    expect(b.server.count("GET", URL), "a restore, and an idle handle, fetch nothing").toBe(0);

    // The core's own state outside stores (S22): the server's address, then the server's answer.
    await configureRemote({ baseUrl: BASE_URL }, b.core);
    b.server.on("GET", URL, replies.json(200, [milk, dog]));
    await sleep(100);
    expect(b.server.count("GET", URL), "configuring is not using the handle").toBe(0);

    // The remote handle, observed again as after any reload: answered at once with `fetching` and no data, then the data.
    const remoteB = watch(b.core, handles.remote);
    await b.core.observe(handles.remote, ALL_SIGNALS, true);
    b.core.mirror.flush();
    const first = new Map(remoteB.map((e) => [e.signalId, e.value]));
    expect(decodeValue(QueryStatusCodec, first.get(1) as Uint8Array), "the observe is answered with the handle's values").toBe("fetching");
    expect(decodeValue(todos, first.get(0) as Uint8Array), "nothing cached: no data yet").toBeNull();
    await waitFor("the data fetched by the fresh core", () => {
      b.core.mirror.flush();
      const data = remoteB.filter((e) => e.signalId === 0).at(-1);
      return data !== undefined && decodeValue(todos, data.value)?.length === 2;
    });
    expect(b.server.count("GET", URL)).toBe(1);

    // `refetch` on the handle the host kept is accepted: status 0.
    await b.core.call({ target: CallTarget.ObjectMethod, handle: handles.remote }, UndraIds.Objects.RemoteTodosQueryHandle.refetch, new Uint8Array(0));
    await waitFor("the refetch", () => b.server.count("GET", URL) === 2);

    // The ticker's handle delivers ticks, from the fresh core's own counter.
    const tickerB = watch(b.core, handles.ticker);
    await b.core.observe(handles.ticker, ALL_SIGNALS, true);
    await waitFor("two ticks", () => {
      b.core.mirror.flush();
      return tickerB.filter((e) => e.signalId === 0).length >= 2;
    });

    // The library's `books` pages through the page server its op 0 names (a new server in the fresh core).
    const libraryB = watch(b.core, handles.library);
    await b.core.observe(handles.library, ALL_SIGNALS, true);
    b.core.mirror.flush();
    const books = libraryB.find((e) => e.signalId === 0);
    expect(books, "the books entry").toBeDefined();
    const lazy = decodeLazyValue((books as SignalUpdate).value);
    const page = b.core.callSync({ target: CallTarget.LazyListPage, handle: lazy.handle, offset: 0, limit: 3 }, 0, new Uint8Array(0));
    expect(readLazyPageHeader(new UndraReader(page)).total, "the library's rows through the new page server").toBe(10_000);

    // The handle of the query whose parameter type build B changed is refused, and the core logged why.
    const refused = await b.core
      .call({ target: CallTarget.ObjectMethod, handle: handles.roster }, UndraIds.Objects.RosterQueryHandle.refetch, new Uint8Array(0))
      .then(
        () => undefined,
        (e: unknown) => e,
      );
    expect(refused, "a refused record's handle is stale").toBeDefined();
    const named = `Handle(index=${Number(handles.roster & 0xffffffn)}, gen=${handles.roster >> 24n})`;
    const warns = b.log.records.filter((r) => r.level === 3 && r.message.includes(named));
    expect(warns.length, `a WARN names ${named}: ${JSON.stringify(b.log.records.filter((r) => r.level >= 3).map((r) => r.message))}`).toBeGreaterThanOrEqual(1);
    expect(warns[0]?.message).toContain("is not re-issued");
    expect(warns[0]?.message).toContain("the types it was made from changed");
  });
});
