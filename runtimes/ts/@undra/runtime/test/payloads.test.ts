import { describe, expect, it } from "vitest";
import { codecs, decodeValue, encodeValue } from "../src/wire/codec.js";
import { WireError } from "../src/wire/errors.js";
import {
  ALL_SIGNALS,
  CallTarget,
  ChangeOp,
  PortStatus,
  ReplyStatus,
  StreamFlag,
  decodeCall,
  decodeCancel,
  decodeChangeSet,
  decodeEvent,
  decodeHello,
  decodeLog,
  decodeObserve,
  decodePortCall,
  decodePortReply,
  decodeRelease,
  decodeReply,
  decodeSnapshot,
  decodeStreamCredit,
  decodeStreamFailure,
  decodeStreamItem,
  decodeTimerFired,
  encodeCall,
  encodeCancel,
  encodeChangeSet,
  encodeEvent,
  encodeHello,
  encodeLog,
  encodeObserve,
  encodePortCall,
  encodePortReply,
  encodeRelease,
  encodeReply,
  encodeSnapshot,
  encodeStreamCredit,
  encodeStreamFailure,
  encodeStreamItem,
  encodeTimerFired,
  iterateChangeSet,
  readChangeSetHeader,
  streamFailureReplyBody,
  type CallPayload,
  type ChangeEntry,
  type ReplyPayload,
  type StreamFailure,
  type StreamItemPayload,
} from "../src/wire/payloads.js";
import { catchWireError, embedded, expectWireError, fromHex, toHex } from "./helpers.js";

const i32 = (n: number): Uint8Array => encodeValue(codecs.i32, n);
const HANDLE = 4294967297n;

describe("Call", () => {
  it("encodes an object method call (contract vector)", () => {
    const call: CallPayload = {
      target: CallTarget.ObjectMethod,
      handle: HANDLE,
      methodId: 2353348832,
      callId: 9,
      args: new Uint8Array([...i32(2), ...i32(3)]),
    };
    expect(toHex(encodeCall(call))).toBe("010100000001000000e040458c090000000200000003000000");
  });

  it("decodes the contract vector", () => {
    const call = decodeCall(fromHex("010100000001000000e040458c090000000200000003000000"));
    expect(call).toMatchObject({
      target: CallTarget.ObjectMethod,
      handle: HANDLE,
      methodId: 2353348832,
      callId: 9,
    });
    if (call.target !== CallTarget.ObjectMethod) throw new Error("wrong target");
    expect(toHex(call.args)).toBe("0200000003000000");
  });

  it("encodes a free function call with a zero handle", () => {
    const bytes = encodeCall({ target: CallTarget.FreeFunction, methodId: 0x11223344, callId: 5, args: fromHex("aa") });
    expect(toHex(bytes)).toBe("00" + "0000000000000000" + "44332211" + "05000000" + "aa");
  });

  it("encodes a constructor call without a handle field", () => {
    const bytes = encodeCall({
      target: CallTarget.Constructor,
      typeId: 0x01020304,
      methodId: 0x0a0b0c0d,
      callId: 6,
      args: fromHex("bbcc"),
    });
    expect(toHex(bytes)).toBe("02" + "04030201" + "0d0c0b0a" + "06000000" + "bbcc");
  });

  it("encodes a lazy-list page call", () => {
    const bytes = encodeCall({ target: CallTarget.LazyListPage, handle: HANDLE, offset: 20, limit: 10, callId: 7 });
    expect(toHex(bytes)).toBe("03" + "0100000001000000" + "14000000" + "0a000000" + "07000000");
  });

  const calls: [string, CallPayload][] = [
    ["free function", { target: CallTarget.FreeFunction, methodId: 1, callId: 2, args: new Uint8Array(0) }],
    ["free function with args", { target: CallTarget.FreeFunction, methodId: 0xffffffff, callId: 0xffffffff, args: fromHex("0102") }],
    ["method", { target: CallTarget.ObjectMethod, handle: 2n ** 64n - 1n, methodId: 3, callId: 4, args: fromHex("ff") }],
    ["method without args", { target: CallTarget.ObjectMethod, handle: HANDLE, methodId: 3, callId: 4, args: new Uint8Array(0) }],
    ["constructor", { target: CallTarget.Constructor, typeId: 5, methodId: 6, callId: 7, args: fromHex("0a0b") }],
    ["lazy page", { target: CallTarget.LazyListPage, handle: HANDLE, offset: 0xffffffff, limit: 0, callId: 1 }],
  ];
  it.each(calls)("round-trips a %s call", (_name, call) => {
    expect(decodeCall(encodeCall(call))).toEqual(call);
    expect(decodeCall(embedded(encodeCall(call)))).toEqual(call);
  });

  it("args of a decoded call is a borrowed view", () => {
    const bytes = encodeCall({ target: CallTarget.FreeFunction, methodId: 1, callId: 2, args: fromHex("010203") });
    const call = decodeCall(bytes);
    if (call.target !== CallTarget.FreeFunction) throw new Error("wrong target");
    expect(call.args.buffer).toBe(bytes.buffer);
  });

  it("ignores a non-zero handle on a free function call", () => {
    const bytes = fromHex("00" + "0100000001000000" + "44332211" + "05000000" + "aa");
    expect(decodeCall(bytes)).toMatchObject({ target: CallTarget.FreeFunction, methodId: 0x11223344, callId: 5 });
  });

  it("rejects an unknown target", () => {
    for (const t of [4, 5, 255]) {
      expectWireError(() => decodeCall(fromHex(`${t.toString(16).padStart(2, "0")}00000000`)), "invalid_tag", {
        tag: t,
        at: 0,
        ty: "CallTarget",
      });
    }
  });

  it("rejects trailing bytes after a lazy page call", () => {
    const bytes = encodeCall({ target: CallTarget.LazyListPage, handle: HANDLE, offset: 0, limit: 1, callId: 1 });
    const longer = new Uint8Array(bytes.length + 1);
    longer.set(bytes);
    expectWireError(() => decodeCall(longer), "trailing_bytes", { count: 1 });
  });

  it("rejects every truncation of a fixed-size call", () => {
    const bytes = encodeCall({ target: CallTarget.LazyListPage, handle: HANDLE, offset: 0, limit: 1, callId: 1 });
    for (let n = 0; n < bytes.length; n++) {
      expectWireError(() => decodeCall(bytes.subarray(0, n)), "unexpected_eof");
    }
  });

  it("rejects a truncated header on the tail-carrying targets", () => {
    for (const call of [
      { target: CallTarget.FreeFunction, methodId: 1, callId: 2, args: new Uint8Array(0) },
      { target: CallTarget.ObjectMethod, handle: HANDLE, methodId: 1, callId: 2, args: new Uint8Array(0) },
      { target: CallTarget.Constructor, typeId: 1, methodId: 1, callId: 2, args: new Uint8Array(0) },
    ] as CallPayload[]) {
      const bytes = encodeCall(call);
      for (let n = 0; n < bytes.length; n++) {
        expectWireError(() => decodeCall(bytes.subarray(0, n)), "unexpected_eof");
      }
      expect(decodeCall(bytes)).toEqual(call);
    }
  });
});

