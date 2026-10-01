import { describe, expect, it } from "vitest";
import {
  DbConstraintCodec,
  DbErrorCodec,
  DbExecutedCodec,
  DbMigrationCodec,
  DbOpenedCodec,
  DbRowsCodec,
  DbValueCodec,
  SseErrorCodec,
  SseEventCodec,
  WsErrorCodec,
  WsMessageCodec,
  WsOpenedCodec,
} from "../src/adapters/codecs.js";
import { PortIds } from "../src/adapters/ids.js";
import { DB_CONSTRAINTS, DbError, type DbRows, type DbValue, SseError, type SseEvent, WsError, type WsMessage } from "../src/adapters/types.js";
import { UndraError } from "../src/errors.js";
import * as main from "../src/index.js";
import { type Codec, WireError, decodeValue, encodeValue } from "../src/wire/index.js";

/*
 * The twelve standard types of the opt-in ports (ADR-047, ADR-048): exact bytes (the vectors of
 * the Rust unit tests in crates/undra-ports/src/{ws,sse,db}.rs), round trips, the error texts (the
 * Rust `#[error]` strings) and the pinned ids of the brief.
 */

const bytes = (...b: number[]): Uint8Array => Uint8Array.from(b);

function roundTrip<T>(codec: Codec<T>, value: T): T {
  return decodeValue(codec, encodeValue(codec, value));
}

describe("exact bytes (the Rust vectors)", () => {
  it("WsMessage, WsOpened, WsError", () => {
    expect(encodeValue(WsMessageCodec, { kind: "text", value: "hi" })).toEqual(bytes(0, 0, 2, 0, 0, 0, 0x68, 0x69));
    expect(encodeValue(WsMessageCodec, { kind: "binary", value: bytes(7) })).toEqual(bytes(1, 0, 1, 0, 0, 0, 7));
    expect(encodeValue(WsOpenedCodec, { conn: 3, protocol: "p" })).toEqual(bytes(3, 0, 0, 0, 1, 0, 0, 0, 0x70));
    expect(encodeValue(WsErrorCodec, new WsError.Closed(1000, ""))).toEqual(bytes(3, 0, 0xe8, 0x03, 0, 0, 0, 0));
    expect(encodeValue(WsErrorCodec, new WsError.Refused(401, ""))).toEqual(bytes(0, 0, 1, 0x91, 0x01, 0, 0, 0, 0));
  });

  it("SseEvent, SseError", () => {
    const bare: SseEvent = { id: null, event: "", data: "", retryMs: null };
    expect(encodeValue(SseEventCodec, bare)).toEqual(bytes(0, 0, 0, 0, 0, 0, 0, 0, 0, 0));
    expect(encodeValue(SseErrorCodec, new SseError.Ended())).toEqual(bytes(3, 0));
  });

  it("DbValue, DbOpened, DbError", () => {
    expect(encodeValue(DbValueCodec, { kind: "null" })).toEqual(bytes(0, 0));
    expect(encodeValue(DbValueCodec, { kind: "integer", value: -2n })).toEqual(bytes(1, 0, 0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff));
    expect(encodeValue(DbValueCodec, { kind: "real", value: 1.0 })).toEqual(bytes(2, 0, 0, 0, 0, 0, 0, 0, 0xf0, 0x3f));
    expect(encodeValue(DbValueCodec, { kind: "text", value: "a" })).toEqual(bytes(3, 0, 1, 0, 0, 0, 0x61));
    expect(encodeValue(DbValueCodec, { kind: "blob", value: bytes(9) })).toEqual(bytes(4, 0, 1, 0, 0, 0, 9));
    expect(encodeValue(DbOpenedCodec, { db: 1, version: 2 })).toEqual(bytes(1, 0, 0, 0, 2, 0, 0, 0));
    expect(encodeValue(DbErrorCodec, new DbError.Constraint("notNull", ""))).toEqual(bytes(1, 0, 1, 0, 0, 0, 0, 0));
    expect(encodeValue(DbErrorCodec, new DbError.Migration(3, ""))).toEqual(bytes(6, 0, 3, 0, 0, 0, 0, 0, 0, 0));
  });

  it("DbMigration, DbExecuted, DbRows, DbConstraint (SPEC 3.1 layouts)", () => {
    expect(encodeValue(DbMigrationCodec, { version: 2, sql: "x" })).toEqual(bytes(2, 0, 0, 0, 1, 0, 0, 0, 0x78));
    expect(encodeValue(DbExecutedCodec, { changes: 1n, lastInsertId: -1n })).toEqual(
      bytes(1, 0, 0, 0, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff),
    );
    expect(encodeValue(DbRowsCodec, { columns: ["n"], rows: [[{ kind: "null" }]] })).toEqual(
      bytes(1, 0, 0, 0, 1, 0, 0, 0, 0x6e, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0),
    );
    DB_CONSTRAINTS.forEach((kind, index) => {
      expect(encodeValue(DbConstraintCodec, kind)).toEqual(bytes(index, 0));
    });
  });
});

