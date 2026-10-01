import { fromHex, toHex } from "./hex.js";
import { standardName } from "./names.js";

/** The `format` of a recording. */
export const RECORDING_FORMAT = "undra.recording";
/** The `version` this package reads and writes. */
export const RECORDING_VERSION = 1;

/** Which call a `call` event is (docs/SPEC.md section 3.3). Handles are bigints. */
export type RecordedTarget =
  | { readonly kind: "function"; readonly method: number }
  | { readonly kind: "method"; readonly handle: bigint; readonly method: number }
  | { readonly kind: "constructor"; readonly type: number; readonly method: number }
  | { readonly kind: "page"; readonly handle: bigint; readonly offset: number; readonly limit: number };

/** The status of a recorded `reply`. */
export type ReplyStatusName = "ok" | "error" | "panic" | "cancelled" | "stream_opened" | "bad_request";
/** The status of a recorded `port_reply`. */
export type PortStatusName = "ok" | "error" | "unavailable";
/** What a recorded `stream_item` carries. */
export type StreamFlagName = "item" | "end" | "error" | "failed";
/** How a recorded change-set entry's value reads. */
export type ChangeOpName = "full" | "patch" | "lazy_invalidated";

/** One signal update of a recorded `change_set`. */
export interface RecordedEntry {
  readonly handle: bigint;
  readonly signal: number;
  readonly op: ChangeOpName;
  readonly value: Uint8Array;
}

/** What happened (the fields beyond `t`). */
export type RecordedKind =
  | { readonly kind: "call"; readonly target: RecordedTarget; readonly call: number; readonly args: Uint8Array }
  | { readonly kind: "reply"; readonly call: number; readonly status: ReplyStatusName; readonly body: Uint8Array }
  | { readonly kind: "change_set"; readonly txn: number; readonly entries: readonly RecordedEntry[] }
  | { readonly kind: "stream_item"; readonly call: number; readonly flag: StreamFlagName; readonly body: Uint8Array }
  | { readonly kind: "port_call"; readonly port: number; readonly method: number; readonly call: number; readonly args: Uint8Array }
  | { readonly kind: "port_reply"; readonly call: number; readonly status: PortStatusName; readonly body: Uint8Array }
  | { readonly kind: "event"; readonly port: number; readonly method: number; readonly payload: Uint8Array }
  | { readonly kind: "timer_fired"; readonly timer: number }
  | { readonly kind: "observe"; readonly handle: bigint; readonly signal: number; readonly on: boolean }
  | { readonly kind: "release"; readonly handle: bigint }
  | { readonly kind: "cancel"; readonly call: number };

/** One event and when it happened: whole milliseconds since the session started. */
export type RecordedEvent = RecordedKind & { readonly t: number };

/** A recorded session (`undra.recording`, version 1). */
export interface Recording {
  /** The schema hash of the core the bytes belong to. */
  readonly schemaHash: bigint;
  /** Where it came from, informational: `"dev-server"`, `"adapters"`, `"test"`, `"hand"`. */
  readonly source: string;
  /** The host platform, informational. */
  readonly platform?: string;
  /** The events, in the order they happened. */
  readonly events: readonly RecordedEvent[];
}

/** Why a recording could not be read: names the event (`undefined` for the document header) and the field. */
export class RecordingError extends Error {
  constructor(
    message: string,
    readonly event?: number,
    readonly field?: string,
  ) {
    super(message);
    this.name = "RecordingError";
  }
}

const REPLY_STATUSES: readonly ReplyStatusName[] = ["ok", "error", "panic", "cancelled", "stream_opened", "bad_request"];
const PORT_STATUSES: readonly PortStatusName[] = ["ok", "error", "unavailable"];
const STREAM_FLAGS: readonly StreamFlagName[] = ["item", "end", "error", "failed"];
const CHANGE_OPS: readonly ChangeOpName[] = ["full", "patch", "lazy_invalidated"];

const hex64 = (value: bigint): string => `0x${value.toString(16).padStart(16, "0")}`;

/** A JSON string escaped only where JSON requires, so every language's writer gives the same bytes. */
function jsonString(text: string): string {
  let out = '"';
  for (const ch of text) {
    const code = ch.codePointAt(0)!;
    if (ch === '"') out += '\\"';
    else if (ch === "\\") out += "\\\\";
    else if (ch === "\n") out += "\\n";
    else if (ch === "\r") out += "\\r";
    else if (ch === "\t") out += "\\t";
    else if (code < 0x20) out += `\\u${code.toString(16).padStart(4, "0")}`;
    else out += ch;
  }
  return `${out}"`;
}

