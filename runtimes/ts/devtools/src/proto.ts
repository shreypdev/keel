/** The devtools connection's messages (ADR-054), mirroring `undra-transport/src/devtools/proto.rs`. */

import { UndraReader, WireError } from "@undra/runtime/wire";

export const PROTOCOL = 1;

export type Cause =
  | { readonly kind: "other" }
  | { readonly kind: "call"; readonly methodId: number }
  | { readonly kind: "restore"; readonly step: number };

export interface Welcome {
  readonly protocol: number;
  readonly undraVersion: string;
  readonly schemaHash: bigint;
  readonly platform: string;
  readonly mode: string;
  readonly coreEpoch: bigint;
  readonly startedUnixMs: number;
  readonly ringSteps: number;
  readonly ringBytes: number;
  readonly ringStepBytes: number;
  readonly schemaJson: string;
}

export interface StoreRef {
  readonly handle: bigint;
  readonly typeId: number;
}

export interface StepInfo {
  readonly step: number;
  readonly throughSeq: number;
  readonly txn: bigint;
  readonly atMs: number;
  readonly bytes: number;
  readonly stores: number;
  readonly restorable: boolean;
  readonly restoredFrom: number;
}

export type PortRecord =
  | {
      readonly phase: "start";
      readonly id: number;
      readonly portId: number;
      readonly methodId: number;
      readonly atMs: number;
      readonly args: Uint8Array;
    }
  | {
      readonly phase: "end";
      readonly id: number;
      readonly portId: number;
      readonly methodId: number;
      readonly atMs: number;
      /** 0 ok, 1 typed error, 2 unavailable. */
      readonly status: number;
      readonly latencyUs: number;
      readonly reply: Uint8Array;
    };

export interface Traveled {
  readonly requestId: number;
  readonly ok: boolean;
  readonly step: number;
  readonly dropped: number;
  readonly message: string;
}

export type ServerMsg =
  | { readonly t: "welcome"; readonly welcome: Welcome }
  | { readonly t: "stores"; readonly stores: readonly StoreRef[] }
  | {
      readonly t: "changeSet";
      readonly seq: number;
      readonly atMs: number;
      readonly delivery: "commit" | "initial";
      readonly cause: Cause;
      readonly payload: Uint8Array;
    }
  | { readonly t: "step"; readonly step: StepInfo }
  | { readonly t: "evicted"; readonly belowStep: number }
  | { readonly t: "port"; readonly record: PortRecord }
  | { readonly t: "stats"; readonly json: string }
  | { readonly t: "queries"; readonly atMs: number; readonly json: string }
  | { readonly t: "traveled"; readonly result: Traveled }
  | { readonly t: "app"; readonly connected: boolean; readonly platform: string };

const tag = (at: number, value: number, ty: string): WireError => new WireError({ code: "invalid_tag", tag: value, at, ty });

function readCause(r: UndraReader): Cause {
  const at = r.position;
  const kind = r.readU8();
  const arg = r.readU32();
  if (kind === 0) return { kind: "other" };
  if (kind === 1) return { kind: "call", methodId: arg };
  if (kind === 2) return { kind: "restore", step: arg };
  throw tag(at, kind, "Cause");
}

/** Decodes one message of the server. Throws a `WireError` for anything malformed. */
export function decodeServerMsg(bytes: Uint8Array): ServerMsg {
  const r = new UndraReader(bytes);
  const at = r.position;
  const kind = r.readU8();
  let msg: ServerMsg;
  switch (kind) {
    case 1:
      msg = {
        t: "welcome",
        welcome: {
          protocol: r.readU16(),
          undraVersion: r.readStr(),
          schemaHash: r.readU64(),
          platform: r.readStr(),
          mode: r.readStr(),
          coreEpoch: r.readU64(),
          startedUnixMs: r.readU64Number(),
          ringSteps: r.readU32(),
          ringBytes: r.readU64Number(),
          ringStepBytes: r.readU64Number(),
          schemaJson: r.readStr(),
        },
      };
      break;
    case 2: {
      const count = r.readLen(12);
      const stores: StoreRef[] = [];
      for (let i = 0; i < count; i++) stores.push({ handle: r.readU64(), typeId: r.readU32() });
      msg = { t: "stores", stores };
      break;
    }
    case 3: {
      const seq = r.readU64Number();
      const atMs = r.readU64Number();
      const d = r.readU8();
      if (d > 1) throw tag(r.position - 1, d, "Delivery");
      msg = {
        t: "changeSet",
        seq,
        atMs,
        delivery: d === 0 ? "commit" : "initial",
        cause: readCause(r),
        payload: r.readBytes(),
      };
      break;
    }
    case 4:
      msg = {
        t: "step",
        step: {
          step: r.readU32(),
          throughSeq: r.readU64Number(),
          txn: r.readU64(),
          atMs: r.readU64Number(),
          bytes: r.readU32(),
          stores: r.readU32(),
          restorable: r.readBool(),
          restoredFrom: r.readU32(),
        },
      };
      break;
    case 5:
      msg = { t: "evicted", belowStep: r.readU32() };
      break;
    case 6: {
      const phase = r.readU8();
      const id = r.readU32();
      const portId = r.readU32();
      const methodId = r.readU32();
      const atMs = r.readU64Number();
      if (phase === 0) {
        msg = { t: "port", record: { phase: "start", id, portId, methodId, atMs, args: r.readBytes() } };
      } else if (phase === 1) {
        const status = r.readU8();
        const latencyUs = r.readU64Number();
        msg = { t: "port", record: { phase: "end", id, portId, methodId, atMs, status, latencyUs, reply: r.readBytes() } };
      } else {
        throw tag(r.position - 1, phase, "PortRecord");
      }
      break;
    }
    case 7:
      msg = { t: "stats", json: r.readStr() };
      break;
    case 8:
      msg = { t: "queries", atMs: r.readU64Number(), json: r.readStr() };
      break;
    case 9:
      msg = {
        t: "traveled",
        result: {
          requestId: r.readU32(),
          ok: r.readBool(),
          step: r.readU32(),
          dropped: r.readU32(),
          message: r.readStr(),
        },
      };
      break;
    case 10:
      msg = { t: "app", connected: r.readBool(), platform: r.readStr() };
      break;
    default:
      throw tag(at, kind, "ServerMsg");
  }
  r.finish();
  return msg;
}

/** `Restore { request_id, step }`. */
export function encodeRestore(requestId: number, step: number): Uint8Array {
  const out = new Uint8Array(9);
  const view = new DataView(out.buffer);
  out[0] = 1;
  view.setUint32(1, requestId, true);
  view.setUint32(5, step, true);
  return out;
}

/** `Resync`. */
export function encodeResync(): Uint8Array {
  return Uint8Array.of(2);
}
