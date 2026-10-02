import { type Codec, type UndraReader, type UndraWriter, WireError, codecs } from "../wire/index.js";
import {
  APP_STATES,
  type AppState,
  DB_CONSTRAINTS,
  type DbConstraint,
  DbError,
  type DbExecuted,
  type DbMigration,
  type DbOpened,
  type DbRows,
  type DbValue,
  FsError,
  HTTP_METHODS,
  type Header,
  HttpError,
  type HttpMethod,
  type HttpRequest,
  type HttpResponse,
  NET_KINDS,
  type NetKind,
  SseError,
  type SseEvent,
  StorageError,
  type UndraBackgroundReport,
  type UndraPanicFrame,
  type UndraPanicReport,
  WsError,
  type WsMessage,
  type WsOpened,
} from "./types.js";

/*
 * Hand-written codecs of the SPEC section 8 records, enums and errors. The
 * runtime cannot depend on generated code, and these types are part of the
 * platform contract, so they are spelled out here. Every codec follows
 * section 3.1: records are their fields in declaration order, enums and
 * errors are a `u16` variant index followed by the variant's fields.
 */

/** `Header { name: String, value: String }`. */
export const HeaderCodec: Codec<Header> = {
  encode(w, v) {
    w.writeStr(v.name);
    w.writeStr(v.value);
  },
  decode(r) {
    const name = r.readStr();
    return { name, value: r.readStr() };
  },
};

const headers = codecs.vec(HeaderCodec);
const optionBytes = codecs.option(codecs.bytes);
const optionU32 = codecs.option(codecs.u32);
const optionName = codecs.option(codecs.string);

/** Writes `v` as its `u16` index in `variants` (the encoding half of a unit enum's codec). */
function writeIndex<T extends string>(w: UndraWriter, name: string, variants: readonly T[], v: T): void {
  const index = variants.indexOf(v);
  if (index < 0) throw new RangeError(`unknown ${name} variant: ${String(v)}`);
  w.writeU16(index);
}

/** Reads a `u16` index into `variants` (the decoding half of a unit enum's codec). */
function readIndex<T extends string>(r: UndraReader, name: string, variants: readonly T[]): T {
  const at = r.position;
  const tag = r.readU16();
  const variant = variants[tag];
  if (variant === undefined) throw new WireError({ code: "invalid_tag", tag, at, ty: name });
  return variant;
}

/** A unit enum as its `u16` variant index. */
function unitEnum<T extends string>(name: string, variants: readonly T[]): Codec<T> {
  return {
    encode: (w, v) => {
      writeIndex(w, name, variants, v);
    },
    decode: (r) => readIndex(r, name, variants),
  };
}

/** `HttpMethod`. */
export const HttpMethodCodec: Codec<HttpMethod> = unitEnum("HttpMethod", HTTP_METHODS);
/** `NetKind`. */
export const NetKindCodec: Codec<NetKind> = unitEnum("NetKind", NET_KINDS);
/** `AppState`. */
export const AppStateCodec: Codec<AppState> = unitEnum("AppState", APP_STATES);

/*
 * The halves the standard ports use (`ports.ts`): the host decodes what the core sends (an `HttpRequest`) and encodes
 * what it answers (a response, a typed error, an event), so an app ships only those, not the whole codecs below.
 */

/** Writes a `NetKind` (`NetKindCodec.encode`): the payload of `Connectivity.changed`. */
export function writeNetKind(w: UndraWriter, v: NetKind): void {
  writeIndex(w, "NetKind", NET_KINDS, v);
}

/** Writes an `AppState` (`AppStateCodec.encode`): the payload of `Lifecycle.changed`. */
export function writeAppState(w: UndraWriter, v: AppState): void {
  writeIndex(w, "AppState", APP_STATES, v);
}

/** Reads an `HttpRequest` (`HttpRequestCodec.decode`). */
export function readHttpRequest(r: UndraReader): HttpRequest {
  const method = readIndex(r, "HttpMethod", HTTP_METHODS);
  const url = r.readStr();
  const hs = headers.decode(r);
  const body = optionBytes.decode(r);
  return { method, url, headers: hs, body, timeoutMs: optionU32.decode(r) };
}

/** Writes an `HttpResponse` (`HttpResponseCodec.encode`). */
export function writeHttpResponse(w: UndraWriter, v: HttpResponse): void {
  w.writeU16(v.status);
  headers.encode(w, v.headers as Header[]);
  w.writeBytes(v.body);
}

