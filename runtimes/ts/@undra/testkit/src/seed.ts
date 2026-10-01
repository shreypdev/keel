import { FsError, HttpError, type AppState, type HttpMethod, type HttpResponse, type NetKind } from "@undra/runtime";
import type { Fakes } from "./fakes.js";
import { matches, response } from "./fakes.js";
import { fromHex, utf8 } from "./hex.js";

/** The `version` of the seed document this package reads. */
export const SEED_VERSION = 1;

/** One scripted HTTP rule of a {@link Seed}. */
export interface SeedHttpRule {
  readonly url?: string;
  readonly urlPrefix?: string;
  readonly method?: HttpMethod;
  /** What the rule answers: a response, or an error to fail with. */
  readonly reply: HttpResponse | HttpError;
}

/** The starting state of the fakes (`testkit/fixtures/seed.json` is an example): the same document seeds `undra::ports::fakes` in Rust and the fakes of the Swift and Kotlin kits. Every field is optional. */
export interface Seed {
  readonly nowMs?: number;
  readonly rngSeed?: bigint;
  readonly kv?: ReadonlyArray<readonly [string, Uint8Array]>;
  readonly secureStore?: ReadonlyArray<readonly [string, Uint8Array]>;
  readonly fs?: ReadonlyArray<readonly [string, Uint8Array]>;
  readonly http?: readonly SeedHttpRule[];
  readonly connectivity?: { readonly online: boolean; readonly kind: NetKind };
  readonly lifecycle?: AppState;
}

/** Why a seed could not be read: `path` says where (`http[1].status`). */
export class SeedError extends Error {
  constructor(
    readonly path: string,
    problem: string,
  ) {
    super(`seed ${path}: ${problem}`);
    this.name = "SeedError";
  }
}

const METHODS: readonly HttpMethod[] = ["get", "post", "put", "delete", "patch", "head", "options"];
const KINDS: readonly NetKind[] = ["wifi", "cellular", "wired", "unknown", "none"];
const STATES: readonly AppState[] = ["active", "inactive", "background"];

type Json = unknown;
const isObject = (v: Json): v is Record<string, Json> => typeof v === "object" && v !== null && !Array.isArray(v);

/** A string is its UTF-8 bytes, `{"hex": ".."}` is raw bytes. */
function bytesOf(path: string, v: Json): Uint8Array {
  if (typeof v === "string") return utf8(v);
  if (isObject(v) && typeof v["hex"] === "string") {
    const bytes = fromHex(v["hex"]);
    if (bytes !== null) return bytes;
  }
  throw new SeedError(path, 'must be a string or {"hex": ".."}');
}

function entries(doc: Record<string, Json>, key: string): Array<readonly [string, Uint8Array]> {
  const v = doc[key];
  if (v === undefined) return [];
  if (!isObject(v)) throw new SeedError(key, "must be an object");
  return Object.entries(v).map(([k, value]) => [k, bytesOf(`${key}.${k}`, value)] as const);
}

function named<T extends string>(table: readonly T[], path: string, v: Json): T {
  const found = table.find((n) => n === v);
  if (found === undefined) throw new SeedError(path, `is not one of ${table.join(", ")}`);
  return found;
}

function httpError(path: string, v: Json): HttpError {
  if (v === "timeout") return new HttpError.Timeout();
  if (v === "cancelled") return new HttpError.Cancelled();
  if (isObject(v)) {
    if (typeof v["network"] === "string") return new HttpError.Network(v["network"]);
    if (typeof v["invalid_url"] === "string") return new HttpError.InvalidUrl(v["invalid_url"]);
  }
  throw new SeedError(path, 'must be "timeout", "cancelled", {"network": ..} or {"invalid_url": ..}');
}

function httpRule(i: number, v: Json): SeedHttpRule {
  const at = (field: string): string => `http[${i}].${field}`;
  if (!isObject(v)) throw new SeedError(`http[${i}]`, "must be an object");
  const text = (field: string): string | undefined => {
    const x = v[field];
    if (x === undefined) return undefined;
    if (typeof x !== "string") throw new SeedError(at(field), "must be a string");
    return x;
  };
  const method = v["method"] === undefined ? undefined : named(METHODS, at("method"), v["method"]);
  let reply: HttpResponse | HttpError;
  if (v["error"] !== undefined) reply = httpError(at("error"), v["error"]);
  else {
    let status = 200;
    if (v["status"] !== undefined) {
      const s = v["status"];
      if (typeof s !== "number" || !Number.isInteger(s) || s < 0 || s > 0xffff) throw new SeedError(at("status"), "must be a status code");
      status = s;
    }
    const body = v["body"] === undefined ? new Uint8Array(0) : bytesOf(at("body"), v["body"]);
    const headers: Array<readonly [string, string]> = [];
    if (v["headers"] !== undefined) {
      const list = v["headers"];
      if (!Array.isArray(list)) throw new SeedError(at("headers"), "must be a list of [name, value]");
      for (const h of list) {
        if (!Array.isArray(h) || h.length !== 2 || typeof h[0] !== "string" || typeof h[1] !== "string") {
          throw new SeedError(at("headers"), "must be a list of [name, value]");
        }
        headers.push([h[0], h[1]]);
      }
    }
    reply = response(status, body, headers);
  }
  const url = text("url");
  const urlPrefix = text("url_prefix");
  return {
    ...(url !== undefined && { url }),
    ...(urlPrefix !== undefined && { urlPrefix }),
    ...(method !== undefined && { method }),
    reply,
  };
}