describe("Reply", () => {
  it("encodes and decodes the contract vector", () => {
    const reply: ReplyPayload = { callId: 9, status: ReplyStatus.Ok, body: i32(5) };
    expect(toHex(encodeReply(reply))).toBe("090000000005000000");
    expect(decodeReply(fromHex("090000000005000000"))).toEqual(reply);
  });

  it("has the six status codes of SPEC 3.4", () => {
    expect(ReplyStatus.Ok).toBe(0);
    expect(ReplyStatus.Error).toBe(1);
    expect(ReplyStatus.Panic).toBe(2);
    expect(ReplyStatus.Cancelled).toBe(3);
    expect(ReplyStatus.StreamOpened).toBe(4);
    expect(ReplyStatus.BadRequest).toBe(5);
  });

  const replies: [string, ReplyPayload, string][] = [
    ["ok with unit body", { callId: 1, status: ReplyStatus.Ok, body: new Uint8Array(0) }, "01000000" + "00"],
    ["ok", { callId: 1, status: ReplyStatus.Ok, body: fromHex("0a0b") }, "01000000" + "00" + "0a0b"],
    ["error", { callId: 2, status: ReplyStatus.Error, body: fromHex("0300") }, "02000000" + "01" + "0300"],
    [
      "panic",
      { callId: 3, status: ReplyStatus.Panic, message: "boom", backtrace: "at x" },
      "03000000" + "02" + "04000000626f6f6d" + "0400000061742078",
    ],
    ["cancelled", { callId: 4, status: ReplyStatus.Cancelled }, "04000000" + "03"],
    ["stream opened", { callId: 5, status: ReplyStatus.StreamOpened }, "05000000" + "04"],
    [
      "bad request",
      { callId: 6, status: ReplyStatus.BadRequest, reason: "unknown method" },
      "06000000" + "05" + "0e000000" + toHex(new TextEncoder().encode("unknown method")),
    ],
  ];
  it.each(replies)("round-trips %s", (_name, reply, hex) => {
    expect(toHex(encodeReply(reply))).toBe(hex);
    expect(decodeReply(fromHex(hex))).toEqual(reply);
    expect(decodeReply(embedded(fromHex(hex)))).toEqual(reply);
  });

  it("carries unicode and empty strings in panic and bad-request replies", () => {
    for (const reply of [
      { callId: 1, status: ReplyStatus.Panic, message: "\u{1f4a5} café", backtrace: "" },
      { callId: 1, status: ReplyStatus.Panic, message: "", backtrace: "日本".repeat(500) },
      { callId: 1, status: ReplyStatus.BadRequest, reason: "" },
    ] as ReplyPayload[]) {
      expect(decodeReply(encodeReply(reply))).toEqual(reply);
    }
  });

  it("rejects an unknown status", () => {
    expectWireError(() => decodeReply(fromHex("0100000006")), "invalid_tag", { tag: 6, at: 4, ty: "ReplyStatus" });
    expectWireError(() => decodeReply(fromHex("01000000ff")), "invalid_tag", { tag: 255 });
  });

  it("rejects a body on statuses that have none", () => {
    expectWireError(() => decodeReply(fromHex("01000000 03 00")), "trailing_bytes", { count: 1 });
    expectWireError(() => decodeReply(fromHex("01000000 04 00")), "trailing_bytes", { count: 1 });
  });

  it("rejects trailing bytes after a panic or bad request", () => {
    const panic = encodeReply({ callId: 1, status: ReplyStatus.Panic, message: "m", backtrace: "b" });
    const longer = new Uint8Array(panic.length + 1);
    longer.set(panic);
    expectWireError(() => decodeReply(longer), "trailing_bytes", { count: 1 });
  });

  it("rejects truncated and invalid strings in a panic", () => {
    expectWireError(() => decodeReply(fromHex("01000000 02 04000000 62")), "length_too_large");
    expectWireError(() => decodeReply(fromHex("01000000 02 01000000 ff 00000000")), "invalid_utf8");
    expectWireError(() => decodeReply(fromHex("01000000 02 00000000")), "unexpected_eof");
  });

  it("rejects a truncated header", () => {
    for (let n = 0; n < 5; n++) {
      expectWireError(() => decodeReply(fromHex("0100000000").subarray(0, n)), "unexpected_eof");
    }
  });
});

