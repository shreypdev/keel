// The "all features" page: every generated export of the playground's bindings (stores, keyed and derived lists, lazy
// lists, queries and mutations, objects, callbacks, streams, ports), crash recovery, a panic handler, snapshot and stats.
import { UndraCore, UndraCoreRestarted, UndraSessionLostError, crashRecovery, emitConnectivity } from "@undra/runtime";
import * as bindings from "@playground/core";

export { UndraCoreRestarted, UndraSessionLostError, emitConnectivity };
export const everything = bindings;

export async function start(wasm: URL): Promise<UndraCore> {
  const core = await bindings.UndraPlaygroundCore.load({ mode: "wasm-main", wasm, recovery: crashRecovery(), onPanic: (report) => console.error(report.message) });
  await core.stats();
  await core.restore(await core.snapshot());
  await core.runInBackground(1000);
  return core;
}