/** Writes a typed error as its variant's index in `kinds`, then its `value` when it has one. */
function writeVariant(w: UndraWriter, kinds: readonly string[], v: { readonly kind: string }, name: string): void {
  const index = kinds.indexOf(v.kind);
  if (index < 0) throw new TypeError(`unknown ${name} variant: ${v.kind}`);
  w.writeU16(index);
  if ("value" in v) w.writeStr((v as { value: string }).value);
}

/** Writes an `HttpError` (`HttpErrorCodec.encode`). */
export function writeHttpError(w: UndraWriter, v: HttpError): void {
  writeVariant(w, ["network", "timeout", "cancelled", "invalidUrl"], v, "HttpError");
}

/** Writes an `FsError` (`FsErrorCodec.encode`). */
export function writeFsError(w: UndraWriter, v: FsError): void {
  writeVariant(w, ["notFound", "denied", "io", "full", "unavailable"], v, "FsError");
}

/** Writes a `StorageError` (`StorageErrorCodec.encode`). */
export function writeStorageError(w: UndraWriter, v: StorageError): void {
  writeVariant(w, ["unavailable", "full", "locked", "corrupt", "io"], v, "StorageError");
}

/** `HttpRequest { method, url, headers, body, timeout_ms }`. */
export const HttpRequestCodec: Codec<HttpRequest> = {
  encode(w, v) {
    HttpMethodCodec.encode(w, v.method);
    w.writeStr(v.url);
    headers.encode(w, v.headers as Header[]);
    optionBytes.encode(w, v.body);
    optionU32.encode(w, v.timeoutMs);
  },
  decode: readHttpRequest,
};

/** `HttpResponse { status: u16, headers, body: Bytes }`. */
export const HttpResponseCodec: Codec<HttpResponse> = {
  encode: writeHttpResponse,
  decode(r) {
    const status = r.readU16();
    const hs = headers.decode(r);
    return { status, headers: hs, body: codecs.bytes.decode(r) };
  },
};

/** `HttpError { Network(String), Timeout, Cancelled, InvalidUrl(String) }`. */
export const HttpErrorCodec: Codec<HttpError> = {
  encode: writeHttpError,
  decode(r) {
    const at = r.position;
    const tag = r.readU16();
    switch (tag) {
      case 0:
        return new HttpError.Network(r.readStr());
      case 1:
        return new HttpError.Timeout();
      case 2:
        return new HttpError.Cancelled();
      case 3:
        return new HttpError.InvalidUrl(r.readStr());
      default:
        throw new WireError({ code: "invalid_tag", tag, at, ty: "HttpError" });
    }
  },
};

/** `FsError { NotFound, Denied, Io(String), Full, Unavailable(String) }`. */
export const FsErrorCodec: Codec<FsError> = {
  encode: writeFsError,
  decode(r) {
    const at = r.position;
    const tag = r.readU16();
    switch (tag) {
      case 0:
        return new FsError.NotFound();
      case 1:
        return new FsError.Denied();
      case 2:
        return new FsError.Io(r.readStr());
      case 3:
        return new FsError.Full();
      case 4:
        return new FsError.Unavailable(r.readStr());
      default:
        throw new WireError({ code: "invalid_tag", tag, at, ty: "FsError" });
    }
  },
};

/** `StorageError { Unavailable(String), Full, Locked, Corrupt(String), Io(String) }` (ADR-049). */
export const StorageErrorCodec: Codec<StorageError> = {
  encode: writeStorageError,
  decode(r) {
    const at = r.position;
    const tag = r.readU16();
    switch (tag) {
      case 0:
        return new StorageError.Unavailable(r.readStr());
      case 1:
        return new StorageError.Full();
      case 2:
        return new StorageError.Locked();
      case 3:
        return new StorageError.Corrupt(r.readStr());
      case 4:
        return new StorageError.Io(r.readStr());
      default:
        throw new WireError({ code: "invalid_tag", tag, at, ty: "StorageError" });
    }
  },
};

/*
 * The records of ADR-046 (`undra-ports`: `PanicFrame`, `PanicReport` of the `Diagnostics` port, `BackgroundReport` of
 * `run_background`). Golden bytes: `crates/undra-ports/tests/encoding.rs`. The halves are separate so that an app ships only
 * the one it uses (a native core's report is decoded by the runtime, encoded only by tests and fakes).
 */