/**
 * Reads a seed document.
 *
 * @throws SeedError naming the path of the first malformed value.
 */
export function parseSeed(text: string): Seed {
  let doc: Json;
  try {
    doc = JSON.parse(text);
  } catch (error) {
    throw new SeedError("$", `is not valid JSON: ${error instanceof Error ? error.message : String(error)}`);
  }
  if (!isObject(doc)) throw new SeedError("$", "must be an object");
  if (doc["version"] !== undefined && doc["version"] !== SEED_VERSION) {
    throw new SeedError("version", `${String(doc["version"])} is not supported (this reader knows ${SEED_VERSION})`);
  }
  let nowMs: number | undefined;
  if (doc["now_ms"] !== undefined) {
    if (typeof doc["now_ms"] !== "number" || !Number.isInteger(doc["now_ms"])) throw new SeedError("now_ms", "must be an integer");
    nowMs = doc["now_ms"];
  }
  let rngSeed: bigint | undefined;
  const rng = doc["rng_seed"];
  if (typeof rng === "number" && Number.isInteger(rng) && rng >= 0) rngSeed = BigInt(rng);
  else if (typeof rng === "string" && /^0x[0-9a-fA-F]+$/.test(rng)) rngSeed = BigInt(rng);
  else if (rng !== undefined) throw new SeedError("rng_seed", 'must be a non-negative integer or a "0x.." string');
  let http: SeedHttpRule[] = [];
  if (doc["http"] !== undefined) {
    if (!Array.isArray(doc["http"])) throw new SeedError("http", "must be a list");
    http = doc["http"].map((r, i) => httpRule(i, r));
  }
  let connectivity: { online: boolean; kind: NetKind } | undefined;
  const c = doc["connectivity"];
  if (c !== undefined) {
    if (!isObject(c) || typeof c["online"] !== "boolean") throw new SeedError("connectivity.online", "must be true or false");
    connectivity = { online: c["online"], kind: named(KINDS, "connectivity.kind", c["kind"]) };
  }
  const lifecycle = doc["lifecycle"] === undefined ? undefined : named(STATES, "lifecycle", doc["lifecycle"]);
  return {
    ...(nowMs !== undefined && { nowMs }),
    ...(rngSeed !== undefined && { rngSeed }),
    kv: entries(doc, "kv"),
    secureStore: entries(doc, "secure_store"),
    fs: entries(doc, "fs"),
    http,
    ...(connectivity !== undefined && { connectivity }),
    ...(lifecycle !== undefined && { lifecycle }),
  };
}

/**
 * Puts `seed` into `fakes`.
 *
 * @throws SeedError for an `fs` path the fake refuses (an empty path, `..`, a file in the way).
 */
export function applySeed(fakes: Fakes, seed: Seed): void {
  if (seed.nowMs !== undefined) fakes.clock.setNowMs(seed.nowMs);
  if (seed.rngSeed !== undefined) fakes.rng.reseed(seed.rngSeed);
  for (const [key, value] of seed.kv ?? []) fakes.kv.insert(key, value);
  for (const [key, value] of seed.secureStore ?? []) fakes.secureStore.insert(key, value);
  for (const [path, contents] of seed.fs ?? []) {
    try {
      fakes.fs.seed(path, contents);
    } catch (error) {
      throw new SeedError(`fs.${path}`, error instanceof FsError ? error.message : String(error));
    }
  }
  for (const rule of seed.http ?? []) {
    let matcher = matches.any();
    if (rule.url !== undefined) matcher = matches.and(matcher, matches.url(rule.url));
    if (rule.urlPrefix !== undefined) matcher = matches.and(matcher, matches.urlPrefix(rule.urlPrefix));
    if (rule.method !== undefined) matcher = matches.and(matcher, matches.method(rule.method));
    fakes.http.respond(matcher, rule.reply);
  }
  if (seed.connectivity !== undefined) fakes.connectivity.set(seed.connectivity.online, seed.connectivity.kind);
  if (seed.lifecycle !== undefined) fakes.lifecycle.set(seed.lifecycle);
}
