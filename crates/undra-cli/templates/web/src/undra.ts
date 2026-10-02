import { UndraSessionLostError } from "@undra/runtime";
import { @@CORE_ENTRY@@, Todos } from "@@TS_PACKAGE@@";
// The core, compiled to wasm by `undra build --platform web`.
import wasmUrl from "@@WASM_IMPORT@@?url";
import { onDevNotice, showDevConnection } from "./dev-banner";

/** The debug build of the core, which `vite dev` serves (see vite.config.ts); empty in a production build. */
declare const __UNDRA_DEBUG_WASM__: string;

/**
 * Attaches the page to its Rust core and creates the store.
 *
 * By default the core runs in the browser (wasm, on this thread). With `?undra=ws://127.0.0.1:7443`
 * in the page URL (or `VITE_UNDRA_DEV_URL` in the environment) of a development build (`vite dev`; a production
 * build ignores both) it is the core that `undra dev`
 * serves instead: edit the Rust, save, and the page is on the rebuilt core with its state, no rebuild of the page. A
 * dropped connection is reconnected by the runtime; a bar at the top of the page shows what it is doing.
 */
export async function startUndra(): Promise<Todos> {
  // Development builds only (`vite dev`): a production page that took its core's address from a link would hand
  // whoever wrote the link its ports (Kv, Http, SecureStore) and its screen. Android and iOS gate it the same way.
  const devUrl = import.meta.env.DEV
    ? (new URLSearchParams(location.search).get("undra") ?? import.meta.env["VITE_UNDRA_DEV_URL"])
    : undefined;
  if (typeof devUrl === "string" && devUrl.length > 0) {
    const core = await @@CORE_ENTRY@@.load({
      mode: "remote",
      url: devUrl,
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
    await @@CORE_ENTRY@@.load({
      mode: "wasm-main",
      // Under `vite dev` the debug build of the core (DWARF line tables and names: breakpoints in .rs files in Chrome's
      // DevTools); in a production build the stripped module.
      wasm: new URL(import.meta.env.DEV && __UNDRA_DEBUG_WASM__ !== "" ? __UNDRA_DEBUG_WASM__ : wasmUrl, location.href),
    });
  }
  return Todos.create();
}
