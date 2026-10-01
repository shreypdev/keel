import { bytesText, h } from "../dom.js";
import type { DevtoolsState, StatsSample } from "../state.js";

const n = (v: unknown): number => (typeof v === "number" ? v : 0);

/** Per second between the two newest samples, for a counter that only grows. */
function rate(samples: readonly StatsSample[], read: (s: StatsSample) => number): string {
  const last = samples.at(-1);
  const before = samples.at(-2);
  if (last === undefined || before === undefined || last.atMs <= before.atMs) return "–";
  const per = ((read(last) - read(before)) * 1000) / (last.atMs - before.atMs);
  return per < 10 ? per.toFixed(1) : String(Math.round(per));
}

function group(title: string, note: string, rows: readonly [string, string][]): HTMLElement {
  return h(
    "section",
    { class: "counter-group" },
    h("h3", null, title),
    h("p", { class: "note" }, note),
    h("dl", null, ...rows.flatMap(([k, v]) => [h("dt", null, k), h("dd", { class: "mono" }, v)])),
  );
}

export function renderCounters(state: DevtoolsState): HTMLElement {
  const root = h("div", { class: "tab-body counters" });
  const last = state.stats.at(-1);
  if (last === undefined) {
    root.append(h("p", { class: "empty" }, "Waiting for the first sample."));
    return root;
  }
  const core = last.core;
  const crossings = (core["crossings"] ?? {}) as Record<string, unknown>;
  const s = last.server;
  const commits = n(s["commits"]);
  const steps = n(s["steps"]);
  root.append(
    group("Core", "undra_stats_json, sampled every second.", [
      ["stores / handles", `${n(core["live_stores"])} / ${n(core["live_handles"])}`],
      ["tasks", String(n(core["tasks"]))],
      ["calls in flight", String(n(core["active_calls"]))],
      ["port calls pending", String(n(core["pending_port_calls"]))],
      ["timers pending", String(n(core["pending_timers"]))],
      ["change-sets", `${n(crossings["change_sets"])}  (${rate(state.stats, (x) => n((x.core["crossings"] as Record<string, unknown> | undefined)?.["change_sets"]))}/s)`],
      ["change-set bytes", bytesText(n(crossings["change_set_bytes"]))],
      ["core turns", String(n(core["turns"]))],
      ["panics", String(n(core["panics"]))],
    ]),
    group("Dev server", "What the server sees. The app's own mirror counts its drains, merges and backlog itself (SPEC 11.1); this page cannot see those.", [
      ["commits", `${commits}  (${rate(state.stats, (x) => n(x.server["commits"]))}/s)`],
      ["entries forwarded", String(n(s["entries"]))],
      ["steps taken", String(steps)],
      ["commits merged per step", steps === 0 ? "–" : (commits / steps).toFixed(1)],
      ["history kept", `${n(s["ring_steps"])} steps, ${bytesText(n(s["ring_bytes"]))}`],
      ["app connection backlog", s["app_connected"] === true ? bytesText(n(s["app_backlog_bytes"])) : "no app attached"],
      ["port calls to the app", String(n(s["port_calls"]))],
      ["port calls with no app", String(n(s["unattended_port_calls"]))],
      ["pages open", String(n(s["pages"]))],
    ]),
  );
  return root;
}
