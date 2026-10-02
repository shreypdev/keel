// The two-core test app on Node (ADR-044): two copies of the playground core, built under the
// namespaces playground_a and playground_b, loaded into one process through their generated entries.
// Each gets a call and an observed change, their statistics are independent, and one is closed while
// the other keeps working. Both use the default adapters, whose Kv is per core namespace (ADR-044 amendment A): each
// writes the same key and reads its own value back, in two IndexedDB databases. Every check prints one `two-cores node:` line; any failure exits non-zero.
//
//   examples/two-cores/node/run.sh
import "fake-indexeddb/auto";
import { readFile } from "node:fs/promises";
import { UndraCallError, type UndraCore } from "@undra/runtime";
import { Counter as CounterA, UndraPlaygroundA, add as addA, kvGet as kvGetA, kvKeys as kvKeysA, kvPut as kvPutA, kvRemove as kvRemoveA } from "@two-cores/a";
import { Counter as CounterB, UndraPlaygroundB, add as addB, kvGet as kvGetB, kvKeys as kvKeysB, kvPut as kvPutB, kvRemove as kvRemoveB } from "@two-cores/b";

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

// The default stores of two cores are two stores: the same Kv key written through each core reads back that core's
// own value, a key one core wrote is not the other's, and the databases are `undra.<namespace>.kv`.
{
  const encode = (text: string): Uint8Array => new TextEncoder().encode(text);
  const decode = (bytes: Uint8Array | null): string | null => (bytes === null ? null : new TextDecoder().decode(bytes));
  const nonce = Date.now().toString(16);
  const key = "two-cores.key";
  const onlyInA = "two-cores.only-a";
  await kvPutA(key, encode(`value-of-a-${nonce}`));
  await kvPutB(key, encode(`value-of-b-${nonce}`));
  await kvPutA(onlyInA, new Uint8Array([1]));
  const readA = decode(await kvGetA(key));
  const readB = decode(await kvGetB(key));
  check(`Kv: both wrote ${key} and read their own value back: A has ${readA}, B has ${readB}`, readA === `value-of-a-${nonce}` && readB === `value-of-b-${nonce}`);
  const keysA = await kvKeysA("two-cores.");
  const keysB = await kvKeysB("two-cores.");
  check(`Kv: a key only A wrote is not B's: A lists ${JSON.stringify(keysA)}, B lists ${JSON.stringify(keysB)}`, keysA.includes(onlyInA) && !keysB.includes(onlyInA) && keysB.includes(key));
  const databases = (await indexedDB.databases()).map((d) => d.name).sort();
  check(
    `Kv databases: ${databases.join(" and ")}`,
    databases.length === 2 && databases[0] === `undra.${UndraPlaygroundA.namespace}.kv` && databases[1] === `undra.${UndraPlaygroundB.namespace}.kv`,
  );
  await kvRemoveA(key);
  await kvRemoveA(onlyInA);
  await kvRemoveB(key);
}

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
