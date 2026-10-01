// Shared helpers for the wasm boundary tests: a byte writer/reader for the wire format, a
// harness around one instance of the fixture core (crates/undra-ffi/tests/fixture) that records
// every import call, and the ids the tests use.
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));

/** Where `cargo build --manifest-path .../fixture/Cargo.toml --target wasm32-unknown-unknown` puts the module. */
export function fixturePath() {
  if (process.env.UNDRA_FFI_FIXTURE_WASM) return process.env.UNDRA_FFI_FIXTURE_WASM;
  for (const profile of ["release-wasm", "release", "debug"]) {
    const p = resolve(here, `../fixture/target/wasm32-unknown-unknown/${profile}/undra_fixture.wasm`);
    if (existsSync(p)) return p;
  }
  throw new Error("build the fixture first: see crates/undra-ffi/tests/wasm/README.md");
}

export function loadModule(path = fixturePath()) {
  return new WebAssembly.Module(readFileSync(path));
}

export function fnv1a32(text) {
  let h = 0x811c9dc5;
  for (const b of new TextEncoder().encode(text)) {
    h ^= b;
    h = Math.imul(h, 0x01000193) >>> 0;
  }
  return h >>> 0;
}

export function fnv1a64(bytes) {
  let h = 0xcbf29ce484222325n;
  for (const b of bytes) {
    h ^= BigInt(b);
    h = BigInt.asUintN(64, h * 0x100000001b3n);
  }
  return h;
}

export const ids = {
  fn: (name) => fnv1a32(`fn.${name}`),
  type: (name) => fnv1a32(name),
  method: (type, name) => fnv1a32(`${type}.${name}`),
  port: (trait) => fnv1a32(`port.${trait}`),
  portMethod: (trait, name) => fnv1a32(`${trait}.${name}`),
};

