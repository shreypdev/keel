import { UndraSessionLostError, emitConnectivity } from "@undra/runtime";
import { Auth, Notebook, UndraFieldbookCore, configureServer } from "@app/fieldbook-core";
// The core, compiled to wasm by `undra build --platform web`.
import wasmUrl from "../../build/web/fieldbook_core.wasm?url";
import { onDevNotice, showDevConnection } from "./dev-banner";
import { DemoServer, SERVER_URL } from "./demo-server";

/** What the app holds: the two stores (each platform's mirror of the core's state) and the demo backend. */
export interface Fieldbook {
  readonly auth: Auth;
  readonly notebook: Notebook;
  readonly server: DemoServer;
}

/**
 * Attaches the page to its Rust core, tells it where the server is, and starts the stores.
 *
 * By default the core runs in the browser (wasm, on this thread) with the browser's own ports for storage
 * (IndexedDB for notes and the offline queue, WebCrypto for the tokens, OPFS for photos) and the page's
 * in-memory {@link DemoServer} behind `Http`, so the sample needs no backend. With `?undra=ws://127.0.0.1:7443`
 * in the page URL of a development build (`vite dev`) it is the core that `undra dev` serves instead: edit the
 * Rust, save, and the page is on the rebuilt core with its state (ADR-053).
 */
export async function startUndra(): Promise<Fieldbook> {
  const server = new DemoServer();
  const adapters = { http: server };
  const devUrl = import.meta.env.DEV
    ? (new URLSearchParams(location.search).get("undra") ?? import.meta.env["VITE_UNDRA_DEV_URL"])
    : undefined;
  if (typeof devUrl === "string" && devUrl.length > 0) {
    const core = await UndraFieldbookCore.load({
      mode: "remote",
      url: devUrl,
      adapters,
      onClose: (error) => {
        if (error instanceof UndraSessionLostError) location.reload();
      },
      onDevNotice,
    });
    showDevConnection(core, devUrl);
  } else {
    await UndraFieldbookCore.load({ mode: "wasm-main", wasm: new URL(wasmUrl, location.href), adapters });
  }
  configureServer({ baseUrl: SERVER_URL });
  const [auth, notebook] = await Promise.all([Auth.create(), Notebook.create()]);
  await auth.resume();
  await notebook.load();
  return { auth, notebook, server };
}

/** The demo's offline switch: the server stops answering and the core is told, which is what makes it queue writes. */
export function setOffline(app: Fieldbook, offline: boolean): void {
  app.server.offline = offline;
  emitConnectivity(UndraFieldbookCore.core, !offline, offline ? "none" : "wifi");
}
