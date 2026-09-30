import { useState } from "react";
import type { Playground } from "./keel";
import { type PlaygroundParams, type TabId, resolveTab } from "./url-params";
import { BigListView } from "./views/BigListView";
import { CounterView } from "./views/CounterView";
import { RemoteView } from "./views/RemoteView";
import { TodosView } from "./views/TodosView";

const TABS = [
  { id: "todos", label: "Todos" },
  { id: "counter", label: "Counter" },
  { id: "biglist", label: "10k list" },
  { id: "remote", label: "Remote" },
] as const satisfies readonly { readonly id: TabId; readonly label: string }[];

/**
 * Four views over one core. Each reads the signals of its store with `useSignal` (`@keel/runtime/react`) and calls its methods.
 *
 * The first view comes from the URL: `?screen=` (`todos`, `counter`, `list`, `remote`), else the
 * `#fragment` (`#counter`, so a view can be linked to and reloaded). With `?embed=1` the page is
 * only that view, with no tab bar or heading, for the landing page's iframe.
 */
export function App({ playground, params }: { readonly playground: Playground; readonly params: PlaygroundParams }) {
  const [tab, setTab] = useState<TabId>(() => resolveTab(params, location.hash));

  const choose = (next: TabId): void => {
    setTab(next);
    history.replaceState(null, "", `#${next}`);
  };

  const view = (
    <>
      {tab === "todos" && <TodosView todos={playground.todos} />}
      {tab === "counter" && <CounterView />}
      {tab === "biglist" && <BigListView bigList={playground.bigList} autoStream={params.stream} />}
      {tab === "remote" && <RemoteView playground={playground} />}
    </>
  );

  // Embedded, there are no tabs for a tabpanel to belong to: the page is just the one view.
  if (params.embed) {
    return (
      <main>
        <section className="panel" aria-label={TABS.find(({ id }) => id === tab)?.label}>
          {view}
        </section>
      </main>
    );
  }

  return (
    <main>
      <header>
        <h1>Keel playground</h1>
        <p className="tagline">One Rust core. This page is only the UI.</p>
      </header>
      <div role="tablist" aria-label="Views" className="tabs">
        {TABS.map(({ id, label }) => (
          <button
            key={id}
            role="tab"
            id={`tab-${id}`}
            aria-selected={tab === id}
            aria-controls={`panel-${id}`}
            data-testid={`tab-${id}`}
            onClick={() => choose(id)}
          >
            {label}
          </button>
        ))}
      </div>
      <section role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`} className="panel">
        {view}
      </section>
    </main>
  );
}
