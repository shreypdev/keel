import { describe, expect, it, vi } from "vitest";
import { UndraCallError, type UndraCallFailure, UndraUnhandledError } from "../src/call-error.js";
import { UndraCore } from "../src/core.js";
import {
  UndraError,
  UndraModeError,
  UndraPortError,
  UndraReplyError,
  UndraRestoreError,
  UndraSchemaMismatchError,
  UndraTransportError,
  type TransportFailure,
} from "../src/errors.js";
import { UndraStore } from "../src/object.js";
import { Signal } from "../src/signal.js";
import {
  ALL_SIGNALS,
  CallTarget,
  type Codec,
  type ChangeOp,
  ReplyStatus,
  UndraWriter,
  WireError,
  codecs,
  encodeValue,
} from "../src/wire/index.js";
import { FakeCoreTransport, SCHEMA } from "./support/fake-core.js";
import { captureLog, macrotask, track } from "./support/harness.js";

const FREE = { target: CallTarget.FreeFunction } as const;
const M = 7;

const strings = (...text: string[]): Uint8Array => {
  const w = new UndraWriter();
  for (const t of text) w.writeStr(t);
  return w.finish();
};
const reply = (status: ReplyStatus, body: Uint8Array = new Uint8Array(0)) => new UndraReplyError(status, body);

/** A typed error shaped like the generated ones: an `UndraError` subclass with a codec. */
class LabError extends UndraError {
  constructor(
    kind: "empty" | "rejected",
    readonly code: number = 0,
  ) {
    super(kind, kind === "empty" ? "empty" : `rejected ${code}`);
  }
}
const LabErrorCodec: Codec<LabError> = {
  encode(w, v) {
    w.writeU16(v.kind === "empty" ? 0 : 1);
    if (v.kind === "rejected") w.writeU16(v.code);
  },
  decode(r) {
    const tag = r.readU16();
    if (tag === 0) return new LabError("empty");
    if (tag === 1) return new LabError("rejected", r.readU16());
    throw new WireError({ code: "invalid_tag", tag, at: 0, ty: "LabError" });
  },
};
const typed = (error: LabError): Uint8Array => encodeValue(LabErrorCodec, error);

async function setup(onError?: (error: UndraUnhandledError) => void) {
  const fake = new FakeCoreTransport();
  const log = captureLog();
  const core = track(
    await UndraCore.attach(fake, {
      expectedSchemaHash: SCHEMA,
      shared: false,
      adapters: { log, http: null, timer: null },
      ...(onError && { onError }),
    }),
  );
  return { fake, core, log };
}

