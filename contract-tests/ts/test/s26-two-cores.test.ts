import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { expect, test } from "vitest";
import { type AdapterOverrides, UndraCallError, UndraCore, UndraError } from "@undra/runtime";
import { Counter as CounterA, UndraIds as IdsA, UndraPlaygroundA, add as addA } from "@two-cores/a";
import { Counter as CounterB, UndraIds as IdsB, UndraPlaygroundB, add as addB } from "@two-cores/b";
import { CapturingLog } from "../src/capturing-log.js";
import { FakeServer } from "../src/fake-server.js";
import { ManualClock } from "../src/manual-clock.js";
import { MemoryKv } from "../src/memory-kv.js";
import { counters } from "../src/stats.js";
import { step } from "../src/wait.js";

// S26 two cores (ADR-044): the playground core built under two namespaces, `playground_a` and
// `playground_b`, loaded next to each other through their generated entries; calls, observed
// changes and statistics stay with their own core, one shuts down while the other keeps working,
// and a namespace loads once.

const wasmOf = (ns: "a" | "b"): Promise<WebAssembly.Module> =>
  readFile(fileURLToPath(new URL(`../../../examples/two-cores/${ns}/build/web/playground_${ns}.wasm`, import.meta.url))).then(
    (bytes) => WebAssembly.compile(bytes),
    (cause: unknown) => {
      throw new Error(`cannot read playground_${ns}.wasm; build it with \`undra build -C examples/two-cores/${ns} --platform web\` (contract-tests/ts/run.sh does)`, { cause });
    },
  );

/** The harness adapters (scenarios.md, "The harness"), one world per core. */
const adapters = (): AdapterOverrides => ({
  http: new FakeServer(),
  kv: new MemoryKv(),
  log: new CapturingLog(),
  clock: new ManualClock(),
  connectivity: null,
  lifecycle: null,
  secureStore: null,
  fs: null,
});

const hex = (n: bigint): string => `0x${n.toString(16).padStart(16, "0")}`;

test("S26 two cores", async () => {
  const [wasmA, wasmB] = await Promise.all([wasmOf("a"), wasmOf("b")]);
  const errors: unknown[] = [];
  const onError = (error: unknown): void => {
    errors.push(error);
  };
  let a: UndraCore | undefined;
  let b: UndraCore | undefined;
  try {
    await step("1. both load through their generated entries, each its own", async () => {
      expect(UndraPlaygroundA.core).toBe(UndraCore.unloaded);
      a = await UndraPlaygroundA.load({ mode: "wasm-main", wasm: wasmA, shared: false, adapters: adapters(), onError });
      b = await UndraPlaygroundB.load({ mode: "wasm-main", wasm: wasmB, shared: false, adapters: adapters(), onError });
      expect(a).not.toBe(b);
      expect(UndraPlaygroundA.core).toBe(a);
      expect(UndraPlaygroundB.core).toBe(b);
      expect(IdsA.namespace).toBe("playground_a");
      expect(IdsB.namespace).toBe("playground_b");
      expect(UndraPlaygroundA.namespace).toBe("playground_a");
      expect(UndraPlaygroundB.namespace).toBe("playground_b");
      // The same source, so the same schema; each core reports it for itself.
      expect(IdsA.schemaHash).toBe(IdsB.schemaHash);
      expect((await counters(a)).schemaHash).toBe(hex(IdsA.schemaHash));
      expect((await counters(b)).schemaHash).toBe(hex(IdsB.schemaHash));
    });
    const coreA = a as UndraCore;
    const coreB = b as UndraCore;

    await step("2. a call on each, through each package's own default core", async () => {
      const [beforeA, beforeB] = [await counters(coreA), await counters(coreB)];
      expect(await addA(2, 3)).toBe(5);
      const [midA, midB] = [await counters(coreA), await counters(coreB)];
      expect(midA.calls - beforeA.calls).toBe(1);
      expect(midB.calls - beforeB.calls, "a call of package A does not reach core B").toBe(0);
      expect(await addB(2, 3)).toBe(5);
      const [afterA, afterB] = [await counters(coreA), await counters(coreB)];
      expect(afterA.calls - midA.calls, "a call of package B does not reach core A").toBe(0);
      expect(afterB.calls - midB.calls).toBe(1);
    });

    const [handlesA, handlesB] = [(await counters(coreA)).liveHandles, (await counters(coreB)).liveHandles];
    const counterA = await CounterA.create();
    const counterB = await CounterB.create();
    expect(counterA.core).toBe(coreA);
    expect(counterB.core).toBe(coreB);

    await step("3. an observed change on each, independent", async () => {
      const changeSetsB = coreB.mirror.changeSets;
      const changeSetsA = coreA.mirror.changeSets;
      await counterA.add(2);
      expect(counterA.count.peek()).toBe(2);
      expect(coreA.mirror.changeSets - changeSetsA).toBe(1);
      expect(coreB.mirror.changeSets, "core A's write is not delivered to core B").toBe(changeSetsB);
      await counterB.add(5);
      expect(counterB.count.peek()).toBe(5);
      expect(counterA.count.peek(), "core B's write does not touch core A's counter").toBe(2);
      expect(coreB.mirror.changeSets - changeSetsB).toBe(1);
    });

    await step("4. independent statistics; each object keeps its own core", async () => {
      expect((await counters(coreA)).liveHandles - handlesA).toBe(1);
      expect((await counters(coreB)).liveHandles - handlesB).toBe(1);
      // Two cores with the same history issue the same handle numbers: a handle means something only
      // with the core that issued it, which is why every generated object carries its core.
      expect(counterA.core).toBe(coreA);
      expect(counterB.core).toBe(coreB);
      counterA.close();
      expect((await counters(coreA)).liveHandles - handlesA).toBe(0);
      expect((await counters(coreB)).liveHandles - handlesB, "releasing A's counter leaves B's").toBe(1);
      expect(counterB.count.peek()).toBe(5);
    });

    await step("5. one shut down while the other keeps working", async () => {
      coreA.close();
      expect(coreA.closed).toBe(true);
      expect(UndraPlaygroundA.core).toBe(UndraCore.unloaded);
      await counterB.add(1);
      expect(counterB.count.peek()).toBe(6);
      expect(await addB(2, 3)).toBe(5);
      const viaClosed = await addA(2, 3, coreA).then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(viaClosed).toBeInstanceOf(UndraCallError.Unavailable);
      const viaEntry = await addA(2, 3).then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(viaEntry, "package A's default core is the closed placeholder now").toBeInstanceOf(UndraCallError.Unavailable);
    });

    await step("6. a namespace loads once; a closed one loads again", async () => {
      const again = await UndraPlaygroundB.load({ mode: "wasm-main", wasm: wasmB, shared: false, adapters: adapters(), onError }).then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(again).toBeInstanceOf(UndraError);
      expect((again as UndraError).kind).toBe("state");
      expect(UndraPlaygroundB.core).toBe(coreB);
      a = await UndraPlaygroundA.load({ mode: "wasm-main", wasm: wasmA, shared: false, adapters: adapters(), onError });
      expect(UndraPlaygroundA.core).toBe(a);
      expect(await addA(2, 3)).toBe(5);
    });
    counterB.close();
  } finally {
    a?.close();
    b?.close();
  }
  // The runtime's unhandled failures: the closed placeholder reports nothing, a refused load is a rejection.
  expect(errors).toEqual([]);
});
