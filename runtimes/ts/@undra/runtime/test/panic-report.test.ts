import { afterEach, describe, expect, it, vi } from "vitest";
import type { UndraPanicReport } from "../src/adapters/types.js";
import type { UndraCallError } from "../src/call-error.js";
import { UndraCore } from "../src/core.js";
import { UndraTransportError } from "../src/errors.js";
import { type PanicContext, panicSupport, parsePanicRecord, trapFrames, trapReport, wasmImageId } from "../src/panic-report.js";
import { type UndraCoreRestarted, crashRecovery } from "../src/recovery.js";
import type { WasmSource } from "../src/transport/wasm-main.js";
import { CallTarget } from "../src/wire/index.js";
import { captureLog, macrotask, track } from "./support/harness.js";
import { STUB, compileStub } from "./support/stub-core.js";
import { channelWorker } from "./support/worker.js";

/*
 * The panic report of a wasm core that trapped (ADR-046 decision 4.4): the core's FATAL `undra::panic` record (message, then
 * `    at <file>:<line>:<col>` and `    in <operation>` on lines of their own) and the trap's JavaScript stack, as the same
 * `UndraPanicReport` a native core hands the Diagnostics port.
 */

const CONTEXT: PanicContext = { thread: "main", namespace: "playground_core", coreVersion: "1.2.3", schemaHash: 0xabcdn, imageId: "ff" };

/** A trap as the wasm transports report one: an `UndraTransportError("trap")` whose cause is the engine's error. */
function trapError(stack: string, text = "unreachable"): UndraTransportError {
  const cause = new Error(text);
  cause.name = "RuntimeError";
  cause.stack = stack;
  return new UndraTransportError("trap", `the wasm core trapped: ${text}`, { cause });
}

const V8_STACK = [
  "RuntimeError: unreachable",
  "    at core::panicking::panic (wasm://wasm/0012abcd:wasm-function[123]:0x4567)",
  "    at undra_core::lab::explode (wasm://wasm/0012abcd:wasm-function[77]:0x1f2c)",
  "    at wasm://wasm/0012abcd:wasm-function[9]:0x89",
  "    at WasmMainTransport.send (file:///runtime.js:1:1)",
].join("\n");

describe("the FATAL record of a panic", () => {
  it("is the message, then `at` and `in` on lines of their own", () => {
    expect(parsePanicRecord("kaboom\n    at core/src/lab.rs:42:9\n    in explode")).toEqual({
      message: "kaboom",
      location: "core/src/lab.rs:42:9",
      operation: "explode",
    });
  });

  it("has no `in` line when nothing was running, and keeps newlines of the message (the trailer is read from the end)", () => {
    expect(parsePanicRecord("first line\nsecond line\n    at a.rs:1:2")).toEqual({ message: "first line\nsecond line", location: "a.rs:1:2", operation: "" });
    expect(parsePanicRecord("two\nlines\n    at a.rs:1:2\n    in Todos.add")).toEqual({ message: "two\nlines", location: "a.rs:1:2", operation: "Todos.add" });
  });

  it("does not take a line of the message for a trailer unless it has the four spaces", () => {
    expect(parsePanicRecord("at home\nin the kitchen\n    at a.rs:1:1")).toEqual({ message: "at home\nin the kitchen", location: "a.rs:1:1", operation: "" });
    expect(parsePanicRecord("see\n  at x.rs")).toEqual({ message: "see\n  at x.rs", location: "", operation: "" });
  });

  it("is the older single line before `undra_init`: `<message> at <file>:<line>`", () => {
    expect(parsePanicRecord("index out of bounds at src/lib.rs:12")).toEqual({ message: "index out of bounds", location: "src/lib.rs:12", operation: "" });
    expect(parsePanicRecord("x at a at b.rs:3:4")).toEqual({ message: "x at a", location: "b.rs:3:4", operation: "" });
  });

  it("is just the message when it carries no location", () => {
    expect(parsePanicRecord("something broke")).toEqual({ message: "something broke", location: "", operation: "" });
    expect(parsePanicRecord("")).toEqual({ message: "", location: "", operation: "" });
    expect(parsePanicRecord("boom\n    in task")).toEqual({ message: "boom", location: "", operation: "task" });
  });
});

