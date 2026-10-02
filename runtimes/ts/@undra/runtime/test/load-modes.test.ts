import { afterEach, describe, expect, it, vi } from "vitest";
import { UndraCore } from "../src/core.js";
import { UndraError, UndraTransportError } from "../src/errors.js";
import { info } from "./support/module-graph.js";
import { SCHEMA } from "./support/fake-core.js";

/*
 * `UndraCore.load` knows one mode (wasm-main) and fetches the others (ADR-052, ADR-057): the worker and remote transports each
 * check and map the options of their own mode (`workerTransport`, `remoteTransport`), so a page that asks for neither carries none
 * of it, and hand the core the transport framed, so the adapter arrives with them: no mode gains a round trip. The errors are the
 * ones `load` always threw; a mode that is missing an option now says so after its chunk loaded, and a chunk that cannot be fetched
 * is a typed failure (R6).
 */

afterEach(() => {
  vi.doUnmock("../src/transport/remote.js");
  vi.doUnmock("../src/transport/wasm-worker.js");
  vi.doUnmock("../src/adapters/ports.js");
  vi.resetModules();
});

describe("the options each mode needs", () => {
  it("wasm-main without `wasm`, wasm-worker without `wasm` and remote without `url` reject UndraError('options') naming the option", async () => {
    await expect(UndraCore.load({ mode: "wasm-main", expectedSchemaHash: SCHEMA })).rejects.toMatchObject({ kind: "options", message: expect.stringContaining("`wasm`") });
    await expect(UndraCore.load({ mode: "wasm-worker", expectedSchemaHash: SCHEMA })).rejects.toMatchObject({ kind: "options", message: expect.stringContaining("'wasm-worker' needs the `wasm` option") });
    await expect(UndraCore.load({ mode: "remote", expectedSchemaHash: SCHEMA })).rejects.toMatchObject({ kind: "options", message: expect.stringContaining("'remote' needs the `url` option") });
    expect(await UndraCore.load({ mode: "nope" as "remote", expectedSchemaHash: SCHEMA }).catch((e: unknown) => e)).toBeInstanceOf(UndraError);
  });

  it("a worker without WebCrypto is refused before the worker is made, with the same error as for wasm-main", async () => {
    const original = Object.getOwnPropertyDescriptor(globalThis, "crypto");
    Object.defineProperty(globalThis, "crypto", { value: undefined, configurable: true });
    try {
      let made = 0;
      const failure = await UndraCore.load({
        mode: "wasm-worker",
        wasm: new Uint8Array(8),
        expectedSchemaHash: SCHEMA,
        worker: () => {
          made++;
          throw new Error("no worker should be made");
        },
      }).catch((e: unknown) => e);
      // (A module registry reset by an earlier test made the transport's errors a new class: compare by what they say.)
      expect((failure as Error).name).toBe("UndraTransportError");
      expect((failure as UndraTransportError).reason).toBe("unsupported");
      expect(made).toBe(0);
    } finally {
      if (original !== undefined) Object.defineProperty(globalThis, "crypto", original);
    }
  });
});

describe("a transport chunk that cannot be fetched fails load typed, and the next load tries again", () => {
  for (const [mode, chunk, options] of [
    ["remote", "../src/transport/remote.js", { url: "ws://127.0.0.1:1" }],
    ["wasm-worker", "../src/transport/wasm-worker.js", { wasm: new Uint8Array(8) }],
  ] as const) {
    it(`${mode}: UndraTransportError('closed') with the failed import as its cause, no half-made core`, async () => {
      vi.resetModules();
      let attempts = 0;
      vi.doMock(chunk, async (importOriginal) => {
        attempts++;
        if (attempts === 1) throw new TypeError(`Failed to fetch dynamically imported module: https://app.test/assets/${mode}-abc.js`);
        return importOriginal();
      });
      const { UndraCore: Core } = await import("../src/core.js");
      const { UndraTransportError: Typed } = await import("../src/errors.js");
      const failure = await Core.load({ mode, ...options, expectedSchemaHash: SCHEMA, shared: false } as never).catch((e: unknown) => e);
      expect(failure).toBeInstanceOf(Typed);
      expect((failure as UndraTransportError).reason).toBe("closed");
      // (vitest wraps the factory's own failure; a real failed import carries its TypeError.)
      expect((failure as UndraTransportError).cause).toBeInstanceOf(Error);
      expect(Core.current, "nothing was left loaded").toBeNull();
      // The next load imports the chunk again (and then fails for want of a server, as it should: not as a missing chunk).
      void Core.load({ mode, ...options, expectedSchemaHash: SCHEMA, shared: false, handshakeTimeoutMs: 50 } as never).catch(() => {});
      await vi.waitFor(() => expect(attempts).toBe(2));
    });
  }
});

describe("the other chunks a core loads fail typed too", () => {
  it("the ports a native core is served by: a chunk that cannot be fetched rejects attach, and the transport is not started", async () => {
    vi.resetModules();
    vi.doMock("../src/adapters/ports.js", () => {
      throw new TypeError("Failed to fetch dynamically imported module: https://app.test/assets/ports-abc.js");
    });
    const [{ UndraCore: Core }, { FakeCoreTransport: Fake }] = await Promise.all([import("../src/core.js"), import("./support/fake-core.js")]);
    const fake = new Fake({ mode: "remote" });
    let started = 0;
    const start = fake.start.bind(fake);
    fake.start = (handler) => {
      started++;
      return start(handler);
    };
    const failure = await Core.attach(fake, { expectedSchemaHash: SCHEMA, shared: false, adapters: { http: null, timer: null } }).catch((e: unknown) => e);
    expect((failure as Error).name).toBe("UndraTransportError");
    expect((failure as UndraTransportError).reason).toBe("closed");
    expect(started, "nothing started before the ports were there").toBe(0);
  });
});

describe("no mode gains a round trip for the framing adapter", () => {
  it("the remote and the worker transport import the framed adapter statically (it arrives in their own fetch wave)", () => {
    for (const transport of ["transport/remote.ts", "transport/wasm-worker.ts"]) {
      expect(
        info(transport).edges.some((edge) => edge.target === "transport/framed.ts" && edge.names !== "all" && edge.names.includes("framed")),
        `${transport} must use framed() at run time, or a bundler drops the import and the adapter is a second fetch`,
      ).toBe(true);
    }
  });
});
