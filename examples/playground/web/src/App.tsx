import { useState } from "react";
import type { Playground } from "./keel";
import { BigListView } from "./views/BigListView";
import { CounterView } from "./views/CounterView";
import { RemoteView } from "./views/RemoteView";
import { TodosView } from "./views/TodosView";

const TABS = [
  { id: "todos", label: "Todos" },
  { id: "counter", label: "Counter" },
  { id: "biglist", label: "10k list" },
  { id: "remote", label: "Remote" },
] as const;

type TabId = (typeof TABS)[number]["id"];

/** The tab named by the URL fragment (`#counter`), so a view can be linked to and reloaded. */
function tabFromHash(): TabId {
  const wanted = location.hash.slice(1);
  return TABS.find((tab) => tab.id === wanted)?.id ?? "todos";
}

/** Four views over one core. Each reads the signals of its store with `useSignal` (`@keel/runtime/react`) and calls its methods. */
export function App({ playground }: { readonly playground: Playground }) {
  const [tab, setTab] = useState<TabId>(tabFromHash);

  const choose = (next: TabId): void => {
    setTab(next);
    history.replaceState(null, "", `#${next}`);
  };

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
        {tab === "todos" && <TodosView todos={playground.todos} />}
        {tab === "counter" && <CounterView />}
        {tab === "biglist" && <BigListView bigList={playground.bigList} />}
        {tab === "remote" && <RemoteView playground={playground} />}
      </section>
    </main>
  );
}
