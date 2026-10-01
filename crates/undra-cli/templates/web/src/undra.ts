import { UndraCore } from "@undra/runtime";
import { UndraIds, Todos } from "@@TS_PACKAGE@@";
// The core, compiled to wasm by `undra build --platform web`.
import wasmUrl from "@@WASM_IMPORT@@?url";

/**
 * Attaches the page to its Rust core and creates the store.
 *
 * By default the core runs in the browser (wasm, on this thread). With `?undra=ws://127.0.0.1:7443`
 * in the page URL (or `VITE_UNDRA_DEV_URL` in the environment) it is the core that `undra dev`
 * serves instead: edit the Rust, save, reload the page, no rebuild of the page.
 */
export async function startUndra(): Promise<Todos> {
  const devUrl = new URLSearchParams(location.search).get("undra") ?? import.meta.env["VITE_UNDRA_DEV_URL"];
  if (typeof devUrl === "string" && devUrl.length > 0) {
    await UndraCore.load({ mode: "remote", url: devUrl, expectedSchemaHash: UndraIds.schemaHash });
  } else {
    await UndraCore.load({
      mode: "wasm-main",
      wasm: new URL(wasmUrl, location.href),
      expectedSchemaHash: UndraIds.schemaHash,
    });
  }
  return Todos.create();
}
