import { type Codec, WireError, codecs } from "../wire/index.js";
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

/** A unit enum as its `u16` variant index. */
function unitEnum<T extends string>(name: string, variants: readonly T[]): Codec<T> {
  return {
    encode(w, v) {
      const index = variants.indexOf(v);
      if (index < 0) throw new RangeError(`unknown ${name} variant: ${String(v)}`);
      w.writeU16(index);
    },
    decode(r) {
      const at = r.position;
      const tag = r.readU16();
      const variant = variants[tag];
      if (variant === undefined) throw new WireError({ code: "invalid_tag", tag, at, ty: name });
      return variant;
    },
  };
}

/** `HttpMethod`. */
export const HttpMethodCodec: Codec<HttpMethod> = unitEnum("HttpMethod", HTTP_METHODS);
/** `NetKind`. */
export const NetKindCodec: Codec<NetKind> = unitEnum("NetKind", NET_KINDS);
/** `AppState`. */
export const AppStateCodec: Codec<AppState> = unitEnum("AppState", APP_STATES);

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

/** `HttpRequest { method, url, headers, body, timeout_ms }`. */
export const HttpRequestCodec: Codec<HttpRequest> = {
  encode(w, v) {
    HttpMethodCodec.encode(w, v.method);
    w.writeStr(v.url);
    headers.encode(w, v.headers as Header[]);
    optionBytes.encode(w, v.body);
    optionU32.encode(w, v.timeoutMs);
  },
  decode(r) {
    const method = HttpMethodCodec.decode(r);
    const url = r.readStr();
    const hs = headers.decode(r);
    const body = optionBytes.decode(r);
    return { method, url, headers: hs, body, timeoutMs: optionU32.decode(r) };
  },
};

/** `HttpResponse { status: u16, headers, body: Bytes }`. */
export const HttpResponseCodec: Codec<HttpResponse> = {
  encode(w, v) {
    w.writeU16(v.status);
    headers.encode(w, v.headers as Header[]);
    w.writeBytes(v.body);
  },
  decode(r) {
    const status = r.readU16();
    const hs = headers.decode(r);
    return { status, headers: hs, body: codecs.bytes.decode(r) };
  },
};

/** `HttpError { Network(String), Timeout, Cancelled, InvalidUrl(String) }`. */
export const HttpErrorCodec: Codec<HttpError> = {
  encode(w, v) {
    if (v instanceof HttpError.Network) {
      w.writeU16(0);
      w.writeStr(v.value);
    } else if (v instanceof HttpError.Timeout) {
      w.writeU16(1);
    } else if (v instanceof HttpError.Cancelled) {
      w.writeU16(2);
    } else if (v instanceof HttpError.InvalidUrl) {
      w.writeU16(3);
      w.writeStr(v.value);
    } else {
      throw new TypeError(`unknown HttpError variant: ${v.kind}`);
    }
  },
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

/** `FsError { NotFound, Denied, Io(String) }`. */
export const FsErrorCodec: Codec<FsError> = {
  encode(w, v) {
    if (v instanceof FsError.NotFound) {
      w.writeU16(0);
    } else if (v instanceof FsError.Denied) {
      w.writeU16(1);
    } else if (v instanceof FsError.Io) {
      w.writeU16(2);
      w.writeStr(v.value);
    } else {
      throw new TypeError(`unknown FsError variant: ${v.kind}`);
    }
  },
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
      default:
        throw new WireError({ code: "invalid_tag", tag, at, ty: "FsError" });
    }
  },
};

// ---------------------------------------------------------------------------
// The opt-in ports (ADR-047, ADR-048): WebSocket, Sse and Db
// ---------------------------------------------------------------------------

const optionU16 = /* @__PURE__ */ codecs.option(codecs.u16);
const optionString = /* @__PURE__ */ codecs.option(codecs.string);

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

const strings = /* @__PURE__ */ codecs.vec(codecs.string);
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
