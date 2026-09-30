import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { afterEach } from "vitest";
import { type AdapterOverrides, KeelCore, type LoadOptions, WasmMainTransport } from "@keel/runtime";
import { KeelIds } from "@playground/core";
import { CapturingLog } from "./capturing-log.js";
import { FakeServer } from "./fake-server.js";
import { ManualClock } from "./manual-clock.js";
import { MemoryKv } from "./memory-kv.js";

/** The playground core, built by `keel build -C examples/playground --platform web`. `KEEL_PLAYGROUND_WASM` overrides the path. */
export const PLAYGROUND_WASM: string =
  process.env["KEEL_PLAYGROUND_WASM"] ?? fileURLToPath(new URL("../../../examples/playground/build/web/keel_core.wasm", import.meta.url));

/** The base URL every scenario that talks to the server configures (scenarios.md, "Server fixtures"). */
export const BASE_URL = "https://playground.test";

let compiled: Promise<WebAssembly.Module> | undefined;

/**
 * The compiled playground core, compiled once per test file: instantiating a compiled module is
 * cheap, so every scenario gets a fresh instance (a fresh core) without recompiling.
 */
export function playgroundModule(): Promise<WebAssembly.Module> {
  compiled ??= readFile(PLAYGROUND_WASM).then(
    (bytes) => WebAssembly.compile(bytes),
    (cause: unknown) => {
      throw new Error(
        `cannot read the playground core at ${PLAYGROUND_WASM}; build it with \`keel build -C examples/playground --platform web\` (contract-tests/ts/run.sh does)`,
        { cause },
      );
    },
  );
  return compiled;
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
  /** The core, loaded in `wasm-main` mode. */
  readonly core: KeelCore;
  /** What the runtime reported through `onClose`: one entry when the channel to the core was lost. */
  readonly closed: Error[];
  /** What the runtime reported through `onError` (failures that have no caller to reject). */
  readonly runtimeErrors: unknown[];
}

/** A {@link Booted} core built over a `WasmMainTransport` the scenario holds, for what `KeelCore` does not expose (the wasm exports). */
export interface BootedRaw extends Booted {
  /** The transport `core` was attached to; `transport.instance` is the wasm instance. */
  readonly transport: WasmMainTransport;
}

/** What a scenario may choose when it boots a core. */
export interface BootOptions extends Partial<World> {
  /** The schema hash to demand of the core. Default: the bindings' (`KeelIds.schemaHash`). */
  readonly expectedSchemaHash?: bigint;
}

const cores: KeelCore[] = [];

// A scenario never closes its cores itself: whatever it booted is closed after it, pass or fail.
afterEach(() => {
  for (const core of cores.splice(0)) core.close();
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
 * Loads a fresh core the way an app does: `KeelCore.load({ mode: "wasm-main" })` over the
 * playground's wasm, with the harness adapters (scenarios.md, "The harness"). The core is not
 * `KeelCore.shared`; pass it to the generated constructors. It is closed after the scenario.
 */
export async function boot(options: BootOptions = {}): Promise<Booted> {
  const world = worldOf(options);
  const closed: Error[] = [];
  const runtimeErrors: unknown[] = [];
  const load: LoadOptions = {
    mode: "wasm-main",
    wasm: await playgroundModule(),
    expectedSchemaHash: options.expectedSchemaHash ?? KeelIds.schemaHash,
    shared: false,
    adapters: adaptersOf(world),
    onClose: (error) => {
      closed.push(error);
    },
    onError: (error) => {
      runtimeErrors.push(error);
    },
  };
  const core = await KeelCore.load(load);
  cores.push(core);
  return { ...world, core, closed, runtimeErrors };
}

/**
 * Like {@link boot}, but builds the `WasmMainTransport` itself and attaches the core to it, so the
 * scenario can reach the wasm instance (`keel_snapshot`, `keel_schema_json`, ...). This is what
 * `KeelCore.load` does for mode `wasm-main`, step by step.
 */
export async function bootRaw(options: BootOptions = {}): Promise<BootedRaw> {
  const world = worldOf(options);
  const closed: Error[] = [];
  const runtimeErrors: unknown[] = [];
  const expectedSchemaHash = options.expectedSchemaHash ?? KeelIds.schemaHash;
  const transport = new WasmMainTransport({
    wasm: await playgroundModule(),
    expectedSchemaHash,
    clock: world.clock,
    onError: (error) => {
      runtimeErrors.push(error);
    },
  });
  const core = await KeelCore.attach(transport, {
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
  cores.push(core);
  return { ...world, core, closed, runtimeErrors, transport };
}