/** Reads a panic frame (`PanicFrameCodec.decode`). */
export function readPanicFrame(r: UndraReader): UndraPanicFrame {
  const address = r.readU64();
  const symbol = optionName.decode(r);
  const file = optionName.decode(r);
  return { address, symbol, file, line: optionU32.decode(r) };
}

/** Writes a panic frame (`PanicFrameCodec.encode`). */
export function writePanicFrame(w: UndraWriter, v: UndraPanicFrame): void {
  w.writeU64(v.address);
  optionName.encode(w, v.symbol);
  optionName.encode(w, v.file);
  optionU32.encode(w, v.line);
}

/** The smallest encoding of a panic frame: the `u64` address and three `None`s. */
const PANIC_FRAME_MIN = 11;

/** Reads a panic report (`PanicReportCodec.decode`): the argument of `Diagnostics.panicked`. */
export function readPanicReport(r: UndraReader): UndraPanicReport {
  const message = r.readStr();
  const location = r.readStr();
  const operation = r.readStr();
  const thread = r.readStr();
  const frames: UndraPanicFrame[] = [];
  for (let n = r.readLen(PANIC_FRAME_MIN); n > 0; n--) frames.push(readPanicFrame(r));
  const namespace = r.readStr();
  const coreVersion = r.readStr();
  const schemaHash = r.readU64();
  return { message, location, operation, thread, frames, namespace, coreVersion, schemaHash, imageId: r.readStr() };
}

/** Writes a panic report (`PanicReportCodec.encode`). */
export function writePanicReport(w: UndraWriter, v: UndraPanicReport): void {
  w.writeStr(v.message);
  w.writeStr(v.location);
  w.writeStr(v.operation);
  w.writeStr(v.thread);
  w.writeLen(v.frames.length);
  for (const frame of v.frames) writePanicFrame(w, frame);
  w.writeStr(v.namespace);
  w.writeStr(v.coreVersion);
  w.writeU64(v.schemaHash);
  w.writeStr(v.imageId);
}

/** `PanicFrame { address: u64, symbol: Option<String>, file: Option<String>, line: Option<u32> }` (ADR-046). */
export const PanicFrameCodec: Codec<UndraPanicFrame> = { encode: writePanicFrame, decode: readPanicFrame };

/** `PanicReport { message, location, operation, thread, frames, namespace, core_version, schema_hash: u64, image_id }` (ADR-046). */
export const PanicReportCodec: Codec<UndraPanicReport> = { encode: writePanicReport, decode: readPanicReport };


/** Reads a background report (`BackgroundReportCodec.decode`): the reply of `run_background`. */
export function readBackgroundReport(r: UndraReader): UndraBackgroundReport {
  const finished = r.readBool();
  const replayed = r.readU32();
  const refetched = r.readU32();
  return { finished, replayed, refetched, stillPending: r.readU32() };
}

/** Writes a background report (`BackgroundReportCodec.encode`). */
export function writeBackgroundReport(w: UndraWriter, v: UndraBackgroundReport): void {
  w.writeBool(v.finished);
  w.writeU32(v.replayed);
  w.writeU32(v.refetched);
  w.writeU32(v.stillPending);
}

/** `BackgroundReport { finished: bool, replayed: u32, refetched: u32, still_pending: u32 }` (ADR-046). */
export const BackgroundReportCodec: Codec<UndraBackgroundReport> = { encode: writeBackgroundReport, decode: readBackgroundReport };

// ---------------------------------------------------------------------------
// The opt-in ports (ADR-047, ADR-048): WebSocket, Sse and Db. The shared codecs are built inside pure
// functions so a bundle that uses none of these keeps none of their arguments (ADR-052).
// ---------------------------------------------------------------------------

const optionU16 = /* @__PURE__ */ (() => codecs.option(codecs.u16))();
const optionString = /* @__PURE__ */ (() => codecs.option(codecs.string))();

/** `WsOpened { conn: u32, protocol: String }`. */
export const WsOpenedCodec: Codec<WsOpened> = {
  encode(w, v) {
    w.writeU32(v.conn);
    w.writeStr(v.protocol);
  },
  decode(r) {
    const conn = r.readU32();
    return { conn, protocol: r.readStr() };
  },
};

