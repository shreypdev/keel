import { describe, expect, it } from "vitest";
import { PortIds } from "../src/adapters/ids.js";
import {
  BackgroundReportCodec,
  PanicFrameCodec,
  PanicReportCodec,
  readBackgroundReport,
  readPanicReport,
} from "../src/adapters/codecs.js";
import { diagnosticsPort } from "../src/adapters/ports.js";
import type { UndraBackgroundReport, UndraPanicFrame, UndraPanicReport } from "../src/adapters/types.js";
import { UndraUnhandledError } from "../src/call-error.js";
import { UndraCore } from "../src/core.js";
import { fnv1a32 } from "../src/fnv.js";
import { PortStatus, UndraReader, decodeValue, encodeValue, type Codec } from "../src/wire/index.js";
import * as pkg from "../src/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { captureLog, track } from "./support/harness.js";

/*
 * ADR-046 on the host side of a native core: the records of the Diagnostics port and of `run_background` (the golden bytes
 * of crates/undra-ports/tests/encoding.rs, byte for byte), the standard ids, and the Diagnostics port itself, which a native
 * core (React Native, a `remote` core) calls once per panic it contained.
 */

/** `"0000 01000000 75"` to bytes; whitespace is ignored. */
function hex(text: string): Uint8Array {
  const digits = text.replace(/\s+/g, "");
  expect(digits.length % 2, `odd number of hex digits in ${text}`).toBe(0);
  return Uint8Array.from(digits.match(/../g) ?? [], (pair) => Number.parseInt(pair, 16));
}

/**
 * The shape of `assert_codec` of the Rust test: `value` encodes to exactly `expected`, `expected` decodes back to `value`, and
 * every strict prefix of it and it plus a stray byte is a decode error (never a wrong value).
 */
function assertCodec<T>(codec: Codec<T>, value: T, expected: string): void {
  const bytes = hex(expected);
  expect(encodeValue(codec, value)).toEqual(bytes);
  expect(decodeValue(codec, bytes)).toEqual(value);
  for (let cut = 0; cut < bytes.length; cut++) {
    expect(() => decodeValue(codec, bytes.subarray(0, cut)), `a prefix of ${cut} bytes`).toThrow();
  }
  expect(() => decodeValue(codec, Uint8Array.of(...bytes, 0))).toThrow();
}

const REPORT: UndraPanicReport = {
  message: "m",
  location: "l",
  operation: "o",
  thread: "t",
  frames: [{ address: 2n, symbol: null, file: null, line: 1 }],
  namespace: "n",
  coreVersion: "1.0",
  schemaHash: 0x0102n,
  imageId: "ab",
};

describe("the ADR-046 records (golden bytes of undra-ports)", () => {
  it("a panic frame is the address, the symbol, the file and the line", () => {
    assertCodec<UndraPanicFrame>(PanicFrameCodec, { address: 0x1122n, symbol: null, file: null, line: null }, "2211000000000000 00 00 00");
    assertCodec<UndraPanicFrame>(
      PanicFrameCodec,
      { address: 1n, symbol: "f", file: "a.rs", line: 7 },
      "0100000000000000 01 01000000 66 01 04000000 612e7273 01 07000000",
    );
  });

  it("a panic report is nine fields in declaration order", () => {
    assertCodec(
      PanicReportCodec,
      REPORT,
      "01000000 6d 01000000 6c 01000000 6f 01000000 74 01000000 0200000000000000 00 00 01 01000000 01000000 6e 03000000 312e30 0201000000000000 02000000 6162",
    );
    assertCodec(
      PanicReportCodec,
      { message: "", location: "", operation: "", thread: "", frames: [], namespace: "", coreVersion: "", schemaHash: 0n, imageId: "" },
      "00000000 00000000 00000000 00000000 00000000 00000000 00000000 0000000000000000 00000000",
    );
  });

  it("a background report is a bool and three counts, and a bool that is neither 0 nor 1 is refused", () => {
    const report: UndraBackgroundReport = { finished: true, replayed: 1, refetched: 2, stillPending: 3 };
    assertCodec(BackgroundReportCodec, report, "01 01000000 02000000 03000000");
    expect(() => decodeValue(BackgroundReportCodec, hex("02 00000000 00000000 00000000"))).toThrow();
  });

  it("the read halves read from a reader and leave the rest of it alone", () => {
    const bytes = Uint8Array.of(...encodeValue(PanicReportCodec, REPORT), 0xff);
    const r = new UndraReader(bytes);
    expect(readPanicReport(r)).toEqual(REPORT);
    expect(r.remaining).toBe(1);
    const bg = new UndraReader(hex("00 05000000 00000000 00000000"));
    expect(readBackgroundReport(bg)).toEqual({ finished: false, replayed: 5, refetched: 0, stillPending: 0 });
  });

  it("a hostile frame count cannot ask for more frames than the input holds", () => {
    // message, location, operation, thread (empty), then 2^32 - 1 frames and nothing behind them.
    const hostile = hex("00000000 00000000 00000000 00000000 ffffffff");
    expect(() => decodeValue(PanicReportCodec, hostile)).toThrow();
  });
});

