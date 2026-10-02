import { h } from "../dom.js";
import { formatValue, pretty } from "../format.js";
import type { StoreState } from "../mirror.js";
import type { SignalDef } from "../schema.js";
import { typeName } from "../schema.js";
import type { DevtoolsState } from "../state.js";
import type { Value } from "../value.js";
import type { UiState } from "./ui-state.js";

const ROWS_AT_FIRST = 40;
const ROWS_MORE = 200;
const MAX_COLUMNS = 6;

function valueClass(v: Value | undefined): string {
  if (v === undefined || v === null) return "v-null";
  switch (typeof v) {
    case "string":
      return "v-str";
    case "number":
    case "bigint":
      return "v-num";
    case "boolean":
      return "v-bool";
    default:
      return "v-obj";
  }
}

function isRecord(v: Value | undefined): v is { [field: string]: Value } {
  return typeof v === "object" && v !== null && !Array.isArray(v) && !(v instanceof Uint8Array) && !("$" in v) && !("$ts" in v) && !("$dur" in v);
}

/** A list of records as a table: the columns are the first row's fields. */
function table(key: string, rows: readonly Value[], ui: UiState): HTMLElement {
  const first = rows.find(isRecord);
  const cols = first === undefined ? [] : Object.keys(first).slice(0, MAX_COLUMNS);
  const limit = ui.rows.get(key) ?? ROWS_AT_FIRST;
  const shown = rows.slice(0, limit);
  const head = h("tr", null, h("th", { class: "idx" }, "#"), ...(cols.length === 0 ? [h("th", null, "value")] : cols.map((c) => h("th", null, c))));
  const body = shown.map((row, i) =>
    h(
      "tr",
      null,
      h("td", { class: "idx" }, i),
      ...(cols.length === 0 || !isRecord(row)
        ? [h("td", { class: valueClass(row) }, formatValue(row, 90))]
        : cols.map((c) => h("td", { class: valueClass(row[c]), title: pretty(row[c]) }, formatValue(row[c], 40)))),
    ),
  );
  const more = rows.length - shown.length;
  return h(
    "div",
    { class: "table-wrap" },
    h("table", { class: "grid" }, h("thead", null, head), h("tbody", null, ...body)),
    more > 0 &&
      h("button", { class: "more", onclick: () => { ui.rows.set(key, limit + ROWS_MORE); ui.toggle(`rerender:${key}`); window.dispatchEvent(new Event("undra:repaint")); } }, `show ${Math.min(more, ROWS_MORE)} more of ${more}`),
  );
}

function signalRow(store: StoreState, def: SignalDef | undefined, id: number, ui: UiState, state: DevtoolsState): HTMLElement {
  const sig = store.signals.get(id);
  const value = sig?.value;
  const key = `${store.handle}:${id}`;
  const name = def?.name ?? `signal ${id}`;
  const flash = state.dirty.has(key);
  const isList = Array.isArray(value) && value.length > 0 && (def?.key !== null || value.every(isRecord));
  const label = h("div", { class: "sig-name" }, h("span", { class: "mono" }, name), def?.computed === true && h("span", { class: "tag" }, "computed"), def?.key != null && h("span", { class: "tag" }, `key ${def.key}`));
  const type = h("div", { class: "sig-type mono dim" }, def === undefined ? "" : typeName(def.ty));
  let body: HTMLElement;
  if (sig?.error !== undefined) {
    body = h("div", { class: "sig-error" }, sig.error);
  } else if (value === undefined) {
    body = h("div", { class: "v-null mono" }, sig?.invalidated === true ? "invalidated: the platform pages it" : "waiting for a value");
  } else if (isList && Array.isArray(value)) {
    body = h("div", { class: "sig-list" }, h("div", { class: "dim mono" }, `${value.length} rows`), table(key, value, ui));
  } else if (typeof value === "object" && value !== null && !(value instanceof Uint8Array) && formatValue(value, 400).length > 70) {
    body = h(
      "details",
      { open: ui.open.has(key), ontoggle: (e) => { const d = e.currentTarget as HTMLDetailsElement; if (d.open) ui.open.add(key); else ui.open.delete(key); } },
      h("summary", { class: `mono ${valueClass(value)}` }, formatValue(value, 70)),
      h("pre", { class: "mono" }, pretty(value)),
    );
  } else {
    body = h("div", { class: `mono ${valueClass(value)}` }, formatValue(value, 400));
  }
  return h("div", { class: `sig${flash ? " flash" : ""}`, "data-signal": key }, label, type, body);
}

export function renderStores(state: DevtoolsState, ui: UiState): HTMLElement {
  const root = h("section", { class: "panel stores", "aria-label": "Stores" });
  const mirror = state.mirror;
  const stores = mirror === undefined ? [] : [...mirror.stores.values()];
  root.append(h("h2", null, "Stores", h("span", { class: "count" }, stores.length)));
  if (stores.length === 0) {
    root.append(h("p", { class: "empty" }, "No store yet. Open the app: stores appear as it builds them."));
    return root;
  }
  for (const store of stores) {
    const def = store.def;
    const ids = new Set<number>([...(def?.store?.signals.map((s) => s.signal_id) ?? []), ...store.signals.keys()]);
    const rows = [...ids].map((id) => signalRow(store, def?.store?.signals.find((s) => s.signal_id === id), id, ui, state));
    root.append(
      h(
        "article",
        { class: "store" },
        h("header", null, h("span", { class: "store-name" }, def?.name ?? `store ${store.typeId}`), h("span", { class: "handle mono dim" }, `#${store.handle.toString(16)}`)),
        ...rows,
      ),
    );
  }
  return root;
}