describe("ChangeSet", () => {
  const vectorHex = "2a00000000000000" + "01000000" + "0100000001000000" + "00000000" + "00" + "0c000000" + "020000000100000002000000";
  const vectorValue = encodeValue(codecs.vec(codecs.i32), [1, 2]);

  it("encodes and decodes the contract vector", () => {
    const entries: ChangeEntry[] = [{ handle: HANDLE, signalId: 0, op: ChangeOp.FullValue, value: vectorValue }];
    expect(toHex(encodeChangeSet({ txnId: 42n, entries }))).toBe(vectorHex);
    const decoded = decodeChangeSet(fromHex(vectorHex));
    expect(decoded.txnId).toBe(42n);
    expect(decoded.entries).toEqual(entries);
  });

  it("has the three ops of SPEC 3.5", () => {
    expect(ChangeOp.FullValue).toBe(0);
    expect(ChangeOp.KeyedPatch).toBe(1);
    expect(ChangeOp.LazyInvalidated).toBe(2);
  });

  const entries: ChangeEntry[] = [
    { handle: HANDLE, signalId: 0, op: ChangeOp.FullValue, value: fromHex("01020304") },
    { handle: 2n ** 64n - 1n, signalId: ALL_SIGNALS, op: ChangeOp.KeyedPatch, value: fromHex("00000000") },
    { handle: 5n, signalId: 7, op: ChangeOp.LazyInvalidated, value: new Uint8Array(0) },
    { handle: HANDLE, signalId: 1, op: ChangeOp.FullValue, value: new Uint8Array(1000).fill(9) },
  ];

  it("round-trips several entries in order", () => {
    const bytes = encodeChangeSet({ txnId: 2n ** 64n - 1n, entries });
    const decoded = decodeChangeSet(bytes);
    expect(decoded.txnId).toBe(2n ** 64n - 1n);
    expect(decoded.entries).toEqual(entries);
    expect(decodeChangeSet(embedded(bytes)).entries).toEqual(entries);
  });

  it("round-trips an empty change-set", () => {
    const bytes = encodeChangeSet({ txnId: 0n, entries: [] });
    expect(toHex(bytes)).toBe("0000000000000000" + "00000000");
    expect(decodeChangeSet(bytes)).toEqual({ txnId: 0n, entries: [] });
    expect([...iterateChangeSet(bytes)]).toEqual([]);
  });

  it("reads the header without touching the entries", () => {
    const bytes = encodeChangeSet({ txnId: 42n, entries });
    expect(readChangeSetHeader(bytes)).toEqual({ txnId: 42n, count: 4 });
    expectWireError(() => readChangeSetHeader(fromHex("2a00000000000000 ff000000")), "length_too_large");
    expectWireError(() => readChangeSetHeader(new Uint8Array(11)), "unexpected_eof");
  });

  describe("iterateChangeSet", () => {
    it("yields the same entries as decodeChangeSet", () => {
      const bytes = encodeChangeSet({ txnId: 1n, entries });
      expect([...iterateChangeSet(bytes)]).toEqual(entries);
    });

    it("yields values as views into the input without copying", () => {
      const bytes = embedded(encodeChangeSet({ txnId: 1n, entries }));
      for (const entry of iterateChangeSet(bytes)) {
        expect(entry.value.buffer).toBe(bytes.buffer);
        expect(entry.value.byteOffset).toBeGreaterThanOrEqual(bytes.byteOffset);
        expect(entry.value.byteOffset + entry.value.byteLength).toBeLessThanOrEqual(bytes.byteOffset + bytes.byteLength);
      }
    });

    it("is lazy: nothing is read until the first next()", () => {
      const it = iterateChangeSet(fromHex("00"));
      // Creating the generator does not throw although the input is truncated garbage.
      expect(typeof it.next).toBe("function");
      expectWireError(() => it.next(), "unexpected_eof");
    });

    it("decodes one entry at a time, so a bad later entry surfaces only when reached", () => {
      const good = encodeChangeSet({ txnId: 1n, entries: entries.slice(0, 2) });
      // Corrupt the op byte of the second entry.
      const bytes = good.slice();
      const secondOpAt = 12 + (8 + 4 + 1 + 4 + 4) + 8 + 4;
      bytes[secondOpAt] = 9;
      const it = iterateChangeSet(bytes);
      expect(it.next().value).toEqual(entries[0]);
      expectWireError(() => it.next(), "invalid_tag", { tag: 9, ty: "ChangeOp", at: secondOpAt });
    });

    it("lets a consumer stop early without validating the rest", () => {
      const bytes = encodeChangeSet({ txnId: 1n, entries });
      const truncated = bytes.subarray(0, bytes.length - 500);
      const it = iterateChangeSet(truncated);
      expect(it.next().value).toEqual(entries[0]);
      it.return();
    });

    it("reports trailing bytes once the last entry has been yielded", () => {
      const bytes = encodeChangeSet({ txnId: 1n, entries: entries.slice(0, 1) });
      const longer = new Uint8Array(bytes.length + 3);
      longer.set(bytes);
      const it = iterateChangeSet(longer);
      expect(it.next().done).toBe(false);
      expectWireError(() => it.next(), "trailing_bytes", { count: 3 });
    });
  });

  describe("malformed", () => {
    it("rejects a count that cannot fit in the input", () => {
      expectWireError(() => decodeChangeSet(fromHex("0000000000000000 01000000")), "length_too_large", { len: 1 });
      expectWireError(() => decodeChangeSet(fromHex("0000000000000000 ffffffff")), "length_too_large");
      // 2 entries of at least 17 bytes do not fit in 20.
      expectWireError(
        () => decodeChangeSet(fromHex("0000000000000000 02000000" + "00".repeat(20))),
        "length_too_large",
      );
    });

    it("rejects an unknown op", () => {
      const bytes = encodeChangeSet({ txnId: 1n, entries: entries.slice(0, 1) });
      bytes[12 + 8 + 4] = 3;
      expectWireError(() => decodeChangeSet(bytes), "invalid_tag", { tag: 3, ty: "ChangeOp" });
    });

    it("rejects a value length beyond the input", () => {
      const bytes = encodeChangeSet({ txnId: 1n, entries: entries.slice(0, 1) });
      new DataView(bytes.buffer, bytes.byteOffset).setUint32(12 + 8 + 4 + 1, 1000, true);
      expectWireError(() => decodeChangeSet(bytes), "length_too_large", { len: 1000 });
    });

    it("rejects trailing bytes", () => {
      const bytes = encodeChangeSet({ txnId: 1n, entries });
      const longer = new Uint8Array(bytes.length + 1);
      longer.set(bytes);
      expectWireError(() => decodeChangeSet(longer), "trailing_bytes", { count: 1 });
    });

    it("rejects every strict prefix", () => {
      const bytes = encodeChangeSet({ txnId: 1n, entries: entries.slice(0, 3) });
      for (let n = 0; n < bytes.length; n++) {
        const err = (() => {
          try {
            decodeChangeSet(bytes.subarray(0, n));
          } catch (e) {
            return e;
          }
          return undefined;
        })();
        expect(err, `prefix ${n}`).toBeInstanceOf(WireError);
      }
    });

    it("tolerates a non-empty value on a lazy-invalidated entry (the host ignores it)", () => {
      const bytes = encodeChangeSet({
        txnId: 1n,
        entries: [{ handle: 1n, signalId: 0, op: ChangeOp.LazyInvalidated, value: fromHex("aa") }],
      });
      expect(decodeChangeSet(bytes).entries[0]?.value.length).toBe(1);
    });
  });
});

