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
          return "The schema changed: run undra bindgen, then reload";
        case "requested":
          return "Disconnected";
        case "failed":
          return `Connection failed: ${state.error?.message ?? "unknown error"}`;
      }
  }
}

const COLORS = { connected: "#2e7d32", closed: "#c62828", other: "#ef6c00" } as const;

/**
 * Shows what the connection to `undra dev` is doing as a thin bar at the top of the page: green while
 * connected, amber while the runtime reconnects, red when the connection is over. `core.connection` is
 * a signal, so this is all the code it takes.
 */
export function showDevConnection(core: UndraCore, url: string): void {
  const bar = document.createElement("div");
  bar.setAttribute("role", "status");
  bar.dataset["testid"] = "dev-status";
  bar.style.cssText = "position:fixed;top:0;left:0;right:0;z-index:1000;padding:2px 10px;font:12px system-ui,sans-serif;color:#fff;";
  const paint = (state: ConnectionState): void => {
    bar.textContent = describe(url, state);
    bar.style.background = state.kind === "connected" ? COLORS.connected : state.kind === "closed" ? COLORS.closed : COLORS.other;
  };
  paint(core.connection.peek());
  core.connection.subscribe(paint);
  document.body.append(bar);
}
