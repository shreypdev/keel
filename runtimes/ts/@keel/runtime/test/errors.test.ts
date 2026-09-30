import { describe, expect, it } from "vitest";
import {
  KeelError,
  KeelModeError,
  KeelPortError,
  KeelReplyError,
  KeelSchemaMismatchError,
  KeelTransportError,
} from "../src/errors.js";
import { KeelWriter, ReplyStatus } from "../src/wire/index.js";

describe("errors", () => {
  it("KeelError carries a kind, a message and a cause, and is an Error", () => {
    const cause = new Error("root");
    const error = new KeelError("custom", "went wrong", { cause });
    expect(error).toBeInstanceOf(Error);
    expect(error.kind).toBe("custom");
    expect(error.message).toBe("went wrong");
    expect(error.cause).toBe(cause);
    expect(error.name).toBe("KeelError");
    expect(new KeelError("bare").message).toBe("");
  });

  it("every runtime error is a KeelError with its own name and kind", () => {
    const errors = [
      new KeelReplyError(ReplyStatus.Cancelled, new Uint8Array(0)),
      new KeelModeError("callSync", "remote"),
      new KeelSchemaMismatchError(1n, 2n),
      new KeelPortError(new Uint8Array([1])),
      new KeelTransportError("closed", "gone"),
    ];
    expect(errors.map((e) => [e.name, e.kind])).toEqual([
      ["KeelReplyError", "reply"],
      ["KeelModeError", "mode"],
      ["KeelSchemaMismatchError", "schemaMismatch"],
      ["KeelPortError", "port"],
      ["KeelTransportError", "transport"],
    ]);
    for (const e of errors) expect(e).toBeInstanceOf(KeelError);
  });

  it("KeelReplyError describes each status", () => {
    const messages = new Map<ReplyStatus, string>([
      [ReplyStatus.Error, "the call failed with a typed error"],
      [ReplyStatus.Cancelled, "the call was cancelled"],
      [ReplyStatus.StreamOpened, "unexpected reply status 4"],
    ]);
    for (const [status, message] of messages) {
      expect(new KeelReplyError(status, new Uint8Array(0)).message).toBe(message);
    }
    const w = new KeelWriter();
    w.writeStr("boom");
    w.writeStr("bt");
    const panic = new KeelReplyError(ReplyStatus.Panic, w.finish());
    expect([panic.status, panic.reason, panic.backtrace]).toEqual([ReplyStatus.Panic, "boom", "bt"]);
    const bad = new KeelWriter();
    bad.writeStr("unknown method 0x1");
    const request = new KeelReplyError(ReplyStatus.BadRequest, bad.finish());
    expect(request.reason).toBe("unknown method 0x1");
    expect(request.backtrace).toBeUndefined();
    expect(new KeelReplyError(ReplyStatus.Error, new Uint8Array([9])).reason).toBeUndefined();
  });

  it("KeelSchemaMismatchError shows both hashes as 16 hex digits", () => {
    const error = new KeelSchemaMismatchError(0xabcn, 0xffff_ffff_ffff_ffffn);
    expect(error.message).toContain("0x0000000000000abc");
    expect(error.message).toContain("0xffffffffffffffff");
    expect([error.expected, error.got]).toEqual([0xabcn, 0xffff_ffff_ffff_ffffn]);
  });

  it("KeelModeError names the operation and the mode", () => {
    const error = new KeelModeError("callSync", "wasm-worker");
    expect(error.message).toBe("callSync is not available in mode 'wasm-worker'");
    expect([error.operation, error.mode]).toEqual(["callSync", "wasm-worker"]);
  });

  it("KeelPortError keeps the encoded body", () => {
    const body = new Uint8Array([1, 2, 3]);
    expect(new KeelPortError(body).body).toBe(body);
  });

  it("KeelTransportError exposes its reason", () => {
    const error = new KeelTransportError("trap", "unreachable executed", { cause: "x" });
    expect(error.reason).toBe("trap");
    expect(error.cause).toBe("x");
  });
});