describe("PortCall and PortReply", () => {
  it("encodes a port call as port, method, port call id, args", () => {
    const bytes = encodePortCall({ portId: 1, methodId: 2, portCallId: 3, args: fromHex("aabb") });
    expect(toHex(bytes)).toBe("01000000" + "02000000" + "03000000" + "aabb");
    expect(decodePortCall(bytes)).toEqual({ portId: 1, methodId: 2, portCallId: 3, args: fromHex("aabb") });
  });

  it("round-trips extremes and empty args", () => {
    const call = { portId: 0xffffffff, methodId: 0, portCallId: 0xffffffff, args: new Uint8Array(0) };
    expect(decodePortCall(encodePortCall(call))).toEqual(call);
    expect(decodePortCall(embedded(encodePortCall(call)))).toEqual(call);
  });

  it("rejects a truncated port call", () => {
    for (let n = 0; n < 12; n++) {
      expectWireError(() => decodePortCall(new Uint8Array(n)), "unexpected_eof");
    }
  });

  it("has the three port statuses of SPEC 3.6", () => {
    expect(PortStatus.Ok).toBe(0);
    expect(PortStatus.Error).toBe(1);
    expect(PortStatus.Unavailable).toBe(2);
  });

  it("round-trips a port reply of every status", () => {
    for (const [status, body] of [
      [PortStatus.Ok, fromHex("0102")],
      [PortStatus.Error, fromHex("ff")],
      [PortStatus.Unavailable, new Uint8Array(0)],
    ] as const) {
      const reply = { portCallId: 77, status, body };
      const bytes = encodePortReply(reply);
      expect(toHex(bytes)).toBe("4d000000" + status.toString(16).padStart(2, "0") + toHex(body));
      expect(decodePortReply(bytes)).toEqual(reply);
    }
  });

  it("rejects an unknown port status and a truncated reply", () => {
    expectWireError(() => decodePortReply(fromHex("0100000003")), "invalid_tag", { tag: 3, at: 4, ty: "PortStatus" });
    for (let n = 0; n < 5; n++) {
      expectWireError(() => decodePortReply(new Uint8Array(n)), "unexpected_eof");
    }
  });
});

