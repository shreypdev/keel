import { RemoteTodosQueryHandle, Todos, UndraIds, configureRemote } from "@playground/core";
import { useSignal } from "@undra/runtime/react";
import { PreviewCore, RecordedCore, type Seed, parseSeed, response } from "@undra/testkit";
import { type ReactElement, useEffect, useState } from "react";
import wasmUrl from "../../../build/web/playground_core.wasm?url";
// The testing kit's fixtures (docs/TESTING.md): a recorded session of the Todos store, and a seed for the fakes.
import seedText from "../../../../../testkit/fixtures/seed.json?raw";
import sessionText from "../../../../../testkit/fixtures/session-todos.json?raw";
import { TodosView } from "../views/TodosView";
import type { Meta, Story } from "./story";

export default { title: "Todos" } satisfies Meta;

/** Runs `load` once, shows `fallback` until it resolves, then what `show` makes of the result. */
function useLoaded<T>(load: () => Promise<T>, onDispose: (value: T) => void = () => undefined): T | undefined {
  const [value, setValue] = useState<T>();
  useEffect(() => {
    let live = true;
    let loaded: T | undefined;
    void load().then((v) => {
      loaded = v;
      if (live) setValue(v);
      else onDispose(v);
    });
    return () => {
      live = false;
      if (loaded !== undefined) onDispose(loaded);
    };
    // The story loads once per mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return value;
}

const Loading = (): ReactElement => <p>Loading…</p>;

/** A recorded session played to a point: no core runs, so the state is the same on every machine and costs nothing to reach. */
function RecordedTodos({ startAtMs, playToEnd }: { readonly startAtMs?: number; readonly playToEnd: boolean }): ReactElement {
  const todos = useLoaded(
    async () => {
      const recorded = await RecordedCore.load(sessionText, { expectedSchemaHash: UndraIds.schemaHash, shared: false, ...(startAtMs !== undefined && { startAtMs }) });
      const store = await Todos.create(recorded.core);
      if (playToEnd) await recorded.playAll();
      return { recorded, store };
    },
    ({ recorded }) => recorded.close(),
  );
  return todos === undefined ? <Loading /> : <TodosView todos={todos.store} />;
}

/** The recording played to its end: three items, one done. */
export const RecordedSessionPlayedToTheEnd: Story = {
  name: "A recorded session, played to the end",
  render: () => <RecordedTodos playToEnd />,
};

/** The same recording from 400 ms in: three items, none done yet. */
export const RecordedSessionAt400ms: Story = {
  name: "The same recording at 400 ms",
  render: () => <RecordedTodos startAtMs={400} playToEnd={false} />,
};

/** The app's own wasm core running its real logic, on the deterministic fakes. */
function RealTodos(): ReactElement {
  const todos = useLoaded(
    async () => {
      const preview = await PreviewCore.load({ wasm: wasmUrl, expectedSchemaHash: UndraIds.schemaHash, seed: seedText, shared: false });
      const store = await Todos.create(preview.core);
      for (const title of ["Buy milk", "Walk the dog", "Write the docs"]) await store.add(title);
      return { preview, store };
    },
    ({ preview }) => preview.close(),
  );
  return todos === undefined ? <Loading /> : <TodosView todos={todos.store} />;
}

/** Interactive: type, add, toggle; the logic is the Rust core's. */
export const RealCoreOnFakes: Story = {
  name: "The real core on scripted ports",
  render: () => <RealTodos />,
};

/** A query over the seeded `Http` fake; the buttons move the preview's manual clock. */
function SeededQuery(): ReactElement {
  const loaded = useLoaded(
    async () => {
      const seed: Seed = parseSeed(seedText);
      const preview = await PreviewCore.load({ wasm: wasmUrl, expectedSchemaHash: UndraIds.schemaHash, seed, shared: false });
      await configureRemote({ baseUrl: "https://api.test" }, preview.core);
      const inbox = await RemoteTodosQueryHandle.create("inbox", preview.core);
      await preview.settle();
      return { preview, inbox };
    },
    ({ preview }) => preview.close(),
  );
  return loaded === undefined ? <Loading /> : <QueryPanel preview={loaded.preview} inbox={loaded.inbox} />;
}

function QueryPanel({ preview, inbox }: { readonly preview: PreviewCore; readonly inbox: RemoteTodosQueryHandle }): ReactElement {
  const data = useSignal(inbox.data);
  const status = useSignal(inbox.status);
  const [now, setNow] = useState(preview.clock.nowMs());
  const advance = async (ms: number): Promise<void> => {
    await preview.advance(ms);
    setNow(preview.clock.nowMs());
  };
  const serverChanges = async (): Promise<void> => {
    preview.fakes.http.reset();
    preview.fakes.http.respond("https://api.test/lists/inbox/todos", response(200, JSON.stringify([{ id: 3, title: "A new item on the server", done: false }])));
    await inbox.refetch();
    await preview.settle();
  };
  return (
    <>
      <h2>Inbox ({status})</h2>
      <ul>{data?.map((t) => <li key={t.id}>{t.title}</li>)}</ul>
      <p>
        Clock: {new Date(now).toISOString()} · requests so far: {preview.fakes.http.calls.length}
      </p>
      <p className="row">
        <button onClick={() => void advance(10_000)}>Advance 10 s</button>
        <button onClick={() => void advance(31_000)}>Advance 31 s</button>
        <button onClick={() => void serverChanges()}>The server changes, refetch</button>
      </p>
    </>
  );
}

export const SeededQueryWithAManualClock: Story = {
  name: "A query on a seeded server, with a manual clock",
  render: () => <SeededQuery />,
};
