import { type UndraPanicReport, UndraCore, UndraSessionLostError, type UndraUnhandledError, emitConnectivity } from "@undra/runtime";
import { BigList, UndraIds, RemoteTodosQueryHandle, Todos, configureRemote } from "@playground/core";
// The core, compiled to wasm by `undra build -C examples/playground --platform web`.
import wasmUrl from "../../build/web/undra_core.wasm?url";
import { showDevConnection } from "./dev-banner";
import { memoryKv } from "./memory-kv";
import { INBOX, PlaygroundServer, REMOTE_BASE_URL } from "./playground-server";
import { RECOVERY, RestartLog } from "./recovery-log";

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
  /** The restarts of the wasm core after a crash (ADR-049), for the debug panel. */
  readonly restarts: RestartLog;
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
 * What a panic of the wasm core is handed to (ADR-046): the core's own panic record and the trap's frames, before the
 * runtime restarts the core. Where an app would call its crash reporter.
 */
function onPanic(report: UndraPanicReport): void {
  console.error(`the Undra core panicked: ${report.message}`, report.frames);
}

/**
 * Attaches the page to its Rust core and creates the three long-lived stores.
 *
 * By default the core runs in the browser (wasm, on this thread). With `?undra=ws://127.0.0.1:7443`
 * in the page URL (or `VITE_UNDRA_DEV_URL` in the environment) of a development build (`vite dev`; a production
 * build ignores both) it is the core that `undra dev`
 * serves instead: edit the Rust, save, and the page reloads onto the rebuilt core, no rebuild of the page. A
 * dropped connection is reconnected by the runtime; a bar at the top of the page shows what it is doing.
 *
 * The app supplies its own `Http` port (an in-memory server, so the playground needs no backend)
 * and `Kv` port (in memory, so a reload starts from the server's seed again); the other ports are
 * the browser's defaults. The wasm core runs with crash recovery on (ADR-049): a panic restarts it
 * from its last snapshot, and the debug panel lists the restarts.
 */
export async function startUndra(): Promise<Playground> {
  const server = new PlaygroundServer();
  const restarts = new RestartLog();
  const adapters = { http: server, kv: memoryKv() };
  // Development builds only (`vite dev`): a production page that took its core's address from a link would hand
  // whoever wrote the link its ports (Kv, Http, SecureStore) and its screen. Android and iOS gate it the same way.
  const devUrl = import.meta.env.DEV
    ? (new URLSearchParams(location.search).get("undra") ?? import.meta.env["VITE_UNDRA_DEV_URL"])
    : undefined;
  if (typeof devUrl === "string" && devUrl.length > 0) {
    const core = await UndraCore.load({
      mode: "remote",
      url: devUrl,
      expectedSchemaHash: UndraIds.schemaHash,
      adapters,
      onError,
      // A rebuild restarts the core, and the stores of this page belong to the old one: the runtime reconnects,
      // finds a new core and says so. Reload the page onto it.
      onClose: (error) => {
        if (error instanceof UndraSessionLostError) location.reload();
      },
    });
    showDevConnection(core, devUrl);
  } else {
    await UndraCore.load({
      mode: "wasm-main",
      wasm: new URL(wasmUrl, location.href),
      expectedSchemaHash: UndraIds.schemaHash,
      adapters,
      onError,
      onPanic,
      // A panic traps a wasm core: restart it from its last snapshot instead of leaving the page dead (ADR-049).
      recovery: RECOVERY,
      onCoreRestarted: (event) => {
        restarts.record(event);
      },
    });
  }
  // Tell the core where the server is before anything observes the query.
  await configureRemote({ baseUrl: REMOTE_BASE_URL });
  const [todos, bigList, inbox] = await Promise.all([
    Todos.create(),
    BigList.create(),
    RemoteTodosQueryHandle.create(INBOX),
  ]);
  return { todos, bigList, inbox, server, restarts };
}

/**
 * Turns the simulated network off or on: the server starts or stops failing its requests, and
 * the core is told with a `Connectivity` event, which is what makes it queue an idempotent
 * mutation while offline and replay it when the network comes back.
 */
export function setOffline(playground: Playground, offline: boolean): void {
  playground.server.offline = offline;
  emitConnectivity(UndraCore.shared, !offline, offline ? "none" : "wifi");
}