describe("Cancel, StreamCredit and StreamItem", () => {
  it("round-trips Cancel", () => {
    expect(toHex(encodeCancel({ callId: 9 }))).toBe("09000000");
    expect(decodeCancel(encodeCancel({ callId: 0xffffffff }))).toEqual({ callId: 0xffffffff });
    expectWireError(() => decodeCancel(fromHex("0900000000")), "trailing_bytes", { count: 1 });
    expectWireError(() => decodeCancel(fromHex("090000")), "unexpected_eof");
  });

  it("round-trips StreamCredit", () => {
    expect(toHex(encodeStreamCredit({ callId: 9, credit: 16 }))).toBe("09000000" + "10000000");
    expect(decodeStreamCredit(encodeStreamCredit({ callId: 1, credit: 0xffffffff }))).toEqual({
      callId: 1,
      credit: 0xffffffff,
    });
    expectWireError(() => decodeStreamCredit(fromHex("0900000010")), "unexpected_eof");
    expectWireError(() => decodeStreamCredit(fromHex("090000001000000000")), "trailing_bytes");
  });

  it("has the four stream flags of SPEC 3.7 (ADR-036)", () => {
    expect(StreamFlag.Item).toBe(0);
    expect(StreamFlag.End).toBe(1);
    expect(StreamFlag.Error).toBe(2);
    expect(StreamFlag.Failed).toBe(3);
  });

  const boom: StreamFailure = { status: ReplyStatus.Panic, message: "boom", detail: "at x" };
  const items: [string, StreamItemPayload, string][] = [
    ["item", { callId: 4, flag: StreamFlag.Item, body: i32(7) }, "04000000" + "00" + "07000000"],
    ["end", { callId: 4, flag: StreamFlag.End }, "04000000" + "01"],
    ["error", { callId: 4, flag: StreamFlag.Error, body: fromHex("0300") }, "04000000" + "02" + "0300"],
    ["empty item", { callId: 4, flag: StreamFlag.Item, body: new Uint8Array(0) }, "04000000" + "00"],
    [
      "failure",
      { callId: 4, flag: StreamFlag.Failed, failure: boom },
      "04000000" + "03" + "02" + "04000000626f6f6d" + "0400000061742078",
    ],
    [
      "cancellation by the core",
      { callId: 4, flag: StreamFlag.Failed, failure: { status: ReplyStatus.Cancelled, message: "", detail: "" } },
      "04000000" + "03" + "03" + "00000000" + "00000000",
    ],
  ];
  it.each(items)("round-trips a stream %s", (_name, item, hex) => {
    expect(toHex(encodeStreamItem(item))).toBe(hex);
    expect(decodeStreamItem(fromHex(hex))).toEqual(item);
    expect(decodeStreamItem(embedded(fromHex(hex)))).toEqual(item);
  });

  it("rejects an unknown flag, a body on End, and a truncated item", () => {
    expectWireError(() => decodeStreamItem(fromHex("0400000004")), "invalid_tag", { tag: 4, at: 4, ty: "StreamFlag" });
    expectWireError(() => decodeStreamItem(fromHex("04000000ff")), "invalid_tag", { tag: 255, at: 4, ty: "StreamFlag" });
    expectWireError(() => decodeStreamItem(fromHex("040000000100")), "trailing_bytes", { count: 1 });
    for (let n = 0; n < 5; n++) {
      expectWireError(() => decodeStreamItem(new Uint8Array(n)), "unexpected_eof");
    }
  });

  it("rejects a flag-3 item whose failure is malformed, at the item's offsets", () => {
    const item = encodeStreamItem({ callId: 4, flag: StreamFlag.Failed, failure: boom });
    const badStatus = item.slice();
    badStatus[5] = ReplyStatus.Error;
    expectWireError(() => decodeStreamItem(badStatus), "invalid_tag", { tag: 1, at: 5, ty: "StreamFailure.status" });
    expectWireError(() => decodeStreamItem(new Uint8Array([...item, 0])), "trailing_bytes", { count: 1 });
    // A flag-3 item has no empty body: the status and both strings are required.
    for (let n = 5; n < item.length; n++) {
      const err = catchWireError(() => decodeStreamItem(item.subarray(0, n)));
      expect(["unexpected_eof", "length_too_large"], `prefix ${n}`).toContain(err.code);
    }
  });
});

