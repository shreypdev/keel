import { afterEach, describe, expect, it, vi } from "vitest";
import { WEB_CRYPTO_REQUIRED, cryptoRng, hasCryptoRandom } from "../src/adapters/system.js";
import { UndraCore } from "../src/core.js";
import { UndraTransportError } from "../src/errors.js";
import { WasmMainTransport } from "../src/transport/wasm-main.js";
import { CallTarget } from "../src/wire/index.js";
import { compileStub, STUB } from "./support/stub-core.js";
import { captureLog, track } from "./support/harness.js";

/*
 * Randomness never degrades silently (ADR-049 decision 2.5, gap PO-11): without a CSPRNG the `random`
 * import writes nothing (so the core's canary trips and its `Rng` answers unavailable instead of zeros),
 * and `UndraCore.load` refuses a wasm mode up front. The core's side of the canary is tested over the real
 * module in crates/undra-ffi/tests/wasm.
 */

const FREE = { target: CallTarget.FreeFunction } as const;
const realCrypto = globalThis.crypto;

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("cryptoRng", () => {
  it("knows when a platform has a CSPRNG", () => {
    expect(hasCryptoRandom()).toBe(true);
    expect(hasCryptoRandom(null)).toBe(false);
    expect(hasCryptoRandom({})).toBe(false);
    expect(hasCryptoRandom({ getRandomValues: 1 })).toBe(false);
    expect(hasCryptoRandom({ getRandomValues: () => undefined })).toBe(true);
  });

  it("throws at once without WebCrypto, or with a crypto that has no getRandomValues", () => {
    expect(() => cryptoRng({} as Crypto)).toThrow(WEB_CRYPTO_REQUIRED);
    vi.stubGlobal("crypto", undefined);
    expect(hasCryptoRandom()).toBe(false);
    expect(() => cryptoRng()).toThrow(TypeError);
    expect(() => cryptoRng()).toThrow(/WebCrypto is required/);
  });

  it("fill propagates a failing getRandomValues instead of returning what it has", () => {
    const rng = cryptoRng({
      getRandomValues: () => {
        throw new DOMException("no entropy", "OperationError");
      },
    } as unknown as Crypto);
    expect(() => rng.fill(new Uint8Array(4))).toThrow("no entropy");
  });

  it("fills more than one getRandomValues call can (65,536 bytes) in chunks", () => {
    const sizes: number[] = [];
    const rng = cryptoRng({
      getRandomValues: <T extends ArrayBufferView | null>(out: T): T => {
        sizes.push((out as unknown as Uint8Array).length);
        (out as unknown as Uint8Array).fill(1);
        return out;
      },
    } as unknown as Crypto);
    const out = new Uint8Array(150_000);
    rng.fill(out);
    expect(sizes).toEqual([65_536, 65_536, 18_928]);
    expect(out.every((b) => b === 1)).toBe(true);
  });
});