describe("the frames of a trap's stack", () => {
  it("reads V8's: the offset of each `wasm-function[i]:0x..`, the name when the build has one, else the function", () => {
    expect(trapFrames(V8_STACK)).toEqual([
      { address: 0x4567n, symbol: "core::panicking::panic", file: null, line: null },
      { address: 0x1f2cn, symbol: "undra_core::lab::explode", file: null, line: null },
      { address: 0x89n, symbol: "wasm-function[9]", file: null, line: null },
    ]);
  });

  it("reads Firefox's `name@url:wasm-function[i]:0x..` and Safari's `name@[wasm code]` (best effort)", () => {
    const firefox = ["undra::lab::explode@https://app.test/core.wasm:wasm-function[77]:0x1f2c", "wasm-function[9]@https://app.test/core.wasm:wasm-function[9]:0x89", "send@https://app.test/app.js:10:2"].join("\n");
    expect(trapFrames(firefox)).toEqual([
      { address: 0x1f2cn, symbol: "undra::lab::explode", file: null, line: null },
      { address: 0x89n, symbol: "wasm-function[9]", file: null, line: null },
    ]);
    const safari = ["<?>.wasm-function[77]@[wasm code]", "wasm-function[9]@[wasm code]", "send@https://app.test/app.js:10:2"].join("\n");
    expect(trapFrames(safari)).toEqual([
      { address: 0n, symbol: "wasm-function[77]", file: null, line: null },
      { address: 0n, symbol: "wasm-function[9]", file: null, line: null },
    ]);
  });

  it("reads what a source-map-aware Error.prepareStackTrace makes of V8's frame (`wasm://wasm/hash:1:<offset + 1>`, no function index)", () => {
    const mapped = ["RuntimeError: unreachable", "    at null.<anonymous> (wasm://wasm/6f63fa9a:1:32)", "    at null.<anonymous> (wasm://wasm/6f63fa9a:1:36)", "    at file:///app.js:5:15"].join("\n");
    expect(trapFrames(mapped)).toEqual([
      { address: 0x1fn, symbol: null, file: null, line: null },
      { address: 0x23n, symbol: null, file: null, line: null },
    ]);
  });

  it("ignores every line that is not a frame of the module, and an empty stack has no frames", () => {
    expect(trapFrames("RuntimeError: unreachable\n    at Object.send (file:///a.js:1:1)")).toEqual([]);
    expect(trapFrames("")).toEqual([]);
  });

  it("takes a 64-bit offset as a bigint", () => {
    expect(trapFrames("    at wasm://wasm/x:wasm-function[1]:0xffffffffffff")[0]?.address).toBe(0xffff_ffff_ffffn);
  });
});

describe("the report of a trap", () => {
  it("is the record, the frames and what the runtime knows of the core", () => {
    const report = trapReport("kaboom\n    at core/src/lab.rs:42:9\n    in explode", trapError(V8_STACK), CONTEXT);
    expect(report).toEqual({
      message: "kaboom",
      location: "core/src/lab.rs:42:9",
      operation: "explode",
      thread: "main",
      frames: trapFrames(V8_STACK),
      namespace: "playground_core",
      coreVersion: "1.2.3",
      schemaHash: 0xabcdn,
      imageId: "ff",
    });
    // The same nine fields, and only those, as the report of a native core.
    expect(Object.keys(report).sort()).toEqual(["coreVersion", "frames", "imageId", "location", "message", "namespace", "operation", "schemaHash", "thread"]);
  });

  it("puts the trap's own text in the message when the core logged none (a stack overflow, out of memory)", () => {
    const report = trapReport(null, trapError("RuntimeError: Maximum call stack size exceeded\n    at wasm://wasm/x:wasm-function[5]:0x10", "stack overflow"), { ...CONTEXT, thread: "worker" });
    expect(report.message).toBe("RuntimeError: stack overflow");
    expect(report.location).toBe("");
    expect(report.operation).toBe("");
    expect(report.thread).toBe("worker");
    expect(report.frames).toEqual([{ address: 0x10n, symbol: "wasm-function[5]", file: null, line: null }]);
  });
});

