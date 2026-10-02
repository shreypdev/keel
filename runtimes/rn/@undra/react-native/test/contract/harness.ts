import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { afterEach } from "vitest";
import { type AdapterOverrides, UndraCore } from "@undra/runtime";
import { UndraIds } from "@playground/core";
import { CapturingLog } from "../../../../../../contract-tests/ts/src/capturing-log.js";
import { FakeServer } from "../../../../../../contract-tests/ts/src/fake-server.js";
import { ManualClock } from "../../../../../../contract-tests/ts/src/manual-clock.js";
import { MemoryKv } from "../../../../../../contract-tests/ts/src/memory-kv.js";
import { nativeFrameScheduler } from "../../src/frame.js";
import { NativeTransport } from "../../src/transport.js";
import { WasmNative } from "./wasm-native.js";

/*
 * The contract harness (contract-tests/ts/src/harness.ts) for the React Native column: the same
 * exports, the same world (manual clock, fake server, memory Kv, capturing log), but every core is
 * attached through `NativeTransport` over the module's stand-in (`WasmNative`), with the React Native
 * frame scheduler. vitest.contract.config.ts swaps this file in for the scenarios' `../src/harness.js`.
 */

/** The playground core, built by `undra build -C examples/playground --platform web` (`<namespace>.wasm`, ADR-044). */
export const PLAYGROUND_WASM: string =
  process.env["UNDRA_PLAYGROUND_WASM"] ??
  fileURLToPath(new URL(`../../../../../../examples/playground/build/web/${UndraIds.namespace}.wasm`, import.meta.url));

/** The base URL every scenario that talks to the server configures. */
export const BASE_URL = "https://playground.test";

/**
 * Build B of the playground core (contract-tests/scenarios.md, "Two builds"): what contract-tests/ts/run.sh builds with
 * `UNDRA_PLAYGROUND_V2=1` and copies to `contract-tests/ts/build/b/`. `UNDRA_PLAYGROUND_WASM_B` overrides the path.
 */
export const PLAYGROUND_WASM_B: string =
  process.env["UNDRA_PLAYGROUND_WASM_B"] ??
  fileURLToPath(new URL(`../../../../../../contract-tests/ts/build/b/${UndraIds.namespace}.wasm`, import.meta.url));

/** Which build of the playground core a scenario loads: A (the generated bindings' schema) or B. */
export type Build = "A" | "B";

const compiled = new Map<Build, Promise<WebAssembly.Module>>();

/** The compiled playground core (build A by default), once per test file. */
export function playgroundModule(build: Build = "A"): Promise<WebAssembly.Module> {
  let module = compiled.get(build);
  if (module === undefined) {
    const path = build === "A" ? PLAYGROUND_WASM : PLAYGROUND_WASM_B;
    module = readFile(path).then(
      (bytes) => WebAssembly.compile(bytes),
      (cause: unknown) => {
        throw new Error(
          build === "A"
            ? `cannot read the playground core at ${path}; build it with \`undra build -C examples/playground --platform web\``
            : `cannot read build B of the playground core at ${path}; contract-tests/ts/run.sh builds it (UNDRA_PLAYGROUND_V2=1)`,
          { cause },
        );
      },
    );
    compiled.set(build, module);
  }
  return module;
}

/** The schema hash a module exports (`undra_schema_hash` works before `undra_init`): build B has no bindings of its own. */
export async function schemaHashOf(module: WebAssembly.Module): Promise<bigint> {
  const none = (): void => {};
  const instance = await WebAssembly.instantiate(module, {
    undra: {
      reply: none,
      changeset: none,
      stream: none,
      port_call: () => 2,
      schedule: none,
      timer_set: none,
      log: none,
      now_ms: () => 0,
      random: none,
    },
  });
  const e = instance.exports as unknown as { _initialize?: () => void; undra_schema_hash: () => bigint };
  e._initialize?.();
  return BigInt.asUintN(64, e.undra_schema_hash());
}

/** The fakes a core runs against. */
export interface World {
  readonly clock: ManualClock;
  readonly server: FakeServer;
  readonly kv: MemoryKv;
  readonly log: CapturingLog;
}

/** A loaded core and the world it runs in. */
export interface Booted extends World {
  readonly core: UndraCore;
  readonly closed: Error[];
  readonly runtimeErrors: unknown[];
}

/** A {@link Booted} core with its transport, for what `UndraCore` does not expose (see ./wasm-exports.ts). */
export interface BootedRaw extends Booted {
  readonly transport: NativeTransport;
}

/** What a scenario may choose when it boots a core. */
export interface BootOptions extends Partial<World> {
  readonly expectedSchemaHash?: bigint;
  /** Which build of the playground core to load (default A); build B is loaded with the hash it reports. */
  readonly build?: Build;
}

/** The stand-in behind each transport, for ./wasm-exports.ts. */
export const nativeOf = new WeakMap<NativeTransport, WasmNative>();

const booted: Booted[] = [];

afterEach(() => {
  const all = booted.splice(0);
  for (const { core } of all) core.close();
  const reported = all.flatMap((b) => b.runtimeErrors);
  if (reported.length > 0) {
    throw new Error(`the runtime reported ${reported.length} failure(s) with no caller to reject: ${reported.map(String).join("; ")}`);
  }
});

function worldOf(options: BootOptions): World {
  return {
    clock: options.clock ?? new ManualClock(),
    server: options.server ?? new FakeServer(),
    kv: options.kv ?? new MemoryKv(),
    log: options.log ?? new CapturingLog(),
  };
}

function adaptersOf(world: World): AdapterOverrides {
  return { http: world.server, kv: world.kv, log: world.log, connectivity: null, lifecycle: null, secureStore: null, fs: null };
}

/** Loads a fresh core the way `loadNative` does: `NativeTransport`, the native frame scheduler, `UndraCore.attach`. */
export async function bootRaw(options: BootOptions = {}): Promise<BootedRaw> {
  const world = worldOf(options);
  const closed: Error[] = [];
  const runtimeErrors: unknown[] = [];
  const build = options.build ?? "A";
  const expectedSchemaHash =
    options.expectedSchemaHash ?? (build === "A" ? UndraIds.schemaHash : await schemaHashOf(await playgroundModule("B")));
  const native = await WasmNative.load(await playgroundModule(build), { clock: world.clock, namespace: UndraIds.namespace });
  const transport = new NativeTransport({
    namespace: UndraIds.namespace,
    native,
    expectedSchemaHash,
    platform: "react-native-contract",
    onError: (error) => {
      runtimeErrors.push(error);
    },
  });
  nativeOf.set(transport, native);
  const core = await UndraCore.attach(transport, {
    expectedSchemaHash,
    shared: false,
    adapters: adaptersOf(world),
    mirror: { schedule: nativeFrameScheduler(native, { isActive: () => true }) },
    onClose: (error) => {
      closed.push(error);
    },
    onError: (error) => {
      runtimeErrors.push(error);
    },
  });
  const loaded: BootedRaw = { ...world, core, closed, runtimeErrors, transport };
  booted.push(loaded);
  return loaded;
}

/** As {@link bootRaw}; the scenarios that need no transport call this one. */
export function boot(options: BootOptions = {}): Promise<Booted> {
  return bootRaw(options);
}
