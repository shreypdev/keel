// The two-core test app on Node (ADR-044): two copies of the playground core, built under the
// namespaces playground_a and playground_b, loaded into one process through their generated entries.
// Each gets a call and an observed change, their statistics are independent, and one is closed while
// the other keeps working. Every check prints one `two-cores node:` line; any failure exits non-zero.
//
//   examples/two-cores/node/run.sh
import { readFile } from "node:fs/promises";
import { UndraCallError, type UndraCore } from "@undra/runtime";
import { Counter as CounterA, UndraPlaygroundA, add as addA } from "@two-cores/a";
import { Counter as CounterB, UndraPlaygroundB, add as addB } from "@two-cores/b";

const wasm = async (ns: string): Promise<WebAssembly.Module> =>
  WebAssembly.compile(await readFile(new URL(`../${ns}/build/web/playground_${ns}.wasm`, import.meta.url)));

function check(what: string, ok: boolean): void {
  console.log(`two-cores node: ${ok ? "ok  " : "FAIL"} ${what}`);
  if (!ok) process.exitCode = 1;
}

async function liveHandles(core: UndraCore): Promise<number> {
  const stats = await core.stats();
  return Number(stats.core?.["live_handles"] ?? -1);
}

const a = await UndraPlaygroundA.load({ mode: "wasm-main", wasm: await wasm("a"), shared: false });
const b = await UndraPlaygroundB.load({ mode: "wasm-main", wasm: await wasm("b"), shared: false });
check(`both loaded: ${UndraPlaygroundA.namespace} and ${UndraPlaygroundB.namespace}, two cores`, a !== b && UndraPlaygroundA.core === a && UndraPlaygroundB.core === b);

check("a call on each: add(2, 3) is 5 through A and through B", (await addA(2, 3)) === 5 && (await addB(2, 3)) === 5);

const [handlesA, handlesB] = [await liveHandles(a), await liveHandles(b)];
const counterA = await CounterA.create();
const counterB = await CounterB.create();
await counterA.add(2);
await counterB.add(5);
check(`an observed change on each: A's count is ${counterA.count.peek()}, B's is ${counterB.count.peek()}`, counterA.count.peek() === 2 && counterB.count.peek() === 5);
check(
  `independent statistics: A has ${(await liveHandles(a)) - handlesA} new handle, B has ${(await liveHandles(b)) - handlesB}`,
  (await liveHandles(a)) - handlesA === 1 && (await liveHandles(b)) - handlesB === 1,
);

a.close();
await counterB.add(1);
const closed = await addA(2, 3).then(
  () => undefined,
  (e: unknown) => e,
);
check(
  `one shut down while the other keeps working: B's count is ${counterB.count.peek()}, a call on A is ${closed instanceof UndraCallError.Unavailable ? "unavailable" : String(closed)}`,
  counterB.count.peek() === 6 && closed instanceof UndraCallError.Unavailable && (await addB(2, 3)) === 5,
);
counterB.close();
b.close();
console.log(`two-cores node: ${process.exitCode ? "FAILED" : "passed"}`);