describe("StreamFailure (the body of a flag-3 stream item, ADR-036)", () => {
  it("is status u8, message String, detail String", () => {
    expect(toHex(encodeStreamFailure({ status: ReplyStatus.Panic, message: "boom", detail: "at core.rs:1" }))).toBe(
      "02" + "04000000626f6f6d" + "0c000000" + toHex(new TextEncoder().encode("at core.rs:1")),
    );
    expect(toHex(encodeStreamFailure({ status: ReplyStatus.Cancelled, message: "", detail: "" }))).toBe(
      "03" + "00000000" + "00000000",
    );
  });

  const failures: [string, StreamFailure][] = [
    ["panic", { status: ReplyStatus.Panic, message: "index out of bounds", detail: "at core::foo\nat core::bar" }],
    ["cancellation", { status: ReplyStatus.Cancelled, message: "the runtime shut down", detail: "" }],
    ["refusal", { status: ReplyStatus.BadRequest, message: "stale handle", detail: "" }],
    ["unicode and empty strings", { status: ReplyStatus.Panic, message: "\u{1f4a5} café", detail: "日本".repeat(300) }],
  ];
  it.each(failures)("round-trips a %s", (_name, failure) => {
    const bytes = encodeStreamFailure(failure);
    expect(decodeStreamFailure(bytes)).toEqual(failure);
    expect(decodeStreamFailure(embedded(bytes))).toEqual(failure);
  });

  it("accepts only the failure statuses 2, 3 and 5", () => {
    const body = encodeStreamFailure({ status: ReplyStatus.Cancelled, message: "m", detail: "" });
    for (const status of [ReplyStatus.Ok, ReplyStatus.Error, ReplyStatus.StreamOpened, 6, 255]) {
      const bad = body.slice();
      bad[0] = status;
      expectWireError(() => decodeStreamFailure(bad), "invalid_tag", { tag: status, at: 0, ty: "StreamFailure.status" });
    }
    for (const status of [ReplyStatus.Panic, ReplyStatus.Cancelled, ReplyStatus.BadRequest]) {
      const good = body.slice();
      good[0] = status;
      expect(decodeStreamFailure(good).status).toBe(status);
    }
  });

  it("rejects every truncation, trailing bytes and invalid UTF-8", () => {
    const body = encodeStreamFailure({ status: ReplyStatus.Panic, message: "boom", detail: "at x" });
    for (let n = 0; n < body.length; n++) {
      const err = catchWireError(() => decodeStreamFailure(body.subarray(0, n)));
      expect(["unexpected_eof", "length_too_large"], `prefix ${n}`).toContain(err.code);
    }
    expectWireError(() => decodeStreamFailure(new Uint8Array([...body, 0])), "trailing_bytes", { count: 1 });
    expectWireError(() => decodeStreamFailure(fromHex("02 01000000 ff 00000000")), "invalid_utf8", { at: 1 });
  });

  it("maps to the section 3.4 body of a failed reply with the same status", () => {
    // Panic: String message + String backtrace, i.e. the failure body without its status byte.
    const panic: StreamFailure = { status: ReplyStatus.Panic, message: "boom", detail: "at x" };
    const panicBody = streamFailureReplyBody(panic);
    expect(panicBody).toEqual(encodeStreamFailure(panic).subarray(1));
    expect(decodeReply(encodeReplyWithBody(ReplyStatus.Panic, panicBody))).toEqual({
      callId: 1,
      status: ReplyStatus.Panic,
      message: "boom",
      backtrace: "at x",
    });
    // Cancelled: empty, whatever the reason.
    const cancelled = streamFailureReplyBody({ status: ReplyStatus.Cancelled, message: "the runtime shut down", detail: "" });
    expect(cancelled).toEqual(new Uint8Array(0));
    expect(decodeReply(encodeReplyWithBody(ReplyStatus.Cancelled, cancelled))).toEqual({ callId: 1, status: ReplyStatus.Cancelled });
    // Refused: String reason.
    const refused = streamFailureReplyBody({ status: ReplyStatus.BadRequest, message: "stale handle", detail: "" });
    expect(decodeReply(encodeReplyWithBody(ReplyStatus.BadRequest, refused))).toEqual({
      callId: 1,
      status: ReplyStatus.BadRequest,
      reason: "stale handle",
    });
  });
});

/** A reply payload for call 1 with `status` and a raw `body`, to check a body against `decodeReply`. */
function encodeReplyWithBody(status: ReplyStatus, body: Uint8Array): Uint8Array {
  return new Uint8Array([1, 0, 0, 0, status, ...body]);
}

