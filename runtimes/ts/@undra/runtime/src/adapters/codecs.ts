import { type Codec, WireError, codecs } from "../wire/index.js";
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
