import { UndraCore, UndraSessionLostError } from "@undra/runtime";
import { UndraIds, Todos } from "@@TS_PACKAGE@@";
// The core, compiled to wasm by `undra build --platform web`.
import wasmUrl from "@@WASM_IMPORT@@?url";
import { onDevNotice, showDevConnection } from "./dev-banner";

/**
 * Attaches the page to its Rust core and creates the store.
 *
 * By default the core runs in the browser (wasm, on this thread). With `?undra=ws://127.0.0.1:7443`
 * in the page URL (or `VITE_UNDRA_DEV_URL` in the environment) of a development build (`vite dev`; a production
 * build ignores both) it is the core that `undra dev`
 * serves instead: edit the Rust, save, and the page reloads onto the rebuilt core, no rebuild of the page. A
 * dropped connection is reconnected by the runtime; a bar at the top of the page shows what it is doing.
 */
export async function startUndra(): Promise<Todos> {
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
    await UndraCore.load({
      mode: "wasm-main",
      wasm: new URL(wasmUrl, location.href),
      expectedSchemaHash: UndraIds.schemaHash,
    });
  }
  return Todos.create();
}
