import { CallTarget, type Handle, encodeCall } from "./wire/index.js";

/*
 * The `Call` header (SPEC 3.3) of a free function or a method, written without a writer, a context object or a closure: what the
 * core's call path (`call`, `callSync`, the direct call, a stream's open) builds for every call. Not part of the package's API.
 */

/**
 * What a call addresses: a free function, or a method of the object behind a
 * handle. (Addition to SPEC 17.1: the wire `CallTarget` enum has no room for
 * the handle, so generated code passes this object.) A bare
 * `CallTarget.FreeFunction` is accepted as shorthand for the first form.
 */
export type CallTargetRef =
  | { readonly target: CallTarget.FreeFunction }
  | { readonly target: CallTarget.ObjectMethod; readonly handle: Handle }
  | PageTarget;

/** The target of a page call (ADR-043): the page server's handle and the rows wanted. */
export type PageTarget = { readonly target: CallTarget.LazyListPage; readonly handle: Handle; readonly offset: number; readonly limit: number };

/** The argument type of `call`, `callSync` and `stream`. */
export type CallTargetArg = CallTargetRef | CallTarget.FreeFunction;

/** The 32-bit halves of the last few handles used by calls: BigInt arithmetic allocates, and a store calls with the same handle again and again. */
const HANDLE_HALVES = 4;
const handleKeys: bigint[] = [];
const handleLo: number[] = [];
const handleHi: number[] = [];
let handleNext = 0;

/** Writes the `u64` `handle` at `out[at..at+8]` (little-endian): from the cache of recent handles, or after splitting it. */
function putHandle(out: Uint8Array, at: number, handle: Handle): void {
  let i = handleKeys.length;
  while (i-- > 0) if (handleKeys[i] === handle) break;
  if (i < 0) {
    if (BigInt.asUintN(64, handle) !== handle) throw new RangeError(`u64 out of range: ${String(handle)}`);
    i = handleNext;
    handleNext = (handleNext + 1) % HANDLE_HALVES;
    handleKeys[i] = handle;
    handleLo[i] = Number(handle & 0xffff_ffffn);
    handleHi[i] = Number(handle >> 32n);
  }
  put32(out, at, handleLo[i] as number);
  put32(out, at + 4, handleHi[i] as number);
}

function put32(out: Uint8Array, at: number, v: number): void {
  out[at] = v;
  out[at + 1] = v >>> 8;
  out[at + 2] = v >>> 16;
  out[at + 3] = v >>> 24;
}

/** The length of the `Call` header of a free function or a method (SPEC 3.3): target u8, handle u64, method id u32, call id u32. */
export const HEAD_LEN = 17;

/** Writes that header into `out` (which holds at least {@link HEAD_LEN} bytes), clearing what an earlier call left in it. */
export function writeHead(out: Uint8Array, target: CallTargetArg, methodId: number, callId: number): void {
  let handle: Handle | undefined;
  if (typeof target === "number") {
    if (target !== CallTarget.FreeFunction) {
      throw new TypeError("a bare CallTarget must be FreeFunction; pass { target, handle } for a method");
    }
  } else if (target.target === CallTarget.ObjectMethod) {
    handle = target.handle;
  }
  if (methodId >>> 0 !== methodId) throw new RangeError(`u32 out of range: ${String(methodId)}`);
  if (handle === undefined) {
    out.fill(0, 0, 9);
  } else {
    out[0] = CallTarget.ObjectMethod;
    putHandle(out, 1, handle);
  }
  put32(out, 9, methodId);
  put32(out, 13, callId);
}

/** A `Call` payload (SPEC 3.3) for a free function or a method, in one allocation: `encodeCall` without its writer. */
export function encodeTarget(target: CallTargetArg, methodId: number, callId: number, args: Uint8Array): Uint8Array {
  if ((target as CallTargetRef).target === CallTarget.LazyListPage) return encodeCall({ ...(target as PageTarget), callId });
  const out = new Uint8Array(HEAD_LEN + args.length);
  writeHead(out, target, methodId, callId);
  if (args.length > 0) out.set(args, HEAD_LEN);
  return out;
}
