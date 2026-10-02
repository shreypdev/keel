import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { expect, test } from "vitest";
import { UndraCallError, UndraCoreRestarted, UndraTransportError, UndraUnhandledError, crashRecovery, type UndraPanicReport } from "@undra/runtime";
import { UndraIds, add, explode } from "@playground/core";
import { PLAYGROUND_CORE_VERSION, PLAYGROUND_WASM, boot } from "../src/harness.js";
import { step, waitFor } from "../src/wait.js";

// S29 panic report (ADR-046; the wasm column). The wasm core cannot call out of a panic (it traps), so the runtime builds the
// report of `LoadOptions.onPanic` from the core's FATAL `undra::panic` record (message, `    at <file>:<line>:<col>` and
// `    in <operation>` on lines of their own) and the trap's stack, once per trap, before the restart of S22 begins. The native
// column (Kotlin, Swift) is the Diagnostics port; this one is the trap path.

const FIELDS = ["coreVersion", "frames", "imageId", "location", "message", "namespace", "operation", "schemaHash", "thread"];

/** Whether `error` is what a generated call fails with when the core is out of reach for `reason`. */
function unavailable(error: unknown, reason: string): boolean {
  return error instanceof UndraCallError.Unavailable && error.transport.reason === reason;
}

/** What step 1 asks of the report of `explode("kaboom")`, in any of the three ways the core is loaded. */
function expectTrapReport(report: UndraPanicReport, imageId: string): void {
  expect(Object.keys(report).sort(), "the nine fields of the one report shape, and only those").toEqual(FIELDS);
  expect(report.message).toContain("kaboom");
  expect(report.location, "the panic's location, from the FATAL record").toContain("lab.rs:");
  expect(report.location).toMatch(/:\d+:\d+$/);
  expect(report.operation, "the call the core was running").toBe("explode");
  expect(report.thread).toBe("main");
  expect(report.namespace).toBe(UndraIds.namespace);
  expect(report.namespace).toBe("playground_core");
  expect(report.coreVersion).toBe(PLAYGROUND_CORE_VERSION);
  expect(report.coreVersion).not.toBe("");
  expect(report.schemaHash).toBe(UndraIds.schemaHash);
  expect(report.imageId, "the SHA-256 of the module's bytes").toMatch(/^[0-9a-f]{64}$/);
  expect(report.imageId).toBe(imageId);
  expect(report.frames.length, "the wasm frames of the trap").toBeGreaterThan(0);
  for (const frame of report.frames) {
    expect(typeof frame.address, "the module offset of a wasm-function[i]:0x.. line").toBe("bigint");
    expect(frame.symbol === null || frame.symbol.length > 0).toBe(true);
    expect(frame.file).toBeNull();
    expect(frame.line).toBeNull();
  }
}

test("S29 panic report", async () => {
  const imageId = createHash("sha256")
    .update(await readFile(PLAYGROUND_WASM))
    .digest("hex");
  const load = { namespace: UndraIds.namespace, coreVersion: PLAYGROUND_CORE_VERSION } as const;

  await step("1. explode('kaboom') fails as in S17; onPanic received exactly one report, with everything the app's crash reporter needs", async () => {
    const reports: UndraPanicReport[] = [];
    const { core, closed, runtimeErrors } = await boot({
      wasmBytes: true,
      load: { ...load, onPanic: (report) => reports.push(report) },
    });
    // `load` resolved once the module was hashed: the id is there for any trap after it.
    await waitFor("one call to answer", async () => (await add(1, 2, core)) === 3);
    const failure = await explode("kaboom", core).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(unavailable(failure, "trap"), `the call fails as in S17: ${String(failure)}`).toBe(true);
    expect((failure as UndraCallError.Unavailable).transport).toBeInstanceOf(UndraTransportError);
    await waitFor("the core to close", () => core.closed);
    expect(closed, "onClose, after the report").toHaveLength(1);
    expect(reports, "exactly one report").toHaveLength(1);
    expectTrapReport(reports[0] as UndraPanicReport, imageId);
    expect(runtimeErrors).toEqual([]);
  });

  await step("2. with recovery on (S22) the same report arrives, before onCoreRestarted; and the core keeps working", async () => {
    const events: string[] = [];
    const reports: UndraPanicReport[] = [];
    const restarts: UndraCoreRestarted[] = [];
    const { core, runtimeErrors } = await boot({
      wasmBytes: true,
      load: {
        ...load,
        recovery: crashRecovery({ snapshotEveryMs: 50 }),
        onPanic: (report) => {
          events.push("onPanic");
          reports.push(report);
        },
        onCoreRestarted: (event) => {
          events.push("onCoreRestarted");
          restarts.push(event);
        },
      },
    });
    await waitFor("one call to answer", async () => (await add(1, 2, core)) === 3);
    const failure = await explode("kaboom", core).then(
      () => undefined,
      (e: unknown) => e,
    );
    expect(unavailable(failure, "restarted"), `the call fails as in S22: ${String(failure)}`).toBe(true);
    await waitFor("the restart", () => restarts.length === 1);
    expect(events, "the report first, then the restart").toEqual(["onPanic", "onCoreRestarted"]);
    expect(reports).toHaveLength(1);
    expectTrapReport(reports[0] as UndraPanicReport, imageId);
    expect(restarts[0]?.report, "the restart event carries the very same report").toBe(reports[0]);
    expect(await add(1, 2, core), "the core keeps working").toBe(3);
    expect(runtimeErrors, "onError heard the restart event, nothing else").toEqual([restarts[0]]);
    runtimeErrors.length = 0;
  });

  await step("3. a reporter that throws is reported to onError and changes nothing: the next trap is reported all the same", async () => {
    const reports: UndraPanicReport[] = [];
    const restarts: UndraCoreRestarted[] = [];
    const { core, runtimeErrors } = await boot({
      wasmBytes: true,
      load: {
        ...load,
        recovery: crashRecovery({ snapshotEveryMs: 50, maxRestarts: 3 }),
        onPanic: (report) => {
          reports.push(report);
          if (reports.length === 1) throw new Error("the crash reporter is down");
        },
        onCoreRestarted: (event) => {
          restarts.push(event);
        },
      },
    });
    await waitFor("one call to answer", async () => (await add(1, 2, core)) === 3);
    for (const n of [1, 2]) {
      const failure = await explode(`kaboom ${n}`, core).then(
        () => undefined,
        (e: unknown) => e,
      );
      expect(unavailable(failure, "restarted"), `trap ${n}: ${String(failure)}`).toBe(true);
      await waitFor(`restart ${n}`, () => restarts.length === n);
    }
    expect(reports.map((r) => r.message)).toEqual([expect.stringContaining("kaboom 1"), expect.stringContaining("kaboom 2")]);
    const thrown = runtimeErrors.find((e): e is UndraUnhandledError => e instanceof UndraUnhandledError && e.operation === "onPanic");
    expect(thrown, "the throwing handler was reported to onError").toBeDefined();
    expect(String((thrown as UndraUnhandledError).cause)).toContain("the crash reporter is down");
    expect(runtimeErrors.filter((e) => e instanceof UndraCoreRestarted), "and the restarts were reported as before").toHaveLength(2);
    runtimeErrors.length = 0;
  });
});
