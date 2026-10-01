import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { afterEach } from "vitest";
import { type AdapterOverrides, type AttachOptions, type PortImpl, UndraCore, type LoadOptions, WasmMainTransport, type WorkerLike } from "@undra/runtime";
import { runWorker, type WorkerScope } from "@undra/runtime/worker";
import { UndraIds } from "@playground/core";
import { CapturingLog } from "./capturing-log.js";
import { FakeServer } from "./fake-server.js";
import { ManualClock } from "./manual-clock.js";
import { MemoryKv } from "./memory-kv.js";

/** The playground core, built by `undra build -C examples/playground --platform web`. `UNDRA_PLAYGROUND_WASM` overrides the path. */
export const PLAYGROUND_WASM: string =
  process.env["UNDRA_PLAYGROUND_WASM"] ?? fileURLToPath(new URL("../../../examples/playground/build/web/undra_core.wasm", import.meta.url));

/** The base URL every scenario that talks to the server configures (scenarios.md, "Server fixtures"). */
export const BASE_URL = "https://playground.test";

/**
 * Build B of the playground core (scenarios.md, "Two builds"): `UNDRA_PLAYGROUND_V2=1 undra build ... --platform web`,
 * which run.sh copies to `build/b/undra_core.wasm`. `UNDRA_PLAYGROUND_WASM_B` overrides the path.
 */
export const PLAYGROUND_WASM_B: string =
  process.env["UNDRA_PLAYGROUND_WASM_B"] ?? fileURLToPath(new URL("../build/b/undra_core.wasm", import.meta.url));

/** Which build of the playground core a scenario loads: A (the default, the generated bindings' schema) or B. */
export type Build = "A" | "B";

const compiled = new Map<Build, Promise<WebAssembly.Module>>();

/**
 * The compiled playground core, compiled once per test file: instantiating a compiled module is
 * cheap, so every scenario gets a fresh instance (a fresh core) without recompiling.
 */
export function playgroundModule(build: Build = "A"): Promise<WebAssembly.Module> {
  let module = compiled.get(build);
  if (module === undefined) {
    const path = build === "A" ? PLAYGROUND_WASM : PLAYGROUND_WASM_B;
    module = readFile(path).then(
      (bytes) => WebAssembly.compile(bytes),
      (cause: unknown) => {
        throw new Error(
          build === "A"
            ? `cannot read the playground core at ${path}; build it with \`undra build -C examples/playground --platform web\` (contract-tests/ts/run.sh does)`
            : `cannot read build B of the playground core at ${path}; build it with \`UNDRA_PLAYGROUND_V2=1 undra build -C examples/playground --platform web\` and copy the wasm there (contract-tests/ts/run.sh does)`,
          { cause },
        );
      },
    );
    compiled.set(build, module);
  }
  return module;
}

/**
 * The schema hash a module exports (`undra_schema_hash`, which works before `undra_init`): build B has no generated
 * bindings, so a scenario loads it with the hash it reports (scenarios.md, "Two builds").
 */
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

/** The fakes a core runs against: the adapters a scenario scripts and inspects. */
export interface World {
  /** The `Clock`: manual. */
  readonly clock: ManualClock;
  /** The `Http` port: an in-memory server. */
  readonly server: FakeServer;
  /** The `Kv` port: in memory, remembering every call. */
  readonly kv: MemoryKv;
  /** The `Log` port: captures every record. */
  readonly log: CapturingLog;
}

/** A loaded core and the world it runs in. */
export interface Booted extends World {
  /** The core: loaded in `wasm-main` mode by {@link boot}, in `wasm-worker` mode by {@link bootWorker}. */
  readonly core: UndraCore;
  /** What the runtime reported through `onClose`: one entry when the channel to the core was lost. */
  readonly closed: Error[];
  /** What the runtime reported through `onError` (failures that have no caller to reject). */
  readonly runtimeErrors: unknown[];
}

/** A {@link Booted} core built over a `WasmMainTransport` the scenario holds, for what `UndraCore` does not expose (the wasm exports). */
export interface BootedRaw extends Booted {
  /** The transport `core` was attached to; `transport.instance` is the wasm instance. */
  readonly transport: WasmMainTransport;
}

/** What a scenario may choose when it boots a core. */
export interface BootOptions extends Partial<World> {
  /** The schema hash to demand of the core. Default: the bindings' (`UndraIds.schemaHash`), or what build B reports. */
  readonly expectedSchemaHash?: bigint;
  /** Which build of the core to load. Default A. */
  readonly build?: Build;
  /** More ports to register (on the main thread). */
  readonly ports?: Readonly<Record<number, PortImpl>>;
  /** Options of `UndraCore.load` a scenario needs besides the harness's (`recovery`, `onCoreRestarted`, `onPanic`). */
  readonly load?: Pick<AttachOptions, "recovery" | "onCoreRestarted" | "onPanic">;
}

/** The schema hash `options` asks for: given, else the bindings' for build A, else what build B reports. */
async function expectedHash(options: BootOptions): Promise<bigint> {
  if (options.expectedSchemaHash !== undefined) return options.expectedSchemaHash;
  if ((options.build ?? "A") === "A") return UndraIds.schemaHash;
  return schemaHashOf(await playgroundModule("B"));
}

const booted: Booted[] = [];
/** What a boot that owns more than a core (a worker) closes after the scenario. */
const cleanups: Array<() => void> = [];