describe("Observe, Release and Event", () => {
  it("round-trips Observe", () => {
    const on = { handle: HANDLE, signalId: 3, on: true };
    expect(toHex(encodeObserve(on))).toBe("0100000001000000" + "03000000" + "01");
    expect(decodeObserve(encodeObserve(on))).toEqual(on);
    const off = { handle: 5n, signalId: ALL_SIGNALS, on: false };
    expect(toHex(encodeObserve(off))).toBe("0500000000000000" + "ffffffff" + "00");
    expect(decodeObserve(encodeObserve(off))).toEqual(off);
  });

  it("ALL_SIGNALS is u32::MAX", () => {
    expect(ALL_SIGNALS).toBe(0xffffffff);
  });

  it("rejects an on flag other than 0 or 1, truncation and trailing bytes", () => {
    expectWireError(() => decodeObserve(fromHex("0100000001000000 03000000 02")), "invalid_tag", {
      tag: 2,
      at: 12,
      ty: "bool",
    });
    expectWireError(() => decodeObserve(fromHex("0100000001000000 03000000")), "unexpected_eof");
    expectWireError(() => decodeObserve(fromHex("0100000001000000 03000000 01 00")), "trailing_bytes");
  });

  it("round-trips Release", () => {
    expect(toHex(encodeRelease({ handle: HANDLE }))).toBe("0100000001000000");
    expect(decodeRelease(encodeRelease({ handle: 2n ** 64n - 1n }))).toEqual({ handle: 2n ** 64n - 1n });
    expectWireError(() => decodeRelease(fromHex("01000000")), "unexpected_eof");
    expectWireError(() => decodeRelease(fromHex("010000000100000000")), "trailing_bytes");
  });

  it("round-trips Event with its opaque payload", () => {
    const event = { portId: 1, methodId: 2, payload: fromHex("01") };
    expect(toHex(encodeEvent(event))).toBe("01000000" + "02000000" + "01");
    expect(decodeEvent(encodeEvent(event))).toEqual(event);
    const empty = { portId: 1, methodId: 2, payload: new Uint8Array(0) };
    expect(decodeEvent(encodeEvent(empty))).toEqual(empty);
    expectWireError(() => decodeEvent(fromHex("0100000002")), "unexpected_eof");
  });
});

describe("Hello, Log and TimerFired", () => {
  it("round-trips Hello", () => {
    const hello = { undraVersion: "0.1.0", schemaHash: 72623859790382856n, platform: "web", mode: "dev" };
    const bytes = encodeHello(hello);
    expect(toHex(bytes)).toBe(
      "05000000" + toHex(new TextEncoder().encode("0.1.0")) + "0807060504030201" + "03000000776562" + "03000000646576",
    );
    expect(decodeHello(bytes)).toEqual(hello);
    expect(decodeHello(embedded(bytes))).toEqual(hello);
  });

  it("round-trips Hello with empty and unicode strings and extreme hashes", () => {
    for (const hello of [
      { undraVersion: "", schemaHash: 0n, platform: "", mode: "" },
      { undraVersion: "1.0.0-β", schemaHash: 2n ** 64n - 1n, platform: "\u{1f30a}", mode: "prod" },
    ]) {
      expect(decodeHello(encodeHello(hello))).toEqual(hello);
    }
  });

  it("rejects truncation, invalid UTF-8 and trailing bytes in Hello", () => {
    const bytes = encodeHello({ undraVersion: "0.1.0", schemaHash: 1n, platform: "web", mode: "dev" });
    for (let n = 0; n < bytes.length; n++) {
      expect(() => decodeHello(bytes.subarray(0, n)), `prefix ${n}`).toThrow(WireError);
    }
    expectWireError(() => decodeHello(fromHex("01000000ff")), "invalid_utf8");
    const longer = new Uint8Array(bytes.length + 1);
    longer.set(bytes);
    expectWireError(() => decodeHello(longer), "trailing_bytes");
  });

  it("round-trips Log", () => {
    const log = { level: 3, target: "undra::query", message: "refetch \u{1f501}" };
    expect(decodeLog(encodeLog(log))).toEqual(log);
    expect(toHex(encodeLog({ level: 255, target: "", message: "" }))).toBe("ff" + "00000000" + "00000000");
    expectWireError(() => decodeLog(fromHex("03")), "unexpected_eof");
    expectWireError(() => decodeLog(fromHex("03 00000000 00000000 00")), "trailing_bytes");
  });

  it("round-trips TimerFired", () => {
    expect(toHex(encodeTimerFired({ timerId: 258 }))).toBe("02010000");
    expect(decodeTimerFired(encodeTimerFired({ timerId: 0xffffffff }))).toEqual({ timerId: 0xffffffff });
    expectWireError(() => decodeTimerFired(fromHex("010000")), "unexpected_eof");
    expectWireError(() => decodeTimerFired(fromHex("0100000000")), "trailing_bytes");
  });
});

