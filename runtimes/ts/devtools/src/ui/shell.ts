import { h, replace } from "../dom.js";
import type { DevtoolsState } from "../state.js";
import { renderCounters } from "./counters.js";
import { renderPorts } from "./ports.js";
import { renderQueries } from "./queries.js";
import { renderStores } from "./stores.js";
import { renderTimeline } from "./timeline.js";
import { type Actions, savedTheme, saveTheme, type Tab, UiState } from "./ui-state.js";

const TABS: readonly { readonly id: Tab; readonly label: string }[] = [
  { id: "timeline", label: "Timeline" },
  { id: "ports", label: "Ports" },
  { id: "queries", label: "Queries" },
  { id: "counters", label: "Counters" },
];

function applyTheme(theme: "dark" | "light" | undefined): void {
  if (theme === undefined) document.documentElement.removeAttribute("data-theme");
  else document.documentElement.setAttribute("data-theme", theme);
}

function currentTheme(): "dark" | "light" {
  const set = document.documentElement.getAttribute("data-theme");
  if (set === "dark" || set === "light") return set;
  return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

function header(state: DevtoolsState): HTMLElement {
  const w = state.welcome;
  const conn = state.conn === "open" ? "connected" : state.conn === "closed" ? "disconnected" : state.conn;
  return h(
    "header",
    { class: "bar" },
    h("div", { class: "brand" }, h("span", { class: "mark", "aria-hidden": "true" }), "undra", h("span", { class: "dim" }, "devtools")),
    h("div", { class: "core mono dim" }, w === undefined ? "" : `${w.platform} · ${w.mode} · schema 0x${w.schemaHash.toString(16).padStart(16, "0").slice(0, 8)}`),
    h(
      "div",
      { class: "pills" },
      h("span", { class: `pill ${state.conn === "open" ? "ok" : state.conn === "closed" ? "bad" : "warn"}` }, h("i"), conn),
      h("span", { class: `pill ${state.app.connected ? "ok" : ""}` }, h("i"), state.app.connected ? `app: ${state.app.platform}` : "no app attached"),
      h(
        "button",
        {
          class: "btn ghost",
          title: "Switch between the dark and the light theme",
          "aria-label": "Switch theme",
          onclick: () => {
            const next = currentTheme() === "dark" ? "light" : "dark";
            applyTheme(next);
            saveTheme(next);
          },
        },
        "Theme",
      ),
    ),
  );
}

function scrubber(state: DevtoolsState, ui: UiState, actions: Actions): HTMLElement {
  const steps = state.restorableSteps;
  const latest = state.steps.at(-1);
  const live = latest === undefined ? undefined : latest.restoredFrom !== 0 ? latest.restoredFrom : latest.step;
  const position = Math.max(0, steps.findIndex((s) => s.step === (ui.scrubbing ?? live)));
  const shown = ui.scrubbing ?? live;
  const travelled = latest !== undefined && latest.restoredFrom !== 0;
  const head = [...state.steps].reverse().find((s) => s.restoredFrom === 0);
  const note = state.travel;
  const range = h("input", {
    type: "range",
    min: 0,
    max: Math.max(0, steps.length - 1),
    value: position,
    step: 1,
    class: "range",
    "aria-label": "Time travel: step",
    disabled: steps.length < 2 || state.conn !== "open",
    oninput: (e) => {
      ui.scrubbing = steps[Number((e.currentTarget as HTMLInputElement).value)]?.step;
      const label = document.querySelector(".scrub-label");
      if (label !== null && ui.scrubbing !== undefined) label.textContent = `step ${ui.scrubbing}`;
    },
    onchange: (e) => {
      const target = steps[Number((e.currentTarget as HTMLInputElement).value)]?.step;
      ui.scrubbing = undefined;
      if (target !== undefined) actions.restore(target);
    },
  });
  const nudge = (by: number): (() => void) => () => {
    const to = steps[position + by]?.step;
    if (to !== undefined) actions.restore(to);
  };
  return h(
    "section",
    { class: "scrubber", "aria-label": "Time travel" },
    h("div", { class: "scrub-label mono" }, shown === undefined ? "no steps yet" : `step ${shown}`),
    h("button", { class: "btn ghost", title: "One step back", disabled: position <= 0 || steps.length < 2, onclick: nudge(-1) }, "◂"),
    range,
    h("button", { class: "btn ghost", title: "One step forward", disabled: position >= steps.length - 1, onclick: nudge(1) }, "▸"),
    h("span", { class: "mono dim" }, latest === undefined ? "" : `of ${state.currentStep}`),
    h(
      "button",
      { class: "btn", disabled: !travelled || head === undefined, onclick: () => head !== undefined && actions.restore(head.step) },
      "Live",
    ),
    note !== undefined && h("span", { class: `note ${note.ok ? "ok" : "bad"}` }, note.message),
    steps.length < 2 && note === undefined && h("span", { class: "dim hint" }, "Change something in the app: each change is a step you can go back to."),
  );
}

/** Builds the page into `root` and keeps it up to date with `state`. */
export function mount(root: HTMLElement, state: DevtoolsState, actions: Actions): UiState {
  const theme = savedTheme();
  if (theme !== undefined) applyTheme(theme);
  const ui = new UiState();
  const slots = {
    header: h("div", { class: "slot" }),
    scrubber: h("div", { class: "slot" }),
    stores: h("div", { class: "col stores-col" }),
    tabs: h("div", { class: "tabs", role: "tablist" }),
    body: h("div", { class: "scroll" }),
  };
  const notices = h("div", { class: "notices", "aria-live": "polite" });
  replace(
    root,
    slots.header,
    slots.scrubber,
    h("main", { class: "grid-main" }, slots.stores, h("div", { class: "col right-col" }, slots.tabs, slots.body)),
    notices,
  );

  const paint = (what: ReadonlySet<string>): void => {
    const all = what.has("all");
    if (all || what.has("conn") || what.has("app") || what.has("stores")) replace(slots.header, header(state));
    if (all || ["steps", "travel", "conn"].some((w) => what.has(w))) replace(slots.scrubber, scrubber(state, ui, actions));
    if (all || what.has("stores")) {
      replace(slots.stores, renderStores(state, ui));
      state.dirty.clear();
    }
    const counts: Record<Tab, number> = { timeline: state.timeline.filter((e) => e.kind === "commit").length, ports: state.ports.length, queries: state.queries.rows.length, counters: 0 };
    if (all || ["timeline", "ports", "queries", "steps"].some((w) => what.has(w)) || what.has("tab")) {
      replace(
        slots.tabs,
        ...TABS.map((t) =>
          h(
            "button",
            {
              class: `tab${ui.tab === t.id ? " on" : ""}`,
              role: "tab",
              "aria-selected": ui.tab === t.id,
              onclick: () => {
                ui.tab = t.id;
                paint(new Set(["tab", "body"]));
              },
            },
            t.label,
            counts[t.id] > 0 && h("span", { class: "count" }, counts[t.id]),
          ),
        ),
      );
    }
    const bodyChanged: Record<Tab, string[]> = { timeline: ["timeline", "steps"], ports: ["ports"], queries: ["queries"], counters: ["stats"] };
    if (all || what.has("tab") || what.has("body") || bodyChanged[ui.tab].some((w) => what.has(w))) {
      const keep = slots.body.scrollTop;
      const body =
        ui.tab === "timeline" ? renderTimeline(state, ui, actions) : ui.tab === "ports" ? renderPorts(state, ui) : ui.tab === "queries" ? renderQueries(state, ui) : renderCounters(state);
      replace(slots.body, body);
      slots.body.scrollTop = keep;
    }
    if (all || what.has("notices")) {
      replace(notices, ...state.notices.slice(0, 2).map((n) => h("div", { class: "notice" }, n)));
    }
  };

  let scheduled: Set<string> | undefined;
  const schedule = (what: ReadonlySet<string>): void => {
    if (scheduled === undefined) {
      scheduled = new Set(what);
      requestAnimationFrame(() => {
        const now = scheduled as Set<string>;
        scheduled = undefined;
        paint(now);
      });
    } else for (const w of what) scheduled.add(w);
  };
  state.onChange(schedule);
  window.addEventListener("undra:repaint", () => schedule(new Set(["all"])));
  paint(new Set(["all"]));
  return ui;
}
