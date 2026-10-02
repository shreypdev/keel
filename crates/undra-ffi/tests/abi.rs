//! The native C ABI (SPEC 6) exercised the way a host does: through a core's `UndraApi` table (C ABI
//! version 2, ADR-044; `common/table.rs` spells its entries with their version 1 names), with
//! `extern "C"` callbacks that capture what the core sends.
//!
//! The core under test is real macro-generated code (an object with sync, async, stream,
//! panicking and failing methods, a store, and two ports), linked into this test binary, so
//! every scenario crosses the same functions Swift and Kotlin cross. `undra_init` is
//! process-global, so the tests take turns (`Embedder` holds a lock for its lifetime).
#![deny(clippy::undocumented_unsafe_blocks)]

use core::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration as StdDuration, Instant};

use undra::meta::{Schema, ids};
use undra::prelude::Handle;
use undra::runtime::{Runtime, RuntimeConfig};
use undra::wire::payload::{
    CallTarget, ChangeSet, PortReply, PortStatus, Reply, ReplyStatus, Snapshot, StreamFlag,
    StreamItem,
};
use undra::wire::{Decode, Encode, Reader, Writer};
use undra_ffi::{UndraBuf, init_code, restore_code};

#[path = "common/table.rs"]
mod table;
use table::{
    undra_abi_version, undra_buf_free, undra_call, undra_call_sync, undra_cancel, undra_event,
    undra_init, undra_observe, undra_port_register, undra_port_reply, undra_release, undra_restore,
    undra_schema_hash, undra_schema_json, undra_shutdown, undra_snapshot, undra_stats_json,
    undra_stream_credit, undra_timer_fired,
};

// ---------------------------------------------------------------------------------------------
// The core under test
// ---------------------------------------------------------------------------------------------

#[path = "common/core.rs"]
mod test_core;

// ---------------------------------------------------------------------------------------------
// A host: callbacks that capture what the core sends
// ---------------------------------------------------------------------------------------------