describe("Snapshot (layout 2, ADR-037)", () => {
  const snapshot = {
    generationFloor: 0x01020304,
    schemaHash: 0x1122334455667788n,
    types: [
      { typeId: 0xdeadbeef, fingerprint: 0xfedcba9876543210n },
      { typeId: 1, fingerprint: 0n },
    ],
    description: '{"stores":[]}',
    stores: [
      {
        handle: HANDLE,
        typeId: 0xdeadbeef,
        signals: [
          { signalId: 0, value: fromHex("0102") },
          { signalId: 2, value: new Uint8Array(0) },
        ],
      },
      { handle: 5n, typeId: 1, signals: [] },
    ],
  };
  const empty = { generationFloor: 0, schemaHash: 0n, types: [], description: "", stores: [] };

  it("lays out the header, the type table, the description, then stores and signals per SPEC 5.9", () => {
    expect(toHex(encodeSnapshot(snapshot))).toBe(
      "02000000" + "0403020100000000" + "8877665544332211" +
        "02000000" + "efbeadde" + "1032547698badcfe" + "01000000" + "0000000000000000" +
        "0d000000" + toHex(new TextEncoder().encode('{"stores":[]}')) +
        "0100000001000000" + "efbeadde" + "02000000" +
        "00000000" + "02000000" + "0102" +
        "02000000" + "00000000" +
        "0500000000000000" + "01000000" + "00000000",
    );
  });

  it("round-trips, including empty snapshots and empty stores", () => {
    const bytes = encodeSnapshot(snapshot);
    expect(decodeSnapshot(bytes)).toEqual(snapshot);
    expect(decodeSnapshot(embedded(bytes))).toEqual(snapshot);
    expect(toHex(encodeSnapshot(empty))).toBe("00000000" + "0000000000000000" + "0000000000000000" + "00000000" + "00000000");
    const wide = { ...empty, generationFloor: 2 ** 40 - 1 };
    expect(decodeSnapshot(encodeSnapshot(wide))).toEqual(wide);
    const floor9 = { ...empty, generationFloor: 9, schemaHash: 7n, description: "{}" };
    expect(decodeSnapshot(encodeSnapshot(floor9))).toEqual(floor9);
  });

  it("returns signal values as borrowed views", () => {
    const bytes = encodeSnapshot(snapshot);
    const decoded = decodeSnapshot(bytes);
    expect(decoded.stores[0]?.signals[0]?.value.buffer).toBe(bytes.buffer);
  });

  it("rejects counts that cannot fit in the input", () => {
    expectWireError(() => decodeSnapshot(fromHex("ffffffff 0000000000000000")), "length_too_large");
    // A type count beyond the input.
    expectWireError(() => decodeSnapshot(fromHex("00000000 0000000000000000 0000000000000000 ffffffff")), "length_too_large");
    // A signal count beyond the input.
    expectWireError(
      () =>
        decodeSnapshot(
          fromHex("01000000 0000000000000000 0000000000000000 01000000 02000000 0000000000000000 00000000 0000000000000000 02000000 ffffffff"),
        ),
      "length_too_large",
    );
  });

  it("refuses a store whose type is not in the type table", () => {
    const unlisted = encodeSnapshot({ ...snapshot, types: [{ typeId: 1, fingerprint: 0n }] });
    expectWireError(() => decodeSnapshot(unlisted), "invalid_tag", { tag: 0xdeadbeef });
  });

  it("refuses a type listed twice", () => {
    const twice = encodeSnapshot({ ...snapshot, types: [...snapshot.types, { typeId: 1, fingerprint: 9n }] });
    expectWireError(() => decodeSnapshot(twice), "duplicate_key");
  });

  it("refuses a description that is not UTF-8", () => {
    const bytes = encodeSnapshot({ ...empty, description: "ab" });
    // The description's two bytes are the last two: make them an invalid UTF-8 sequence.
    bytes[bytes.length - 2] = 0xc3;
    bytes[bytes.length - 1] = 0x28;
    expectWireError(() => decodeSnapshot(bytes), "invalid_utf8");
  });

  it("refuses a snapshot in the layout before ADR-037 (and the one before the generation floor)", () => {
    // Layout 1: count, generation_floor, then the stores straight away.
    const layout1 = fromHex(
      "01000000" + "03000000" + "0100000001000000" + "efbeadde" + "01000000" + "00000000" + "04000000" + "07000000",
    );
    expect(() => decodeSnapshot(layout1)).toThrow(WireError);
    // An empty one ends where the schema hash should be.
    expectWireError(() => decodeSnapshot(fromHex("00000000 00000000")), "unexpected_eof");
    // The layout before the generation floor: `count u32` only.
    expectWireError(() => decodeSnapshot(fromHex("00000000")), "unexpected_eof");
  });

  it("rejects every strict prefix and trailing bytes", () => {
    const bytes = encodeSnapshot(snapshot);
    for (let n = 0; n < bytes.length; n++) {
      expect(() => decodeSnapshot(bytes.subarray(0, n)), `prefix ${n}`).toThrow(WireError);
    }
    const longer = new Uint8Array(bytes.length + 1);
    longer.set(bytes);
    expectWireError(() => decodeSnapshot(longer), "trailing_bytes", { count: 1 });
  });

  it("decodes a value written with a codec back through the same codec", () => {
    const value = ["a", "b\u{1f30a}"];
    const codec = codecs.vec(codecs.string);
    const bytes = encodeSnapshot({
      ...empty,
      generationFloor: 1,
      types: [{ typeId: 2, fingerprint: 3n }],
      stores: [{ handle: 1n, typeId: 2, signals: [{ signalId: 0, value: encodeValue(codec, value) }] }],
    });
    const signal = decodeSnapshot(bytes).stores[0]?.signals[0];
    expect(signal && decodeValue(codec, signal.value)).toEqual(value);
  });
});