describe("UndraCallError.mapped", () => {
  it("maps every reply status onto its case", () => {
    expect(UndraCallError.mapped(reply(ReplyStatus.Cancelled))).toBeInstanceOf(UndraCallError.CancelledByCore);
    const panic = UndraCallError.mapped(reply(ReplyStatus.Panic, strings("kaboom", "frame 1"))) as UndraCallError.Panicked;
    expect(panic).toBeInstanceOf(UndraCallError.Panicked);
    expect(panic.panicMessage).toBe("kaboom");
    expect(panic.backtrace).toBe("frame 1");
    expect(panic.message).toContain("kaboom");
    const refused = UndraCallError.mapped(reply(ReplyStatus.BadRequest, strings("stale handle"))) as UndraCallError.Refused;
    expect(refused).toBeInstanceOf(UndraCallError.Refused);
    expect(refused.reason).toBe("stale handle");
    expect(refused.message).toContain("stale handle");
    expect(UndraCallError.mapped(reply(ReplyStatus.Error, new Uint8Array([1, 2])))).toBeInstanceOf(UndraCallError.Malformed);
    expect(UndraCallError.mapped(reply(ReplyStatus.StreamOpened))).toBeInstanceOf(UndraCallError.Malformed);
    expect(UndraCallError.mapped(reply(ReplyStatus.Ok))).toBeInstanceOf(UndraCallError.Malformed);
  });

  it("keeps the raw reply as the cause", () => {
    const raw = reply(ReplyStatus.Panic, strings("p", "b"));
    expect((UndraCallError.mapped(raw) as Error).cause).toBe(raw);
  });

  it("degrades an unreadable panic or refusal body to a placeholder text", () => {
    const panic = UndraCallError.mapped(reply(ReplyStatus.Panic, new Uint8Array([9, 9]))) as UndraCallError.Panicked;
    expect(panic.panicMessage).toBe("<undecodable panic report>");
    const refused = UndraCallError.mapped(reply(ReplyStatus.BadRequest, new Uint8Array([9]))) as UndraCallError.Refused;
    expect(refused.reason).toBe("<undecodable reason>");
  });

  it("passes an abort reason, a closed-set member, a typed error and anything that is not Undra's through unchanged", () => {
    const abort = new DOMException("The operation was aborted", "AbortError");
    expect(UndraCallError.mapped(abort)).toBe(abort);
    const own = new UndraCallError.Refused("x");
    expect(UndraCallError.mapped(own)).toBe(own);
    const foreign = new TypeError("a bug of the app");
    expect(UndraCallError.mapped(foreign)).toBe(foreign);
    const typedError = new LabError("empty");
    expect(UndraCallError.mapped(typedError)).toBe(typedError);
    expect(UndraCallError.mapped("a string")).toBe("a string");
  });

  it("maps transport failures to Unavailable, except a protocol violation, which is Malformed", () => {
    const unavailable: TransportFailure[] = ["closed", "handshake", "trap", "timeout", "unsupported"];
    for (const reason of unavailable) {
      const transport = new UndraTransportError(reason, `t ${reason}`);
      const mapped = UndraCallError.mapped(transport) as UndraCallError.Unavailable;
      expect(mapped).toBeInstanceOf(UndraCallError.Unavailable);
      expect(mapped.transport).toBe(transport);
      expect(mapped.cause).toBe(transport);
      expect(mapped.message).toContain("unavailable");
    }
    expect(UndraCallError.mapped(new UndraTransportError("protocol", "the core sent garbage"))).toBeInstanceOf(UndraCallError.Malformed);
  });

  it("maps a remote core that came back with another schema to Unavailable, naming both hashes", () => {
    const mapped = UndraCallError.mapped(new UndraSchemaMismatchError(1n, 2n)) as UndraCallError.Unavailable;
    expect(mapped).toBeInstanceOf(UndraCallError.Unavailable);
    expect(mapped.transport.reason).toBe("closed");
    expect(mapped.message).toContain("0x0000000000000001");
    expect(mapped.message).toContain("0x0000000000000002");
  });

  it("maps wire, port, mode, restore and unclassified runtime errors", () => {
    expect(UndraCallError.mapped(new WireError({ code: "unexpected_eof", needed: 4, at: 0 }))).toBeInstanceOf(UndraCallError.Malformed);
    expect(UndraCallError.mapped(new UndraPortError(new Uint8Array([1])))).toBeInstanceOf(UndraCallError.Malformed);
    expect(UndraCallError.mapped(new UndraModeError("callSync", "wasm-worker"))).toBeInstanceOf(UndraCallError.Refused);
    expect(UndraCallError.mapped(new UndraRestoreError(5))).toBeInstanceOf(UndraCallError.Refused);
    expect(UndraCallError.mapped(new UndraError("state", "this method is a stream"))).toBeInstanceOf(UndraCallError.Malformed);
    expect(UndraCallError.mapped(new UndraError("observe", "no change-set arrived"))).toBeInstanceOf(UndraCallError.Malformed);
  });

  it("is a closed set: kind is the discriminant and an exhaustive switch narrows", () => {
    const all: UndraCallFailure[] = [
      new UndraCallError.CancelledByCore(),
      new UndraCallError.Panicked("m", "b"),
      new UndraCallError.Refused("r"),
      new UndraCallError.Unavailable(new UndraTransportError("closed", "c")),
      new UndraCallError.Malformed("d"),
    ];
    const describe = (error: UndraCallFailure): string => {
      switch (error.kind) {
        case "cancelledByCore":
          return "cancelled";
        case "panicked":
          return error.backtrace; // narrowed to Panicked
        case "refused":
          return error.reason; // narrowed to Refused
        case "unavailable":
          return error.transport.reason; // narrowed to Unavailable
        case "malformed":
          return error.detail; // narrowed to Malformed
      }
    };
    expect(all.map(describe)).toEqual(["cancelled", "b", "r", "closed", "d"]);
    expect(all.map((e) => e.kind)).toEqual(["cancelledByCore", "panicked", "refused", "unavailable", "malformed"]);
    for (const error of all) {
      expect(error).toBeInstanceOf(UndraCallError);
      expect(error).toBeInstanceOf(UndraError);
      expect(error.name.startsWith("UndraCallError.")).toBe(true);
    }
  });

  it("re-roots WireError under UndraError", () => {
    const error = new WireError({ code: "invalid_utf8", at: 3 });
    expect(error).toBeInstanceOf(UndraError);
    expect(error.kind).toBe("wire");
    expect(error.name).toBe("WireError");
    expect(error.code).toBe("invalid_utf8");
  });
});

