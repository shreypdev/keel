import { KeelCore } from "@keel/runtime";
import { KeelIds, Todos } from "@@TS_PACKAGE@@";
// The core, compiled to wasm by `keel build --platform web`.
import wasmUrl from "@@WASM_IMPORT@@?url";

/**
 * Attaches the page to its Rust core and creates the store.
 *
 * By default the core runs in the browser (wasm, on this thread). With `?keel=ws://127.0.0.1:7443`
 * in the page URL (or `VITE_KEEL_DEV_URL` in the environment) it is the core that `keel dev`
 * serves instead: edit the Rust, save, reload the page, no rebuild of the page.
 */
export async function startKeel(): Promise<Todos> {
  const devUrl = new URLSearchParams(location.search).get("keel") ?? import.meta.env["VITE_KEEL_DEV_URL"];
  if (typeof devUrl === "string" && devUrl.length > 0) {
    await KeelCore.load({ mode: "remote", url: devUrl, expectedSchemaHash: KeelIds.schemaHash });
  } else {
    await KeelCore.load({
      mode: "wasm-main",
      wasm: new URL(wasmUrl, location.href),
      expectedSchemaHash: KeelIds.schemaHash,
    });
  }
  return Todos.create();
}