/** `WsMessage { Text(String), Binary(Bytes) }`. */
export const WsMessageCodec: Codec<WsMessage> = {
  encode(w, v) {
    switch (v.kind) {
      case "text":
        w.writeU16(0);
        w.writeStr(v.value);
        break;
      case "binary":
        w.writeU16(1);
        w.writeBytes(v.value);
        break;
      default:
        throw new TypeError(`unknown WsMessage variant: ${String((v as { kind: unknown }).kind)}`);
    }
  },
  decode(r) {
    const at = r.position;
    const tag = r.readU16();
    switch (tag) {
      case 0:
        return { kind: "text", value: r.readStr() };
      case 1:
        return { kind: "binary", value: codecs.bytes.decode(r) };
      default:
        throw new WireError({ code: "invalid_tag", tag, at, ty: "WsMessage" });
    }
  },
};

/** `WsError { Refused { status: Option<u16>, message }, Network(String), Protocol(String), Closed { code: u16, reason } }`. */
export const WsErrorCodec: Codec<WsError> = {
  encode(w, v) {
    if (v instanceof WsError.Refused) {
      w.writeU16(0);
      optionU16.encode(w, v.status);
      w.writeStr(v.message_);
    } else if (v instanceof WsError.Network) {
      w.writeU16(1);
      w.writeStr(v.value);
    } else if (v instanceof WsError.Protocol) {
      w.writeU16(2);
      w.writeStr(v.value);
    } else if (v instanceof WsError.Closed) {
      w.writeU16(3);
      w.writeU16(v.code);
      w.writeStr(v.reason);
    } else {
      throw new TypeError(`unknown WsError variant: ${v.kind}`);
    }
  },
  decode(r) {
    const at = r.position;
    const tag = r.readU16();
    switch (tag) {
      case 0: {
        const status = optionU16.decode(r);
        return new WsError.Refused(status, r.readStr());
      }
      case 1:
        return new WsError.Network(r.readStr());
      case 2:
        return new WsError.Protocol(r.readStr());
      case 3: {
        const code = r.readU16();
        return new WsError.Closed(code, r.readStr());
      }
      default:
        throw new WireError({ code: "invalid_tag", tag, at, ty: "WsError" });
    }
  },
};

/** `SseEvent { id: Option<String>, event: String, data: String, retry_ms: Option<u32> }`. */
export const SseEventCodec: Codec<SseEvent> = {
  encode(w, v) {
    optionString.encode(w, v.id);
    w.writeStr(v.event);
    w.writeStr(v.data);
    optionU32.encode(w, v.retryMs);
  },
  decode(r) {
    const id = optionString.decode(r);
    const event = r.readStr();
    const data = r.readStr();
    return { id, event, data, retryMs: optionU32.decode(r) };
  },
};

/** `SseError { Refused { status: Option<u16>, message }, Network(String), Protocol(String), Ended }`. */
export const SseErrorCodec: Codec<SseError> = {
  encode(w, v) {
    if (v instanceof SseError.Refused) {
      w.writeU16(0);
      optionU16.encode(w, v.status);
      w.writeStr(v.message_);
    } else if (v instanceof SseError.Network) {
      w.writeU16(1);
      w.writeStr(v.value);
    } else if (v instanceof SseError.Protocol) {
      w.writeU16(2);
      w.writeStr(v.value);
    } else if (v instanceof SseError.Ended) {
      w.writeU16(3);
    } else {
      throw new TypeError(`unknown SseError variant: ${v.kind}`);
    }
  },
  decode(r) {
    const at = r.position;
    const tag = r.readU16();
    switch (tag) {
      case 0: {
        const status = optionU16.decode(r);
        return new SseError.Refused(status, r.readStr());
      }
      case 1:
        return new SseError.Network(r.readStr());
      case 2:
        return new SseError.Protocol(r.readStr());
      case 3:
        return new SseError.Ended();
      default:
        throw new WireError({ code: "invalid_tag", tag, at, ty: "SseError" });
    }
  },
};

/** `DbMigration { version: u32, sql: String }`. */
export const DbMigrationCodec: Codec<DbMigration> = {
  encode(w, v) {
    w.writeU32(v.version);
    w.writeStr(v.sql);
  },
  decode(r) {
    const version = r.readU32();
    return { version, sql: r.readStr() };
  },
};

/** `DbOpened { db: u32, version: u32 }`. */
export const DbOpenedCodec: Codec<DbOpened> = {
  encode(w, v) {
    w.writeU32(v.db);
    w.writeU32(v.version);
  },
  decode(r) {
    const db = r.readU32();
    return { db, version: r.readU32() };
  },
};