function nameField(port: number, method: number): string {
  const name = standardName(port, method);
  return name === undefined ? "" : `,"name":${jsonString(name)}`;
}

/** One event as a line of canonical JSON. */
export function eventToJsonLine(e: RecordedEvent): string {
  const head = `{"t":${e.t}`;
  switch (e.kind) {
    case "call": {
      const t = e.target;
      let target: string;
      switch (t.kind) {
        case "function":
          target = `"target":"function","method":${t.method}`;
          break;
        case "method":
          target = `"target":"method","handle":"${hex64(t.handle)}","method":${t.method}`;
          break;
        case "constructor":
          target = `"target":"constructor","type":${t.type},"method":${t.method}`;
          break;
        case "page":
          target = `"target":"page","handle":"${hex64(t.handle)}","offset":${t.offset},"limit":${t.limit}`;
          break;
      }
      return `${head},"kind":"call",${target},"call":${e.call},"args":"${toHex(e.args)}"}`;
    }
    case "reply":
      return `${head},"kind":"reply","call":${e.call},"status":"${e.status}","body":"${toHex(e.body)}"}`;
    case "change_set": {
      const entries = e.entries
        .map((x) => `{"handle":"${hex64(x.handle)}","signal":${x.signal},"op":"${x.op}","value":"${toHex(x.value)}"}`)
        .join(",");
      return `${head},"kind":"change_set","txn":${e.txn},"entries":[${entries}]}`;
    }
    case "stream_item":
      return `${head},"kind":"stream_item","call":${e.call},"flag":"${e.flag}","body":"${toHex(e.body)}"}`;
    case "port_call":
      return `${head},"kind":"port_call","port":${e.port},"method":${e.method},"call":${e.call},"args":"${toHex(e.args)}"${nameField(e.port, e.method)}}`;
    case "port_reply":
      return `${head},"kind":"port_reply","call":${e.call},"status":"${e.status}","body":"${toHex(e.body)}"}`;
    case "event":
      return `${head},"kind":"event","port":${e.port},"method":${e.method},"payload":"${toHex(e.payload)}"${nameField(e.port, e.method)}}`;
    case "timer_fired":
      return `${head},"kind":"timer_fired","timer":${e.timer}}`;
    case "observe":
      return `${head},"kind":"observe","handle":"${hex64(e.handle)}","signal":${e.signal},"on":${e.on}}`;
    case "release":
      return `${head},"kind":"release","handle":"${hex64(e.handle)}"}`;
    case "cancel":
      return `${head},"kind":"cancel","call":${e.call}}`;
  }
}

/** The canonical JSON text of a recording (ends with a newline): equal recordings are equal bytes, and it is the same text the Rust, Swift and Kotlin writers produce. */
export function writeRecording(recording: Recording): string {
  let out = `{\n  "format": "${RECORDING_FORMAT}",\n  "version": ${RECORDING_VERSION},\n  "schema_hash": "${hex64(recording.schemaHash)}",\n  "source": ${jsonString(recording.source)},\n`;
  if (recording.platform !== undefined) out += `  "platform": ${jsonString(recording.platform)},\n`;
  if (recording.events.length === 0) return `${out}  "events": []\n}\n`;
  out += '  "events": [\n';
  out += recording.events.map((e) => `    ${eventToJsonLine(e)}`).join(",\n");
  return `${out}\n  ]\n}\n`;
}

type Json = unknown;

class Fields {
  constructor(
    private readonly index: number | undefined,
    private readonly obj: Record<string, Json>,
  ) {}

  bad(field: string, problem: string): RecordingError {
    const where = this.index === undefined ? "" : `event ${this.index}: `;
    return new RecordingError(`${where}"${field}" ${problem}`, this.index, field);
  }

  int(field: string): number {
    const v = this.obj[field];
    if (typeof v !== "number" || !Number.isInteger(v) || v < 0) throw this.bad(field, "must be a non-negative integer");
    return v;
  }

  u32(field: string): number {
    const v = this.int(field);
    if (v > 0xffff_ffff) throw this.bad(field, "does not fit a u32");
    return v;
  }

  str(field: string): string {
    const v = this.obj[field];
    if (typeof v !== "string") throw this.bad(field, "must be a string");
    return v;
  }

  handle(field: string): bigint {
    const v = this.str(field);
    if (!/^0x[0-9a-fA-F]+$/.test(v)) throw this.bad(field, 'must be a "0x.." string');
    return BigInt(v);
  }

  bytes(field: string): Uint8Array {
    const v = fromHex(this.str(field));
    if (v === null) throw this.bad(field, "must be hex");
    return v;
  }

