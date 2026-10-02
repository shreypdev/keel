import { clock, h } from "../dom.js";
import { diffLines, diffValues, patchLines, patchSummary } from "../diff.js";
import { formatValue } from "../format.js";
import type { AppliedChange } from "../mirror.js";
import type { CommitEntry, DevtoolsState } from "../state.js";
import type { Actions, UiState } from "./ui-state.js";

function summary(c: AppliedChange): string {
  if (c.error !== undefined) return `${c.signal} ⚠`;
  if (c.op === "patch" && c.patch !== undefined) return `${c.signal} ${patchSummary(c.patch)}`;
  if (c.op === "lazy") return `${c.signal} invalidated`;
  const v = c.after;
  if (Array.isArray(v)) return `${c.signal} ${v.length} items`;
  return `${c.signal} ${formatValue(v, 24)}`;
}

function details(c: AppliedChange): HTMLElement {
  const title = `${c.store}.${c.signal}`;
  let lines: string[];
  if (c.error !== undefined) lines = [c.error];
  else if (c.op === "patch" && c.patch !== undefined) lines = patchLines(c.patch);
  else if (c.op === "lazy") lines = ["the list was invalidated: the platform pages it again"];
  else {
    lines = diffLines(diffValues(c.before, c.after));
    if (lines.length === 0) lines = ["written with the same value"];
  }
  return h("div", { class: "change" }, h("div", { class: "mono dim" }, title), h("pre", { class: "mono" }, lines.join("\n")));
}

function row(e: CommitEntry, state: DevtoolsState, ui: UiState, actions: Actions): HTMLElement {
  const key = `commit:${e.epoch}:${e.id}`;
  const open = ui.open.has(key);
  const shown = e.changes.slice(0, 3).map(summary);
  const more = e.changes.length - shown.length;
  const restorable = e.step !== undefined && e.epoch === state.epoch && state.restorableSteps.some((s) => s.step === e.step);
  const head = h(
    "button",
    { class: "row-head", "aria-expanded": open, onclick: () => { ui.toggle(key); window.dispatchEvent(new Event("undra:repaint")); } },
    h("span", { class: "time mono dim" }, state.welcome === undefined ? "" : clock(state.welcome.startedUnixMs + e.atMs)),
    h("span", { class: `cause${e.cause.kind === "restore" ? " restore" : ""}` }, e.label),
    h("span", { class: "chips mono" }, e.changes.length === 0 ? "nothing changed" : shown.join("  ·  "), more > 0 && `  ·  +${more}`),
    e.step !== undefined && h("span", { class: "step-badge mono" }, `step ${e.step}`),
  );
  return h(
    "li",
    { class: `entry${open ? " open" : ""}${e.epoch === state.epoch ? "" : " old"}` },
    head,
    open &&
      h(
        "div",
        { class: "entry-body" },
        ...e.changes.map(details),
        h(
          "div",
          { class: "entry-foot" },
          h("span", { class: "mono dim" }, `txn ${e.txn}`),
          restorable && h("button", { class: "btn", onclick: () => actions.restore(e.step as number) }, `Restore step ${e.step}`),
        ),
      ),
  );
}

export function renderTimeline(state: DevtoolsState, ui: UiState, actions: Actions): HTMLElement {
  const root = h("div", { class: "tab-body timeline" });
  if (state.timeline.length === 0) {
    root.append(h("p", { class: "empty" }, "Nothing has changed since this page attached."));
    return root;
  }
  root.append(
    h(
      "ol",
      { class: "entries" },
      ...state.timeline.map((e) => (e.kind === "divider" ? h("li", { class: "divider" }, h("span", null, e.text)) : row(e, state, ui, actions))),
    ),
  );
  return root;
}