describe("UndraCallError.mapped with a domain", () => {
  it("decodes a typed reply into the method's own error; a body that does not decode is Malformed", () => {
    const rejected = UndraCallError.mapped(reply(ReplyStatus.Error, typed(new LabError("rejected", 7))), LabErrorCodec);
    expect(rejected).toBeInstanceOf(LabError);
    expect((rejected as LabError).code).toBe(7);
    expect(UndraCallError.mapped(reply(ReplyStatus.Error, typed(new LabError("empty"))), LabErrorCodec)).toBeInstanceOf(LabError);
    expect(UndraCallError.mapped(reply(ReplyStatus.Error, new Uint8Array([9, 9, 9])), LabErrorCodec)).toBeInstanceOf(UndraCallError.Malformed);
    const trailing = new Uint8Array([...typed(new LabError("empty")), 1]);
    expect(UndraCallError.mapped(reply(ReplyStatus.Error, trailing), LabErrorCodec)).toBeInstanceOf(UndraCallError.Malformed);
  });

  it("maps every other failure as it does without a domain", () => {
    expect(UndraCallError.mapped(reply(ReplyStatus.Panic, strings("p", "b")), LabErrorCodec)).toBeInstanceOf(UndraCallError.Panicked);
    expect(UndraCallError.mapped(reply(ReplyStatus.Cancelled), LabErrorCodec)).toBeInstanceOf(UndraCallError.CancelledByCore);
    expect(UndraCallError.mapped(new UndraTransportError("closed", "c"), LabErrorCodec)).toBeInstanceOf(UndraCallError.Unavailable);
    const abort = new DOMException("aborted", "AbortError");
    expect(UndraCallError.mapped(abort, LabErrorCodec)).toBe(abort);
  });
});

describe("UndraCallError.mappedStream", () => {
  it("reads a stream error item as the core's String: cancelled, or a panic message", () => {
    expect(UndraCallError.mappedStream(reply(ReplyStatus.Error, strings("cancelled: a restore replaced the receiver")))).toBeInstanceOf(
      UndraCallError.CancelledByCore,
    );
    const panic = UndraCallError.mappedStream(reply(ReplyStatus.Error, strings("the stream panicked: boom"))) as UndraCallError.Panicked;
    expect(panic).toBeInstanceOf(UndraCallError.Panicked);
    expect(panic.panicMessage).toBe("the stream panicked: boom");
    expect(panic.backtrace).toBe("");
    expect(UndraCallError.mappedStream(reply(ReplyStatus.Error, new Uint8Array([1])))).toBeInstanceOf(UndraCallError.Malformed);
    expect(UndraCallError.mappedStream(reply(ReplyStatus.Error, new Uint8Array([...strings("a"), 1])))).toBeInstanceOf(UndraCallError.Malformed);
  });

  it("with an error type tries E first, then the String; other failures map as usual", () => {
    const rejected = UndraCallError.mappedStream(reply(ReplyStatus.Error, typed(new LabError("rejected", 3))), LabErrorCodec);
    expect((rejected as LabError).code).toBe(3);
    expect(UndraCallError.mappedStream(reply(ReplyStatus.Error, strings("cancelled: the runtime shut down")), LabErrorCodec)).toBeInstanceOf(
      UndraCallError.CancelledByCore,
    );
    expect(UndraCallError.mappedStream(reply(ReplyStatus.Error, strings("the stream panicked: x")), LabErrorCodec)).toBeInstanceOf(UndraCallError.Panicked);
    expect(UndraCallError.mappedStream(reply(ReplyStatus.Error, new Uint8Array([9, 9, 9])), LabErrorCodec)).toBeInstanceOf(UndraCallError.Malformed);
    expect(UndraCallError.mappedStream(new UndraTransportError("closed", "c"), LabErrorCodec)).toBeInstanceOf(UndraCallError.Unavailable);
  });

  it("a body that reads as an E wins over the String reading (the Swift tie-break; ADR-036 removes it)", () => {
    // `empty` is the two bytes 00 00, which cannot be a String (a four-byte length prefix).
    expect(UndraCallError.mappedStream(reply(ReplyStatus.Error, new Uint8Array([0, 0])), LabErrorCodec)).toBeInstanceOf(LabError);
    // Four zero bytes are the empty String; as an E they would leave two bytes over, so the String reading wins.
    expect(UndraCallError.mappedStream(reply(ReplyStatus.Error, new Uint8Array([0, 0, 0, 0])), LabErrorCodec)).toBeInstanceOf(UndraCallError.Panicked);
  });
});