unsafe extern "C" {
    fn malloc(size: usize) -> *mut c_void;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PortMode {
    /// Answer inline through `out_reply` (malloc'd, `cap` reserved).
    Sync,
    /// Return 1 and let the test call `undra_port_reply`.
    Async,
    /// Return 2.
    Unavailable,
}

#[derive(Debug)]
struct PortCall {
    port_id: u32,
    method_id: u32,
    port_call_id: u32,
    args: Vec<u8>,
}

#[derive(Default)]
struct Inner {
    replies: Vec<(u32, Vec<u8>)>,
    change_sets: Vec<Vec<u8>>,
    stream_items: Vec<(u32, Vec<u8>)>,
    port_calls: Vec<PortCall>,
    logs: Vec<(u8, String, String)>,
    /// The `PanicReport` arguments of every `Diagnostics.panicked` call (ADR-046).
    reports: Vec<Vec<u8>>,
}

struct Capture {
    inner: Mutex<Inner>,
    changed: Condvar,
    echo_mode: Mutex<PortMode>,
    /// When set, `on_reply` calls back into the core (which SPEC 5.1 forbids) and records the answer.
    reenter: std::sync::atomic::AtomicBool,
    reentered: Mutex<Vec<u32>>,
    /// When set, `on_reply` exercises every entry point `undra.h` lists (as callable or not
    /// callable from a callback) and records what happened in `probed`.
    probe: std::sync::atomic::AtomicBool,
    probed: Mutex<Vec<(&'static str, bool)>>,
}

impl Capture {
    fn new() -> Arc<Capture> {
        Arc::new(Capture {
            inner: Mutex::new(Inner::default()),
            changed: Condvar::new(),
            echo_mode: Mutex::new(PortMode::Sync),
            reenter: std::sync::atomic::AtomicBool::new(false),
            reentered: Mutex::new(Vec::new()),
            probe: std::sync::atomic::AtomicBool::new(false),
            probed: Mutex::new(Vec::new()),
        })
    }

    fn with<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let out = f(&mut guard);
        drop(guard);
        self.changed.notify_all();
        out
    }

    /// Blocks until `pick` finds something (or 10 s pass).
    fn wait<R>(&self, what: &str, mut pick: impl FnMut(&mut Inner) -> Option<R>) -> R {
        let deadline = Instant::now() + StdDuration::from_secs(10);
        let mut guard = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(found) = pick(&mut guard) {
                return found;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {what}");
            guard = self
                .changed
                .wait_timeout(guard, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// The reply to `call_id` (waits for it).
    fn reply(&self, call_id: u32) -> (ReplyStatus, Vec<u8>) {
        let payload = self.wait("a reply", |inner| {
            let at = inner.replies.iter().position(|(id, _)| *id == call_id)?;
            Some(inner.replies.remove(at).1)
        });
        parse_reply(call_id, &payload)
    }

    fn take_change_sets(&self) -> Vec<ChangeSet> {
        self.with(|inner| std::mem::take(&mut inner.change_sets))
            .iter()
            .map(|bytes| ChangeSet::decode(&mut Reader::new(bytes)).expect("a change-set"))
            .collect()
    }

    fn wait_change_set(&self) -> ChangeSet {
        let bytes = self.wait("a change-set", |inner| {
            (!inner.change_sets.is_empty()).then(|| inner.change_sets.remove(0))
        });
        ChangeSet::decode(&mut Reader::new(&bytes)).expect("a change-set")
    }

    fn set_echo_mode(&self, mode: PortMode) {
        *self
            .echo_mode
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = mode;
    }

    fn echo_mode(&self) -> PortMode {
        *self
            .echo_mode
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

fn parse_reply(call_id: u32, payload: &[u8]) -> (ReplyStatus, Vec<u8>) {
    let reply = Reply::decode(&mut Reader::new(payload)).expect("a reply payload");
    assert_eq!(reply.call_id, call_id, "reply for another call");
    (reply.status, reply.body.to_vec())
}

/// The capture behind a callback's `user` pointer.
///
/// # Safety
///
/// `user` must be the pointer `Embedder` passed to `undra_init` / `undra_port_register`, which
/// stays valid until `undra_shutdown` returns.
unsafe fn capture<'a>(user: *mut c_void) -> &'a Capture {
    // SAFETY: by the caller's contract `user` points to a live `Capture`.
    unsafe { &*user.cast::<Capture>() }
}

/// # Safety
///
/// `ptr` is valid for `len` bytes (or null with `len == 0`), as the core guarantees.
unsafe fn copy(ptr: *const u8, len: u32) -> Vec<u8> {
    if ptr.is_null() || len == 0 {
        return Vec::new();
    }
    // SAFETY: by the caller's contract.
    unsafe { std::slice::from_raw_parts(ptr, len as usize) }.to_vec()
}

extern "C" fn on_reply(user: *mut c_void, call_id: u32, ptr: *const u8, len: u32) {
    // SAFETY: `user` is the capture given to `undra_init`; `ptr`/`len` are valid during the call.
    let (cap, bytes) = unsafe { (capture(user), copy(ptr, len)) };
    if cap.reenter.load(Ordering::Acquire) {
        // A host must not do this. The core answers "refused" instead of deadlocking.
        let refused = submit(&call_payload(function("version"), 9_999_999, &[]));
        let sync = call_sync_raw(&call_payload(function("version"), 9_999_998, &[]));
        // SAFETY: `sync` came from the core and is read before it is freed.
        let status = unsafe { sync.as_slice() }
            .get(4)
            .copied()
            .map_or(255, u32::from);
        // SAFETY: freed once; `undra_buf_free` is the one call a callback may make.
        unsafe { undra_buf_free(sync) };
        cap.reentered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend([refused, status]);
    }
    if cap.probe.load(Ordering::Acquire) {
        probe_entries(cap);
    }
    cap.with(|inner| inner.replies.push((call_id, bytes)));
}

/// Calls, from inside a callback, every entry point `undra.h` lists, and records whether it did
/// what the header says. Nothing here may panic (this runs in an `extern "C"` callback).
fn probe_entries(cap: &Capture) {
    let mut seen: Vec<(&'static str, bool)> = Vec::new();

    // Callable from a callback: they never take the core lock.
    let stats = undra_stats_json();
    // SAFETY: a buffer the core returned, read and freed once (`undra_buf_free` is on the list).
    let stats_ok = unsafe {
        let ok = serde_json::from_slice::<serde_json::Value>(stats.as_slice())
            .is_ok_and(|json| json["platform"] == "test");
        undra_buf_free(stats);
        ok
    };
    seen.push(("undra_stats_json", stats_ok));
    // SAFETY: the empty buffer owns nothing.
    unsafe { undra_buf_free(UndraBuf::EMPTY) };
    seen.push(("undra_buf_free", true));
    undra_stream_credit(0xFFFF_0001, 1);
    seen.push(("undra_stream_credit", true));
    undra_timer_fired(0xFFFF_0002);
    seen.push(("undra_timer_fired", true));
    port_reply(&port_reply_payload(0xFFFF_0003, PortStatus::Ok, &[]));
    seen.push(("undra_port_reply", true));
    seen.push(("undra_abi_version", undra_abi_version() == 2));
    seen.push(("undra_schema_hash", undra_schema_hash() != 0));
    let json = undra_schema_json();
    // SAFETY: a buffer the core returned, read and freed once.
    let json_ok = unsafe {
        let ok = json.as_slice().first() == Some(&b'{');
        undra_buf_free(json);
        ok
    };
    seen.push(("undra_schema_json", json_ok));

    // Refused (E_REENTRANT), answered without deadlocking or running.
    seen.push((
        "undra_call refused",
        submit(&call_payload(function("version"), 9_999_001, &[])) == 5,
    ));
    let sync = call_sync_raw(&call_payload(function("version"), 9_999_002, &[]));
    // SAFETY: a buffer the core returned, read and freed once.
    let sync_refused = unsafe {
        let refused = sync.as_slice().get(4) == Some(&ReplyStatus::BadRequest.as_u8());
        undra_buf_free(sync);
        refused
    };
    seen.push(("undra_call_sync refused", sync_refused));
    undra_cancel(9_999_003);
    undra_observe(0x1_0000_0001, 0, 1);
    undra_release(0x1_0000_0001);
    event(1, 2, &[]);
    seen.push((
        "undra_restore refused",
        // An empty snapshot in layout 2 (ADR-037): it would decode, so only the re-entrancy
        // refuses it.
        restore(&[0; 24]) == restore_code::UNAVAILABLE,
    ));

    cap.probed
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .extend(seen);
}

extern "C" fn on_changes(user: *mut c_void, ptr: *const u8, len: u32) {
    // SAFETY: as in `on_reply`.
    let (cap, bytes) = unsafe { (capture(user), copy(ptr, len)) };
    cap.with(|inner| inner.change_sets.push(bytes));
}

extern "C" fn on_stream(user: *mut c_void, call_id: u32, ptr: *const u8, len: u32) {
    // SAFETY: as in `on_reply`.
    let (cap, bytes) = unsafe { (capture(user), copy(ptr, len)) };
    cap.with(|inner| inner.stream_items.push((call_id, bytes)));
}

/// What `answer_sync` writes into `out_reply.cap`: `0` as `undra.h` asks, or (set by a test) the
/// block's length, the way a host that took "cap" to mean "capacity" would fill it (review M1).
static REPLY_CAP_IS_LEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Hands `payload` to the core as a host-allocated reply: `malloc`ed, `cap` reserved (0).
///
/// # Safety
///
/// `out` must point to a valid `UndraBuf` (the core's `out_reply`).
unsafe fn answer_sync(out: *mut UndraBuf, payload: &[u8]) {
    // SAFETY: `malloc(n)` for a non-empty payload; the block is filled before being handed over.
    let block = unsafe { malloc(payload.len()) }.cast::<u8>();
    assert!(!block.is_null());
    // SAFETY: `block` is valid for `payload.len()` bytes and does not overlap `payload`.
    unsafe { std::ptr::copy_nonoverlapping(payload.as_ptr(), block, payload.len()) };
    // SAFETY: `out` is valid per the caller's contract; ownership of `block` passes to the core.
    unsafe {
        let len = u32::try_from(payload.len()).expect("small");
        *out = UndraBuf {
            ptr: block,
            len,
            cap: if REPLY_CAP_IS_LEN.load(Ordering::Acquire) {
                len
            } else {
                0
            },
        };
    }
}

fn port_reply_payload(port_call_id: u32, status: PortStatus, body: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    PortReply {
        port_call_id,
        status,
        body,
    }
    .encode(&mut w);
    w.into_vec()
}

const LOG_PORT: u32 = ids::port_id("Log");
const DIAGNOSTICS_PORT: u32 = ids::port_id("Diagnostics");
const SUM_PORT: u32 = ids::port_id("Sum");
const ECHO_PORT: u32 = ids::port_id("Echo");

extern "C" fn on_port(
    user: *mut c_void,
    port_id: u32,
    method_id: u32,
    port_call_id: u32,
    ptr: *const u8,
    len: u32,
    out: *mut UndraBuf,
) -> u8 {
    // SAFETY: `user` is the capture given to `undra_port_register`; `ptr`/`len` are valid.
    let (cap, args) = unsafe { (capture(user), copy(ptr, len)) };
    if port_id == LOG_PORT {
        let mut r = Reader::new(&args);
        let level = r.read_u8().expect("level");
        let target = r.read_str().expect("target").to_owned();
        let message = r.read_str().expect("message").to_owned();
        cap.with(|inner| inner.logs.push((level, target, message)));
        // SAFETY: `out` is the core's `out_reply`.
        unsafe { answer_sync(out, &port_reply_payload(port_call_id, PortStatus::Ok, &[])) };
        return 0;
    }
    if port_id == DIAGNOSTICS_PORT {
        // Fire and forget (`port_call_id 0`): the host answers a sync port all the same.
        assert_eq!(port_call_id, 0, "a panic report is not a pending call");
        cap.with(|inner| inner.reports.push(args));
        // SAFETY: `out` is the core's `out_reply`.
        unsafe { answer_sync(out, &port_reply_payload(port_call_id, PortStatus::Ok, &[])) };
        return 0;
    }
    cap.with(|inner| {
        inner.port_calls.push(PortCall {
            port_id,
            method_id,
            port_call_id,
            args: args.clone(),
        });
    });
    match port_id {
        SUM_PORT => {
            let mut r = Reader::new(&args);
            let (a, b) = (r.read_u32().expect("a"), r.read_u32().expect("b"));
            let body = (a + b).to_le_bytes();
            // SAFETY: `out` is the core's `out_reply`.
            unsafe {
                answer_sync(
                    out,
                    &port_reply_payload(port_call_id, PortStatus::Ok, &body),
                )
            };
            0
        }
        ECHO_PORT => match cap.echo_mode() {
            PortMode::Sync => {
                let n = Reader::new(&args).read_u32().expect("n") + 1000;
                // SAFETY: `out` is the core's `out_reply`.
                unsafe {
                    answer_sync(
                        out,
                        &port_reply_payload(port_call_id, PortStatus::Ok, &n.to_le_bytes()),
                    );
                }
                0
            }
            PortMode::Async => 1,
            PortMode::Unavailable => 2,
        },
        _ => 2,
    }
}

/// Serializes the tests: the runtime is one per process.
static SERIAL: Mutex<()> = Mutex::new(());

/// A started core plus the host that receives its traffic.
struct Embedder {
    cap: Arc<Capture>,
    _turn: MutexGuard<'static, ()>,
}

fn config(mode: &str, log_level: u8) -> Vec<u8> {
    RuntimeConfig {
        platform: "test".to_owned(),
        mode: mode.to_owned(),
        core_threads: 1,
        blocking_threads: 2,
        log_level,
    }
    .encode_to_vec()
}

fn init_raw(cfg: &[u8], cap: &Arc<Capture>) -> u32 {
    // SAFETY: `cfg` is valid for its length; the callbacks are `extern "C"` functions of this
    // binary and `user` (the capture) outlives the runtime: `Embedder` shuts it down first.
    unsafe {
        undra_init(
            cfg.as_ptr(),
            u32::try_from(cfg.len()).expect("small"),
            Some(on_reply),
            Some(on_changes),
            Some(on_stream),
            Arc::as_ptr(cap).cast_mut().cast(),
        )
    }
}

impl Embedder {
    /// Starts a runtime in `inproc` mode at log level 2 with the Log, Sum and Echo ports registered.
    fn start() -> Embedder {
        Embedder::start_with(&config("inproc", 2), true)
    }

    fn start_with(cfg: &[u8], register_ports: bool) -> Embedder {
        let turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
        undra_shutdown();
        let cap = Capture::new();
        let embedder = Embedder { cap, _turn: turn };
        if register_ports {
            embedder.register_ports();
        }
        assert_eq!(init_raw(cfg, &embedder.cap), init_code::OK);
        embedder
    }

    fn user(&self) -> *mut c_void {
        Arc::as_ptr(&self.cap).cast_mut().cast()
    }

    fn register_ports(&self) {
        for port in [LOG_PORT, DIAGNOSTICS_PORT, SUM_PORT, ECHO_PORT] {
            // SAFETY: `on_port` is an `extern "C"` function and the capture outlives the runtime.
            unsafe { undra_port_register(port, Some(on_port), self.user()) };
        }
    }

    fn call_id(&self) -> u32 {
        static NEXT: AtomicU32 = AtomicU32::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    }

    fn sync(&self, target: CallTarget, args: &[u8]) -> (ReplyStatus, Vec<u8>) {
        let id = self.call_id();
        sync_call(&call_payload(target, id, args), id)
    }

    /// Submits an asynchronous-path call and returns its id.
    fn start_call(&self, target: CallTarget, args: &[u8]) -> u32 {
        let id = self.call_id();
        let payload = call_payload(target, id, args);
        let code = submit(&payload);
        assert_eq!(code, 0, "the call was refused");
        id
    }

    fn run(&self, target: CallTarget, args: &[u8]) -> (ReplyStatus, Vec<u8>) {
        let id = self.start_call(target, args);
        self.cap.reply(id)
    }

    fn construct(&self, type_name: &str, args: &[u8]) -> Handle {
        let (status, body) = self.sync(
            CallTarget::Constructor {
                type_id: ids::type_id(type_name),
                method_id: ids::method_id(type_name, "new"),
            },
            args,
        );
        assert_eq!(status, ReplyStatus::Ok, "{type_name}::new");
        Handle::decode_exact(&body).expect("a handle")
    }

    fn calculator(&self, base: i64) -> Handle {
        self.construct("Calculator", &base.encode_to_vec())
    }
}

impl Drop for Embedder {
    fn drop(&mut self) {
        undra_shutdown();
    }
}

/// `undra_call` over a slice.
fn submit(payload: &[u8]) -> u32 {
    let ptr = if payload.is_empty() {
        std::ptr::null()
    } else {
        payload.as_ptr()
    };
    // SAFETY: `ptr` is null (an empty payload) or valid for `payload.len()` bytes.
    unsafe { undra_call(ptr, payload.len() as u32) }
}

/// `undra_restore` over a slice.
fn restore(payload: &[u8]) -> u32 {
    // SAFETY: `payload` is valid for its length.
    unsafe { undra_restore(payload.as_ptr(), payload.len() as u32) }
}

/// `undra_port_reply` over a slice.
fn port_reply(payload: &[u8]) {
    // SAFETY: `payload` is valid for its length.
    unsafe { undra_port_reply(payload.as_ptr(), payload.len() as u32) };
}

/// `undra_event` over a slice.
fn event(port_id: u32, method_id: u32, payload: &[u8]) {
    // SAFETY: `payload` is valid for its length.
    unsafe { undra_event(port_id, method_id, payload.as_ptr(), payload.len() as u32) };
}

fn call_payload(target: CallTarget, call_id: u32, args: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    undra::wire::payload::Call {
        target,
        call_id,
        args,
    }
    .encode(&mut w);
    w.into_vec()
}

/// Calls `undra_call_sync` and frees the returned buffer.
fn sync_call(payload: &[u8], call_id: u32) -> (ReplyStatus, Vec<u8>) {
    let buf = call_sync_raw(payload);
    parse_reply(call_id, &take(buf))
}

fn call_sync_raw(payload: &[u8]) -> UndraBuf {
    // SAFETY: `payload` is valid for its length.
    unsafe { undra_call_sync(payload.as_ptr(), payload.len() as u32) }
}

/// Copies a core buffer and frees it.
fn take(buf: UndraBuf) -> Vec<u8> {
    // SAFETY: `buf` was returned by the core and is freed exactly once, after the copy.
    unsafe {
        let bytes = buf.as_slice().to_vec();
        undra_buf_free(buf);
        bytes
    }
}

fn method(handle: Handle, type_name: &str, name: &str) -> CallTarget {
    CallTarget::Method {
        handle,
        method_id: ids::method_id(type_name, name),
    }
}

fn function(name: &str) -> CallTarget {
    CallTarget::Function {
        method_id: ids::function_id(name),
    }
}

fn reason(body: &[u8]) -> String {
    String::decode_exact(body).expect("a String reason")
}

// ---------------------------------------------------------------------------------------------
// Scenarios: the ABI without a running core
// ---------------------------------------------------------------------------------------------

#[test]
fn abi_version_schema_hash_and_json_work_before_init() {
    let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    undra_shutdown();
    assert_eq!(undra_abi_version(), 2);
    let hash = undra_schema_hash();
    assert_ne!(hash, 0);
    let json = take(undra_schema_json());
    let doc: serde_json::Value = serde_json::from_slice(&json).expect("the schema is JSON");
    let text = String::from_utf8(json).expect("UTF-8");
    for expected in ["Calculator", "Counter", "Echo", "Sum", "version"] {
        assert!(text.contains(expected), "{expected} is in the schema");
    }
    assert!(doc.is_object());
    // The JSON is the whole schema, docs and labels included (SPEC 2.3 and 6), so the hash is
    // not the fnv1a64 of these bytes but of their canonical form, which has neither.
    let schema = Schema::from_json(&text).expect("the schema JSON reads back");
    assert_eq!(schema.hash(), hash);
    assert_eq!(ids::fnv1a64(schema.canonical_json().as_bytes()), hash);
}

/// C ABI version 2 (ADR-044): what a host reads from the table before it calls anything. The wire
/// did not change with it, so the schema hash is the one the runtime computes, as in version 1.
#[test]
fn the_table_carries_its_version_size_namespace_and_the_unchanged_schema_hash() {
    let api = table::api();
    assert_eq!(api.abi_version, 2);
    assert_eq!(api.abi_version, undra_ffi::ABI_VERSION);
    assert_eq!(
        api.size as usize,
        core::mem::size_of::<undra_ffi::UndraApi>()
    );
    // SAFETY: the table's `name_space` is a static NUL-terminated string.
    let name = unsafe { core::ffi::CStr::from_ptr(api.name_space) };
    assert_eq!(name, table::NAMESPACE);
    assert_eq!(
        api.schema_hash,
        undra::meta::collect_schema("undra-core").hash(),
        "the table carries the hash the runtime computes; ADR-044 changes no hash"
    );
    // Immutable: the entry hands out the same table every time.
    assert!(core::ptr::eq(api, table::api()));
}

/// `undra.h` declares the table field for field as `UndraApi` is laid out here: same names, same
/// order (a wrong order is a host calling the wrong entry). The C smoke test checks `size`.
#[test]
fn undra_h_declares_the_table_in_the_order_of_the_rust_struct() {
    let header = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../runtimes/swift/UndraRuntime/Sources/UndraFFI/include/undra.h"),
    )
    .expect("undra.h");
    let start = header
        .find("typedef struct UndraApi {")
        .expect("undra.h declares UndraApi");
    let body = &header[start..header[start..].find("} UndraApi;").unwrap() + start];
    let fields: Vec<&str> = body
        .lines()
        .skip(1)
        .filter_map(|line| {
            let line = line.split("/*").next().unwrap().trim();
            if line.is_empty() {
                return None;
            }
            Some(match line.find("(*") {
                Some(at) => line[at + 2..].split(')').next().unwrap().trim(),
                None => line
                    .trim_end_matches(';')
                    .rsplit([' ', '*'])
                    .next()
                    .unwrap(),
            })
        })
        .collect();
    assert_eq!(
        fields,
        [
            "abi_version",
            "size",
            "schema_hash",
            "name_space",
            "schema_json",
            "init",
            "shutdown",
            "call",
            "call_sync",
            "cancel",
            "stream_credit",
            "observe",
            "release",
            "port_register",
            "port_reply",
            "event",
            "timer_fired",
            "snapshot",
            "restore",
            "stats_json",
            "buf_free",
        ]
    );
    assert!(header.contains("#define UNDRA_ABI_VERSION 2u"));
    // Version 2 declares no function: a core exports one entry, declared by its own header.
    assert!(
        !header.contains("undra_init("),
        "undra.h v2 declares the table, not the v1 functions"
    );
}

/// There is one `undra.h`, kept in two places: the Swift runtime's `UndraFFI` module and the React
/// Native module's C++ (ADR-038). They are the same bytes, so a table change cannot reach one host
/// and miss the other.
#[test]
fn the_swift_and_react_native_copies_of_undra_h_are_identical() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let read = |path: &str| {
        std::fs::read(root.join(path)).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
    };
    let swift = read("runtimes/swift/UndraRuntime/Sources/UndraFFI/include/undra.h");
    let react_native = read("runtimes/rn/@undra/react-native/cpp/undra.h");
    assert!(
        swift == react_native,
        "runtimes/rn/@undra/react-native/cpp/undra.h differs from the Swift runtime's undra.h: copy it over"
    );
}

#[test]
fn the_exported_schema_carries_the_doc_comments_the_hash_ignores() {
    let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    undra_shutdown();
    let hash = undra_schema_hash();
    let schema = Schema::from_json(&String::from_utf8(take(undra_schema_json())).unwrap())
        .expect("the schema JSON reads back");

    let echo = schema.ports.iter().find(|p| p.name == "Echo").unwrap();
    assert_eq!(echo.docs, "Answered by the host, asynchronously.");
    let calculator = schema
        .objects
        .iter()
        .find(|o| o.name == "Calculator")
        .unwrap();
    let ready_add = calculator
        .methods
        .iter()
        .find(|m| m.name == "ready_add")
        .unwrap();
    assert_eq!(
        ready_add.docs,
        "An async method that is ready at once: measures the executor hop, not a timer."
    );
    // Stripping every doc changes nothing the wire depends on.
    assert_eq!(schema.without_docs().hash(), hash);
    assert_eq!(schema.hash(), hash);
    assert_ne!(schema.to_json(), schema.without_docs().to_json());
}

#[test]
fn the_exported_schema_is_what_the_dev_runner_prints() {
    // `undra-dev-runner --print-schema` prints `collect_schema(..).to_json_pretty()`; the C ABI
    // returns the same document compact. `undra bindgen` must generate the same bindings from
    // either (docs included), so they have to read back as the same schema once labelled alike.
    let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    undra_shutdown();
    let exported = Schema::from_json(&String::from_utf8(take(undra_schema_json())).unwrap())
        .expect("the schema JSON reads back");
    let printed =
        Schema::from_json(&undra::meta::collect_schema("playground-core").to_json_pretty())
            .expect("the runner's schema JSON reads back");
    let mut relabelled = exported.clone();
    "playground-core".clone_into(&mut relabelled.crate_name);
    assert_eq!(relabelled, printed);
    assert!(
        exported.ports.iter().any(|p| !p.docs.is_empty()),
        "the comparison is only worth something when there are docs"
    );
}

#[test]
fn schema_is_the_same_before_and_after_init() {
    let before = (undra_schema_hash(), take(undra_schema_json()));
    let _host = Embedder::start();
    assert_eq!((undra_schema_hash(), take(undra_schema_json())), before);
}

#[test]
fn calls_before_init_fail_softly() {
    let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    undra_shutdown();
    let payload = call_payload(function("version"), 77, &[]);
    assert_eq!(submit(&payload), 5);
    let (status, body) = sync_call(&payload, 77);
    assert_eq!(status, ReplyStatus::BadRequest);
    assert!(reason(&body).contains("not initialized"));
    // Every other entry is a quiet no-op.
    undra_cancel(1);
    undra_stream_credit(1, 1);
    undra_observe(0x1_0000_0001, 0, 1);
    undra_release(0x1_0000_0001);
    undra_timer_fired(9);
    port_reply(&[]);
    event(1, 2, &[]);
    let snapshot = take(undra_snapshot());
    let decoded = Snapshot::decode(&mut Reader::new(&snapshot)).expect("layout 2");
    assert!(
        decoded.stores.is_empty() && decoded.types.is_empty(),
        "no stores, the generation floor, this core's hash"
    );
    assert_eq!(snapshot[..4], [0; 4]);
    assert_eq!(restore(&snapshot), restore_code::UNAVAILABLE);
    let stats: serde_json::Value = serde_json::from_slice(&take(undra_stats_json())).expect("JSON");
    assert_eq!(stats["initialized"], false);
}

// ---------------------------------------------------------------------------------------------
// Scenarios: init and shutdown
// ---------------------------------------------------------------------------------------------

#[test]
fn init_is_idempotent_for_the_same_embedder_and_refuses_another() {
    let host = Embedder::start();
    // The same callbacks and user pointer again: a no-op that succeeds.
    assert_eq!(init_raw(&config("inproc", 2), &host.cap), init_code::OK);
    // Another embedder (a different user pointer) is refused and changes nothing.
    let other = Capture::new();
    assert_eq!(
        init_raw(&config("inproc", 2), &other),
        init_code::ALREADY_INITIALIZED
    );
    // The first embedder still receives its replies.
    let (status, _) = host.run(function("version"), &[]);
    assert_eq!(status, ReplyStatus::Ok);
    other.with(|inner| assert!(inner.replies.is_empty()));
}

#[test]
fn core_threads_zero_still_runs_async_calls() {
    // The native ABI has no undra_poll, so "the host polls" cannot be honoured: the core thread runs.
    let cfg = RuntimeConfig {
        core_threads: 0,
        ..RuntimeConfig::default()
    }
    .encode_to_vec();
    let host = Embedder::start_with(&cfg, true);
    let calc = host.calculator(1);
    let (status, body) = host.run(
        method(calc, "Calculator", "slow_add"),
        &[1_i64.encode_to_vec(), 1_i64.encode_to_vec()].concat(),
    );
    assert_eq!(
        (status, i64::decode_exact(&body).unwrap()),
        (ReplyStatus::Ok, 3)
    );
}

#[test]
fn shutdown_is_idempotent_and_init_can_follow() {
    let host = Embedder::start();
    let (status, body) = host.sync(function("version"), &[]);
    assert_eq!(status, ReplyStatus::Ok);
    assert_eq!(
        String::decode_exact(&body).unwrap(),
        "undra-ffi test core 1"
    );
    undra_shutdown();
    undra_shutdown();
    // Calls after shutdown are answered with status 5 / refused.
    let id = host.call_id();
    let payload = call_payload(function("version"), id, &[]);
    let (status, _) = sync_call(&payload, id);
    assert_eq!(status, ReplyStatus::BadRequest);
    assert_eq!(submit(&payload), 5);
    // A new init works and the port registrations were dropped by the shutdown.
    assert_eq!(init_raw(&config("inproc", 2), &host.cap), init_code::OK);
    let (status, _) = host.sync(function("version"), &[]);
    assert_eq!(status, ReplyStatus::Ok);
    let calc = host.calculator(1);
    let (status, _) = host.sync(
        method(calc, "Calculator", "sum_on_host"),
        &[1, 0, 0, 0, 2, 0, 0, 0],
    );
    assert_eq!(
        status,
        ReplyStatus::Panic,
        "no Sum port is registered any more"
    );
}

#[test]
fn init_rejects_bad_arguments_with_distinct_codes() {
    let _turn = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
    undra_shutdown();
    let cap = Capture::new();
    let good = config("inproc", 2);
    let user: *mut c_void = Arc::as_ptr(&cap).cast_mut().cast();
    // A missing callback.
    // SAFETY: `good` is valid for its length; none of the calls below starts a runtime.
    let code = unsafe {
        undra_init(
            good.as_ptr(),
            good.len() as u32,
            None,
            Some(on_changes),
            Some(on_stream),
            user,
        )
    };
    assert_eq!(code, init_code::BAD_ARGUMENT);
    // No config at all, garbage, trailing bytes, an unknown mode.
    for bad in [
        Vec::new(),
        vec![0xff, 0xff, 0xff, 0xff, 1],
        [good.clone(), vec![0]].concat(),
        config("sideways", 2),
    ] {
        assert_eq!(init_raw(&bad, &cap), init_code::BAD_CONFIG, "{bad:?}");
    }
    // None of them left a runtime behind.
    assert!(Runtime::global().is_none());
}

#[test]
fn dev_mode_starts_and_logs_devtools_records_to_the_log_port() {
    let host = Embedder::start_with(&config("dev", 0), true);
    let counter = host.construct("Counter", &[]);
    undra_observe(counter.0, u32::MAX, 1);
    host.cap.wait_change_set();
    let (status, _) = host.sync(method(counter, "Counter", "bump"), &[]);
    assert_eq!(status, ReplyStatus::Ok);
    host.cap.with(|inner| {
        assert!(
            inner.logs.iter().any(|(level, target, message)| {
                *level == 1 && target == "undra::devtools" && message.starts_with("commit txn=")
            }),
            "dev mode logs every commit: {:?}",
            inner.logs
        );
    });
}

// ---------------------------------------------------------------------------------------------
// Scenarios: calls
// ---------------------------------------------------------------------------------------------

#[test]
fn unknown_method_is_status_5_through_both_paths() {
    let host = Embedder::start();
    let unknown = CallTarget::Function {
        method_id: 0xDEAD_BEEF,
    };
    let (status, body) = host.sync(unknown, &[]);
    assert_eq!(status, ReplyStatus::BadRequest);
    assert!(!reason(&body).is_empty());
    let (status, body) = host.run(unknown, &[]);
    assert_eq!(status, ReplyStatus::BadRequest);
    assert!(!reason(&body).is_empty());
}

#[test]
fn malformed_calls_are_rejected_without_a_crash() {
    let host = Embedder::start();
    // undra_call: garbage and call_id 0 are refused with 5 and never replied to.
    for bad in [
        &[][..],
        &[0xff][..],
        &[9, 9, 9, 9][..],
        &call_payload(function("version"), 0, &[])[..],
    ] {
        assert_eq!(submit(bad), 5, "{bad:?}");
    }
    // A null pointer is an empty payload.
    assert_eq!(submit(&[]), 5);
    // undra_call_sync answers garbage with status 5 and call id 0.
    let reply = take(call_sync_raw(&[0xff, 0xff]));
    assert_eq!(parse_reply(0, &reply).0, ReplyStatus::BadRequest);
    // Arguments that do not decode are status 5 with a reason naming the problem.
    let calc = host.calculator(1);
    let (status, body) = host.sync(method(calc, "Calculator", "add"), &[1, 2, 3]);
    assert_eq!(status, ReplyStatus::BadRequest);
    assert!(
        reason(&body).contains("Calculator.add"),
        "{}",
        reason(&body)
    );
    host.cap
        .with(|inner| assert!(inner.replies.is_empty(), "no stray replies"));
}

#[test]
fn calling_back_into_the_core_from_a_callback_is_refused_not_deadlocked() {
    let host = Embedder::start();
    host.cap.reenter.store(true, Ordering::Release);
    // The async path replies from inside `undra_call`, on this thread, under the core lock.
    let (status, _) = host.run(function("version"), &[]);
    assert_eq!(status, ReplyStatus::Ok, "the original call is unaffected");
    let answers = host.cap.reentered.lock().unwrap().clone();
    assert_eq!(answers, [5, u32::from(ReplyStatus::BadRequest.as_u8())]);
    host.cap.reenter.store(false, Ordering::Release);
    assert_eq!(host.sync(function("version"), &[]).0, ReplyStatus::Ok);
}

/// M2: the list of entry points `undra.h` and SPEC 5.1 allow from inside a callback is the list
/// that works; every other core-lock entry is refused with `E_REENTRANT` (logged, never a
/// deadlock), and none of the allowed ones is.
#[test]
fn callbacks_may_call_exactly_the_documented_entry_points() {
    let host = Embedder::start();
    host.cap.probe.store(true, Ordering::Release);
    // The async path replies from inside `undra_call`, on this thread, under the core lock.
    let (status, _) = host.run(function("version"), &[]);
    host.cap.probe.store(false, Ordering::Release);
    assert_eq!(status, ReplyStatus::Ok, "the original call is unaffected");

    let probed = host.cap.probed.lock().unwrap().clone();
    assert!(probed.len() == 11, "the probe ran: {probed:?}");
    for (what, ok) in &probed {
        assert!(*ok, "{what} did not behave as undra.h says: {probed:?}");
    }
    host.cap.with(|inner| {
        let refused: Vec<&str> = inner
            .logs
            .iter()
            .filter(|(_, target, message)| {
                target == "undra::runtime" && message.contains("E_REENTRANT")
            })
            .map(|(_, _, message)| message.split(':').next().unwrap_or(""))
            .collect();
        for entry in ["cancel", "observe", "release", "event"] {
            assert!(
                refused.contains(&entry),
                "{entry} from a callback must be refused: {refused:?}"
            );
        }
        // The allowed entries are not among the refusals (the list is exactly the allowed set).
        for entry in ["stream_credit", "timer_fired", "port_reply", "stats_json"] {
            assert!(
                !refused.contains(&entry),
                "{entry} is allowed from a callback: {refused:?}"
            );
        }
    });
    // Nothing was left wedged.
    assert_eq!(host.sync(function("version"), &[]).0, ReplyStatus::Ok);
}

#[test]
fn duplicate_call_ids_are_refused() {
    let host = Embedder::start();
    let calc = host.calculator(0);
    let id = host.start_call(method(calc, "Calculator", "never"), &[]);
    let dup = call_payload(method(calc, "Calculator", "never"), id, &[]);
    assert_eq!(submit(&dup), 5);
    undra_cancel(id);
    assert_eq!(host.cap.reply(id).0, ReplyStatus::Cancelled);
}

#[test]
fn sync_and_async_calls_reply_with_values_typed_errors_and_cancellation() {
    let host = Embedder::start();
    let calc = host.calculator(100);
    let two_and_three: Vec<u8> = [2_i64.encode_to_vec(), 3_i64.encode_to_vec()].concat();

    // Sync method through undra_call_sync.
    let (status, body) = host.sync(method(calc, "Calculator", "add"), &two_and_three);
    assert_eq!(
        (status, i64::decode_exact(&body).unwrap()),
        (ReplyStatus::Ok, 105)
    );
    // The same sync method through undra_call replies through the callback.
    let (status, body) = host.run(method(calc, "Calculator", "add"), &two_and_three);
    assert_eq!(
        (status, i64::decode_exact(&body).unwrap()),
        (ReplyStatus::Ok, 105)
    );
    // Async method: undra_call_sync refuses (5), undra_call replies later.
    let (status, _) = host.sync(method(calc, "Calculator", "slow_add"), &two_and_three);
    assert_eq!(status, ReplyStatus::BadRequest);
    let (status, body) = host.run(method(calc, "Calculator", "slow_add"), &two_and_three);
    assert_eq!(
        (status, i64::decode_exact(&body).unwrap()),
        (ReplyStatus::Ok, 105)
    );
    // A typed error is status 1 with the encoded E.
    let (status, body) = host.sync(method(calc, "Calculator", "fail"), &[]);
    assert_eq!(status, ReplyStatus::Error);
    assert_eq!(body, 0_u16.to_le_bytes());
    // Cancelling a call that never finishes answers status 3 once.
    let id = host.start_call(method(calc, "Calculator", "never"), &[]);
    undra_cancel(id);
    assert_eq!(host.cap.reply(id).0, ReplyStatus::Cancelled);
    undra_cancel(id);
    // Unknown ids are ignored.
    undra_cancel(0xFFFF_0000);
    undra_stream_credit(0xFFFF_0000, 5);
    undra_timer_fired(0xFFFF_0000);
    event(1, 2, &[]);
    host.cap
        .with(|inner| assert!(inner.replies.is_empty(), "exactly one reply per call"));
}

#[test]
fn a_panic_in_a_dispatched_call_is_status_2_not_an_abort() {
    let host = Embedder::start();
    let calc = host.calculator(0);
    // Sync path.
    let (status, body) = host.sync(method(calc, "Calculator", "boom"), &[]);
    assert_eq!(status, ReplyStatus::Panic);
    let mut r = Reader::new(&body);
    assert_eq!(r.read_str().unwrap(), "kaboom");
    assert!(
        !r.read_str().unwrap().is_empty(),
        "a backtrace follows the message"
    );
    // Callback path, sync method.
    let (status, body) = host.run(method(calc, "Calculator", "boom"), &[]);
    assert_eq!(status, ReplyStatus::Panic);
    assert_eq!(Reader::new(&body).read_str().unwrap(), "kaboom");
    // Async method: the panic happens on the core thread.
    let (status, body) = host.run(method(calc, "Calculator", "async_boom"), &[]);
    assert_eq!(status, ReplyStatus::Panic);
    assert_eq!(Reader::new(&body).read_str().unwrap(), "async kaboom");
    // The core keeps working, and the panics were counted and logged at level 5.
    let (status, _) = host.sync(function("version"), &[]);
    assert_eq!(status, ReplyStatus::Ok);
    let stats: serde_json::Value = serde_json::from_slice(&take(undra_stats_json())).unwrap();
    assert_eq!(stats["panics"], 3);
    host.cap.with(|inner| {
        assert!(
            inner
                .logs
                .iter()
                .any(|(level, target, _)| *level == 5 && target == "undra::panic")
        );
    });
    // ADR-046: each contained panic reached the app's `Diagnostics` adapter once, as the record,
    // with frames read by this crate's frame source and the core's identity.
    assert_eq!(stats["panic_reports"], 3);
    let reports: Vec<undra::ports::PanicReport> = host.cap.with(|inner| {
        inner
            .reports
            .iter()
            .map(|bytes| undra::wire::Decode::decode_exact(bytes).expect("a PanicReport"))
            .collect()
    });
    let seen: Vec<(&str, &str)> = reports
        .iter()
        .map(|r| (r.message.as_str(), r.operation.as_str()))
        .collect();
    assert_eq!(
        seen,
        [
            ("kaboom", "Calculator.boom"),
            ("kaboom", "Calculator.boom"),
            ("async kaboom", "Calculator.async_boom")
        ]
    );
    let first = &reports[0];
    assert!(first.location.contains("core.rs:"), "{}", first.location);
    assert_eq!(first.namespace, "undra_ffi_abi_test");
    assert_eq!(first.core_version, "7.7.7");
    assert!(!first.thread.is_empty());
    assert_eq!(first.schema_hash, undra_schema_hash());
    assert!(!first.frames.is_empty(), "the frame source read the stack");
    assert!(
        first
            .frames
            .iter()
            .all(|f| f.address != 0 || f.symbol.is_some()),
        "{:?}",
        first.frames
    );
    if cfg!(target_vendor = "apple") {
        assert_eq!(
            first.image_id.len(),
            32,
            "a Mach-O UUID: {:?}",
            first.image_id
        );
    }
}

undra::runtime::inventory::submit! {
    undra::runtime::CoreIdentity { namespace: "undra_ffi_abi_test", version: "7.7.7" }
}

#[test]
fn streams_honor_credit_and_end() {
    let host = Embedder::start();
    let calc = host.calculator(0);
    let id = host.start_call(method(calc, "Calculator", "ticks"), &4_u32.encode_to_vec());
    assert_eq!(host.cap.reply(id).0, ReplyStatus::StreamOpened);
    undra_stream_credit(id, 16);
    let mut seen = Vec::new();
    loop {
        let payload = host.cap.wait("a stream item", |inner| {
            let at = inner.stream_items.iter().position(|(c, _)| *c == id)?;
            Some(inner.stream_items.remove(at).1)
        });
        let item = StreamItem::decode(&mut Reader::new(&payload)).unwrap();
        assert_eq!(item.call_id, id);
        match item.flag {
            StreamFlag::Item => seen.push(u32::decode_exact(item.body).unwrap()),
            StreamFlag::End => break,
            StreamFlag::Error | StreamFlag::Failed => panic!("stream failed"),
        }
    }
    assert_eq!(seen, [0, 1, 2, 3]);
}

// ---------------------------------------------------------------------------------------------
// Scenarios: stores, observation, snapshots
// ---------------------------------------------------------------------------------------------

#[test]
fn observe_release_and_bogus_handles() {
    let host = Embedder::start();
    let counter = host.construct("Counter", &[]);
    // Unknown, null, stale and wrongly-typed handles are ignored, not fatal.
    for bogus in [0_u64, 0x7777_7777_0000_0001, u64::MAX] {
        undra_observe(bogus, 0, 1);
        undra_observe(bogus, u32::MAX, 0);
        undra_release(bogus);
    }
    let calc = host.calculator(0);
    undra_observe(calc.0, 0, 1); // not a store
    assert!(host.cap.take_change_sets().is_empty());

    undra_observe(counter.0, u32::MAX, 1);
    let initial = host.cap.wait_change_set();
    assert_eq!(initial.entries.len(), 1);
    assert_eq!(initial.entries[0].handle, counter);
    assert_eq!(u32::decode_exact(&initial.entries[0].value).unwrap(), 0);

    let (status, _) = host.sync(method(counter, "Counter", "bump"), &[]);
    assert_eq!(status, ReplyStatus::Ok);
    let update = host.cap.wait_change_set();
    assert_eq!(u32::decode_exact(&update.entries[0].value).unwrap(), 1);

    // Stop observing: further writes are silent. Release invalidates the handle.
    undra_observe(counter.0, u32::MAX, 0);
    host.sync(method(counter, "Counter", "bump"), &[]);
    assert!(host.cap.take_change_sets().is_empty());
    undra_release(counter.0);
    let (status, body) = host.sync(method(counter, "Counter", "bump"), &[]);
    assert_eq!(status, ReplyStatus::BadRequest);
    assert!(
        reason(&body).contains("stale") || reason(&body).contains("handle"),
        "{}",
        reason(&body)
    );
    // Releasing twice is harmless.
    undra_release(counter.0);
}

#[test]
fn snapshot_and_restore_round_trip_and_reject_garbage() {
    let host = Embedder::start();
    // Empty snapshot restores.
    let empty = take(undra_snapshot());
    assert_eq!(
        Snapshot::decode(&mut Reader::new(&empty))
            .unwrap()
            .stores
            .len(),
        0
    );
    assert_eq!(restore(&empty), 0);

    let counter = host.construct("Counter", &[]);
    for _ in 0..3 {
        host.sync(method(counter, "Counter", "bump"), &[]);
    }
    let snapshot = take(undra_snapshot());
    let decoded = Snapshot::decode(&mut Reader::new(&snapshot)).unwrap();
    assert_eq!(decoded.stores.len(), 1);
    assert_eq!(decoded.stores[0].handle, counter);
    assert!(
        decoded.generation_floor >= counter.generation(),
        "the snapshot carries the generation counter (ADR-022)"
    );
    // Change the state, then restore: the same handle is valid and shows the old value.
    host.sync(method(counter, "Counter", "bump"), &[]);
    assert_eq!(restore(&snapshot), 0);
    undra_observe(counter.0, 0, 1);
    let cs = host.cap.wait_change_set();
    assert_eq!(u32::decode_exact(&cs.entries[0].value).unwrap(), 3);
    // Garbage and truncations are rejected and leave the core alone.
    for bad in [
        &[0xff_u8, 0xff, 0xff, 0xff][..],
        &snapshot[..snapshot.len() - 1],
        &[1][..],
    ] {
        assert_eq!(restore(bad), restore_code::BAD_SNAPSHOT);
    }
    // A generation floor of u32::MAX would leave nothing to issue: refused like any bad snapshot.
    let mut hostile = snapshot.clone();
    hostile[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(restore(&hostile), restore_code::BAD_SNAPSHOT);
    let (status, _) = host.sync(method(counter, "Counter", "bump"), &[]);
    assert_eq!(status, ReplyStatus::Ok);
}

/// L1: the generation counter outlives `undra_shutdown`, so a snapshot taken with no runtime
/// carries the true process-wide floor (not 0), and a runtime started afterwards continues above
/// it: a handle from before the shutdown never names anything in the new one (ADR-022).
#[test]
fn a_snapshot_with_no_runtime_keeps_the_process_generation_floor() {
    let host = Embedder::start();
    let mut last = host.construct("Counter", &[]);
    for _ in 0..3 {
        last = host.construct("Counter", &[]);
    }
    let floor_of = |bytes: Vec<u8>| {
        Snapshot::decode(&mut Reader::new(&bytes))
            .unwrap()
            .generation_floor
    };
    let running = floor_of(take(undra_snapshot()));
    assert!(running >= last.generation());

    undra_shutdown();
    let down = take(undra_snapshot());
    let decoded = Snapshot::decode(&mut Reader::new(&down)).unwrap();
    assert_eq!(decoded.stores.len(), 0, "a stopped runtime has no stores");
    assert!(
        decoded.generation_floor >= running,
        "floor after shutdown {} fell below the {} of the running runtime",
        decoded.generation_floor,
        running
    );

    // The next runtime issues above it, so the old handle cannot be mistaken for a new object.
    assert_eq!(init_raw(&config("inproc", 2), &host.cap), init_code::OK);
    let fresh = host.construct("Counter", &[]);
    assert!(fresh.generation() > last.generation());
    assert!(fresh.generation() > decoded.generation_floor);
    assert_eq!(
        host.sync(method(last, "Counter", "bump"), &[]).0,
        ReplyStatus::BadRequest
    );
}

/// H1 over the C ABI: a handle the host still holds (here `b`, issued after the snapshot) must
/// never name an object created after the restore.
#[test]
fn restore_never_reissues_a_generation_the_host_may_hold() {
    let host = Embedder::start();
    let a = host.construct("Counter", &[]);
    let snapshot = take(undra_snapshot());
    undra_release(a.0);
    let b = host.construct("Counter", &[]);
    assert_eq!(b.index(), a.index(), "the slot is reused");
    assert_eq!(restore(&snapshot), 0);

    let (stale, _) = host.sync(method(b, "Counter", "bump"), &[]);
    assert_eq!(stale, ReplyStatus::BadRequest, "b was not in the snapshot");
    undra_release(a.0);
    let c = host.construct("Counter", &[]);
    assert_eq!(c.index(), b.index());
    assert_ne!(c, b, "the stale handle must not name the new object");
    assert!(c.generation() > b.generation());
    let (still_stale, _) = host.sync(method(b, "Counter", "bump"), &[]);
    assert_eq!(still_stale, ReplyStatus::BadRequest);
    let (live, _) = host.sync(method(c, "Counter", "bump"), &[]);
    assert_eq!(live, ReplyStatus::Ok);
}

#[test]
fn stats_json_parses_and_counts_crossings() {
    let host = Embedder::start();
    let counter = host.construct("Counter", &[]);
    host.sync(method(counter, "Counter", "bump"), &[]);
    let stats: serde_json::Value = serde_json::from_slice(&take(undra_stats_json())).unwrap();
    assert_eq!(stats["platform"], "test");
    assert_eq!(stats["mode"], "inproc");
    assert_eq!(stats["live_handles"], 1);
    assert!(stats["crossings"]["calls"].as_u64().unwrap() >= 2);
}

// ---------------------------------------------------------------------------------------------
// Scenarios: ports, logs and events
// ---------------------------------------------------------------------------------------------

#[test]
fn sync_port_answers_through_out_reply_and_the_core_frees_it() {
    let host = Embedder::start();
    let calc = host.calculator(0);
    let args: Vec<u8> = [20_u32.encode_to_vec(), 22_u32.encode_to_vec()].concat();
    for _ in 0..100 {
        let (status, body) = host.sync(method(calc, "Calculator", "sum_on_host"), &args);
        assert_eq!(
            (status, u32::decode_exact(&body).unwrap()),
            (ReplyStatus::Ok, 42)
        );
    }
    host.cap.with(|inner| {
        let call = inner.port_calls.last().unwrap();
        assert_eq!(call.port_id, SUM_PORT);
        assert_eq!(call.method_id, ids::port_method_id("Sum", "add"));
        assert_eq!(call.args, args);
    });
}

/// M1: a host that fills `cap` (the natural reading of the field) on its `malloc`ed reply must
/// not make the core free a C block with Rust's allocator: `out_reply` is always `free`d. Under
/// Miri a mismatched deallocator is reported as undefined behaviour, so this is the test
/// `cargo +nightly miri test -p undra-ffi --test abi -- cap_set` runs.
#[test]
fn a_host_that_sets_cap_on_its_malloc_block_is_still_freed_with_free() {
    let host = Embedder::start();
    let calc = host.calculator(0);
    let args: Vec<u8> = [20_u32.encode_to_vec(), 22_u32.encode_to_vec()].concat();
    REPLY_CAP_IS_LEN.store(true, Ordering::Release);
    for _ in 0..10 {
        let (status, body) = host.sync(method(calc, "Calculator", "sum_on_host"), &args);
        assert_eq!(
            (status, u32::decode_exact(&body).unwrap()),
            (ReplyStatus::Ok, 42)
        );
    }
    REPLY_CAP_IS_LEN.store(false, Ordering::Release);
}

#[test]
fn async_port_replies_later_through_undra_port_reply() {
    let host = Embedder::start();
    host.cap.set_echo_mode(PortMode::Async);
    let calc = host.calculator(0);
    let id = host.start_call(
        method(calc, "Calculator", "ping_host"),
        &7_u32.encode_to_vec(),
    );
    let port_call_id = host.cap.wait("the port call", |inner| {
        inner.port_calls.last().map(|c| c.port_call_id)
    });
    host.cap
        .with(|inner| assert!(inner.replies.is_empty(), "no reply before the host answers"));
    let reply = port_reply_payload(port_call_id, PortStatus::Ok, &1234_u32.to_le_bytes());
    port_reply(&reply);
    let (status, body) = host.cap.reply(id);
    assert_eq!(
        (status, u32::decode_exact(&body).unwrap()),
        (ReplyStatus::Ok, 1234)
    );
    // A late duplicate, a reply for an unknown id and garbage are ignored.
    port_reply(&reply);
    port_reply(&[1, 2, 3]);
    port_reply(&[]);
}

#[test]
fn sync_answer_from_an_async_capable_port_also_works() {
    let host = Embedder::start();
    host.cap.set_echo_mode(PortMode::Sync);
    let calc = host.calculator(0);
    let (status, body) = host.run(
        method(calc, "Calculator", "ping_host"),
        &5_u32.encode_to_vec(),
    );
    assert_eq!(
        (status, u32::decode_exact(&body).unwrap()),
        (ReplyStatus::Ok, 1005)
    );
}

#[test]
fn unavailable_and_unregistered_ports_fail_the_call_as_a_contained_panic() {
    let host = Embedder::start();
    host.cap.set_echo_mode(PortMode::Unavailable);
    let calc = host.calculator(0);
    let (status, body) = host.run(
        method(calc, "Calculator", "ping_host"),
        &7_u32.encode_to_vec(),
    );
    assert_eq!(status, ReplyStatus::Panic);
    assert!(Reader::new(&body).read_str().unwrap().contains("Echo"));
    // Unregister the port entirely (null callback): behaves as unavailable too.
    // SAFETY: a null callback only removes the registration.
    unsafe { undra_port_register(ECHO_PORT, None, std::ptr::null_mut()) };
    host.cap.set_echo_mode(PortMode::Sync);
    let (status, _) = host.run(
        method(calc, "Calculator", "ping_host"),
        &7_u32.encode_to_vec(),
    );
    assert_eq!(status, ReplyStatus::Panic);
}

#[test]
fn ports_registered_before_init_apply_after_it() {
    let host = Embedder::start_with(&config("inproc", 2), true);
    let calc = host.calculator(0);
    let args: Vec<u8> = [1_u32.encode_to_vec(), 2_u32.encode_to_vec()].concat();
    let (status, body) = host.sync(method(calc, "Calculator", "sum_on_host"), &args);
    assert_eq!(
        (status, u32::decode_exact(&body).unwrap()),
        (ReplyStatus::Ok, 3)
    );
}

#[test]
fn core_log_records_reach_the_log_port_and_respect_the_level() {
    let host = Embedder::start();
    // A bogus observe logs a warning (level 3 >= 2) through the runtime.
    undra_observe(0x7777_7777_0000_0001, 0, 1);
    host.cap.with(|inner| {
        let warn = inner
            .logs
            .iter()
            .find(|(level, target, _)| *level == 3 && target == "undra::runtime");
        assert!(warn.is_some(), "logs: {:?}", inner.logs);
    });
    // A release of a stale handle logs at debug (1 < 2): filtered by the configured level.
    host.cap.with(|inner| inner.logs.clear());
    undra_release(0x7777_7777_0000_0001);
    host.cap
        .with(|inner| assert!(inner.logs.is_empty(), "{:?}", inner.logs));
}

#[test]
fn events_reach_subscribers_on_the_core() {
    let host = Embedder::start();
    let seen = Arc::new(Mutex::new(Vec::<Vec<u8>>::new()));
    let sink = seen.clone();
    let runtime = Runtime::global().expect("a running runtime");
    let _subscription = runtime.events().subscribe(
        0xE0E0_0001,
        0xE0E0_0002,
        Box::new(move |_ctx: &undra::runtime::Ctx, payload: &[u8]| {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(payload.to_vec());
        }),
    );
    event(0xE0E0_0001, 0xE0E0_0002, &[9, 8, 7]);
    // An event nobody subscribed to is dropped.
    event(0xE0E0_0001, 0x1234, &[1]);
    assert_eq!(*seen.lock().unwrap(), [vec![9, 8, 7]]);
    drop(host);
}

// ---------------------------------------------------------------------------------------------
// Scenarios: hostile input
// ---------------------------------------------------------------------------------------------

/// Random and mutated bytes into every entry point that takes a payload: nothing panics, aborts
/// or answers status 2 (a decode failure is status 5), and the core still works afterwards.
#[test]
fn arbitrary_bytes_never_break_the_boundary() {
    use proptest::prelude::*;
    use proptest::test_runner::{Config, TestRunner};

    let host = Embedder::start();
    let calc = host.calculator(1);
    let counter = host.construct("Counter", &[]);
    let valid_add = call_payload(
        method(calc, "Calculator", "add"),
        1,
        &[2_i64.encode_to_vec(), 3_i64.encode_to_vec()].concat(),
    );
    let valid_snapshot = take(undra_snapshot());

    // Random bytes, plus a valid payload with a few bytes flipped and a random truncation.
    let strategy = (
        proptest::collection::vec(any::<u8>(), 0..96),
        proptest::collection::vec((0..valid_add.len(), any::<u8>()), 0..4),
        0..=valid_add.len(),
        any::<u32>(),
        any::<u64>(),
    );
    let mut runner = TestRunner::new(Config {
        cases: 3000,
        failure_persistence: None,
        ..Config::default()
    });
    runner
        .run(&strategy, |(noise, flips, cut, id, handle)| {
            let mut mutated = valid_add.clone();
            for (at, byte) in &flips {
                mutated[*at] = *byte;
            }
            mutated.truncate(cut);
            for payload in [&noise, &mutated] {
                // undra_call: accepted (0) or refused (5); never a panic reply.
                let code = submit(payload);
                prop_assert!(code == 0 || code == 5, "undra_call answered {code}");
                // undra_call_sync: always a decodable Reply that is not a panic.
                let reply = take(call_sync_raw(payload));
                let parsed = Reply::decode(&mut Reader::new(&reply));
                prop_assert!(parsed.is_ok(), "undra_call_sync returned a malformed reply");
                prop_assert_ne!(parsed.unwrap().status, ReplyStatus::Panic);
                port_reply(payload);
                event(id, id.rotate_left(7), payload);
                // Bytes that happen to be a valid snapshot (four zero bytes: no stores) would
                // legitimately restore, and drop the objects the test still uses.
                if Snapshot::decode(&mut Reader::new(payload)).is_err() {
                    prop_assert_ne!(restore(payload), 0, "random bytes are not a snapshot");
                }
            }
            undra_cancel(id);
            undra_stream_credit(id, id);
            undra_timer_fired(id);
            undra_observe(handle, id, 1);
            undra_observe(handle, id, 0);
            undra_release(handle);
            Ok(())
        })
        .expect("the boundary survives arbitrary bytes");

    // Nothing asynchronous is left running that we did not start, no panic was counted, and
    // both objects still work.
    let stats: serde_json::Value = serde_json::from_slice(&take(undra_stats_json())).unwrap();
    assert_eq!(stats["panics"], 0);
    let args: Vec<u8> = [2_i64.encode_to_vec(), 3_i64.encode_to_vec()].concat();
    let (status, body) = host.sync(method(calc, "Calculator", "add"), &args);
    assert_eq!(
        (status, i64::decode_exact(&body).unwrap()),
        (ReplyStatus::Ok, 6)
    );
    assert_eq!(
        host.sync(method(counter, "Counter", "bump"), &[]).0,
        ReplyStatus::Ok
    );
    // The valid snapshot from before still restores.
    assert_eq!(restore(&valid_snapshot), 0);
}

// ---------------------------------------------------------------------------------------------
// Scenarios: threads
// ---------------------------------------------------------------------------------------------

#[test]
fn many_threads_call_sync_and_async_at_once() {
    let host = Embedder::start();
    let calc = host.calculator(1);
    let next_id = Arc::new(AtomicU32::new(1_000_000));
    let mut workers = Vec::new();
    for t in 0..8_i64 {
        let next_id = next_id.clone();
        let cap = host.cap.clone();
        workers.push(std::thread::spawn(move || {
            for i in 0..100_i64 {
                let args: Vec<u8> = [t.encode_to_vec(), i.encode_to_vec()].concat();
                // Sync path on this thread.
                let id = next_id.fetch_add(1, Ordering::Relaxed);
                let (status, body) = sync_call(
                    &call_payload(method(calc, "Calculator", "add"), id, &args),
                    id,
                );
                assert_eq!(
                    (status, i64::decode_exact(&body).unwrap()),
                    (ReplyStatus::Ok, 1 + t + i)
                );
                // Async path: the reply arrives on the core thread.
                let id = next_id.fetch_add(1, Ordering::Relaxed);
                let payload = call_payload(method(calc, "Calculator", "add"), id, &args);
                assert_eq!(submit(&payload), 0);
                let (status, body) = cap.reply(id);
                assert_eq!(
                    (status, i64::decode_exact(&body).unwrap()),
                    (ReplyStatus::Ok, 1 + t + i)
                );
            }
        }));
    }
    for worker in workers {
        worker.join().expect("a worker finished without panicking");
    }
    host.cap.with(|inner| assert!(inner.replies.is_empty()));
}

/// Review (runtime-lifecycle, surface 1): `undra_shutdown` is what the JNI `shutdown` native runs
/// (`session::stop`, ADR-034). Here it races host threads that are inside `undra_call`,
/// `undra_call_sync`, `undra_stream_credit`, `undra_cancel` and `undra_stats_json`, and a second
/// `undra_shutdown` on another thread; then a third thread starts a new core. Nothing crashes
/// (Miri runs this too), every call is answered at most once, and the new core works.
#[test]
fn shutdown_racing_host_threads_and_a_second_shutdown_answers_each_call_at_most_once() {
    use std::sync::atomic::AtomicBool;
    let host = Embedder::start();
    let calc = host.calculator(0);
    let stop = Arc::new(AtomicBool::new(false));
    let workers: Vec<_> = (0..3_u32)
        .map(|n| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut i = 0_u32;
                while !stop.load(Ordering::SeqCst) {
                    let id = 1_000_000 + n * 100_000 + i;
                    let args = [1_i64.encode_to_vec(), 2_i64.encode_to_vec()].concat();
                    let (status, _) = sync_call(
                        &call_payload(method(calc, "Calculator", "add"), id, &args),
                        id,
                    );
                    assert!(
                        matches!(status, ReplyStatus::Ok | ReplyStatus::BadRequest),
                        "{status:?}"
                    );
                    let id = id + 50_000;
                    let _ = submit(&call_payload(method(calc, "Calculator", "never"), id, &[]));
                    undra_stream_credit(id, 1);
                    if i % 2 == 0 {
                        undra_cancel(id);
                    }
                    take(undra_stats_json());
                    i += 1;
                }
                i
            })
        })
        .collect();
    std::thread::sleep(StdDuration::from_millis(5));
    let second = std::thread::spawn(undra_shutdown);
    undra_shutdown();
    second.join().expect("the second shutdown returned");
    stop.store(true, Ordering::SeqCst);
    for worker in workers {
        worker.join().expect("a host thread survived the shutdown");
    }
    assert!(Runtime::global().is_none());
    // At most one reply per call id, whoever answered it (the call, a cancel or the shutdown).
    host.cap.with(|inner| {
        let mut ids: Vec<u32> = inner.replies.iter().map(|(id, _)| *id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "a call was answered twice");
    });
    // A new core, started from another thread, works.
    let cap = host.cap.clone();
    let code = std::thread::spawn(move || init_raw(&config("inproc", 2), &cap))
        .join()
        .unwrap();
    assert_eq!(code, init_code::OK);
    assert_eq!(host.sync(function("version"), &[]).0, ReplyStatus::Ok);
}

#[test]
fn shutdown_with_calls_in_flight_returns_and_leaves_no_thread_behind() {
    let host = Embedder::start();
    let calc = host.calculator(0);
    for _ in 0..10 {
        host.start_call(method(calc, "Calculator", "never"), &[]);
    }
    undra_shutdown();
    assert!(Runtime::global().is_none());
    // Nothing of the core is left running: its threads were joined (ADR-034), and the stats of
    // "no runtime" say so, which is what the Kotlin and Swift contract runners check after close.
    let stats: serde_json::Value = serde_json::from_slice(&take(undra_stats_json())).unwrap();
    assert_eq!(stats["initialized"], false);
    assert_eq!(stats["runtime_threads"], 0, "{stats}");
    // The registrations were dropped with the runtime and a fresh one starts cleanly.
    assert_eq!(init_raw(&config("inproc", 2), &host.cap), init_code::OK);
    assert_eq!(host.sync(function("version"), &[]).0, ReplyStatus::Ok);
}
