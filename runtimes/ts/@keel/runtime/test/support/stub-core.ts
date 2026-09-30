import wabt from "wabt";

/*
 * A stub Keel core written in WebAssembly text: just enough of the section 7
 * ABI to drive `WasmMainTransport` from tests. It is not a Keel runtime; each
 * `method_id` selects a canned behaviour, and exported globals expose what the
 * host did to it.
 *
 * Methods (functions and methods, `target` 0 or 1):
 *   1  ECHO           replies at once with the arguments
 *   2  ECHO_ASYNC     replies from `keel_poll` (asks for it through `schedule`)
 *   3  PORT           calls the host port (PORT_ID, PORT_METHOD) with the arguments; replies with the
 *                     complete PortReply payload it got back (immediately, or when the async reply arrives)
 *   4  TIMER          asks for timer TIMER_ID after 50 ms; replies with the id when it fires
 *   5  LOG            logs the arguments at level 3 and replies empty
 *   6  RANDOM_NOW     replies with 8 bytes from `random` and `now_ms` as an f64
 *   7  STREAM         replies status 4, then one item with the arguments, then the end
 *   8  PANIC          logs at level 5 and traps
 *  any other          status 5 "unknown method"
 * Constructors (`target` 2) reply with the handle HANDLE. `call_id == 0` is refused with 5.
 * `keel_observe(on)` delivers one entry (value 42) for the signal, or signal 0 for ALL_SIGNALS.
 */

export const STUB = {
  SCHEMA_HASH: 0x0123_4567_89ab_cdefn,
  PORT_ID: 0xc0de_c0de,
  PORT_METHOD: 0xdead_beef,
  TIMER_ID: 0x8000_0007,
  HANDLE: 0x0000_0001_0000_0007n,
  ECHO: 1,
  ECHO_ASYNC: 2,
  PORT: 3,
  TIMER: 4,
  LOG: 5,
  RANDOM_NOW: 6,
  STREAM: 7,
  PANIC: 8,
} as const;

/** The stub's exported globals, read as numbers. */
export interface StubGlobals {
  credit_total: WebAssembly.Global;
  cancel_last: WebAssembly.Global;
  cancel_count: WebAssembly.Global;
  release_count: WebAssembly.Global;
  release_lo: WebAssembly.Global;
  release_hi: WebAssembly.Global;
  free_count: WebAssembly.Global;
  alloc_count: WebAssembly.Global;
  poll_count: WebAssembly.Global;
  initialized: WebAssembly.Global;
  init_len: WebAssembly.Global;
  event_port: WebAssembly.Global;
  event_method: WebAssembly.Global;
  event_len: WebAssembly.Global;
  buf_free_count: WebAssembly.Global;
}

/** `s` as a WAT string literal with every byte escaped. */
function watString(s: string): string {
  return [...new TextEncoder().encode(s)].map((b) => `\\${b.toString(16).padStart(2, "0")}`).join("");
}

const STATS_JSON = '{"live_handles":3,"platform":"stub","mode":"inproc"}';

/** Options of {@link compileStub}. */
export interface StubOptions {
  readonly schemaHash?: bigint;
  readonly abiVersion?: number;
  /** What `keel_init` returns. */
  readonly initResult?: number;
}