describe("the reporter of a core (what UndraCore starts when the app wants reports)", () => {
  const options = { expectedSchemaHash: 0xabcdn, namespace: "playground_core", coreVersion: "1.2.3" };
  const RECORD = "kaboom\n    at core/src/lab.rs:42:9\n    in explode";

  it("builds the report from the options (the namespace and version the app gave, the schema hash, the thread of the mode) and hands it to onPanic once", () => {
    const seen: UndraPanicReport[] = [];
    const reporter = panicSupport.start({ mode: "wasm-worker", report: () => {} }, { ...options, onPanic: (r) => seen.push(r) });
    const report = reporter.trapped(RECORD, trapError(V8_STACK));
    expect(seen).toEqual([report]);
    expect(report).toMatchObject({ message: "kaboom", thread: "worker", namespace: "playground_core", coreVersion: "1.2.3", schemaHash: 0xabcdn, imageId: "" });
  });

  it("still builds the report with no onPanic (crashRecovery's UndraCoreRestarted carries it), and says \"\" for what the app did not give", () => {
    const reporter = panicSupport.start({ mode: "wasm-main", report: () => {} }, { expectedSchemaHash: 1n });
    expect(reporter.trapped(null, trapError(V8_STACK))).toMatchObject({ thread: "main", namespace: "", coreVersion: "", imageId: "" });
  });

  it("passes a handler's failure to the host and carries on: the report is returned and the next one is delivered", () => {
    const failures: Array<[string, unknown]> = [];
    let calls = 0;
    const reporter = panicSupport.start(
      { mode: "wasm-main", report: (error, operation) => failures.push([operation, error]) },
      {
        ...options,
        onPanic: () => {
          if (++calls === 1) throw new Error("the reporter is down");
        },
      },
    );
    expect(reporter.trapped(RECORD, trapError(V8_STACK)).message).toBe("kaboom");
    expect(reporter.trapped(RECORD, trapError(V8_STACK)).message).toBe("kaboom");
    expect(calls).toBe(2);
    expect(failures).toHaveLength(1);
    expect(failures[0]?.[0]).toBe("onPanic");
    expect(String(failures[0]?.[1])).toContain("the reporter is down");
  });

  it("hashes the module in the background only when the app set onPanic: the image id of a later report", async () => {
    const bytes = new TextEncoder().encode("abc");
    const quiet = panicSupport.start({ mode: "wasm-main", report: () => {} }, { ...options, wasm: bytes });
    const loud = panicSupport.start({ mode: "wasm-main", report: () => {} }, { ...options, wasm: bytes, onPanic: () => {} });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(quiet.trapped(null, trapError(V8_STACK)).imageId).toBe("");
    expect(loud.trapped(null, trapError(V8_STACK)).imageId).toBe("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
  });
});

describe("the module's image id", () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });
  const ABC_SHA256 = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

  it("is the SHA-256 of the module's bytes as lowercase hex, whether given as a buffer or as a view", async () => {
    const bytes = new TextEncoder().encode("abc");
    expect(await wasmImageId(bytes)).toBe(ABC_SHA256);
    expect(await wasmImageId(bytes.buffer.slice(0) as ArrayBuffer)).toBe(ABC_SHA256);
    const padded = new Uint8Array(8);
    padded.set(bytes, 3);
    expect(await wasmImageId(padded.subarray(3, 6))).toBe(ABC_SHA256);
  });

  it("fetches a URL again (from the cache the page just loaded it from) and hashes what it got", async () => {
    const fetched: Array<[string, RequestInit | undefined]> = [];
    vi.stubGlobal("fetch", (url: URL, init?: RequestInit) => {
      fetched.push([String(url), init]);
      return Promise.resolve(new Response(new TextEncoder().encode("abc")));
    });
    expect(await wasmImageId(new URL("https://app.test/core.wasm"))).toBe(ABC_SHA256);
    expect(fetched).toEqual([["https://app.test/core.wasm", { cache: "force-cache" }]]);
  });

  it("is empty when it cannot be had: a failed download, a compiled module (its bytes are gone), no WebCrypto", async () => {
    vi.stubGlobal("fetch", () => Promise.resolve(new Response("nope", { status: 404 })));
    expect(await wasmImageId(new URL("https://app.test/core.wasm"))).toBe("");
    const module = new WebAssembly.Module(Uint8Array.of(0, 0x61, 0x73, 0x6d, 1, 0, 0, 0));
    expect(await wasmImageId(module)).toBe("");
    vi.stubGlobal("crypto", {});
    expect(await wasmImageId(new TextEncoder().encode("abc"))).toBe("");
  });
});