/** `DbValue { Null, Integer(i64), Real(f64), Text(String), Blob(Bytes) }`. */
export const DbValueCodec: Codec<DbValue> = {
  encode(w, v) {
    switch (v.kind) {
      case "null":
        w.writeU16(0);
        break;
      case "integer":
        w.writeU16(1);
        w.writeI64(v.value);
        break;
      case "real":
        w.writeU16(2);
        w.writeF64(v.value);
        break;
      case "text":
        w.writeU16(3);
        w.writeStr(v.value);
        break;
      case "blob":
        w.writeU16(4);
        w.writeBytes(v.value);
        break;
      default:
        throw new TypeError(`unknown DbValue variant: ${String((v as { kind: unknown }).kind)}`);
    }
  },
  decode(r) {
    const at = r.position;
    const tag = r.readU16();
    switch (tag) {
      case 0:
        return { kind: "null" };
      case 1:
        return { kind: "integer", value: r.readI64() };
      case 2:
        return { kind: "real", value: r.readF64() };
      case 3:
        return { kind: "text", value: r.readStr() };
      case 4:
        return { kind: "blob", value: codecs.bytes.decode(r) };
      default:
        throw new WireError({ code: "invalid_tag", tag, at, ty: "DbValue" });
    }
  },
};

/** `DbExecuted { changes: u64, last_insert_id: i64 }`. */
export const DbExecutedCodec: Codec<DbExecuted> = {
  encode(w, v) {
    w.writeU64(v.changes);
    w.writeI64(v.lastInsertId);
  },
  decode(r) {
    const changes = r.readU64();
    return { changes, lastInsertId: r.readI64() };
  },
};

const strings = /* @__PURE__ */ (() => codecs.vec(codecs.string))();
const rows = /* @__PURE__ */ codecs.vec(/* @__PURE__ */ codecs.vec(DbValueCodec));

/** `DbRows { columns: Vec<String>, rows: Vec<Vec<DbValue>> }`. */
export const DbRowsCodec: Codec<DbRows> = {
  encode(w, v) {
    strings.encode(w, v.columns);
    rows.encode(w, v.rows);
  },
  decode(r) {
    const columns = strings.decode(r);
    return { columns, rows: rows.decode(r) };
  },
};

/** `DbConstraint { Unique, NotNull, ForeignKey, Check, Other }`. */
export const DbConstraintCodec: Codec<DbConstraint> = /* @__PURE__ */ unitEnum("DbConstraint", DB_CONSTRAINTS);

/** `DbError { Busy, Constraint { kind, message }, Corrupt(String), Full, Unavailable(String), Sql { message }, Migration { version: u32, message } }`. */
export const DbErrorCodec: Codec<DbError> = {
  encode(w, v) {
    if (v instanceof DbError.Busy) {
      w.writeU16(0);
    } else if (v instanceof DbError.Constraint) {
      w.writeU16(1);
      DbConstraintCodec.encode(w, v.kind_);
      w.writeStr(v.message_);
    } else if (v instanceof DbError.Corrupt) {
      w.writeU16(2);
      w.writeStr(v.value);
    } else if (v instanceof DbError.Full) {
      w.writeU16(3);
    } else if (v instanceof DbError.Unavailable) {
      w.writeU16(4);
      w.writeStr(v.value);
    } else if (v instanceof DbError.Sql) {
      w.writeU16(5);
      w.writeStr(v.message_);
    } else if (v instanceof DbError.Migration) {
      w.writeU16(6);
      w.writeU32(v.version);
      w.writeStr(v.message_);
    } else {
      throw new TypeError(`unknown DbError variant: ${v.kind}`);
    }
  },
  decode(r) {
    const at = r.position;
    const tag = r.readU16();
    switch (tag) {
      case 0:
        return new DbError.Busy();
      case 1: {
        const kind = DbConstraintCodec.decode(r);
        return new DbError.Constraint(kind, r.readStr());
      }
      case 2:
        return new DbError.Corrupt(r.readStr());
      case 3:
        return new DbError.Full();
      case 4:
        return new DbError.Unavailable(r.readStr());
      case 5:
        return new DbError.Sql(r.readStr());
      case 6: {
        const version = r.readU32();
        return new DbError.Migration(version, r.readStr());
      }
      default:
        throw new WireError({ code: "invalid_tag", tag, at, ty: "DbError" });
    }
  },
};