/** The WAT source of the stub. */
export function stubWat(options: StubOptions = {}): string {
  const hash = BigInt.asIntN(64, options.schemaHash ?? STUB.SCHEMA_HASH);
  const abi = options.abiVersion ?? 1;
  const initRc = options.initResult ?? 0;
  const statsLen = new TextEncoder().encode(STATS_JSON).length;
  return `(module
  (import "keel" "reply" (func $reply (param i32 i32 i32)))
  (import "keel" "changeset" (func $changeset (param i32 i32)))
  (import "keel" "stream" (func $stream (param i32 i32 i32)))
  (import "keel" "port_call" (func $port_call (param i32 i32 i32 i32 i32) (result i32)))
  (import "keel" "schedule" (func $schedule))
  (import "keel" "timer_set" (func $timer_set (param i32 i32 i32)))
  (import "keel" "log" (func $log (param i32 i32 i32)))
  (import "keel" "now_ms" (func $now_ms (result f64)))
  (import "keel" "random" (func $random (param i32 i32)))

  (memory (export "memory") 2)

  ;; Strings: "unknown method" at 0x300 (18 bytes with its length prefix), "port unavailable" at 0x320 (20 bytes), stats JSON at 0x340.
  (data (i32.const 0x300) "\\0e\\00\\00\\00unknown method")
  (data (i32.const 0x320) "\\10\\00\\00\\00port unavailable")
  (data (i32.const 0x340) "${watString(STATS_JSON)}")

  (global $heap (mut i32) (i32.const 65536))
  (global $credit_total (export "credit_total") (mut i32) (i32.const 0))
  (global $cancel_last (export "cancel_last") (mut i32) (i32.const 0))
  (global $cancel_count (export "cancel_count") (mut i32) (i32.const 0))
  (global $release_count (export "release_count") (mut i32) (i32.const 0))
  (global $release_lo (export "release_lo") (mut i32) (i32.const 0))
  (global $release_hi (export "release_hi") (mut i32) (i32.const 0))
  (global $free_count (export "free_count") (mut i32) (i32.const 0))
  (global $alloc_count (export "alloc_count") (mut i32) (i32.const 0))
  (global $poll_count (export "poll_count") (mut i32) (i32.const 0))
  (global $initialized (export "initialized") (mut i32) (i32.const 0))
  (global $init_len (export "init_len") (mut i32) (i32.const -1))
  (global $event_port (export "event_port") (mut i32) (i32.const 0))
  (global $event_method (export "event_method") (mut i32) (i32.const 0))
  (global $event_len (export "event_len") (mut i32) (i32.const -1))
  (global $buf_free_count (export "buf_free_count") (mut i32) (i32.const 0))
  (global $async_call (mut i32) (i32.const 0))
  (global $async_ptr (mut i32) (i32.const 0))
  (global $async_len (mut i32) (i32.const 0))
  (global $port_wait_call (mut i32) (i32.const 0))
  (global $port_reply_ptr (mut i32) (i32.const 0))
  (global $port_reply_len (mut i32) (i32.const 0))
  (global $timer_call (mut i32) (i32.const 0))

  (func $align8 (param $n i32) (result i32)
    (i32.and (i32.add (local.get $n) (i32.const 7)) (i32.const -8)))

  (func $alloc (export "keel_alloc") (param $len i32) (result i32)
    (local $ptr i32) (local $end i32) (local $have i32)
    (global.set $alloc_count (i32.add (global.get $alloc_count) (i32.const 1)))
    (local.set $ptr (global.get $heap))
    (local.set $end (i32.add (local.get $ptr) (call $align8 (local.get $len))))
    (local.set $have (i32.mul (memory.size) (i32.const 65536)))
    (if (i32.gt_u (local.get $end) (local.get $have))
      (then
        (drop (memory.grow
          (i32.div_u (i32.add (i32.sub (local.get $end) (local.get $have)) (i32.const 65535)) (i32.const 65536))))))
    (global.set $heap (local.get $end))
    (local.get $ptr))

  (func (export "keel_free") (param $ptr i32) (param $len i32)
    (global.set $free_count (i32.add (global.get $free_count) (i32.const 1))))

  (func (export "_initialize") (global.set $initialized (i32.const 1)))
  (func (export "keel_abi_version") (result i32) (i32.const ${abi}))
  (func (export "keel_schema_hash") (result i64) (i64.const ${hash}))

  (func (export "keel_init") (param $ptr i32) (param $len i32) (result i32)
    (global.set $init_len (local.get $len))
    (memory.copy (i32.const 0x400) (local.get $ptr)
      (select (local.get $len) (i32.const 64) (i32.lt_u (local.get $len) (i32.const 64))))
    (i32.const ${initRc}))

  ;; A Reply payload (call_id u32, status u8, body) in fresh memory; returns its address, its length is body_len + 5.
  (func $build_reply (param $call i32) (param $status i32) (param $body i32) (param $len i32) (result i32)
    (local $p i32)
    (local.set $p (call $alloc (i32.add (local.get $len) (i32.const 5))))
    (i32.store (local.get $p) (local.get $call))
    (i32.store8 (i32.add (local.get $p) (i32.const 4)) (local.get $status))
    (memory.copy (i32.add (local.get $p) (i32.const 5)) (local.get $body) (local.get $len))
    (local.get $p))

  (func $send_reply (param $call i32) (param $status i32) (param $body i32) (param $len i32)
    (call $reply (local.get $call)
      (call $build_reply (local.get $call) (local.get $status) (local.get $body) (local.get $len))
      (i32.add (local.get $len) (i32.const 5))))

  (func $send_item (param $call i32) (param $flag i32) (param $body i32) (param $len i32)
    (call $stream (local.get $call)
      (call $build_reply (local.get $call) (local.get $flag) (local.get $body) (local.get $len))
      (i32.add (local.get $len) (i32.const 5))))

  (func (export "keel_call") (param $ptr i32) (param $len i32) (result i32)
    (local $method i32) (local $call i32) (local $args i32) (local $alen i32) (local $rc i32) (local $tmp i32)
    (if (i32.eq (i32.load8_u (local.get $ptr)) (i32.const 2))
      (then
        (local.set $method (i32.load offset=5 (local.get $ptr)))
        (local.set $call (i32.load offset=9 (local.get $ptr)))
        (local.set $args (i32.add (local.get $ptr) (i32.const 13)))
        (local.set $alen (i32.sub (local.get $len) (i32.const 13)))
        (if (i32.eqz (local.get $call)) (then (return (i32.const 5))))
        (i64.store (i32.const 0x500) (i64.const ${STUB.HANDLE}))
        (call $send_reply (local.get $call) (i32.const 0) (i32.const 0x500) (i32.const 8))
        (return (i32.const 0))))
    (local.set $method (i32.load offset=9 (local.get $ptr)))
    (local.set $call (i32.load offset=13 (local.get $ptr)))
    (local.set $args (i32.add (local.get $ptr) (i32.const 17)))
    (local.set $alen (i32.sub (local.get $len) (i32.const 17)))
    (if (i32.eqz (local.get $call)) (then (return (i32.const 5))))

    (if (i32.eq (local.get $method) (i32.const ${STUB.ECHO}))
      (then
        (call $send_reply (local.get $call) (i32.const 0) (local.get $args) (local.get $alen))
        (return (i32.const 0))))

    (if (i32.eq (local.get $method) (i32.const ${STUB.ECHO_ASYNC}))
      (then
        (local.set $tmp (call $alloc (local.get $alen)))
        (memory.copy (local.get $tmp) (local.get $args) (local.get $alen))
        (global.set $async_call (local.get $call))
        (global.set $async_ptr (local.get $tmp))
        (global.set $async_len (local.get $alen))
        (call $schedule)
        (return (i32.const 0))))

    (if (i32.eq (local.get $method) (i32.const ${STUB.PORT}))
      (then
        (local.set $rc (call $port_call (i32.const ${STUB.PORT_ID}) (i32.const ${STUB.PORT_METHOD}) (i32.const 1) (local.get $args) (local.get $alen)))
        (if (i32.eqz (local.get $rc))
          (then
            (call $send_reply (local.get $call) (i32.const 0) (global.get $port_reply_ptr) (global.get $port_reply_len))
            (return (i32.const 0))))
        (if (i32.eq (local.get $rc) (i32.const 1))
          (then
            (global.set $port_wait_call (local.get $call))
            (return (i32.const 0))))
        (call $send_reply (local.get $call) (i32.const 5) (i32.const 0x320) (i32.const 20))
        (return (i32.const 0))))

    (if (i32.eq (local.get $method) (i32.const ${STUB.TIMER}))
      (then
        (global.set $timer_call (local.get $call))
        (call $timer_set (i32.const ${STUB.TIMER_ID}) (i32.const 50) (i32.const 0))
        (return (i32.const 0))))

    (if (i32.eq (local.get $method) (i32.const ${STUB.LOG}))
      (then
        (call $log (i32.const 3) (local.get $args) (local.get $alen))
        (call $send_reply (local.get $call) (i32.const 0) (i32.const 0) (i32.const 0))
        (return (i32.const 0))))

    (if (i32.eq (local.get $method) (i32.const ${STUB.RANDOM_NOW}))
      (then
        (call $random (i32.const 0x500) (i32.const 8))
        (f64.store (i32.const 0x508) (call $now_ms))
        (call $send_reply (local.get $call) (i32.const 0) (i32.const 0x500) (i32.const 16))
        (return (i32.const 0))))

    (if (i32.eq (local.get $method) (i32.const ${STUB.STREAM}))
      (then
        (call $send_reply (local.get $call) (i32.const 4) (i32.const 0) (i32.const 0))
        (call $send_item (local.get $call) (i32.const 0) (local.get $args) (local.get $alen))
        (call $send_item (local.get $call) (i32.const 1) (i32.const 0) (i32.const 0))
        (return (i32.const 0))))

    (if (i32.eq (local.get $method) (i32.const ${STUB.PANIC}))
      (then
        (call $log (i32.const 5) (i32.const 0x304) (i32.const 14))
        unreachable))

    (call $send_reply (local.get $call) (i32.const 5) (i32.const 0x300) (i32.const 18))
    (i32.const 0))

  ;; KeelBuf at 0x100 { ptr, len, cap }.
  (func $keelbuf (param $ptr i32) (param $len i32) (result i32)
    (i32.store (i32.const 0x100) (local.get $ptr))
    (i32.store (i32.const 0x104) (local.get $len))
    (i32.store (i32.const 0x108) (local.get $len))
    (i32.const 0x100))

  (func (export "keel_call_sync") (param $ptr i32) (param $len i32) (result i32)
    (local $method i32) (local $call i32)
    (local.set $method (i32.load offset=9 (local.get $ptr)))
    (local.set $call (i32.load offset=13 (local.get $ptr)))
    (if (i32.eq (local.get $method) (i32.const ${STUB.ECHO}))
      (then
        (return (call $keelbuf
          (call $build_reply (local.get $call) (i32.const 0)
            (i32.add (local.get $ptr) (i32.const 17)) (i32.sub (local.get $len) (i32.const 17)))
          (i32.sub (local.get $len) (i32.const 12))))))
    (call $keelbuf
      (call $build_reply (local.get $call) (i32.const 5) (i32.const 0x300) (i32.const 18))
      (i32.const 23)))

  (func (export "keel_cancel") (param $id i32)
    (global.set $cancel_last (local.get $id))
    (global.set $cancel_count (i32.add (global.get $cancel_count) (i32.const 1))))

  (func (export "keel_stream_credit") (param $id i32) (param $credit i32)
    (global.set $credit_total (i32.add (global.get $credit_total) (local.get $credit))))

  (func (export "keel_observe") (param $lo i32) (param $hi i32) (param $signal i32) (param $on i32)
    (if (i32.eqz (local.get $on)) (then (return)))
    (i64.store (i32.const 0x600) (i64.const 1))
    (i32.store (i32.const 0x608) (i32.const 1))
    (i32.store (i32.const 0x60c) (local.get $lo))
    (i32.store (i32.const 0x610) (local.get $hi))
    (i32.store (i32.const 0x614) (select (i32.const 0) (local.get $signal) (i32.eq (local.get $signal) (i32.const -1))))
    (i32.store8 (i32.const 0x618) (i32.const 0))
    (i32.store (i32.const 0x619) (i32.const 4))
    (i32.store (i32.const 0x61d) (i32.const 42))
    (call $changeset (i32.const 0x600) (i32.const 33)))

  (func (export "keel_release") (param $lo i32) (param $hi i32)
    (global.set $release_count (i32.add (global.get $release_count) (i32.const 1)))
    (global.set $release_lo (local.get $lo))
    (global.set $release_hi (local.get $hi)))

  (func (export "keel_port_reply") (param $ptr i32) (param $len i32)
    (local $tmp i32)
    (local.set $tmp (call $alloc (local.get $len)))
    (memory.copy (local.get $tmp) (local.get $ptr) (local.get $len))
    (global.set $port_reply_ptr (local.get $tmp))
    (global.set $port_reply_len (local.get $len))
    (if (i32.ne (global.get $port_wait_call) (i32.const 0))
      (then
        (call $send_reply (global.get $port_wait_call) (i32.const 0) (local.get $tmp) (local.get $len))
        (global.set $port_wait_call (i32.const 0)))))

  (func (export "keel_event") (param $port i32) (param $method i32) (param $ptr i32) (param $len i32)
    (global.set $event_port (local.get $port))
    (global.set $event_method (local.get $method))
    (global.set $event_len (local.get $len)))

  (func (export "keel_timer_fired") (param $id i32)
    (if (i32.ne (global.get $timer_call) (i32.const 0))
      (then
        (i32.store (i32.const 0x500) (local.get $id))
        (call $send_reply (global.get $timer_call) (i32.const 0) (i32.const 0x500) (i32.const 4))
        (global.set $timer_call (i32.const 0)))))

  (func (export "keel_poll")
    (global.set $poll_count (i32.add (global.get $poll_count) (i32.const 1)))
    (if (i32.ne (global.get $async_call) (i32.const 0))
      (then
        (call $send_reply (global.get $async_call) (i32.const 0) (global.get $async_ptr) (global.get $async_len))
        (global.set $async_call (i32.const 0)))))

  (func (export "keel_stats_json") (result i32)
    (call $keelbuf (i32.const 0x340) (i32.const ${statsLen})))

  (func (export "keel_buf_free") (param $ptr i32)
    (global.set $buf_free_count (i32.add (global.get $buf_free_count) (i32.const 1))))
)`;
}

let toolkit: Awaited<ReturnType<typeof wabt>> | undefined;

/** Assembles WAT source to a wasm binary. */
export async function assemble(source: string): Promise<Uint8Array> {
  toolkit ??= await wabt();
  const module = toolkit.parseWat("stub.wat", source, {
    bulk_memory: true,
    mutable_globals: true,
    multi_value: true,
    sat_float_to_int: true,
    sign_extension: true,
  });
  try {
    return new Uint8Array(module.toBinary({ log: false, write_debug_names: false }).buffer);
  } finally {
    module.destroy();
  }
}

/** The stub core as a wasm binary. */
export function compileStub(options: StubOptions = {}): Promise<Uint8Array> {
  return assemble(stubWat(options));
}

/** Reads the stub's exported globals from a running instance. */
export function stubGlobals(instance: WebAssembly.Instance): Record<keyof StubGlobals, number> {
  const out: Record<string, number> = {};
  for (const [name, value] of Object.entries(instance.exports)) {
    if (value instanceof WebAssembly.Global) out[name] = value.value as number;
  }
  return out as Record<keyof StubGlobals, number>;
}
