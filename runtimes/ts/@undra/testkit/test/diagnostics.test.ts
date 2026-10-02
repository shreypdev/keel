import type { UndraPanicReport } from "@undra/runtime";
import { describe, expect, it } from "vitest";
import { CaptureDiagnostics, createFakes, methodId, portId, standardName } from "../src/index.js";

// The fake of the Diagnostics port (ADR-046): the same API as `undra::ports::fakes::CaptureDiagnostics`, in the testkit's conventions.

const report = (operation: string, message = "boom"): UndraPanicReport => ({
  message,
  location: "src/lib.rs:1:1",
  operation,
  thread: "main",
  frames: [],
  namespace: "app_core",
  coreVersion: "1.0.0",
  schemaHash: 7n,
  imageId: "",
});

describe("CaptureDiagnostics", () => {
  it("keeps every report, oldest first, and the last one", () => {
    const diagnostics = new CaptureDiagnostics();
    expect(diagnostics.length).toBe(0);
    expect(diagnostics.last).toBeUndefined();
    diagnostics.panicked(report("Todos.add"));
    diagnostics.panicked(report("task", "later"));
    expect(diagnostics.length).toBe(2);
    expect(diagnostics.reports.map((r) => r.operation)).toEqual(["Todos.add", "task"]);
    expect(diagnostics.last?.message).toBe("later");
  });

  it("take removes and returns the reports, clear forgets them", () => {
    const diagnostics = new CaptureDiagnostics();
    diagnostics.panicked(report("a"));
    diagnostics.panicked(report("b"));
    expect(diagnostics.take().map((r) => r.operation)).toEqual(["a", "b"]);
    expect(diagnostics.length).toBe(0);
    diagnostics.panicked(report("c"));
    diagnostics.clear();
    expect(diagnostics.reports).toEqual([]);
  });

  it("onPanic is bound: it can be handed to UndraCore.load as it is", () => {
    const diagnostics = new CaptureDiagnostics();
    const { onPanic } = diagnostics;
    onPanic(report("explode"));
    expect(diagnostics.last?.operation).toBe("explode");
  });

  it("createFakes gives a fresh one with the others", () => {
    const a = createFakes();
    const b = createFakes();
    a.diagnostics.panicked(report("x"));
    expect(a.diagnostics.length).toBe(1);
    expect(b.diagnostics.length).toBe(0);
  });

  it("the standard port is named for a recording", () => {
    expect(standardName(portId("Diagnostics"), methodId("Diagnostics", "panicked"))).toBe("Diagnostics.panicked");
    expect(portId("Diagnostics")).toBe(0xab68cd7c);
    expect(methodId("Diagnostics", "panicked")).toBe(0xbd147e2e);
  });
});
