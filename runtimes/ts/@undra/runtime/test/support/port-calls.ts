import { DbErrorCodec, DbExecutedCodec, DbMigrationCodec, DbOpenedCodec, DbRowsCodec, DbValueCodec, HeaderCodec, SseErrorCodec, SseEventCodec, WsErrorCodec, WsMessageCodec, WsOpenedCodec } from "../../src/adapters/codecs.js";
import { OptInPortIds } from "../../src/adapters/opt-in-ids.js";
import type { DbMigration, DbValue, Header, WsMessage } from "../../src/adapters/types.js";
import { UndraPortError } from "../../src/errors.js";
import type { PortImpl } from "../../src/port.js";
import { type Codec, UndraWriter, codecs, decodeValue } from "../../src/wire/index.js";

/*
 * Calls a port's methods the way the core does (the argument bytes of SPEC 3.6) and decodes the
 * answer: `{ ok }` with the reply, or `{ err }` with the typed error a binding failed with.
 */

/** A port call's outcome. */
export type Outcome<T, E> = { readonly ok: T } | { readonly err: E };

/** Calls `method` of `port` and decodes the reply with `ok`, or the typed error with `err`. */
export async function call<T, E>(port: PortImpl, method: number, args: Uint8Array, ok: Codec<T>, err: Codec<E>): Promise<Outcome<T, E>> {
  const fn = port.methods[method];
  if (fn === undefined) throw new Error(`no method 0x${method.toString(16)}`);
  try {
    return { ok: decodeValue(ok, await fn(args)) };
  } catch (error) {
    if (error instanceof UndraPortError) return { err: decodeValue(err, error.body) };
    throw error;
  }
}

/** The value of `outcome`, or a failure naming its error. */
export function ok<T, E>(outcome: Outcome<T, E>): T {
  if ("ok" in outcome) return outcome.ok;
  throw new Error(`expected success, got ${String((outcome.err as { message?: string }).message ?? outcome.err)}`);
}

/** The error of `outcome`, or a failure. */
export function err<T, E>(outcome: Outcome<T, E>): E {
  if ("err" in outcome) return outcome.err;
  throw new Error(`expected a typed error, got ${JSON.stringify(outcome.ok, (_, v: unknown) => (typeof v === "bigint" ? `${v}n` : v))}`);
}

/** Encodes arguments. */
export function args(write: (w: UndraWriter) => void): Uint8Array {
  const w = new UndraWriter();
  write(w);
  return w.finish();
}

const unit: Codec<undefined> = { encode() {}, decode: () => undefined };
const headerList = codecs.vec(HeaderCodec);
const stringList = codecs.vec(codecs.string);
const optionString = codecs.option(codecs.string);
const messageList = codecs.vec(WsMessageCodec);
const eventList = codecs.vec(SseEventCodec);
const migrationList = codecs.vec(DbMigrationCodec);
const valueList = codecs.vec(DbValueCodec);

/** The `WebSocket` port's four methods, as the core calls them. */
export function wsCalls(port: PortImpl) {
  const ids = OptInPortIds.WebSocket;
  return {
    connect: (url: string, protocols: readonly string[] = [], headers: readonly Header[] = []) =>
      call(
        port,
        ids.connect,
        args((w) => {
          w.writeStr(url);
          stringList.encode(w, [...protocols]);
          headerList.encode(w, [...headers]);
        }),
        WsOpenedCodec,
        WsErrorCodec,
      ),
    send: (conn: number, message: WsMessage) =>
      call(
        port,
        ids.send,
        args((w) => {
          w.writeU32(conn);
          WsMessageCodec.encode(w, message);
        }),
        unit,
        WsErrorCodec,
      ),
    receive: (conn: number, max: number) =>
      call(
        port,
        ids.receive,
        args((w) => {
          w.writeU32(conn);
          w.writeU32(max);
        }),
        messageList,
        WsErrorCodec,
      ),
    close: (conn: number, code: number, reason: string) =>
      call(
        port,
        ids.close,
        args((w) => {
          w.writeU32(conn);
          w.writeU16(code);
          w.writeStr(reason);
        }),
        unit,
        WsErrorCodec,
      ),
  };
}

/** The `Sse` port's three methods, as the core calls them. */
export function sseCalls(port: PortImpl) {
  const ids = OptInPortIds.Sse;
  return {
    open: (url: string, headers: readonly Header[] = [], lastEventId: string | null = null) =>
      call(
        port,
        ids.open,
        args((w) => {
          w.writeStr(url);
          headerList.encode(w, [...headers]);
          optionString.encode(w, lastEventId);
        }),
        codecs.u32,
        SseErrorCodec,
      ),
    next: (stream: number, max: number) =>
      call(
        port,
        ids.next,
        args((w) => {
          w.writeU32(stream);
          w.writeU32(max);
        }),
        eventList,
        SseErrorCodec,
      ),
    close: (stream: number) =>
      call(
        port,
        ids.close,
        args((w) => w.writeU32(stream)),
        unit,
        SseErrorCodec,
      ),
  };
}

/** The `Db` port's seven methods, as the core calls them. */
export function dbCalls(port: PortImpl) {
  const ids = OptInPortIds.Db;
  const statement = (db: number, sql: string, params: readonly DbValue[]) =>
    args((w) => {
      w.writeU32(db);
      w.writeStr(sql);
      valueList.encode(w, [...params]);
    });
  const one = (id: number) => args((w) => w.writeU32(id));
  return {
    open: (name: string, migrations: readonly DbMigration[] = []) =>
      call(
        port,
        ids.open,
        args((w) => {
          w.writeStr(name);
          migrationList.encode(w, [...migrations]);
        }),
        DbOpenedCodec,
        DbErrorCodec,
      ),
    execute: (db: number, sql: string, params: readonly DbValue[] = []) =>
      call(port, ids.execute, statement(db, sql, params), DbExecutedCodec, DbErrorCodec),
    query: (db: number, sql: string, params: readonly DbValue[] = []) =>
      call(port, ids.query, statement(db, sql, params), DbRowsCodec, DbErrorCodec),
    begin: (db: number) => call(port, ids.begin, one(db), codecs.u32, DbErrorCodec),
    commit: (tx: number) => call(port, ids.commit, one(tx), unit, DbErrorCodec),
    rollback: (tx: number) => call(port, ids.rollback, one(tx), unit, DbErrorCodec),
    close: (db: number) => call(port, ids.close, one(db), unit, DbErrorCodec),
  };
}

/** A text message. */
export const text = (value: string): WsMessage => ({ kind: "text", value });

/** Cells, written short: `null`, `bigint` → integer, `number` → real, `string` → text, `Uint8Array` → blob. */
export function cells(...values: Array<null | bigint | number | string | Uint8Array>): DbValue[] {
  return values.map((v): DbValue => {
    if (v === null) return { kind: "null" };
    if (typeof v === "bigint") return { kind: "integer", value: v };
    if (typeof v === "number") return { kind: "real", value: v };
    if (typeof v === "string") return { kind: "text", value: v };
    return { kind: "blob", value: v };
  });
}
