import { useSignal } from "@undra/runtime/react";
import { useState } from "react";
import type { StatsChannel } from "./embed-stats";
import type { Playground } from "./undra";
import { type PlaygroundParams, type TabId, resolveTab } from "./url-params";
import { BigListView } from "./views/BigListView";
import { CounterView } from "./views/CounterView";
import { DebugPanel } from "./views/DebugPanel";
import { LiveView } from "./views/LiveView";
import { NotesView } from "./views/NotesView";
import { RemoteView } from "./views/RemoteView";
import { StressView } from "./views/StressView";
import { TodosView } from "./views/TodosView";

const TABS = [
  { id: "todos", label: "Todos" },
  { id: "counter", label: "Counter" },
  { id: "biglist", label: "10k list" },
  { id: "remote", label: "Remote" },
  { id: "stress", label: "Stress" },
  { id: "live", label: "Live" },
  { id: "notes", label: "Notes" },
] as const satisfies readonly { readonly id: TabId; readonly label: string }[];

/**
 * Seven views over one core. Each reads the signals of its store with `useSignal` (`@undra/runtime/react`) and calls its methods.
 * Live and Notes use the opt-in WebSocket and Db ports (ADR-047, ADR-048); Live's address is `?ws=`.
 *
 * The first view comes from the URL: `?screen=` (`todos`, `counter`, `list`, `remote`, `stress`, `live`, `notes`), else the
 * `#fragment` (`#counter`, so a view can be linked to and reloaded). With `?embed=1` the page is
 * only that view, with no tab bar or heading, for the landing page's iframe; `channel` is then the
 * line to that page, which the stress screen posts its numbers through.
 */
export function App({
  playground,
  params,
  channel,
}: {
  readonly playground: Playground;
  readonly params: PlaygroundParams;
  readonly channel?: StatsChannel | undefined;
}) {
  const [tab, setTab] = useState<TabId>(() => resolveTab(params, location.hash));
  // `?autostart=1` is for the view the page opened on: coming back to the stress screen later does not start it again.
  const [autostart, setAutostart] = useState(params.autostart);
  // A crash restart (ADR-049) keeps the stores but not what they held outside their signals: the Live view's `Live`
  // goes stale and the Notes store's open database is gone, so both views are mounted afresh after each restart (a new
  // `Live`, the database opened again).
  const lastRestart = useSignal(playground.restarts.entries)?.[0]?.at ?? 0;

  const choose = (next: TabId): void => {
    setAutostart(false);
    setTab(next);
    history.replaceState(null, "", `#${next}`);
  };

  const view = (
    <>
      {tab === "todos" && <TodosView todos={playground.todos} />}
      {tab === "counter" && <CounterView />}
      {tab === "biglist" && <BigListView bigList={playground.bigList} autoStream={params.stream} />}
      {tab === "remote" && <RemoteView playground={playground} />}
      {tab === "stress" && <StressView channel={channel} initialRate={params.rate} initialMode={params.mode} autostart={autostart} />}
      {tab === "live" && <LiveView key={lastRestart} initialUrl={params.ws} />}
      {tab === "notes" && <NotesView key={lastRestart} />}
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
        <h1>Undra playground</h1>
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
      <DebugPanel playground={playground} />
    </main>
  );
}