/** A little-endian byte writer. */
export class Writer {
  #bytes = [];
  u8(v) {
    this.#bytes.push(v & 0xff);
    return this;
  }
  u32(v) {
    for (let i = 0; i < 4; i++) this.#bytes.push((v >>> (8 * i)) & 0xff);
    return this;
  }
  u64(v) {
    let x = BigInt.asUintN(64, BigInt(v));
    for (let i = 0; i < 8; i++) {
      this.#bytes.push(Number(x & 0xffn));
      x >>= 8n;
    }
    return this;
  }
  str(s) {
    const b = new TextEncoder().encode(s);
    this.u32(b.length);
    for (const x of b) this.#bytes.push(x);
    return this;
  }
  raw(bytes) {
    for (const x of bytes) this.#bytes.push(x);
    return this;
  }
  done() {
    return Uint8Array.from(this.#bytes);
  }
}

/** A little-endian byte reader. */
export class Reader {
  #view;
  #pos = 0;
  constructor(bytes) {
    this.bytes = bytes;
    this.#view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }
  u8() {
    return this.#view.getUint8(this.#pos++);
  }
  u16() {
    const v = this.#view.getUint16(this.#pos, true);
    this.#pos += 2;
    return v;
  }
  u32() {
    const v = this.#view.getUint32(this.#pos, true);
    this.#pos += 4;
    return v;
  }
  i64() {
    const v = this.#view.getBigInt64(this.#pos, true);
    this.#pos += 8;
    return v;
  }
  u64() {
    const v = this.#view.getBigUint64(this.#pos, true);
    this.#pos += 8;
    return v;
  }
  str() {
    const n = this.u32();
    const s = new TextDecoder().decode(this.bytes.subarray(this.#pos, this.#pos + n));
    this.#pos += n;
    return s;
  }
  take(n) {
    const r = this.bytes.subarray(this.#pos, this.#pos + n);
    this.#pos += n;
    return r;
  }
  rest() {
    return this.take(this.bytes.length - this.#pos);
  }
  get remaining() {
    return this.bytes.length - this.#pos;
  }
}

export const Status = { Ok: 0, Error: 1, Panic: 2, Cancelled: 3, StreamOpened: 4, BadRequest: 5 };

export function config({ platform = "test", mode = "inproc", coreThreads = 0, logLevel = 2 } = {}) {
  return new Writer().str(platform).str(mode).u8(coreThreads).u8(0).u8(logLevel).done();
}

/** A Call payload (SPEC 3.3). */
export const call = {
  fn: (methodId, callId, args = []) => new Writer().u8(0).u64(0).u32(methodId).u32(callId).raw(args).done(),
  method: (handle, methodId, callId, args = []) => new Writer().u8(1).u64(handle).u32(methodId).u32(callId).raw(args).done(),
  ctor: (typeId, methodId, callId, args = []) => new Writer().u8(2).u32(typeId).u32(methodId).u32(callId).raw(args).done(),
};

export function parseReply(bytes) {
  const r = new Reader(bytes);
  return { callId: r.u32(), status: r.u8(), body: r.rest() };
}

/** Decodes a ChangeSet payload (SPEC 3.5). */
export function parseChangeSet(bytes) {
  const r = new Reader(bytes);
  const txn = r.u64();
  const count = r.u32();
  const entries = [];
  for (let i = 0; i < count; i++) {
    const handle = r.u64();
    const signalId = r.u32();
    const op = r.u8();
    const len = r.u32();
    entries.push({ handle, signalId, op, value: r.take(len) });
  }
  return { txn, entries, trailing: r.remaining };
}

/** A PortReply payload (SPEC 3.6). */
export function portReply(portCallId, status, body = []) {
  return new Writer().u32(portCallId).u8(status).raw(body).done();
}

/** One instance of the fixture core with every import recorded. */
export class Instance {
  /**
   * `random(view)` fills the bytes the core asked for (default: the deterministic `(i * 7 + 1) & 0xff`); a
   * host without a random source leaves them alone or throws, and the core answers its `Rng` unavailable.
   */
  constructor(module, { nowMs = () => 1_700_000_000_000.5, portCall, random } = {}) {
    this.replies = [];
    this.changes = [];
    this.streams = [];
    this.portCalls = [];
    this.timers = [];
    this.logs = [];
    this.schedules = 0;
    this.nowMs = nowMs;
    this.randomCalls = [];
    this.portCall = portCall ?? (() => 2);
    this.instance = new WebAssembly.Instance(module, {
      undra: {
        reply: (callId, ptr, len) => this.replies.push({ callId, payload: this.copy(ptr, len) }),
        changeset: (ptr, len) => this.changes.push(this.copy(ptr, len)),
        stream: (callId, ptr, len) => this.streams.push({ callId, payload: this.copy(ptr, len) }),
        port_call: (portId, methodId, portCallId, ptr, len) => {
          const call = { portId: portId >>> 0, methodId: methodId >>> 0, portCallId: portCallId >>> 0, args: this.copy(ptr, len) };
          this.portCalls.push(call);
          return this.portCall(call, this);
        },
        schedule: () => {
          this.schedules++;
        },
        timer_set: (timerId, lo, hi) => this.timers.push({ timerId: timerId >>> 0, delayMs: (BigInt(hi >>> 0) << 32n) | BigInt(lo >>> 0) }),
        log: (level, ptr, len) => {
          const bytes = this.copy(ptr, len);
          const r = new Reader(bytes);
          this.logs.push({ level, target: r.str(), message: r.str(), remaining: r.remaining });
        },
        now_ms: () => this.nowMs(),
        random: (ptr, len) => {
          this.randomCalls.push(len >>> 0);
          const view = new Uint8Array(this.x.memory.buffer, ptr >>> 0, len >>> 0);
          if (random) random(view);
          else for (let i = 0; i < view.length; i++) view[i] = (i * 7 + 1) & 0xff;
        },
      },
    });
    this.x = this.instance.exports;
    this.nextCall = 1;
  }

  copy(ptr, len) {
    const start = ptr >>> 0;
    return new Uint8Array(this.x.memory.buffer).slice(start, start + (len >>> 0));
  }

  /** Runs `fn(ptr, len)` with `bytes` copied into wasm memory (freed afterwards). */
  withBytes(bytes, fn) {
    const ptr = this.x.undra_alloc(Math.max(bytes.length, 1));
    new Uint8Array(this.x.memory.buffer).set(bytes, ptr);
    try {
      return fn(ptr, bytes.length);
    } finally {
      this.x.undra_free(ptr, Math.max(bytes.length, 1));
    }
  }

  /** Copies an `UndraBuf*` (`{ptr, len, cap}` i32s) out of memory and frees it. */
  takeBuf(bufPtr) {
    const view = new DataView(this.x.memory.buffer);
    const ptr = view.getUint32(bufPtr, true);
    const len = view.getUint32(bufPtr + 4, true);
    const bytes = this.copy(ptr, len);
    this.x.undra_buf_free(bufPtr);
    return bytes;
  }

  init(options) {
    this.x._initialize?.();
    return this.withBytes(config(options), (p, n) => this.x.undra_init(p, n));
  }

  callId() {
    return this.nextCall++;
  }

  callSync(payload) {
    return parseReply(this.withBytes(payload, (p, n) => this.takeBuf(this.x.undra_call_sync(p, n))));
  }

  submit(payload) {
    return this.withBytes(payload, (p, n) => this.x.undra_call(p, n));
  }

  /** Runs the executor until it stops asking to be polled. */
  drain() {
    for (let i = 0; i < 100 && this.schedules > 0; i++) {
      this.schedules = 0;
      this.x.undra_poll();
    }
  }

  takeReply(callId) {
    const at = this.replies.findIndex((r) => r.callId === callId);
    if (at < 0) return undefined;
    return parseReply(this.replies.splice(at, 1)[0].payload);
  }

  observe(handle, signalId, on) {
    const h = BigInt.asUintN(64, BigInt(handle));
    this.x.undra_observe(Number(h & 0xffff_ffffn), Number(h >> 32n), signalId, on ? 1 : 0);
  }

  release(handle) {
    const h = BigInt.asUintN(64, BigInt(handle));
    this.x.undra_release(Number(h & 0xffff_ffffn), Number(h >> 32n));
  }

  portReplyInto(payload) {
    this.withBytes(payload, (p, n) => this.x.undra_port_reply(p, n));
  }

  construct(type, args = []) {
    const id = this.callId();
    const reply = this.callSync(call.ctor(ids.type(type), ids.method(type, "new"), id, args));
    if (reply.status !== Status.Ok) throw new Error(`${type}.new failed: status ${reply.status}`);
    return new Reader(reply.body).u64();
  }
}