describe("round trips", () => {
  it("every variant of the records and enums", () => {
    const messages: WsMessage[] = [
      { kind: "text", value: "é😀" },
      { kind: "binary", value: bytes() },
      { kind: "binary", value: bytes(0, 255) },
    ];
    for (const m of messages) expect(roundTrip(WsMessageCodec, m)).toEqual(m);
    const event: SseEvent = { id: "7", event: "message", data: "a\nb", retryMs: 3000 };
    expect(roundTrip(SseEventCodec, event)).toEqual(event);
    const cells: DbValue[] = [
      { kind: "null" },
      { kind: "integer", value: -(2n ** 63n) },
      { kind: "integer", value: 2n ** 63n - 1n },
      { kind: "real", value: -0.5 },
      { kind: "text", value: "" },
      { kind: "blob", value: bytes(1, 2, 3) },
    ];
    for (const c of cells) expect(roundTrip(DbValueCodec, c)).toEqual(c);
    const rows: DbRows = { columns: ["a", "b"], rows: [cells.slice(0, 2), cells.slice(2, 4)] };
    expect(roundTrip(DbRowsCodec, rows)).toEqual(rows);
    expect(roundTrip(DbExecutedCodec, { changes: 2n ** 64n - 1n, lastInsertId: -(2n ** 63n) })).toEqual({
      changes: 2n ** 64n - 1n,
      lastInsertId: -(2n ** 63n),
    });
  });

  it("every error variant, keeping its class and fields", () => {
    const ws = [new WsError.Refused(403, "forbidden"), new WsError.Refused(null, "x"), new WsError.Network("reset"), new WsError.Protocol("masked"), new WsError.Closed(1000, "bye")];
    for (const e of ws) {
      const back = roundTrip(WsErrorCodec, e);
      expect(back.constructor).toBe(e.constructor);
      expect(back).toEqual(e);
    }
    const sse = [new SseError.Refused(204, "no content"), new SseError.Network("x"), new SseError.Protocol("text/html"), new SseError.Ended()];
    for (const e of sse) {
      const back = roundTrip(SseErrorCodec, e);
      expect(back.constructor).toBe(e.constructor);
      expect(back).toEqual(e);
    }
    const db = [
      new DbError.Busy(),
      new DbError.Constraint("unique", "UNIQUE constraint failed: t.id"),
      new DbError.Corrupt("file is not a database"),
      new DbError.Full(),
      new DbError.Unavailable("closed"),
      new DbError.Sql("no such table: x"),
      new DbError.Migration(2, "boom"),
    ];
    for (const e of db) {
      const back = roundTrip(DbErrorCodec, e);
      expect(back.constructor).toBe(e.constructor);
      expect(back).toEqual(e);
    }
  });

  it("an unknown tag is a WireError naming the type", () => {
    for (const [codec, ty] of [
      [WsMessageCodec, "WsMessage"],
      [WsErrorCodec, "WsError"],
      [SseErrorCodec, "SseError"],
      [DbValueCodec, "DbValue"],
      [DbErrorCodec, "DbError"],
      [DbConstraintCodec, "DbConstraint"],
    ] as const) {
      const error = (() => {
        try {
          decodeValue(codec as Codec<unknown>, bytes(9, 0));
        } catch (e) {
          return e;
        }
        return undefined;
      })();
      expect(error).toBeInstanceOf(WireError);
      expect((error as WireError).message).toContain(ty);
    }
  });
});