  bool(field: string): boolean {
    const v = this.obj[field];
    if (typeof v !== "boolean") throw this.bad(field, "must be true or false");
    return v;
  }

  oneOf<T extends string>(field: string, names: readonly T[], what: string): T {
    const v = this.str(field);
    const found = names.find((n) => n === v);
    if (found === undefined) throw this.bad(field, `is not ${what}`);
    return found;
  }
}

function parseEvent(index: number, raw: Json): RecordedEvent {
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) throw new RecordingError(`event ${index} must be an object`, index);
  const f = new Fields(index, raw as Record<string, Json>);
  const t = f.int("t");
  const kind = f.str("kind");
  switch (kind) {
    case "call": {
      const targetName = f.str("target");
      let target: RecordedTarget;
      if (targetName === "function") target = { kind: "function", method: f.u32("method") };
      else if (targetName === "method") target = { kind: "method", handle: f.handle("handle"), method: f.u32("method") };
      else if (targetName === "constructor") target = { kind: "constructor", type: f.u32("type"), method: f.u32("method") };
      else if (targetName === "page") target = { kind: "page", handle: f.handle("handle"), offset: f.u32("offset"), limit: f.u32("limit") };
      else throw f.bad("target", "is not function, method, constructor or page");
      return { t, kind, target, call: f.u32("call"), args: f.bytes("args") };
    }
    case "reply":
      return { t, kind, call: f.u32("call"), status: f.oneOf("status", REPLY_STATUSES, "a reply status"), body: f.bytes("body") };
    case "change_set": {
      const list = (raw as Record<string, Json>)["entries"];
      if (!Array.isArray(list)) throw f.bad("entries", "must be an array");
      const entries = list.map((e): RecordedEntry => {
        const ef = new Fields(index, e as Record<string, Json>);
        return { handle: ef.handle("handle"), signal: ef.u32("signal"), op: ef.oneOf("op", CHANGE_OPS, "full, patch or lazy_invalidated"), value: ef.bytes("value") };
      });
      return { t, kind, txn: f.int("txn"), entries };
    }
    case "stream_item":
      return { t, kind, call: f.u32("call"), flag: f.oneOf("flag", STREAM_FLAGS, "item, end, error or failed"), body: f.bytes("body") };
    case "port_call":
      return { t, kind, port: f.u32("port"), method: f.u32("method"), call: f.u32("call"), args: f.bytes("args") };
    case "port_reply":
      return { t, kind, call: f.u32("call"), status: f.oneOf("status", PORT_STATUSES, "ok, error or unavailable"), body: f.bytes("body") };
    case "event":
      return { t, kind, port: f.u32("port"), method: f.u32("method"), payload: f.bytes("payload") };
    case "timer_fired":
      return { t, kind, timer: f.u32("timer") };
    case "observe":
      return { t, kind, handle: f.handle("handle"), signal: f.u32("signal"), on: f.bool("on") };
    case "release":
      return { t, kind, handle: f.handle("handle") };
    case "cancel":
      return { t, kind, call: f.u32("call") };
    default:
      throw f.bad("kind", "is not a known event kind");
  }
}

/**
 * Reads a recording.
 *
 * @throws RecordingError for text that is not JSON, another `format`, a `version` this package does not read,
 *   or a field that is missing or malformed (named, with the event's index).
 */
export function parseRecording(text: string): Recording {
  let doc: Json;
  try {
    doc = JSON.parse(text);
  } catch (error) {
    throw new RecordingError(`the recording is not valid JSON: ${error instanceof Error ? error.message : String(error)}`);
  }
  if (typeof doc !== "object" || doc === null || Array.isArray(doc)) throw new RecordingError("the recording must be a JSON object");
  const obj = doc as Record<string, Json>;
  if (obj["format"] !== RECORDING_FORMAT) {
    throw new RecordingError(`not a recording: format is ${JSON.stringify(obj["format"] ?? "")}, expected "${RECORDING_FORMAT}"`);
  }
  if (obj["version"] !== RECORDING_VERSION) {
    throw new RecordingError(`recording version ${String(obj["version"])} is not supported (this reader knows version ${RECORDING_VERSION})`);
  }
  const head = new Fields(undefined, obj);
  const schemaHash = head.handle("schema_hash");
  const source = head.str("source");
  const platform = obj["platform"];
  if (platform !== undefined && platform !== null && typeof platform !== "string") throw head.bad("platform", "must be a string");
  const events = obj["events"];
  if (!Array.isArray(events)) throw head.bad("events", "must be an array");
  const parsed = events.map((e, i) => parseEvent(i, e));
  return typeof platform === "string" ? { schemaHash, source, platform, events: parsed } : { schemaHash, source, events: parsed };
}
