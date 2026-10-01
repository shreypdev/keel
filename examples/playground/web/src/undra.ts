import { PortIds, type PortImpl, UndraSessionLostError, type UndraUnhandledError, emitConnectivity } from "@undra/runtime";
import { dbPort, waSqliteDb } from "@undra/runtime/db";
import { browserWebSocket, fetchSse, ssePort, webSocketPort } from "@undra/runtime/realtime";
import { BigList, RemoteTodosQueryHandle, Todos, UndraPlaygroundCore, configureRemote } from "@playground/core";
// The core, compiled to wasm by `undra build -C examples/playground --platform web`.
import wasmUrl from "../../build/web/playground_core.wasm?url";
import { onDevNotice, showDevConnection } from "./dev-banner";
import { memoryKv } from "./memory-kv";
import { INBOX, PlaygroundServer, REMOTE_BASE_URL } from "./playground-server";

/**
 * What the views share: the one core's long-lived stores, and the server behind its `Http` port.
 * (The counter is not here: its view creates it with `useUndra`.)
 */
export interface Playground {
  /** The to-do list. */
  readonly todos: Todos;
  /** The 10,000-row list. */
  readonly bigList: BigList;
  /** The observed query of the `inbox` list on the (fake) server. */
  readonly inbox: RemoteTodosQueryHandle;
  /** The app's own in-memory server: the Remote tab's offline switch is its `offline` flag. */
  readonly server: PlaygroundServer;
}

/**
 * What a command that failed (`todos.toggle(id)`, `inbox.refetch()`) and a change the page could not apply are handed
 * to (ADR-032): a command never rejects into a click handler, so this is where an app would send the failure to its
 * error reporter. The runtime has already logged it at error level.
 */
function onError(unhandled: UndraUnhandledError): void {
  console.warn(`${unhandled.operation} failed: ${unhandled.error.message}`);
}

/**
 * Attaches the page to its Rust core and creates the three long-lived stores.
 *
 * By default the core runs in the browser (wasm, on this thread). With `?undra=ws://127.0.0.1:7443`
 * in the page URL (or `VITE_UNDRA_DEV_URL` in the environment) of a development build (`vite dev`; a production
 * build ignores both) it is the core that `undra dev`
 * serves instead: edit the Rust, save, and the page is on the rebuilt core with its state, no rebuild or reload of
 * the page. A dropped connection is reconnected by the runtime; a bar at the top of the page shows what it is doing.
 *
 * The app supplies its own `Http` port (an in-memory server, so the playground needs no backend)
 * and `Kv` port (in memory, so a reload starts from the server's seed again); the other ports are
 * the browser's defaults, plus the opt-in ones this core enables (see {@link optInPorts}).
 */
export async function startUndra(): Promise<Playground> {
  const server = new PlaygroundServer();
  const adapters = { http: server, kv: memoryKv() };
  const ports = optInPorts();
  // Development builds only (`vite dev`): a production page that took its core's address from a link would hand
  // whoever wrote the link its ports (Kv, Http, SecureStore) and its screen. Android and iOS gate it the same way.
  const devUrl = import.meta.env.DEV
    ? (new URLSearchParams(location.search).get("undra") ?? import.meta.env["VITE_UNDRA_DEV_URL"])
    : undefined;
  if (typeof devUrl === "string" && devUrl.length > 0) {
    const core = await UndraPlaygroundCore.load({
      mode: "remote",
      url: devUrl,
      adapters,
      ports,
      onError,
      // `undra dev` carries the core's state across a rebuild and the runtime reconnects by itself, so the page
      // usually stays where it is. When the state could not be carried (a schema change, a state too big), the
      // runtime finds a new core and says so: reload the page onto it.
      onClose: (error) => {
        if (error instanceof UndraSessionLostError) location.reload();
      },
      // What the dev server says about a reload ("Reloaded, state kept"), for the status bar.
      onDevNotice,
    });
    showDevConnection(core, devUrl);
  } else {
    await UndraPlaygroundCore.load({
      mode: "wasm-main",
      wasm: new URL(wasmUrl, location.href),
      adapters,
      ports,
      onError,
    });
  }
  // Tell the core where the server is before anything observes the query.
  await configureRemote({ baseUrl: REMOTE_BASE_URL });
  const [todos, bigList, inbox] = await Promise.all([
    Todos.create(),
    BigList.create(),
    RemoteTodosQueryHandle.create(INBOX),
  ]);
  return { todos, bigList, inbox, server };
}

/**
 * The opt-in ports the playground core enables (`undra = { features = ["websocket", "sse", "db"] }`,
 * ADR-047 and ADR-048), registered before the first use. They live in `@undra/runtime/realtime` and
 * `@undra/runtime/db`, out of the main entry, so an app that does not use them does not ship them.
 *
 * * `WebSocket`: the browser's `WebSocket` (the Live view). A browser cannot send headers with the
 *   upgrade, so a connect with headers is refused rather than sent without them.
 * * `Sse`: `fetch` with a streamed body.
 * * `Db`: SQLite (wa-sqlite) in a dedicated worker over the origin private file system (the Notes
 *   view); the web has no WAL.
 */
function optInPorts(): Record<number, PortImpl> {
  return {
    [PortIds.WebSocket.portId]: webSocketPort(browserWebSocket()),
    [PortIds.Sse.portId]: ssePort(fetchSse()),
    [PortIds.Db.portId]: dbPort(waSqliteDb(), { wal: false }),
  };
}

/**
 * Turns the simulated network off or on: the server starts or stops failing its requests, and
 * the core is told with a `Connectivity` event, which is what makes it queue an idempotent
 * mutation while offline and replay it when the network comes back.
 */
export function setOffline(playground: Playground, offline: boolean): void {
  playground.server.offline = offline;
  emitConnectivity(UndraPlaygroundCore.core, !offline, offline ? "none" : "wifi");
}
