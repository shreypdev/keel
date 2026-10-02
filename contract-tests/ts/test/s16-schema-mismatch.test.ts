import { expect, test } from "vitest";
import { UndraCallError, UndraCore, UndraSchemaMismatchError } from "@undra/runtime";
import { Counter, UndraIds, add } from "@playground/core";
import { CapturingLog } from "../src/capturing-log.js";
import { boot, bootRaw } from "../src/harness.js";
import { MemoryKv } from "../src/memory-kv.js";
import { counters } from "../src/stats.js";
import { step, waitFor } from "../src/wait.js";
import { exportedSchema, exportedSchemaHash } from "../src/wasm-exports.js";

// S16 schema mismatch rejection: a core built from another schema than the bindings is refused
// at load, before it runs anything, with an error that names both hashes; the failed attempt
// leaves nothing behind; the bindings, the core's statistics and its exported schema agree.

const hex = (n: bigint): string => `0x${n.toString(16).padStart(16, "0")}`;

interface SchemaNamed {
  readonly name: string;
  readonly type_id?: number;
  readonly method_id?: number;
  readonly query_id?: number;
  readonly port_id?: number;
}
interface ExportedSchema {
  readonly records: readonly SchemaNamed[];
  readonly enums: readonly SchemaNamed[];
  readonly objects: readonly SchemaNamed[];
  readonly functions: readonly SchemaNamed[];
  readonly ports: readonly SchemaNamed[];
  readonly queries: readonly SchemaNamed[];
}

/** `add_later` -> `addLater`: how the bindings spell what the schema spells in snake case. */
const camel = (name: string): string => name.replace(/_([a-z])/g, (_, c: string) => c.toUpperCase());

test("S16 schema mismatch rejection", async () => {
  const generated = UndraIds.schemaHash;
  const kv = new MemoryKv();
  const log = new CapturingLog();

  await step("1. a core built from another schema is refused before it is initialised", async () => {
    const failure = await boot({ expectedSchemaHash: generated ^ 1n, kv, log }).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(failure).toBeInstanceOf(UndraSchemaMismatchError);
    const mismatch = failure as UndraSchemaMismatchError;
    expect(mismatch.expected).toBe(generated ^ 1n);
    expect(mismatch.got).toBe(generated);
    expect(mismatch.message).toContain(hex(generated ^ 1n));
    expect(mismatch.message).toContain(hex(generated));
    // A core that ran `undra_init` would have read its persisted query cache (see step 2): this one touched nothing.
    expect(kv.operations, "the refused core never reached the Kv port").toEqual([]);
    expect(log.records, "and logged nothing").toEqual([]);
  });

  await step("2. a later load with the right hash succeeds: nothing was left half-initialised", async () => {
    const { core } = await boot({ kv, log });
    expect(await add(40, 2, core)).toBe(42);
    expect(core.hello.schemaHash).toBe(generated);
    // The contrast for step 1: an initialised core does read the Kv port.
    await waitFor("an initialised core to read its cache from Kv", () => kv.operations.some((operation) => operation.op === "list"));
  });

  await step("3. the bindings, the stats and the exported hash are one hash", async () => {
    const { core, transport } = await bootRaw();
    expect(BigInt((await counters(core)).schemaHash)).toBe(generated);
    expect(exportedSchemaHash(transport)).toBe(generated);
    expect(core.hello.schemaHash).toBe(generated);
  });

  await step("4. the exported schema lists the playground's types and the standard ports, with the bindings' ids", async () => {
    const { transport } = await bootRaw();
    const schema = exportedSchema(transport) as ExportedSchema;
    const names = (items: readonly SchemaNamed[]): string[] => items.map((item) => item.name);

    for (const object of ["Todos", "Counter", "BigList", "Bench", "Probe"]) expect(names(schema.objects)).toContain(object);
    for (const query of ["remote_todos", "post_remote_todo", "patch_remote_todo"]) expect(names(schema.queries)).toContain(query);
    for (const port of ["Clock", "Rng", "Log", "Http", "Kv", "SecureStore", "Fs", "Timer", "Connectivity", "Lifecycle"]) {
      expect(names(schema.ports)).toContain(port);
    }

    // What the bindings carry is what the core exports. (The handle of a query is constructed with the query's own id.)
    // A query handle is not an object of the schema: its type id is the query's own id (`RemoteTodosQueryHandle` is `remoteTodos`).
    const isHandle = (name: string): boolean => name.endsWith("QueryHandle");
    for (const [name, ids] of Object.entries(UndraIds.Objects)) {
      if (isHandle(name)) continue;
      expect(schema.objects.find((o) => o.name === name)?.type_id, `type id of ${name}`).toBe(ids.typeId);
    }
    const queryIds = UndraIds.Queries as Readonly<Record<string, number>>;
    const handles = Object.entries(UndraIds.Objects).filter(([name]) => isHandle(name));
    expect(handles.map(([name]) => name).sort(), "the query handles of the bindings").toEqual(["FeedQueryHandle", "RemoteTodosQueryHandle", "TickerQueryHandle"]);
    for (const [name, ids] of handles) {
      const query = name.slice(0, -"QueryHandle".length);
      expect(ids.typeId, `type id of ${name}`).toBe(queryIds[`${query.charAt(0).toLowerCase()}${query.slice(1)}`]);
    }
    for (const [name, id] of Object.entries(UndraIds.Functions)) {
      expect(schema.functions.find((f) => camel(f.name) === name)?.method_id, `method id of ${name}`).toBe(id);
    }
    for (const [name, id] of Object.entries(UndraIds.Queries)) {
      expect(schema.queries.find((q) => camel(q.name) === name)?.query_id, `query id of ${name}`).toBe(id);
    }
  });

  await step("5. with no core loaded, shared is a closed placeholder: calls reject unavailable, nothing throws on access", async () => {
    // The harness never makes a core shared (`shared: false`), so none is loaded here.
    expect(UndraCore.current).toBeNull();
    const shared = UndraCore.shared;
    expect(UndraCore.current, "the placeholder never becomes the shared core").toBeNull();
    expect(shared.closed).toBe(true);
    const outcome = async (run: () => Promise<unknown>): Promise<unknown> => {
      try {
        await run();
      } catch (error) {
        return error;
      }
      throw new Error("expected the call to fail, but it succeeded");
    };
    // A generated call and a generated constructor, both with the default core.
    for (const [what, run] of [
      ["add(1, 2)", () => add(1, 2)],
      ["Counter.create()", () => Counter.create()],
    ] as const) {
      const failure = await outcome(run);
      expect(failure, `${what} with no core loaded`).toBeInstanceOf(UndraCallError.Unavailable);
      expect((failure as UndraCallError.Unavailable).transport.reason).toBe("closed");
      expect((failure as Error).message, "it says how to fix it").toContain("UndraCore.load");
    }
    // A failure reported on the placeholder only logs (it has no `onError`).
    expect(() => {
      shared.report(new Error("a command on the placeholder"), "Counter.increment");
    }).not.toThrow();
    expect(UndraCore.current).toBeNull();
  });
});