describe("UndraCore and the panic report of a trap (over the stub core, a real wasm module)", () => {
  const FREE = { target: CallTarget.FreeFunction } as const;
  const NO_ARGS = new Uint8Array(0);

  async function load(
    options: { onPanic?: (report: UndraPanicReport) => void; mode?: "wasm-main" | "wasm-worker"; namespace?: string; coreVersion?: string; wasm?: WasmSource; recovery?: boolean } = {},
  ) {
    const bytes = (await compileStub({ snapshot: true })) as Uint8Array<ArrayBuffer>;
    const events: string[] = [];
    const reports: UndraPanicReport[] = [];
    const errors: unknown[] = [];
    const closed: Error[] = [];
    const worker = options.mode === "wasm-worker" ? channelWorker() : undefined;
    const core = track(
      await UndraCore.load({
        mode: options.mode ?? "wasm-main",
        wasm: options.wasm ?? bytes,
        ...(worker && { worker: worker.host }),
        expectedSchemaHash: STUB.SCHEMA_HASH,
        shared: false,
        adapters: { log: captureLog(), http: null, timer: null, kv: null, secureStore: null, fs: null, connectivity: null, lifecycle: null },
        ...(options.namespace !== undefined && { namespace: options.namespace }),
        ...(options.coreVersion !== undefined && { coreVersion: options.coreVersion }),
        ...(options.recovery === true && { recovery: crashRecovery({ snapshotEveryMs: 0 }) }),
        ...(options.onPanic && {
          onPanic: (report: UndraPanicReport) => {
            events.push("onPanic");
            reports.push(report);
            options.onPanic?.(report);
          },
        }),
        onError: (e) => {
          events.push("onError");
          errors.push(e);
        },
        onClose: (e) => {
          events.push("onClose");
          closed.push(e);
        },
        onCoreRestarted: () => events.push("onCoreRestarted"),
      }),
    );
    return { core, bytes, events, reports, errors, closed, stop: () => worker?.close() };
  }

  it("without onPanic (and without recovery) a trap is only onClose: nothing builds a report", async () => {
    const { core, events, reports, closed } = await load({ namespace: "stub_core" });
    await macrotask();
    await expect(core.call(FREE, STUB.PANIC, NO_ARGS)).rejects.toMatchObject({ reason: "trap" });
    await macrotask();
    expect(events).toEqual(["onClose"]);
    expect(reports).toEqual([]);
    expect(closed).toHaveLength(1);
  });

  it("with onPanic: one report, before onClose; the core's namespace and version, the thread, the schema hash, image id and the wasm frames", async () => {
    const { core, bytes, events, reports, closed } = await load({ onPanic: () => {}, namespace: "stub_core", coreVersion: "3.1.4" });
    const wanted = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), (b) => b.toString(16).padStart(2, "0")).join("");
    await macrotask();
    await expect(core.call(FREE, STUB.PANIC, NO_ARGS)).rejects.toMatchObject({ reason: "trap" });
    await macrotask();
    expect(events).toEqual(["onPanic", "onClose"]);
    expect(closed).toHaveLength(1);
    const report = reports[0] as UndraPanicReport;
    expect(reports).toHaveLength(1);
    expect(report.thread).toBe("main");
    expect(report.namespace).toBe("stub_core");
    expect(report.coreVersion).toBe("3.1.4");
    expect(report.schemaHash).toBe(STUB.SCHEMA_HASH);
    expect(report.imageId).toBe(wanted);
    // The stub's panic record carries no target, so the report falls back to the trap's own text.
    expect(report.message).toBe("RuntimeError: unreachable");
    expect(report.frames.length).toBeGreaterThan(0);
    for (const frame of report.frames) {
      expect(typeof frame.address).toBe("bigint");
      expect(frame.file).toBeNull();
      expect(frame.line).toBeNull();
      expect(frame.symbol === null || frame.symbol.length > 0).toBe(true);
    }
  });

  it("says `worker` in wasm-worker mode", async () => {
    const { core, reports, stop } = await load({ mode: "wasm-worker", onPanic: () => {} });
    try {
      await macrotask();
      await expect(core.call(FREE, STUB.PANIC, NO_ARGS)).rejects.toMatchObject({ reason: "trap" });
      await macrotask();
      expect(reports).toHaveLength(1);
      expect(reports[0]?.thread).toBe("worker");
      expect(reports[0]?.frames.length).toBeGreaterThan(0);
    } finally {
      stop();
    }
  });

  it("is empty for the namespace, the version and (a compiled module has no bytes to hash) the image id unless the app says", async () => {
    const module = await WebAssembly.compile((await compileStub({ snapshot: true })) as Uint8Array<ArrayBuffer>);
    const { core, reports } = await load({ onPanic: () => {}, wasm: module });
    await macrotask();
    await expect(core.call(FREE, STUB.PANIC, NO_ARGS)).rejects.toMatchObject({ reason: "trap" });
    await macrotask();
    expect(reports[0]).toMatchObject({ namespace: "", coreVersion: "", imageId: "" });
  });

  it("hashes a module that was given as a URL by fetching it again", async () => {
    const bytes = (await compileStub({ snapshot: true })) as Uint8Array<ArrayBuffer>;
    const fetched: string[] = [];
    vi.stubGlobal("fetch", (url: URL) => {
      fetched.push(String(url));
      return Promise.resolve(new Response(bytes.slice(), { headers: { "content-type": "application/wasm" } }));
    });
    try {
      const { core, reports } = await load({ onPanic: () => {}, wasm: new URL("https://app.test/core.wasm") });
      await vi.waitFor(() => expect(fetched.length).toBeGreaterThanOrEqual(2)); // the load, then the hash
      await macrotask();
      await expect(core.call(FREE, STUB.PANIC, NO_ARGS)).rejects.toMatchObject({ reason: "trap" });
      await macrotask();
      const wanted = Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)), (b) => b.toString(16).padStart(2, "0")).join("");
      expect(reports[0]?.imageId).toBe(wanted);
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("a handler that throws is reported to onError and changes nothing: onClose still follows", async () => {
    const { core, events, errors, closed } = await load({
      onPanic: () => {
        throw new Error("the reporter is down");
      },
    });
    await macrotask();
    await expect(core.call(FREE, STUB.PANIC, NO_ARGS)).rejects.toMatchObject({ reason: "trap" });
    await macrotask();
    expect(events).toEqual(["onPanic", "onError", "onClose"]);
    expect(String((errors[0] as Error).message)).toContain("the reporter is down");
    expect(closed).toHaveLength(1);
  });

  it("with recovery the report comes first, then the restart and onCoreRestarted, which carries the same report", async () => {
    const { core, events, reports } = await load({ onPanic: () => {}, recovery: true, namespace: "stub_core" });
    await macrotask();
    await expect(core.call(FREE, STUB.PANIC, NO_ARGS)).rejects.toMatchObject({ reason: "restarted" });
    await vi.waitFor(() => expect(events).toContain("onCoreRestarted"));
    expect(events.slice(0, 1)).toEqual(["onPanic"]);
    expect(events.indexOf("onPanic")).toBeLessThan(events.indexOf("onCoreRestarted"));
    expect(reports).toHaveLength(1);
    expect(reports[0]?.namespace).toBe("stub_core");
  });

  it("a recovering core without onPanic still has the report (onCoreRestarted's), and the restarted event names the frames", async () => {
    const restarted: UndraCoreRestarted[] = [];
    const core = track(
      await UndraCore.load({
        mode: "wasm-main",
        wasm: await compileStub({ snapshot: true }),
        expectedSchemaHash: STUB.SCHEMA_HASH,
        shared: false,
        adapters: { log: captureLog(), http: null, timer: null, kv: null, secureStore: null, fs: null, connectivity: null, lifecycle: null },
        recovery: crashRecovery({ snapshotEveryMs: 0 }),
        onCoreRestarted: (event) => restarted.push(event),
      }),
    );
    await expect(core.call(FREE, STUB.PANIC, NO_ARGS)).rejects.toMatchObject({ reason: "restarted" });
    await vi.waitFor(() => expect(restarted).toHaveLength(1));
    const event = restarted[0] as UndraCoreRestarted;
    expect(event.report.frames.length).toBeGreaterThan(0);
    expect(event.report.message).toBe("RuntimeError: unreachable");
    expect(event.report.imageId, "the module is hashed only for an app that has onPanic").toBe("");
    expect((event.error as UndraCallError.Panicked).backtrace).toMatch(/wasm-function\[\d+\] \(0x[0-9a-f]+\)|\(0x[0-9a-f]+\)/);
  });
});
