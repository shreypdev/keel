import { describe, expect, it } from "vitest";
import {
  UndraError,
  UndraModeError,
  UndraPortError,
  UndraReplyError,
  UndraRestoreError,
  UndraSchemaMismatchError,
  UndraTransportError,
} from "../src/errors.js";
import { UndraWriter, ReplyStatus } from "../src/wire/index.js";

describe("errors", () => {
  it("UndraError carries a kind, a message and a cause, and is an Error", () => {
    const cause = new Error("root");
    const error = new UndraError("custom", "went wrong", { cause });
    expect(error).toBeInstanceOf(Error);
    expect(error.kind).toBe("custom");
    expect(error.message).toBe("went wrong");
    expect(error.cause).toBe(cause);
    expect(error.name).toBe("UndraError");
    expect(new UndraError("bare").message).toBe("");
  });

  it("every runtime error is an UndraError with its own name and kind", () => {
    const errors = [
      new UndraReplyError(ReplyStatus.Cancelled, new Uint8Array(0)),
      new UndraModeError("callSync", "remote"),
      new UndraSchemaMismatchError(1n, 2n),
      new UndraPortError(new Uint8Array([1])),
      new UndraTransportError("closed", "gone"),
    ];
    expect(errors.map((e) => [e.name, e.kind])).toEqual([
      ["UndraReplyError", "reply"],
      ["UndraModeError", "mode"],
      ["UndraSchemaMismatchError", "schemaMismatch"],
      ["UndraPortError", "port"],
      ["UndraTransportError", "transport"],
    ]);
    for (const e of errors) expect(e).toBeInstanceOf(UndraError);
  });

  it("UndraReplyError describes each status", () => {
    const messages = new Map<ReplyStatus, string>([
      [ReplyStatus.Error, "the call failed with a typed error"],
      [ReplyStatus.Cancelled, "the call was cancelled"],
      [ReplyStatus.StreamOpened, "unexpected reply status 4"],
    ]);
    for (const [status, message] of messages) {
      expect(new UndraReplyError(status, new Uint8Array(0)).message).toBe(message);
    }
    const w = new UndraWriter();
    w.writeStr("boom");
    w.writeStr("bt");
    const panic = new UndraReplyError(ReplyStatus.Panic, w.finish());
    expect([panic.status, panic.reason, panic.backtrace]).toEqual([ReplyStatus.Panic, "boom", "bt"]);
    const bad = new UndraWriter();
    bad.writeStr("unknown method 0x1");
    const request = new UndraReplyError(ReplyStatus.BadRequest, bad.finish());
    expect(request.reason).toBe("unknown method 0x1");
    expect(request.backtrace).toBeUndefined();
    expect(new UndraReplyError(ReplyStatus.Error, new Uint8Array([9])).reason).toBeUndefined();
  });

  it("UndraSchemaMismatchError shows both hashes as 16 hex digits", () => {
    const error = new UndraSchemaMismatchError(0xabcn, 0xffff_ffff_ffff_ffffn);
    expect(error.message).toContain("0x0000000000000abc");
    expect(error.message).toContain("0xffffffffffffffff");
    expect([error.expected, error.got]).toEqual([0xabcn, 0xffff_ffff_ffff_ffffn]);
  });

  it("UndraModeError names the operation and the mode", () => {
    const error = new UndraModeError("callSync", "wasm-worker");
    expect(error.message).toBe("callSync is not available in mode 'wasm-worker'");
    expect([error.operation, error.mode]).toEqual(["callSync", "wasm-worker"]);
  });

  it("UndraPortError keeps the encoded body", () => {
    const body = new Uint8Array([1, 2, 3]);
    expect(new UndraPortError(body).body).toBe(body);
  });

  it("UndraTransportError exposes its reason", () => {
    const error = new UndraTransportError("trap", "unreachable executed", { cause: "x" });
    expect(error.reason).toBe("trap");
    expect(error.cause).toBe("x");
  });

  it("UndraRestoreError names its codes, 7 INCOMPATIBLE included (ADR-037)", () => {
    expect([UndraRestoreError.PANICKED, UndraRestoreError.BAD_SNAPSHOT, UndraRestoreError.UNAVAILABLE, UndraRestoreError.INCOMPATIBLE]).toEqual([2, 5, 6, 7]);
    const incompatible = new UndraRestoreError(7);
    expect(incompatible.code).toBe(UndraRestoreError.INCOMPATIBLE);
    expect(incompatible.message).toContain("code 7)");
  });
});