describe("the package", () => {
  it("exports the records, their codecs and the Diagnostics port from its entry", () => {
    const names: Array<keyof typeof pkg> = ["PanicFrameCodec", "PanicReportCodec", "BackgroundReportCodec", "diagnosticsPort", "readPanicReport", "writePanicReport", "readBackgroundReport", "PortIds"];
    for (const name of names) expect(pkg[name], name).toBeDefined();
    // The trap-to-report code is the runtime's own: a page that wants no reports must not ship it (it is loaded on demand).
    expect(Object.keys(pkg)).not.toContain("trapReport");
    expect(Object.keys(pkg)).not.toContain("wasmImageId");
  });
});

describe("the standard ids (pinned: the Kotlin and Swift runtimes hard-code the same)", () => {
  it("the Diagnostics port and its method", () => {
    expect(PortIds.Diagnostics.portId).toBe(0xab68cd7c);
    expect(PortIds.Diagnostics.panicked).toBe(0xbd147e2e);
    expect(fnv1a32("port.Diagnostics")).toBe(0xab68cd7c);
    expect(fnv1a32("Diagnostics.panicked")).toBe(0xbd147e2e);
  });

  it("the run_background function", () => {
    expect(fnv1a32("fn.run_background")).toBe(0x0e5b14ff);
  });
});

async function native(options: { onPanic?: (report: UndraPanicReport) => void; onError?: (error: UndraUnhandledError) => void; mode?: string } = {}) {
  const fake = new FakeCoreTransport({ mode: options.mode ?? "native" });
  const log = captureLog();
  const core = track(
    await UndraCore.attach(fake, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: { log, http: null, timer: null, kv: null, secureStore: null, fs: null, connectivity: null, lifecycle: null },
      ...(options.onPanic && { onPanic: options.onPanic }),
      ...(options.onError && { onError: options.onError }),
    }),
  );
  return { fake, core, log };
}

const report = (n: number): UndraPanicReport => ({ ...REPORT, message: `panic ${n}`, operation: `op ${n}` });
const panicked = (r: UndraPanicReport): Uint8Array => encodeValue(PanicReportCodec, r);