describe("the random import without a CSPRNG", () => {
  /** The stub's RANDOM_NOW reads 8 bytes from `random` into 0x500 and replies with them. */
  async function stubWithMarker(options: { rng?: { fill(out: Uint8Array): void } } = {}) {
    const log = captureLog();
    const transport = new WasmMainTransport({
      wasm: await compileStub(),
      expectedSchemaHash: STUB.SCHEMA_HASH,
      ...(options.rng && { rng: options.rng }),
      onError: (error) => {
        log.log(4, "undra::runtime", `import failed: ${error instanceof Error ? error.message : String(error)}`);
      },
    });
    const core = track(await UndraCore.attach(transport, { expectedSchemaHash: STUB.SCHEMA_HASH, shared: false, adapters: { log } }));
    // A marker where the stub asks for random bytes: what the import leaves of it is what the core sees.
    const memory = (transport.instance as WebAssembly.Instance).exports.memory as WebAssembly.Memory;
    new Uint8Array(memory.buffer, 0x500, 8).set([0xca, 0xfe, 0xca, 0xfe, 0xca, 0xfe, 0xca, 0xfe]);
    return { core, log };
  }

  it("writes nothing when the platform has no WebCrypto, and reports why", async () => {
    vi.stubGlobal("crypto", undefined);
    const { core, log } = await stubWithMarker();
    const body = await core.call(FREE, STUB.RANDOM_NOW, new Uint8Array(0));
    expect([...body.subarray(0, 8)]).toEqual([0xca, 0xfe, 0xca, 0xfe, 0xca, 0xfe, 0xca, 0xfe]);
    expect(log.records.some((r) => r.message.includes("WebCrypto is required"))).toBe(true);
  });

  it("writes nothing when getRandomValues throws", async () => {
    vi.stubGlobal("crypto", {
      getRandomValues: () => {
        throw new Error("the entropy source failed");
      },
    });
    const { core, log } = await stubWithMarker();
    const body = await core.call(FREE, STUB.RANDOM_NOW, new Uint8Array(0));
    expect([...body.subarray(0, 8)]).toEqual([0xca, 0xfe, 0xca, 0xfe, 0xca, 0xfe, 0xca, 0xfe]);
    expect(log.records.some((r) => r.message.includes("the entropy source failed"))).toBe(true);
  });

  it("with WebCrypto, the bytes are random", async () => {
    const { core } = await stubWithMarker();
    const body = await core.call(FREE, STUB.RANDOM_NOW, new Uint8Array(0));
    expect([...body.subarray(0, 8)]).not.toEqual([0xca, 0xfe, 0xca, 0xfe, 0xca, 0xfe, 0xca, 0xfe]);
  });
});

describe("UndraCore.load needs WebCrypto in the wasm modes", () => {
  it("wasm-main: rejects with UndraTransportError('unsupported', 'WebCrypto is required ...') before instantiating", async () => {
    vi.stubGlobal("crypto", undefined);
    const instantiate = vi.spyOn(WebAssembly, "instantiate");
    const failure = await UndraCore.load({ mode: "wasm-main", wasm: await compileStub(), expectedSchemaHash: STUB.SCHEMA_HASH, shared: false }).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(failure).toBeInstanceOf(UndraTransportError);
    expect((failure as UndraTransportError).reason).toBe("unsupported");
    expect((failure as UndraTransportError).message).toMatch(/^WebCrypto is required/);
    expect(instantiate).not.toHaveBeenCalled();
    instantiate.mockRestore();
  });

  it("wasm-main: an app that supplies its own rng adapter has a random source, and loads", async () => {
    vi.stubGlobal("crypto", undefined);
    const core = track(
      await UndraCore.load({
        mode: "wasm-main",
        wasm: await compileStub(),
        expectedSchemaHash: STUB.SCHEMA_HASH,
        shared: false,
        adapters: { log: captureLog(), rng: { fill: (out) => out.fill(4) } },
      }),
    );
    const body = await core.call(FREE, STUB.RANDOM_NOW, new Uint8Array(0));
    expect([...body.subarray(0, 8)]).toEqual([4, 4, 4, 4, 4, 4, 4, 4]);
  });

  it("wasm-worker: rejects on the main thread before the worker is created", async () => {
    vi.stubGlobal("crypto", { subtle: realCrypto.subtle });
    const createWorker = vi.fn(() => {
      throw new Error("the worker must not be created");
    });
    const failure = await UndraCore.load({ mode: "wasm-worker", wasm: new Uint8Array(8), worker: createWorker, expectedSchemaHash: 1n, shared: false }).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(failure).toBeInstanceOf(UndraTransportError);
    expect((failure as UndraTransportError).reason).toBe("unsupported");
    expect((failure as UndraTransportError).message).toBe(WEB_CRYPTO_REQUIRED);
    expect(createWorker).not.toHaveBeenCalled();
  });
});
