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

let compiled: Promise<WebAssembly.Module> | undefined;

/** The compiled playground core, once per test file. */
export function playgroundModule(): Promise<WebAssembly.Module> {
  compiled ??= readFile(PLAYGROUND_WASM).then(
    (bytes) => WebAssembly.compile(bytes),
    (cause: unknown) => {
      throw new Error(`cannot read the playground core at ${PLAYGROUND_WASM}; build it with \`undra build -C examples/playground --platform web\``, {
        cause,
      });
    },
  );
  return compiled;
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
  const expectedSchemaHash = options.expectedSchemaHash ?? UndraIds.schemaHash;
  const native = await WasmNative.load(await playgroundModule(), { clock: world.clock, namespace: UndraIds.namespace });
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