describe("UndraCore.report", () => {
  it("logs at error level, maps the failure and calls onError once, synchronously", async () => {
    const reports: UndraUnhandledError[] = [];
    const { core, log } = await setup((e) => reports.push(e));
    const raw = reply(ReplyStatus.BadRequest, strings("stale handle"));
    core.report(raw, "Todos.toggle");
    expect(reports).toHaveLength(1);
    const report = reports[0] as UndraUnhandledError;
    expect(report).toBeInstanceOf(UndraUnhandledError);
    expect(report).toBeInstanceOf(UndraError);
    expect(report.operation).toBe("Todos.toggle");
    expect(report.error).toBeInstanceOf(UndraCallError.Refused);
    expect(report.cause).toBe(raw);
    expect(report.message).toBe(`Todos.toggle failed: ${report.error.message}`);
    expect(log.records).toEqual([{ level: 4, target: "undra::runtime", message: report.message }]);
  });

  it("without a handler it only logs", async () => {
    const { core, log } = await setup();
    core.report(new UndraTransportError("closed", "closed"), "configureRemote");
    expect(log.records).toHaveLength(1);
    expect(log.records[0]?.message).toContain("configureRemote failed");
  });

  it("reports a failure that is not Undra's as Malformed, keeping the original as the cause", async () => {
    const reports: UndraUnhandledError[] = [];
    const { core } = await setup((e) => reports.push(e));
    const bug = new TypeError("a bug");
    core.report(bug, "Todos.toggle");
    const report = reports[0] as UndraUnhandledError;
    expect(report.error).toBeInstanceOf(UndraCallError.Malformed);
    expect(report.error.cause).toBe(bug);
    expect(report.cause).toBe(bug);
  });

  it("a handler that reports again is not called again; the guard is released afterwards", async () => {
    const seen: string[] = [];
    const holder: { core?: UndraCore } = {};
    const { core } = await setup((e) => {
      seen.push(e.operation);
      holder.core?.report(new UndraTransportError("closed", "closed"), "nested");
    });
    holder.core = core;
    core.report(reply(ReplyStatus.Cancelled), "outer");
    expect(seen).toEqual(["outer"]);
    core.report(reply(ReplyStatus.Cancelled), "later");
    expect(seen).toEqual(["outer", "later"]);
  });

  it("a handler that calls a failing command is not called again for it, though the command fails after the handler returned", async () => {
    // Shaped like a generated command: its whole body is one try/catch around an awaited call.
    const command = async (on: UndraCore): Promise<void> => {
      try {
        await on.call(FREE, M, new Uint8Array(0));
      } catch (error) {
        on.report(error, "Lab.poke");
      }
    };
    const seen: string[] = [];
    const holder: { core?: UndraCore } = {};
    const { core, log } = await setup((e) => {
      seen.push(e.operation);
      // Bounded so that a broken guard fails the test instead of looping on the microtask queue for ever.
      if (seen.length < 50 && holder.core !== undefined) void command(holder.core);
    });
    holder.core = core;
    core.close();
    await command(core);
    await macrotask();
    expect(seen).toEqual(["Lab.poke"]);
    // The nested failure was logged, not lost.
    expect(log.records.filter((r) => r.message.startsWith("Lab.poke failed")).length).toBe(2);
    // The guard is about the calls the handler started, not about later, unrelated failures.
    await command(core);
    await macrotask();
    expect(seen).toEqual(["Lab.poke", "Lab.poke"]);
  });

  it("the failure the handler's call shares with other pending calls still reaches the handler for them", async () => {
    const seen: string[] = [];
    const holder: { core?: UndraCore } = {};
    const { fake, core } = await setup((e) => {
      seen.push(e.operation);
      // The first report starts a call that stays pending until the core closes.
      if (seen.length === 1 && holder.core !== undefined) void holder.core.call(FREE, M, new Uint8Array(0)).catch(() => {});
    });
    holder.core = core;
    fake.on(M, () => {}); // never answered
    const pending = core.call(FREE, M, new Uint8Array(0)).catch((error: unknown) => {
      core.report(error, "Lab.pending");
    });
    core.report(new UndraTransportError("closed", "closed"), "Lab.first");
    core.close();
    await pending;
    await macrotask();
    expect(seen).toEqual(["Lab.first", "Lab.pending"]);
  });

  it("a thrown value without a string form is still reported, and report does not throw", async () => {
    const reports: UndraUnhandledError[] = [];
    const { core } = await setup((e) => reports.push(e));
    const odd: unknown = Object.create(null);
    expect(() => {
      core.report(odd, "Lab.odd");
    }).not.toThrow();
    expect(reports[0]?.error).toBeInstanceOf(UndraCallError.Malformed);
    expect((reports[0]?.error as UndraCallError.Malformed).detail).toBe("[object Object]");
  });

  it("a handler that throws is contained and logged; report never throws", async () => {
    const { core, log } = await setup(() => {
      throw new Error("the handler is broken");
    });
    expect(() => {
      core.report(reply(ReplyStatus.Cancelled), "Todos.toggle");
    }).not.toThrow();
    expect(log.records.some((r) => r.message.includes("the onError handler threw") && r.message.includes("the handler is broken"))).toBe(true);
  });

  it("a malformed change-set, a failed port and a store that cannot apply a change reach onError with an operation", async () => {
    const reports: UndraUnhandledError[] = [];
    const { fake, core } = await setup((e) => reports.push(e));
    fake.emitRawChangeSet(new Uint8Array([1, 2, 3]));
    await macrotask();
    expect(reports.map((r) => r.operation)).toEqual(["mirror"]);
    expect(reports[0]?.error).toBeInstanceOf(UndraCallError.Malformed);
    expect(core.closed).toBe(false);
  });
});

