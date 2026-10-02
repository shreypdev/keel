import type { UndraPanicReport } from "@undra/runtime";
import { describe, expect, it } from "vitest";
import { PanicLog, describePanic } from "./panic-log";

const report = (patch: Partial<UndraPanicReport> = {}): UndraPanicReport => ({
  message: "crashed on purpose",
  location: "core/src/lab.rs:42:9",
  operation: "explode",
  thread: "main",
  frames: [
    { address: 0x4567n, symbol: "core::panicking::panic", file: null, line: null },
    { address: 0x89n, symbol: null, file: "lab.rs", line: 7 },
  ],
  namespace: "playground_core",
  coreVersion: "1.0.0",
  schemaHash: 0x53241303b2d08c5en,
  imageId: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  ...patch,
});

describe("the panic log of the debug panel", () => {
  it("keeps the newest report and counts them", () => {
    const log = new PanicLog();
    expect(log.last.peek()).toBeNull();
    expect(log.count).toBe(0);
    log.record(report({ message: "first" }));
    log.record(report({ message: "second" }));
    expect(log.last.peek()?.message).toBe("second");
    expect(log.count).toBe(2);
  });

  it("describes what panicked, where, in which build, and the frames", () => {
    expect(describePanic(report())).toEqual([
      "crashed on purpose",
      "at core/src/lab.rs:42:9, in explode, thread main",
      "playground_core 1.0.0, schema 0x53241303b2d08c5e, image 0123456789ab…",
      "2 frames:",
      "  0x4567 core::panicking::panic",
      "  0x89 <unknown> lab.rs:7",
    ]);
  });

  it("says what it does not know, and cuts a long stack", () => {
    const lines = describePanic(
      report({
        location: "",
        operation: "",
        namespace: "",
        coreVersion: "",
        imageId: "",
        frames: Array.from({ length: 6 }, (_, i) => ({ address: BigInt(i), symbol: null, file: null, line: null })),
      }),
    );
    expect(lines.slice(0, 4)).toEqual(["crashed on purpose", "at no location, nothing running, thread main", "unnamed core, schema 0x53241303b2d08c5e, image not hashed", "6 frames:"]);
    expect(lines.at(-1)).toBe("  … 2 more");
    expect(describePanic(report({ frames: [] })).at(-1)).toBe("no frames");
  });
});
