import type { ConnectionState, UndraCore } from "@undra/runtime";

/** What a user of `undra dev` reads about the connection, in one line. */
function describe(url: string, state: ConnectionState): string {
  switch (state.kind) {
    case "connecting":
      return `Connecting to ${url}`;
    case "connected":
      return `Dev server: ${url}`;
    case "reconnecting":
      return `Reconnecting to ${url} (attempt ${String(state.attempt)})`;
    case "closed":
      switch (state.reason) {
        case "sessionLost":
          return "The core was rebuilt: reloading";
        case "schemaMismatch":
          return "The schema changed, state reset: run undra bindgen, then reload";
        case "requested":
          return "Disconnected";
        case "failed":
          return `Connection failed: ${state.error?.message ?? "unknown error"}`;
      }
  }
}

const COLORS = { connected: "#2e7d32", closed: "#c62828", other: "#ef6c00" } as const;

/** How long a message from the dev server stays in the bar. */
const NOTICE_MS = 4000;

let show: ((message: string) => void) | undefined;
let early: string | undefined;

/**
 * What `undra dev` says about itself ("Reloaded, state kept" after it rebuilt the core, ADR-053): pass it as the
 * `onDevNotice` option of `UndraCore.load`. The bar shows it for a few seconds. It is only ever called for a core
 * that `undra dev` serves.
 */
export function onDevNotice(message: string): void {
  if (show === undefined) early = message;
  else show(message);
}

/**
 * Shows what the connection to `undra dev` is doing as a thin bar at the top of the page: green while
 * connected, amber while the runtime reconnects, red when the connection is over, and for a few seconds what the
 * dev server says about a reload. `core.connection` is a signal, so this is all the code it takes.
 */
export function showDevConnection(core: UndraCore, url: string): void {
  const bar = document.createElement("div");
  bar.setAttribute("role", "status");
  bar.dataset["testid"] = "dev-status";
  bar.style.cssText = "position:fixed;top:0;left:0;right:0;z-index:1000;padding:2px 10px;font:12px system-ui,sans-serif;color:#fff;";
  let notice: string | undefined;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const paint = (): void => {
    const state = core.connection.peek();
    bar.textContent = notice ?? describe(url, state);
    bar.style.background = state.kind === "connected" ? COLORS.connected : state.kind === "closed" ? COLORS.closed : COLORS.other;
  };
  show = (message) => {
    notice = message;
    paint();
    clearTimeout(timer);
    timer = setTimeout(() => {
      notice = undefined;
      paint();
    }, NOTICE_MS);
  };
  paint();
  core.connection.subscribe(paint);
  document.body.append(bar);
  if (early !== undefined) {
    show(early);
    early = undefined;
  }
}