describe("the error classes", () => {
  it("say what the Rust #[error] texts say", () => {
    expect(new WsError.Refused(401, "no").message).toBe("the WebSocket was refused: no");
    expect(new WsError.Network("dns").message).toBe("WebSocket network error: dns");
    expect(new WsError.Protocol("x").message).toBe("WebSocket protocol error: x");
    expect(new WsError.Closed(1001, "away").message).toBe("the WebSocket was closed (1001): away");
    expect(new SseError.Refused(500, "x").message).toBe("the event stream was refused: x");
    expect(new SseError.Network("x").message).toBe("event stream network error: x");
    expect(new SseError.Protocol("x").message).toBe("event stream protocol error: x");
    expect(new SseError.Ended().message).toBe("the server ended the event stream");
    expect(new DbError.Busy().message).toBe("the database is busy");
    expect(new DbError.Constraint("unique", "m").message).toBe("constraint failed: m");
    expect(new DbError.Corrupt("m").message).toBe("the database is corrupt: m");
    expect(new DbError.Full().message).toBe("the database is full");
    expect(new DbError.Unavailable("m").message).toBe("the database is unavailable: m");
    expect(new DbError.Sql("m").message).toBe("SQL error: m");
    expect(new DbError.Migration(4, "x").message).toBe("migration 4 failed: x");
  });

  it("are UndraErrors with a stable kind, and each variant is an instance of its family only", () => {
    const refused = new WsError.Refused(null, "x");
    expect(refused).toBeInstanceOf(UndraError);
    expect(refused).toBeInstanceOf(WsError);
    expect(refused).toBeInstanceOf(WsError.Refused);
    expect(refused).not.toBeInstanceOf(WsError.Closed);
    expect(refused).not.toBeInstanceOf(SseError);
    expect(refused.kind).toBe("refused");
    expect(new SseError.Ended().kind).toBe("ended");
    const constraint = new DbError.Constraint("foreignKey", "FOREIGN KEY constraint failed");
    expect(constraint.kind).toBe("constraint");
    expect(constraint.kind_).toBe("foreignKey");
    expect(constraint.message_).toBe("FOREIGN KEY constraint failed");
    const narrowed: unknown = constraint;
    if (narrowed instanceof DbError.Constraint) expect(narrowed.kind_).toBe("foreignKey");
  });
});

describe("the main entry", () => {
  it("exports the twelve types' codecs and the error families, and no binding", () => {
    for (const name of [
      "WsOpenedCodec",
      "WsMessageCodec",
      "WsErrorCodec",
      "SseEventCodec",
      "SseErrorCodec",
      "DbMigrationCodec",
      "DbOpenedCodec",
      "DbValueCodec",
      "DbExecutedCodec",
      "DbRowsCodec",
      "DbConstraintCodec",
      "DbErrorCodec",
      "WsError",
      "SseError",
      "DbError",
    ]) {
      expect(main, name).toHaveProperty(name);
    }
    for (const binding of ["webSocketPort", "ssePort", "dbPort", "browserWebSocket", "nodeWebSocket", "fetchSse", "nodeSqliteDb", "waSqliteDb", "SseParser"]) {
      expect(main, binding).not.toHaveProperty(binding);
    }
  });

  it("pins the port and method ids of the brief (FNV-1a, SPEC 1.1)", () => {
    expect(PortIds.WebSocket).toEqual({ portId: 0x7388b95f, connect: 0x83477638, send: 0x117b2158, receive: 0x8f31f08f, close: 0x60154b86 });
    expect(PortIds.Sse).toEqual({ portId: 0x75d2ef19, open: 0xc0033c14, next: 0x4035cbed, close: 0x5bfe2c88 });
    expect(PortIds.Db).toEqual({
      portId: 0x559eda82,
      open: 0xee6f26db,
      execute: 0xffac2f0a,
      query: 0x3a4deefd,
      begin: 0xae2ba428,
      commit: 0xf866d5ae,
      rollback: 0x3e7b24b3,
      close: 0xde3dc7ed,
    });
  });
});