describe("a call through the runtime", () => {
  it("rejects with the raw UndraReplyError (the raw API), which generated code maps", async () => {
    const { fake, core } = await setup();
    fake.on(M, (_call, r) => r.panic("kaboom", "frame"));
    const failure = await core.call(FREE, M, new Uint8Array(0)).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(UndraReplyError);
    const mapped = UndraCallError.mapped(failure) as UndraCallError.Panicked;
    expect(mapped.panicMessage).toBe("kaboom");
  });

  it("a call on a closed core rejects with a closed transport error, which maps to Unavailable", async () => {
    const { core } = await setup();
    core.close();
    const failure = await core.call(FREE, M, new Uint8Array(0)).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(UndraTransportError);
    expect(UndraCallError.mapped(failure)).toBeInstanceOf(UndraCallError.Unavailable);
  });

  it("a caller's AbortSignal rejects with its reason, which is not an UndraCallError", async () => {
    const { fake, core } = await setup();
    fake.on(M, (_call, r) => r.defer());
    const controller = new AbortController();
    const pending = core.call(FREE, M, new Uint8Array(0), controller.signal).catch((e: unknown) => e);
    controller.abort();
    const failure = await pending;
    expect(failure).toBe(controller.signal.reason);
    expect(UndraCallError.mapped(failure)).toBe(failure);
  });
});

