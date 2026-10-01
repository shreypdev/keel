import { UndraCore, type AdapterOverrides, type AttachOptions, type PortImpl, type WasmSource } from "@undra/runtime";
import { createFakes, type Fakes, type FakeClock } from "./fakes.js";
import { settleCore } from "./recorded.js";
import { applySeed, parseSeed, type Seed } from "./seed.js";

/** Options of {@link PreviewCore.load}. */
export interface PreviewOptions extends Pick<AttachOptions, "onError" | "shared" | "mirror"> {
  /** The app's own core, built for the web (`undra build --platform web`): a URL (a string is resolved against the page), its bytes or a compiled module. */
  readonly wasm: WasmSource | string;
  /** The schema hash of the bindings (`UndraIds.schemaHash`). */
  readonly expectedSchemaHash: bigint;
  /** The starting state of the fakes: a {@link Seed} or its JSON text (`testkit/fixtures/seed.json` is an example). */
  readonly seed?: Seed | string;
  /** Fakes to use instead of fresh ones (for example ones a test already holds). The seed, if any, is applied to them. */
  readonly fakes?: Fakes;
  /** Further adapter overrides on top of the fakes; `null` removes a port. */
  readonly adapters?: AdapterOverrides;
  /** Ports of the app's own, by port id. */
  readonly ports?: Readonly<Record<number, PortImpl>>;
}

/**
 * The app's own core, loaded in this page with the deterministic fakes as its ports and a manual clock, for a Storybook story, a component
 * test or a preview: the real logic, scripted ports, and time that moves when you say so.
 *
 * ```ts
 * const preview = await PreviewCore.load({
 *   wasm: "/undra_core.wasm",
 *   expectedSchemaHash: UndraIds.schemaHash,
 *   seed: { http: [{ url: "https://api.test/todos", reply: response(200, "[]") }] },
 * });
 * const todos = new Todos(preview.core);
 * await preview.advance(31_000);               // the cached list goes stale; the core refetches
 * ```
 *
 * The fakes sit in this thread, so only the `wasm-main` mode is offered.
 */
export class PreviewCore {
  /** The core. */
  readonly core: UndraCore;
  /** The fakes it runs on: script `fakes.http`, seed `fakes.kv`, read `fakes.log`. */
  readonly fakes: Fakes;

  private constructor(core: UndraCore, fakes: Fakes) {
    this.core = core;
    this.fakes = fakes;
  }

  /** Loads the core with the fakes installed. */
  static async load(options: PreviewOptions): Promise<PreviewCore> {
    const fakes = options.fakes ?? createFakes();
    if (options.seed !== undefined) applySeed(fakes, typeof options.seed === "string" ? parseSeed(options.seed) : options.seed);
    const core = await UndraCore.load({
      mode: "wasm-main",
      wasm: typeof options.wasm === "string" ? new URL(options.wasm, (globalThis as { location?: { href: string } }).location?.href ?? "file:///") : options.wasm,
      expectedSchemaHash: options.expectedSchemaHash,
      adapters: {
        clock: fakes.clock,
        timer: fakes.clock,
        rng: fakes.rng,
        log: fakes.log,
        http: fakes.http,
        kv: fakes.kv,
        secureStore: fakes.secureStore,
        fs: fakes.fs,
        connectivity: fakes.connectivity,
        lifecycle: fakes.lifecycle,
        ...options.adapters,
      },
      ...(options.ports !== undefined && { ports: options.ports }),
      ...(options.onError !== undefined && { onError: options.onError }),
      ...(options.shared !== undefined && { shared: options.shared }),
      ...(options.mirror !== undefined && { mirror: options.mirror }),
    });
    const preview = new PreviewCore(core, fakes);
    await preview.settle();
    return preview;
  }

  /** The manual clock: `nowMs()` reads it, `setNowMs(ms)` jumps the wall clock, {@link PreviewCore.advance} moves time. */
  get clock(): FakeClock {
    return this.fakes.clock;
  }

  /** Lets the core and the mirror catch up: ports answer, tasks run, change-sets are applied. */
  settle(): Promise<void> {
    return settleCore();
  }

  /**
   * Moves the manual clock forward by `ms`, one deadline at a time: at each deadline the clock reads exactly that instant, the due timer
   * fires into the core (completing a `ctx.sleep`, or a stale-time refetch), and the core settles before time moves on, so a task that
   * sleeps again inside the window is served within the same call. Returns how many timers fired.
   */
  async advance(ms: number): Promise<number> {
    const clock = this.fakes.clock;
    let left = Math.max(0, Math.ceil(ms));
    let fired = 0;
    await this.settle();
    for (;;) {
      const due = clock.nextDueInMs();
      if (due === undefined || due > left) break;
      if (clock.fireNext(left) === undefined) break;
      left -= due;
      fired += 1;
      await this.settle();
    }
    clock.moveBy(left);
    await this.settle();
    return fired;
  }

  /** Closes the core. */
  close(): void {
    this.core.close();
  }
}
