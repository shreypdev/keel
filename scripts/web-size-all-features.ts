// The "all features" page of the size gate (ADR-057, `web/all-features-runtime-js`; built by scripts/web-size-runtime.mjs, which copies this file
// next to a copy of the playground's generated bindings, `bindings/`, inside the scratch hello project so that `@undra/runtime` resolves as an
// installed app's does). Every generated export of the playground's bindings (stores, keyed and derived and lazy lists, queries and mutations,
// objects, callbacks, streams, ports), crash recovery, a panic handler, `stats`, `snapshot`, `restore` and a background run, in `wasm-main` mode:
// everything a page can ask of the runtime's first chunk and of the chunks it loads when asked. It is only bundled, never run.
import { UndraCore, UndraCoreRestarted, UndraSessionLostError, crashRecovery, emitConnectivity } from "@undra/runtime";
import * as bindings from "./bindings/index.js";

export { UndraCoreRestarted, UndraSessionLostError, emitConnectivity };
export const everything = bindings;

export async function start(wasm: URL): Promise<UndraCore> {
  const core = await bindings.UndraPlaygroundCore.load({ mode: "wasm-main", wasm, recovery: crashRecovery(), onPanic: (report) => console.error(report.message) });
  await core.stats();
  await core.restore(await core.snapshot());
  await core.runInBackground(1000);
  return core;
}
