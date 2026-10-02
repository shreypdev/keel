import { clock, h } from "../dom.js";
import { formatValue, pretty } from "../format.js";
import type { DevtoolsState } from "../state.js";
import type { UiState } from "./ui-state.js";

function age(updatedAt: number | null): string {
  if (updatedAt === null) return "never";
  const s = Math.max(0, Math.round((Date.now() - updatedAt) / 1000));
  return s < 90 ? `${s}s ago` : `${Math.round(s / 60)}m ago`;
}

export function renderQueries(state: DevtoolsState, ui: UiState): HTMLElement {
  const root = h("div", { class: "tab-body queries" });
  const q = state.queries;
  if (q.rows.length === 0 && state.queryLog.length === 0) {
    root.append(h("p", { class: "empty" }, "The query cache is empty. Queries show up when a screen observes one."));
    return root;
  }
  root.append(
    h("div", { class: "meta mono dim" }, `${q.rows.length} cached  ·  ${q.online ? "online" : "offline"}  ·  ${q.pendingMutations} queued mutations`),
    h(
      "ol",
      { class: "entries" },
      ...q.rows.map((r) => {
        const key = `query:${r.queryId}:${r.key}`;
        const open = ui.open.has(key);
        return h(
          "li",
          { class: `entry${open ? " open" : ""}` },
          h(
            "button",
            { class: "row-head", "aria-expanded": open, onclick: () => { ui.toggle(key); window.dispatchEvent(new Event("undra:repaint")); } },
            h("span", { class: "cause" }, r.name),
            h("span", { class: "chips mono" }, r.key),
            h("span", { class: `status ${r.status}` }, r.fetching && r.status === "success" ? "refreshing" : r.status),
            h("span", { class: "latency mono dim" }, `${r.observers} watching · ${age(r.updatedAt)}`),
          ),
          open &&
            h(
              "div",
              { class: "entry-body" },
              h("div", { class: "mono dim" }, `data${r.invalidated ? " (invalidated)" : ""}${r.layers > 0 ? ` (${r.layers} optimistic)` : ""}`),
              h("pre", { class: "mono" }, r.data === undefined ? (r.dataLen > 0 ? `${r.dataLen} bytes (too large to show)` : "(none)") : pretty(r.data)),
              r.error !== undefined && h("div", { class: "mono dim" }, "error"),
              r.error !== undefined && h("pre", { class: "mono" }, pretty(r.error)),
            ),
        );
      }),
    ),
    h("h3", null, "Cache events"),
    h(
      "ul",
      { class: "log mono" },
      ...state.queryLog.slice(0, 60).map((e) =>
        h("li", null, h("span", { class: "dim" }, state.welcome === undefined ? "" : clock(state.welcome.startedUnixMs + e.atMs)), h("span", { class: `status ${e.event}` }, e.event), `${e.name}  ${formatValue(e.key, 50)}`),
      ),
    ),
  );
  return root;
}