describe("the Diagnostics port of a native core", () => {
  it("decodes the report, calls onPanic once per call, in order, and answers the port", async () => {
    const seen: UndraPanicReport[] = [];
    const { fake } = await native({ onPanic: (r) => seen.push(r) });
    for (const n of [1, 2, 3]) {
      const reply = await fake.callPort(PortIds.Diagnostics.portId, PortIds.Diagnostics.panicked, panicked(report(n)));
      expect(reply.status).toBe(PortStatus.Ok);
      expect(reply.body).toEqual(new Uint8Array(0));
    }
    expect(seen).toEqual([report(1), report(2), report(3)]);
    expect(typeof seen[0]?.schemaHash).toBe("bigint");
  });

  it("a handler that throws is reported to onError and changes nothing: the core is answered, the next report still arrives", async () => {
    const seen: string[] = [];
    const errors: UndraUnhandledError[] = [];
    const { fake, log } = await native({
      onPanic: (r) => {
        seen.push(r.message);
        if (r.message === "panic 1") throw new Error("the reporter is down");
      },
      onError: (e) => errors.push(e),
    });
    const first = await fake.callPort(PortIds.Diagnostics.portId, PortIds.Diagnostics.panicked, panicked(report(1)));
    expect(first.status).toBe(PortStatus.Ok);
    const second = await fake.callPort(PortIds.Diagnostics.portId, PortIds.Diagnostics.panicked, panicked(report(2)));
    expect(second.status).toBe(PortStatus.Ok);
    expect(seen).toEqual(["panic 1", "panic 2"]);
    expect(errors).toHaveLength(1);
    expect(errors[0]).toBeInstanceOf(UndraUnhandledError);
    expect(errors[0]?.operation).toBe("onPanic");
    expect(errors[0]?.message).toContain("the reporter is down");
    expect(log.records.some((r) => r.level === 4 && r.message.includes("the reporter is down"))).toBe(true);
  });

  it("without onPanic the report is logged at error level, in one line: operation, message, location", async () => {
    const { fake, log } = await native();
    await fake.callPort(PortIds.Diagnostics.portId, PortIds.Diagnostics.panicked, panicked({ ...REPORT, operation: "Todos.add", message: "boom", location: "src/todos.rs:12:5" }));
    const lines = log.records.filter((r) => r.target === "undra::panic");
    expect(lines).toEqual([{ level: 4, target: "undra::panic", message: "Todos.add: boom (src/todos.rs:12:5)" }]);
  });

  it("arguments that do not decode never reach onPanic: the failure is reported and the core is told 'unavailable'", async () => {
    const seen: UndraPanicReport[] = [];
    const errors: UndraUnhandledError[] = [];
    const { fake } = await native({ onPanic: (r) => seen.push(r), onError: (e) => errors.push(e) });
    const truncated = panicked(REPORT).subarray(0, 9);
    const reply = await fake.callPort(PortIds.Diagnostics.portId, PortIds.Diagnostics.panicked, truncated);
    expect(reply.status).toBe(PortStatus.Unavailable);
    expect(seen).toEqual([]);
    expect(errors).toHaveLength(1);
    expect(errors[0]?.operation).toContain("Diagnostics");
  });

  it("a wasm core never calls it (it traps): the port is not registered there, so a worker never meets a synchronous port of ours", async () => {
    for (const mode of ["wasm-main", "wasm-worker"]) {
      const { fake } = await native({ mode, onPanic: () => {} });
      const reply = await fake.callPort(PortIds.Diagnostics.portId, PortIds.Diagnostics.panicked, panicked(REPORT));
      expect(reply.status, mode).toBe(PortStatus.Unavailable);
    }
  });

  it("diagnosticsPort's host decides what a missing handler and a failing one do: a log line, and a failure that is reported instead of thrown", () => {
    const lines: string[] = [];
    const failures: unknown[] = [];
    const log = { log: (level: number, target: string, message: string) => lines.push(`${level} ${target} ${message}`) };
    const unhandled = diagnosticsPort(undefined, { log });
    expect(unhandled.methods[PortIds.Diagnostics.panicked]?.(panicked({ ...REPORT, operation: "", message: "boom", location: "a.rs:1:2" }))).toEqual(new Uint8Array(0));
    expect(lines).toEqual(["4 undra::panic panic: boom (a.rs:1:2)"]);
    const failing = diagnosticsPort(
      () => {
        throw new Error("the reporter is down");
      },
      { fail: (error) => failures.push(error) },
    );
    expect(failing.methods[PortIds.Diagnostics.panicked]?.(panicked(REPORT))).toEqual(new Uint8Array(0));
    expect(failures).toHaveLength(1);
  });

  it("without a host, diagnosticsPort ignores a report nobody asked for, and lets a failing handler's error out of the method (the embedder's to contain)", () => {
    expect(diagnosticsPort(undefined).methods[PortIds.Diagnostics.panicked]?.(panicked(REPORT))).toEqual(new Uint8Array(0));
    const failing = diagnosticsPort(() => {
      throw new Error("the reporter is down");
    });
    expect(() => failing.methods[PortIds.Diagnostics.panicked]?.(panicked(REPORT))).toThrow("the reporter is down");
  });

  it("diagnosticsPort is the port of an embedder's own transport, too: a sync PortImpl over the same decoder", () => {
    const seen: UndraPanicReport[] = [];
    const port = diagnosticsPort((r) => seen.push(r));
    expect(port.sync).toBe(true);
    expect(port.name).toBe("Diagnostics");
    const answer = port.methods[PortIds.Diagnostics.panicked]?.(panicked(REPORT));
    expect(answer).toEqual(new Uint8Array(0));
    expect(seen).toEqual([REPORT]);
  });
});
