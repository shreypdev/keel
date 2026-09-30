import { KeelCore, emitConnectivity } from "@keel/runtime";
import { BigList, KeelIds, RemoteTodosQueryHandle, Todos, configureRemote } from "@playground/core";
// The core, compiled to wasm by `keel build -C examples/playground --platform web`.
import wasmUrl from "../../build/web/keel_core.wasm?url";
import { memoryKv } from "./memory-kv";
import { INBOX, PlaygroundServer, REMOTE_BASE_URL } from "./playground-server";

/**
 * What the views share: the one core's long-lived stores, and the server behind its `Http` port.
 * (The counter is not here: its view creates it with `useKeel`.)
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
 * Attaches the page to its Rust core and creates the three long-lived stores.
 *
 * By default the core runs in the browser (wasm, on this thread). With `?keel=ws://127.0.0.1:7443`
 * in the page URL (or `VITE_KEEL_DEV_URL` in the environment) it is the core that `keel dev`
 * serves instead: edit the Rust, save, reload the page, no rebuild of the page.
 *
 * The app supplies its own `Http` port (an in-memory server, so the playground needs no backend)
 * and `Kv` port (in memory, so a reload starts from the server's seed again); the other ports are
 * the browser's defaults.
 */
export async function startKeel(): Promise<Playground> {
  const server = new PlaygroundServer();
  const adapters = { http: server, kv: memoryKv() };
  const devUrl = new URLSearchParams(location.search).get("keel") ?? import.meta.env["VITE_KEEL_DEV_URL"];
  if (typeof devUrl === "string" && devUrl.length > 0) {
    await KeelCore.load({ mode: "remote", url: devUrl, expectedSchemaHash: KeelIds.schemaHash, adapters });
  } else {
    await KeelCore.load({
      mode: "wasm-main",
      wasm: new URL(wasmUrl, location.href),
      expectedSchemaHash: KeelIds.schemaHash,
      adapters,
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
 * Turns the simulated network off or on: the server starts or stops failing its requests, and
 * the core is told with a `Connectivity` event, which is what makes it queue an idempotent
 * mutation while offline and replay it when the network comes back.
 */
export function setOffline(playground: Playground, offline: boolean): void {
  playground.server.offline = offline;
  emitConnectivity(KeelCore.shared, !offline, offline ? "none" : "wifi");
}
