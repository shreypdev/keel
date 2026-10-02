import { clock, h } from "../dom.js";
import { formatValue, pretty } from "../format.js";
import type { DevtoolsState, PortEntry } from "../state.js";
import { decodeArgs, decodeReplyBody, type Value } from "../value.js";
import type { UiState } from "./ui-state.js";

const STATUS = ["ok", "error", "unavailable"] as const;

function latency(us: number | undefined): string {
  if (us === undefined) return "…";
  return us >= 1000 ? `${(us / 1000).toFixed(us >= 10000 ? 0 : 1)} ms` : `${us} µs`;
}

function decoded(state: DevtoolsState, p: PortEntry): { args: Value | undefined; reply: Value | undefined; error: string | undefined } {
  const method = state.schema?.portMethod(p.portId, p.methodId);
  if (method === undefined || state.schema === undefined) return { args: undefined, reply: undefined, error: undefined };
  try {
    const args = p.args.length === 0 && method.params.length > 0 ? undefined : decodeArgs(state.schema, method.params, p.args);
    const reply = p.status === undefined || p.reply === undefined ? undefined : decodeReplyBody(state.schema, method.returns, p.status, p.reply);
    return { args, reply, error: undefined };
  } catch (e) {
    return { args: undefined, reply: undefined, error: e instanceof Error ? e.message : String(e) };
  }
}

export function renderPorts(state: DevtoolsState, ui: UiState): HTMLElement {
  const root = h("div", { class: "tab-body ports" });
  if (state.ports.length === 0) {
    root.append(h("p", { class: "empty" }, "No port call yet. Calls the core makes to the platform (Http, Kv, ...) are listed here."));
    return root;
  }
  const rows = state.ports.map((p) => {
    const key = `port:${state.epoch}:${p.id}:${p.startMs}`;
    const open = ui.open.has(key);
    const { args, reply, error } = decoded(state, p);
    const status = p.status === undefined ? "pending" : (STATUS[p.status] ?? String(p.status));
    return h(
      "li",
      { class: `entry${open ? " open" : ""}` },
      h(
        "button",
        { class: "row-head", "aria-expanded": open, onclick: () => { ui.toggle(key); window.dispatchEvent(new Event("undra:repaint")); } },
        h("span", { class: "time mono dim" }, state.welcome === undefined ? "" : clock(state.welcome.startedUnixMs + p.startMs)),
        h("span", { class: "cause" }, p.name),
        h("span", { class: "chips mono" }, args === undefined ? "" : formatValue(args, 60)),
        h("span", { class: `status ${status}` }, status),
        h("span", { class: "latency mono" }, latency(p.latencyUs)),
      ),
      open &&
        h(
          "div",
          { class: "entry-body" },
          h("div", { class: "mono dim" }, "arguments"),
          h("pre", { class: "mono" }, error ?? (args === undefined ? "(none)" : pretty(args))),
          h("div", { class: "mono dim" }, "reply"),
          h("pre", { class: "mono" }, p.status === undefined ? "(waiting)" : p.status === 2 ? "the platform has no such port" : pretty(reply)),
        ),
    );
  });
  root.append(h("ol", { class: "entries" }, ...rows));
  return root;
}