describe("UndraCore.shared with no core loaded", () => {
  it("is a closed placeholder whose calls reject typed instead of throwing at access", async () => {
    expect(UndraCore.current).toBeNull();
    const shared = UndraCore.shared;
    expect(UndraCore.shared).toBe(shared);
    expect(UndraCore.current).toBeNull();
    expect(shared.closed).toBe(true);
    const failure = await shared.call(FREE, M, new Uint8Array(0)).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(UndraTransportError);
    expect((failure as UndraTransportError).reason).toBe("closed");
    expect((failure as UndraTransportError).message).toContain("UndraCore.load");
    const mapped = UndraCallError.mapped(failure) as UndraCallError.Unavailable;
    expect(mapped).toBeInstanceOf(UndraCallError.Unavailable);
    expect(() => shared.callSync(FREE, M, new Uint8Array(0))).toThrow(UndraTransportError);
    await expect(shared.construct(1, 2, new Uint8Array(0))).rejects.toBeInstanceOf(UndraTransportError);
    await expect(shared.observe(1n, ALL_SIGNALS, true)).rejects.toBeInstanceOf(UndraTransportError);
    await expect(
      (async () => {
        for await (const item of shared.stream(FREE, M, new Uint8Array(0))) void item;
      })(),
    ).rejects.toBeInstanceOf(UndraTransportError);
    expect(() => {
      shared.release(1n);
      shared.close();
    }).not.toThrow();
  });

  it("a command on it only logs", async () => {
    // The placeholder has no log adapter and no handler of its own: the report goes to the console's error level.
    // (Its first use logs the teaching message once, so take it before counting.)
    const shared = UndraCore.shared;
    const errors = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      expect(() => {
        shared.report(new UndraTransportError("closed", "no core"), "Todos.toggle");
      }).not.toThrow();
      expect(errors).toHaveBeenCalledTimes(1);
      expect(String(errors.mock.calls[0]?.[0])).toContain("Todos.toggle failed: the Undra core is unavailable");
    } finally {
      errors.mockRestore();
    }
  });
});

/** A store shaped like the generated ones: `_observeAll` after construction. */
class ProbeStore extends UndraStore {
  readonly value = new Signal<number>(0);

  private constructor(core: UndraCore, handle: bigint) {
    super(core, handle);
    this._signals = [this.value];
  }

  static async create(core: UndraCore, handle: bigint): Promise<ProbeStore> {
    const store = new ProbeStore(core, handle);
    await store._observeAll();
    return store;
  }

  protected override _apply(_signalId: number, _op: ChangeOp, _value: Uint8Array): void {}
}

describe("UndraStore._observeAll", () => {
  it("closes the store and rejects with Unavailable when the core is closed", async () => {
    const { fake, core } = await setup();
    core.close();
    const failure = await ProbeStore.create(core, 9n).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(UndraCallError.Unavailable);
    // The handle was released through the closed core (nothing reaches the transport), and the mirror forgot it.
    expect(fake.released).toEqual([]);
    expect(core.mirror.size).toBe(0);
  });

  it("rejects with Malformed when the handle is not a live store (no initial values arrive)", async () => {
    const fake = new FakeCoreTransport();
    const core = track(
      await UndraCore.attach(fake, {
        expectedSchemaHash: SCHEMA,
        shared: false,
        observeTimeoutMs: 20,
        adapters: { log: { log() {} }, http: null, timer: null },
      }),
    );
    const failure = await ProbeStore.create(core, 0x99n).catch((e: unknown) => e);
    expect(failure).toBeInstanceOf(UndraCallError.Malformed);
    expect(core.mirror.size).toBe(0);
    expect(fake.released).toEqual([0x99n]);
  });
});

describe("encoding", () => {
  it("keeps a value the wire cannot represent a programming error, not an outcome of the call", () => {
    const w = new UndraWriter();
    expect(() => {
      w.writeU8(300);
    }).toThrow(RangeError);
    expect(UndraCallError.mapped(new RangeError("u8 out of range"))).toBeInstanceOf(RangeError);
    expect(codecs.u8).toBeDefined();
  });
});
