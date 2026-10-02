import {
  type UndraCore,
  UndraCoreRestarted,
  crashRecovery,
  type UndraPanicReport,
  UndraSessionLostError,
  type UndraUnhandledError,
  emitConnectivity,
} from "@undra/runtime";
import { BigList, RemoteTodosQueryHandle, Todos, UndraPlaygroundCore, configureRemote } from "@playground/core";
// The core, compiled to wasm by `undra build -C examples/playground --platform web`.
import wasmUrl from "../../build/web/playground_core.wasm?url";
import { onDevNotice, showDevConnection } from "./dev-banner";
import { memoryKv } from "./memory-kv";
import { INBOX, PlaygroundServer, REMOTE_BASE_URL } from "./playground-server";
import { PanicLog } from "./panic-log";
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
  /** The panic reports of the core (ADR-046), for the debug panel: the last one, where an app would send each to its crash reporter. */
  readonly panics: PanicLog;
  /** The core itself (`UndraPlaygroundCore.core` becomes a closed placeholder if it goes down for good; this does not). */
  readonly core: UndraCore;
}

/**
 * What a command that failed (`todos.toggle(id)`, `inbox.refetch()`) and a change the page could not apply are handed
 * to (ADR-032): a command never rejects into a click handler, so this is where an app would send the failure to its
 * error reporter. The runtime has already logged it at error level.
 */
function onError(unhandled: UndraUnhandledError): void {
  // A restart after a crash (ADR-049) is reported here too; the debug panel lists it.
  if (unhandled instanceof UndraCoreRestarted) console.warn(unhandled.message);
  else console.warn(`${unhandled.operation} failed: ${unhandled.error.message}`);
}

/**
 * What a panic of the core is handed to (ADR-046), once per panic, before the runtime restarts a wasm core: the message, where
 * and in what it happened, the frames and which build of which core. Where an app would call its crash reporter (Sentry,
 * `reportError`); the playground logs it and shows the last one in the debug panel.
 */
function onPanic(report: UndraPanicReport, panics: PanicLog): void {
  console.error(`the Undra core panicked: ${report.message} (${report.location}, in ${report.operation})`, report);
  panics.record(report);
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
 * the browser's defaults. The wasm core runs with crash recovery on (ADR-049): a panic restarts it
 * from its last snapshot, and the debug panel lists the restarts.
 */
export async function startUndra(): Promise<Playground> {
  const server = new PlaygroundServer();
  const restarts = new RestartLog();
  const panics = new PanicLog();
  let inbox: RemoteTodosQueryHandle | undefined;
  const adapters = { http: server, kv: memoryKv() };
  // Development builds only (`vite dev`): a production page that took its core's address from a link would hand
  // whoever wrote the link its ports (Kv, Http, SecureStore) and its screen. Android and iOS gate it the same way.
  const devUrl = import.meta.env.DEV
    ? (new URLSearchParams(location.search).get("undra") ?? import.meta.env["VITE_UNDRA_DEV_URL"])
    : undefined;
  let core: UndraCore;
  if (typeof devUrl === "string" && devUrl.length > 0) {
    core = await UndraPlaygroundCore.load({
      mode: "remote",
      url: devUrl,
      adapters,
      onError,
      // A native core reports each panic it contained through its Diagnostics port.
      onPanic: (report) => onPanic(report, panics),
      // `undra dev` carries the core's state across a rebuild and the runtime reconnects by itself, so the page
      // usually stays where it is. When the state could not be carried (a change the stores cannot follow, a state too big), the
      // runtime finds a new core and says so: reload the page onto it.
      onClose: (error) => {
        if (error instanceof UndraSessionLostError) location.reload();
      },
      // What the dev server says about a reload ("Reloaded, state kept"), for the status bar.
      onDevNotice,
    });
    showDevConnection(core, devUrl);
  } else {
    core = await UndraPlaygroundCore.load({
      mode: "wasm-main",
      wasm: new URL(wasmUrl, location.href),
      adapters,
      onError,
      onPanic: (report) => onPanic(report, panics),
      // The core's name for the panic report (a wasm module does not carry it).
      namespace: UndraPlaygroundCore.namespace,
      // A panic traps a wasm core: restart it from its last snapshot instead of leaving the page dead (ADR-049).
      recovery: crashRecovery(RECOVERY),
      onCoreRestarted: (event) => {
        restarts.record(event);
        // The stores came back from the snapshot, but what the core held outside them did not: tell the new instance
        // where the server is again, then fetch the re-created query (it tried before it knew).
        void configureRemote({ baseUrl: REMOTE_BASE_URL }).then(() => inbox?.refetch());
      },
    });
  }
  // Tell the core where the server is before anything observes the query.
  await configureRemote({ baseUrl: REMOTE_BASE_URL });
  const [todos, bigList, inboxQuery] = await Promise.all([
    Todos.create(),
    BigList.create(),
    RemoteTodosQueryHandle.create(INBOX),
  ]);
  inbox = inboxQuery;
  return { todos, bigList, inbox: inboxQuery, server, restarts, panics, core };
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
