import { type Codec, type UndraReader, type UndraWriter, WireError, codecs } from "../wire/index.js";
import {
  APP_STATES,
  type AppState,
  FsError,
  HTTP_METHODS,
  type Header,
  HttpError,
  type HttpMethod,
  type HttpRequest,
  type HttpResponse,
  NET_KINDS,
  type NetKind,
  StorageError,
  type UndraBackgroundReport,
  type UndraPanicFrame,
  type UndraPanicReport,
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
const optionString = codecs.option(codecs.string);

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
  const symbol = optionString.decode(r);
  const file = optionString.decode(r);
  return { address, symbol, file, line: optionU32.decode(r) };
}

/** Writes a panic frame (`PanicFrameCodec.encode`). */
export function writePanicFrame(w: UndraWriter, v: UndraPanicFrame): void {
  w.writeU64(v.address);
  optionString.encode(w, v.symbol);
  optionString.encode(w, v.file);
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
