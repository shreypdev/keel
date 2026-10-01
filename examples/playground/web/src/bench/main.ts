import { CallTarget, UndraCore, UndraWriter, codecs, decodeValue } from "@undra/runtime";
import { Bench, UndraIds } from "@playground/core";
// The core, compiled to wasm by `undra build -C examples/playground --platform web`.
import wasmUrl from "../../../build/web/playground_core.wasm?url";
import { memoryKv } from "../memory-kv";
import { type BenchConfig, FULL, QUICK } from "./ops";
import { type RawResult, run } from "./runner";
import { type Summary, summarize } from "./stats";

/** One cold start of the core: what a page pays before its first call. Nanoseconds. */
export interface ColdSample {
  /** Fetching the module's bytes (served from this machine: not a network figure). */
  readonly fetch_ns: number;
  /** `WebAssembly.compile`: what the blueprint's row leaves out ("after wasm compile"). */
  readonly compile_ns: number;
  /** `UndraCore.load` with the compiled module: instantiate, the handshake with the schema check, `undra_init`. */
  readonly load_ns: number;
}

/** What the harness reads from `window.undraBench`. */
export interface UndraBenchApi {
  /** Facts about the browser. */
  info(): Record<string, unknown>;
  /** Runs the operations and the drain experiment on the loaded core. */
  run(quick?: boolean): Promise<RawResult>;
  /** One cold start in this page; the first call in a fresh browser context is the cold one. */
  cold(): Promise<ColdSample>;
  /** `count` more cold starts in this page (the module is warm in the browser's caches by now), summarised. */
  reloads(count: number): Promise<{ readonly compile_ns: Summary; readonly load_ns: Summary }>;
}

declare global {
  interface Window {
    undraBench?: UndraBenchApi;
  }
}

const status = document.getElementById("status") as HTMLElement;
const adapters = () => ({ kv: memoryKv() });

async function coldStart(): Promise<ColdSample> {
  const t0 = performance.now();
  const response = await fetch(wasmUrl);
  if (!response.ok) throw new Error(`GET ${wasmUrl} answered ${response.status}`);
  const bytes = await response.arrayBuffer();
  const t1 = performance.now();
  const module = await WebAssembly.compile(bytes);
  const t2 = performance.now();
  const core = await UndraCore.load({ mode: "wasm-main", wasm: module, expectedSchemaHash: UndraIds.schemaHash, shared: false, adapters: adapters() });
  const t3 = performance.now();
  core.close();
  return { fetch_ns: (t1 - t0) * 1e6, compile_ns: (t2 - t1) * 1e6, load_ns: (t3 - t2) * 1e6 };
}

const info = (): Record<string, unknown> => ({
  user_agent: navigator.userAgent,
  hardware_concurrency: navigator.hardwareConcurrency,
  cross_origin_isolated: crossOriginIsolated,
  device_pixel_ratio: window.devicePixelRatio,
});

async function start(): Promise<void> {
  // `?cold=1`: a page that only measures its own cold start, for a fresh browser context per sample.
  const coldOnly = new URLSearchParams(location.search).get("cold") === "1";
  if (coldOnly) {
    window.undraBench = {
      info,
      cold: coldStart,
      reloads: async (count) => summarizeColds(await repeat(count)),
      run: () => Promise.reject(new Error("this page was opened with ?cold=1")),
    };
    status.textContent = "ready (cold only)";
    return;
  }
  const core = await UndraCore.load({ mode: "wasm-main", wasm: new URL(wasmUrl, location.href), expectedSchemaHash: UndraIds.schemaHash, adapters: adapters() });
  const bench = await Bench.create(core);
  const ids = UndraIds.Objects.Bench;
  const syncAdd = (a: number, b: number): number => {
    const w = new UndraWriter();
    w.writeU32(a);
    w.writeU32(b);
    return decodeValue(codecs.u32, core.callSync({ target: CallTarget.ObjectMethod, handle: bench.handle }, ids.benchAdd, w.finish()));
  };
  window.undraBench = {
    info,
    cold: coldStart,
    reloads: async (count) => summarizeColds(await repeat(count)),
    run: (quick = false) => {
      const config: BenchConfig = quick ? QUICK : FULL;
      status.textContent = "running";
      return run(
        {
          bench,
          now: () => performance.now(),
          nextFrame: () => new Promise((resolve) => requestAnimationFrame(() => resolve())),
          syncAdd,
          onDrain: (listener) => core.mirror.addDrainListener(listener),
        },
        config,
      ).finally(() => {
        status.textContent = "done";
      });
    },
  };
  status.textContent = "ready";
}

async function repeat(count: number): Promise<ColdSample[]> {
  const samples: ColdSample[] = [];
  for (let i = 0; i < count; i++) samples.push(await coldStart());
  return samples;
}

function summarizeColds(samples: readonly ColdSample[]): { readonly compile_ns: Summary; readonly load_ns: Summary } {
  return { compile_ns: summarize(samples.map((s) => s.compile_ns)), load_ns: summarize(samples.map((s) => s.load_ns)) };
}

start().catch((error: unknown) => {
  status.textContent = `failed: ${String(error)}`;
  throw error;
});