// A scenario never closes its cores itself: whatever it booted is closed after it, pass or fail. And a
// failure the runtime could not hand to any caller (a change-set that did not decode, a store that threw
// while applying one, a port that failed) is a failure of the scenario, reported here.
afterEach(() => {
  const all = booted.splice(0);
  for (const { core } of all) core.close();
  for (const cleanup of cleanups.splice(0)) cleanup();
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

/** The adapters of the harness: everything in memory, and no event sources (a scenario emits events itself). */
function adaptersOf(world: World): AdapterOverrides {
  return {
    http: world.server,
    kv: world.kv,
    log: world.log,
    clock: world.clock,
    connectivity: null,
    lifecycle: null,
    secureStore: null,
    fs: null,
  };
}

/**
 * Loads a fresh core the way an app does: `UndraCore.load({ mode: "wasm-main" })` over the
 * playground's wasm, with the harness adapters (scenarios.md, "The harness"). The core is not
 * `UndraCore.shared`; pass it to the generated constructors. It is closed after the scenario.
 */
export async function boot(options: BootOptions = {}): Promise<Booted> {
  const world = worldOf(options);
  const closed: Error[] = [];
  const runtimeErrors: unknown[] = [];
  const load: LoadOptions = {
    ...options.load,
    mode: "wasm-main",
    wasm: await playgroundModule(options.build),
    expectedSchemaHash: await expectedHash(options),
    shared: false,
    adapters: adaptersOf(world),
    ...(options.ports !== undefined && { ports: options.ports }),
    onClose: (error) => {
      closed.push(error);
    },
    onError: (error) => {
      runtimeErrors.push(error);
    },
  };
  const core = await UndraCore.load(load);
  const loaded: Booted = { ...world, core, closed, runtimeErrors };
  booted.push(loaded);
  return loaded;
}

/**
 * Like {@link boot}, but builds the `WasmMainTransport` itself and attaches the core to it, so the
 * scenario can reach the wasm instance (`undra_snapshot`, `undra_schema_json`, ...). This is what
 * `UndraCore.load` does for mode `wasm-main`, step by step.
 */
export async function bootRaw(options: BootOptions = {}): Promise<BootedRaw> {
  const world = worldOf(options);
  const closed: Error[] = [];
  const runtimeErrors: unknown[] = [];
  const expectedSchemaHash = options.expectedSchemaHash ?? UndraIds.schemaHash;
  const transport = new WasmMainTransport({
    wasm: await playgroundModule(),
    expectedSchemaHash,
    clock: world.clock,
    onError: (error) => {
      runtimeErrors.push(error);
    },
  });
  const core = await UndraCore.attach(transport, {
    expectedSchemaHash,
    shared: false,
    adapters: adaptersOf(world),
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

/**
 * Like {@link boot}, but in `wasm-worker` mode: `UndraCore.load({ mode: "wasm-worker" })` over the playground's
 * wasm, with the core running behind a worker. The worker is `runWorker` (the code of `@undra/runtime/worker`)
 * served on one end of a `MessageChannel` in this thread, because the harness cannot load TypeScript in a real
 * worker thread; the messages cross the channel exactly as they would cross to a worker (structured clone,
 * transferred buffers). The real thread is covered by crates/undra-ffi/tests/wasm/ts-runtime.test.mjs.
 *
 * What changes with the mode: `callSync` does not exist, and the core answers `Clock`, `Rng` and `Log`
 * itself (inside the worker), so the world's `clock` is not used; the `Http` and `Kv` ports and the `Log`
 * records still reach the world's adapters, through the main thread.
 */
export async function bootWorker(options: WorkerBootOptions = {}): Promise<BootedWorker> {
  const world = worldOf(options);
  const posted: unknown[] = options.posted ?? [];
  const closed: Error[] = [];
  const runtimeErrors: unknown[] = [];
  const channel = new MessageChannel();
  channel.port1.start();
  channel.port2.start();
  const host: WorkerLike = {
    addEventListener: (type, listener) => {
      channel.port2.addEventListener(type as "message", listener as (event: MessageEvent) => void);
    },
    removeEventListener: (type, listener) => {
      channel.port2.removeEventListener(type as "message", listener as (event: MessageEvent) => void);
    },
    postMessage: (message, transfer) => {
      posted.push(message);
      channel.port2.postMessage(message, transfer ?? []);
    },
    close: () => {
      channel.port2.close();
    },
  };
  const stop = runWorker(channel.port1 as unknown as WorkerScope);
  cleanups.push(() => {
    stop();
    channel.port1.close();
    channel.port2.close();
  });
  const load: LoadOptions = {
    ...options.load,
    mode: "wasm-worker",
    worker: options.workerPorts === undefined ? host : { create: host, ports: options.workerPorts },
    wasm: await playgroundModule(options.build),
    expectedSchemaHash: await expectedHash(options),
    shared: false,
    adapters: adaptersOf(world),
    ...(options.ports !== undefined && { ports: options.ports }),
    onClose: (error) => {
      closed.push(error);
    },
    onError: (error) => {
      runtimeErrors.push(error);
    },
  };
  const core = await UndraCore.load(load);
  const loaded: BootedWorker = { ...world, core, closed, runtimeErrors, posted };
  booted.push(loaded);
  return loaded;
}

/** What a scenario may choose when it boots a core in `wasm-worker` mode. */
export interface WorkerBootOptions extends BootOptions {
  /** The URL of the module of `LoadOptions.worker.ports` (ADR-049): ports that run in the worker. */
  readonly workerPorts?: string;
  /** Where to record what the main thread posts to the worker (also when the load fails). Default a new array. */
  readonly posted?: unknown[];
}

/** A {@link Booted} core in `wasm-worker` mode, and every message the main thread posted to its worker (`init` first). */
export interface BootedWorker extends Booted {
  readonly posted: unknown[];
}
