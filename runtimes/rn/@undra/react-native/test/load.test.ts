import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { afterEach, describe, expect, test } from "vitest";
import { UndraCallError, UndraCore, UndraTransportError, UndraUnhandledError } from "@undra/runtime";
import { loadNative } from "../src/index.js";
import { lifecycleState } from "../src/adapters.js";
import { RecordKind } from "../src/native.js";
import { setAppState, setTurboModule } from "./support/react-native-stub.js";
import { FakeNative } from "./support/fake-native.js";

const g = globalThis as { __undraNative?: unknown };

afterEach(() => {
  delete g.__undraNative;
  setTurboModule("UndraNative", undefined);
  setAppState("active");
});

describe("loadNative", () => {
  test("installs the module, attaches a native core and makes it shared", async () => {
    const native = new FakeNative();
    let installs = 0;
    setTurboModule("UndraNative", {
      install() {
        installs++;
        g.__undraNative = native;
        return true;
      },
    });
    const core = await loadNative({ expectedSchemaHash: native.hash, adapters: { http: null } });
    expect(installs).toBe(1);
    expect(core.mode).toBe("native");
    expect(UndraCore.shared).toBe(core);
    const r = native.started!.config;
    expect(new TextDecoder().decode(r.subarray(4, 4 + "react-native-test".length))).toBe("react-native-test");
    // A second load while it is open is the same core; after close, a new one.
    expect(await loadNative({ expectedSchemaHash: native.hash })).toBe(core);
    core.close();
    const again = await loadNative({ expectedSchemaHash: native.hash });
    expect(again).not.toBe(core);
    again.close();
  });

  test("a failure of the native module that no caller can see reaches onError as an UndraUnhandledError", async () => {
    const native = new FakeNative();
    setTurboModule("UndraNative", {
      install() {
        g.__undraNative = native;
        return true;
      },
    });
    const reported: UndraUnhandledError[] = [];
    const core = await loadNative({
      expectedSchemaHash: native.hash,
      adapters: { http: null },
      onError: (error) => reported.push(error),
    });
    // An inbox record of a kind nobody sends: the transport cannot hand it to a caller.
    native.queue(99 as (typeof RecordKind)[keyof typeof RecordKind], new Uint8Array(0), "core");
    await new Promise((resolve) => setTimeout(resolve, 10));
    expect(reported).toHaveLength(1);
    expect(reported[0]).toBeInstanceOf(UndraUnhandledError);
    expect(reported[0]?.operation).toBe("the native module");
    expect(reported[0]?.error).toBeInstanceOf(UndraCallError.Malformed);
    core.close();
  });

  test("rejects with a clear error when the TurboModule is not linked", async () => {
    await expect(loadNative({ expectedSchemaHash: 1n })).rejects.toSatisfy(
      (e: unknown) => e instanceof UndraTransportError && e.reason === "unsupported" && /not linked/.test(e.message),
    );
  });

  test("the Lifecycle adapter follows AppState", () => {
    expect(lifecycleState("active")).toBe("active");
    expect(lifecycleState("inactive")).toBe("inactive");
    expect(lifecycleState("background")).toBe("background");
    expect(lifecycleState("unknown")).toBe("background");
  });
});

test("cpp/undra.h is byte-identical to the Swift runtime's (the single source of truth of the C ABI)", () => {
  const ours = readFileSync(fileURLToPath(new URL("../cpp/undra.h", import.meta.url)));
  const swift = readFileSync(
    fileURLToPath(new URL("../../../../swift/UndraRuntime/Sources/UndraFFI/include/undra.h", import.meta.url)),
  );
  expect(ours.equals(swift)).toBe(true);
});
