//! The wasm ABI (SPEC 7) for `wasm32-unknown-unknown`: no wasm-bindgen, only `i32`, `i64` and
//! `f64` in exports and imports, memory is the module's exported `memory`.
//!
//! **Exports**: `keel_alloc`, `keel_free`, `keel_abi_version`, `keel_schema_hash`,
//! `keel_schema_json`, `keel_init`, `keel_call`, `keel_call_sync`, `keel_cancel`,
//! `keel_stream_credit`, `keel_observe`, `keel_release`, `keel_port_reply`, `keel_event`,
//! `keel_timer_fired`, `keel_poll`, `keel_snapshot`, `keel_restore`, `keel_buf_free`,
//! `keel_stats_json`, plus the reactor entry `_initialize` (the host calls it once after
//! instantiation; it runs the static constructors that register the core's schema).
//!
//! **Imports** (module `"keel"`): `reply`, `changeset`, `stream`, `port_call`, `schedule`,
//! `timer_set`, `log`, `now_ms`, `random`.
//!
//! Differences from the native ABI, all from SPEC 7: buffers are returned as a pointer to a
//! `KeelBuf { ptr i32, len i32, cap i32 }` struct in linear memory that `keel_buf_free` releases
//! together with its bytes; `u64` handles are split into `lo`/`hi` `i32` halves; there is no
//! `keel_shutdown` (drop the instance) and no `keel_port_register` (an unregistered port is
//! answered by the host's `port_call` import, which returns `2`); the host drives the executor
//! by calling `keel_poll` after `schedule`, and owns timers through `timer_set` and
//! `keel_timer_fired`.
//!
//! The `Clock`, `Rng` and `Log` ports have built-in bindings over the `now_ms`, `random` and
//! `log` imports. A `port_call` the host answers with `2` (unavailable) falls back to them, so a
//! host that registers its own `Clock` still wins.
//!
//! Panics abort on this target: the panic hook logs the message at level 5 through the `log`
//! import, then the module traps, so the host can restart from a snapshot.

use core::alloc::Layout;
use std::alloc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use keel_runtime::{Host, InitError, PortCallOutcome, Runtime};

use crate::api::{self, init_code, init_error_code, parse_config};
use crate::buf::KeelBuf;
use crate::builtin::{self, Monotonic, Platform};

#[link(wasm_import_module = "keel")]
unsafe extern "C" {
    #[link_name = "reply"]
    fn import_reply(call_id: u32, ptr: *const u8, len: u32);
    #[link_name = "changeset"]
    fn import_changeset(ptr: *const u8, len: u32);
    #[link_name = "stream"]
    fn import_stream(call_id: u32, ptr: *const u8, len: u32);
    #[link_name = "port_call"]
    fn import_port_call(
        port_id: u32,
        method_id: u32,
        port_call_id: u32,
        ptr: *const u8,
        len: u32,
    ) -> u32;
    #[link_name = "schedule"]
    fn import_schedule();
    #[link_name = "timer_set"]
    fn import_timer_set(timer_id: u32, delay_ms_lo: u32, delay_ms_hi: u32);
    #[link_name = "log"]
    fn import_log(level: u32, ptr: *const u8, len: u32);
    #[link_name = "now_ms"]
    fn import_now_ms() -> f64;
    #[link_name = "random"]
    fn import_random(ptr: *mut u8, len: u32);
}

unsafe extern "C" {
    /// Synthesized by `wasm-ld`: runs the module's static constructors (`inventory`
    /// registrations).
    fn __wasm_call_ctors();
}

/// The `"keel"` imports as the built-ins' [`Platform`].
struct Imports;

impl Platform for Imports {
    fn now_ms(&self) -> f64 {
        // SAFETY: `now_ms` is a host import with no memory arguments.
        unsafe { import_now_ms() }
    }

    fn random(&self, out: &mut [u8]) {
        let Ok(len) = u32::try_from(out.len()) else {
            return;
        };
        if len == 0 {
            return;
        }
        // SAFETY: `out` is valid for writes of `len` bytes; the host fills exactly that range.
        unsafe { import_random(out.as_mut_ptr(), len) };
    }

    fn log(&self, level: u8, bytes: &[u8]) {
        let Ok(len) = u32::try_from(bytes.len()) else {
            return;
        };
        // SAFETY: `bytes` is valid for reads of `len` bytes for the duration of the import.
        unsafe { import_log(u32::from(level), bytes.as_ptr(), len) };
    }
}

static MONOTONIC: Monotonic = Monotonic::new();

/// How many times the host called `keel_port_reply`: lets `port_call` tell whether a host that
/// answered `0` (synchronously) really delivered its reply before returning.
static PORT_REPLIES: AtomicU64 = AtomicU64::new(0);

/// Set once the runtime is up, so the crate's panic hook only logs when the runtime's own does
/// not (before `keel_init`).
static RUNTIME_LIVE: AtomicBool = AtomicBool::new(false);

/// The runtime's [`Host`]: every callback is an import.
struct WasmHost;

impl Host for WasmHost {
    fn reply(&self, call_id: u32, payload: &[u8]) {
        let Ok(len) = u32::try_from(payload.len()) else {
            return;
        };
        // SAFETY: `payload` is valid for `len` bytes for the duration of the import.
        unsafe { import_reply(call_id, payload.as_ptr(), len) };
    }

    fn change_set(&self, payload: &[u8]) {
        let Ok(len) = u32::try_from(payload.len()) else {
            return;
        };
        // SAFETY: as in `reply`.
        unsafe { import_changeset(payload.as_ptr(), len) };
    }

    fn stream_item(&self, call_id: u32, payload: &[u8]) {
        let Ok(len) = u32::try_from(payload.len()) else {
            return;
        };
        // SAFETY: as in `reply`.
        unsafe { import_stream(call_id, payload.as_ptr(), len) };
    }

    fn port_call(
        &self,
        port_id: u32,
        method_id: u32,
        port_call_id: u32,
        args: &[u8],
    ) -> PortCallOutcome {
        let Ok(len) = u32::try_from(args.len()) else {
            return PortCallOutcome::Unavailable;
        };
        let replies_before = PORT_REPLIES.load(Ordering::Acquire);
        // SAFETY: `args` is valid for `len` bytes for the duration of the import.
        let answer =
            unsafe { import_port_call(port_id, method_id, port_call_id, args.as_ptr(), len) };
        match answer {
            // The host promised to have called `keel_port_reply` before returning. The runtime
            // registered the call before invoking us, so that reply already completed it; if
            // the host lied, fail the call instead of leaving it pending forever.
            0 if PORT_REPLIES.load(Ordering::Acquire) != replies_before => PortCallOutcome::Async,
            0 => PortCallOutcome::Unavailable,
            1 => PortCallOutcome::Async,
            _ => builtin::answer(&Imports, &MONOTONIC, port_id, method_id, port_call_id, args)
                .map_or(PortCallOutcome::Unavailable, PortCallOutcome::Sync),
        }
    }

    fn log(&self, level: u8, target: &str, message: &str) {
        Imports.log(level, &builtin::log_record(target, message));
    }

    fn schedule(&self) {
        // SAFETY: `schedule` is a host import with no arguments.
        unsafe { import_schedule() };
    }

    fn timer_set(&self, timer_id: u32, delay_ms: u64) -> bool {
        // SAFETY: `timer_set` is a host import taking plain integers.
        unsafe {
            import_timer_set(
                timer_id,
                (delay_ms & 0xFFFF_FFFF) as u32,
                (delay_ms >> 32) as u32,
            );
        }
        true
    }
}

/// Runs the static constructors once and installs the panic hook. Called by `_initialize`, and
/// by the entry points that need the registry, so a host that forgets `_initialize` still gets
/// a working (if less tidy) module.
fn ensure_initialized() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::AcqRel) {
        return;
    }
    // SAFETY: `__wasm_call_ctors` is the linker-generated constructor runner; the `DONE` guard
    // makes this its only call from Rust (`inventory` tolerates the linker's own calls too).
    unsafe { __wasm_call_ctors() };
    std::panic::set_hook(Box::new(|info| {
        // The runtime's own hook (installed at `keel_init`) logs through the host once a
        // runtime exists; this one covers everything before it.
        if RUNTIME_LIVE.load(Ordering::Acquire) {
            return;
        }
        let message = info.payload().downcast_ref::<&str>().map_or_else(
            || {
                info.payload()
                    .downcast_ref::<String>()
                    .cloned()
                    .unwrap_or_else(|| "panic with a non-string payload".to_owned())
            },
            |text| (*text).to_owned(),
        );
        let place = info
            .location()
            .map(|l| format!(" at {}:{}", l.file(), l.line()))
            .unwrap_or_default();
        Imports.log(
            keel_runtime::log::FATAL,
            &builtin::log_record("keel::panic", &format!("{message}{place}")),
        );
    }));
}

/// The layout `keel_alloc` and `keel_free` agree on for a length (8-aligned, never empty).
fn layout(len: u32) -> Option<Layout> {
    Layout::from_size_align((len as usize).max(1), 8).ok()
}

/// Borrows `ptr, len` of linear memory as a slice (empty for `len == 0`).
///
/// # Safety
///
/// A non-null `ptr` must be valid for reads of `len` bytes for the returned lifetime.
unsafe fn bytes<'a>(ptr: *const u8, len: u32) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the caller guarantees validity; non-null and `len > 0` here.
        unsafe { core::slice::from_raw_parts(ptr, len as usize) }
    }
}

/// Moves `bytes` into a heap `KeelBuf` struct and returns its address: the `KeelBuf*` of SPEC 7.
fn boxed(bytes: Vec<u8>) -> *mut KeelBuf {
    Box::into_raw(Box::new(KeelBuf::from_vec(bytes)))
}

/// Reactor initialisation: runs the static constructors (the core's schema registers itself
/// there). The host calls it once after instantiation; repeated calls do nothing.
#[unsafe(no_mangle)]
pub extern "C" fn _initialize() {
    ensure_initialized();
}

/// `keel_alloc(len: i32) -> i32`: `len` bytes (8-aligned) of linear memory for the host to fill;
/// release with `keel_free(ptr, len)`. Traps on out-of-memory.
#[unsafe(no_mangle)]
pub extern "C" fn keel_alloc(len: u32) -> *mut u8 {
    let Some(layout) = layout(len) else {
        return core::ptr::null_mut();
    };
    // SAFETY: `layout` has a non-zero size.
    let ptr = unsafe { alloc::alloc(layout) };
    if ptr.is_null() {
        alloc::handle_alloc_error(layout);
    }
    ptr
}

/// `keel_free(ptr: i32, len: i32)`: releases memory from `keel_alloc(len)`.
///
/// # Safety
///
/// `ptr` must be null or come from `keel_alloc(len)` with the same `len`, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_free(ptr: *mut u8, len: u32) {
    if ptr.is_null() {
        return;
    }
    if let Some(layout) = layout(len) {
        // SAFETY: by the caller's contract `ptr` was allocated with exactly this layout.
        unsafe { alloc::dealloc(ptr, layout) };
    }
}

/// `keel_abi_version() -> i32`: `1`.
#[unsafe(no_mangle)]
pub extern "C" fn keel_abi_version() -> u32 {
    api::ABI_VERSION
}

/// `keel_schema_hash() -> i64`: the schema hash of this core (`u64` as `i64` bits).
#[unsafe(no_mangle)]
pub extern "C" fn keel_schema_hash() -> u64 {
    ensure_initialized();
    api::schema_hash()
}

/// `keel_schema_json() -> i32`: pointer to a `KeelBuf` holding the canonical schema JSON.
#[unsafe(no_mangle)]
pub extern "C" fn keel_schema_json() -> *mut KeelBuf {
    ensure_initialized();
    boxed(api::schema_json())
}

/// `keel_init(cfg_ptr, cfg_len) -> i32`: starts the runtime from an encoded `RuntimeConfig`;
/// `0` on success (also when already started), otherwise an [`init_code`]. The runtime is driven
/// by `keel_poll` whatever `core_threads` says.
///
/// # Safety
///
/// `cfg` must be valid for `len` bytes (a `keel_alloc`ed block the host filled).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_init(cfg: *const u8, len: u32) -> u32 {
    ensure_initialized();
    // SAFETY: the caller guarantees `cfg` is valid for `len` bytes.
    let Some(mut config) = parse_config(unsafe { bytes(cfg, len) }) else {
        return init_code::BAD_CONFIG;
    };
    config.core_threads = 0;
    match Runtime::init(config, Arc::new(WasmHost)) {
        Ok(_) => {
            RUNTIME_LIVE.store(true, Ordering::Release);
            init_code::OK
        }
        Err(InitError::AlreadyInitialized) if api::runtime().is_some() => init_code::OK,
        Err(error) => init_error_code(&error),
    }
}

/// `keel_call(ptr, len) -> i32`: submits a `Call` payload; `0` accepted (the reply arrives
/// through the `reply` import, possibly before this returns), `5` rejected.
///
/// # Safety
///
/// `ptr` must be valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_call(ptr: *const u8, len: u32) -> u32 {
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    api::call(unsafe { bytes(ptr, len) })
}

/// `keel_call_sync(ptr, len) -> i32`: pointer to a `KeelBuf` holding the whole `Reply` payload;
/// release it with `keel_buf_free`.
///
/// # Safety
///
/// `ptr` must be valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_call_sync(ptr: *const u8, len: u32) -> *mut KeelBuf {
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    boxed(api::call_sync(unsafe { bytes(ptr, len) }))
}

/// `keel_cancel(call_id)`: cancels an in-flight call or stream.
#[unsafe(no_mangle)]
pub extern "C" fn keel_cancel(call_id: u32) {
    api::cancel(call_id);
}

/// `keel_stream_credit(call_id, credit)`: lets a stream send `credit` more items.
#[unsafe(no_mangle)]
pub extern "C" fn keel_stream_credit(call_id: u32, credit: u32) {
    api::stream_credit(call_id, credit);
}

/// Joins the `lo`/`hi` halves of a handle (the `u64` split SPEC 7 uses to avoid `BigInt`).
fn join(lo: u32, hi: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

/// `keel_observe(handle_lo, handle_hi, signal_id, on)`: starts (`on != 0`) or stops observing;
/// starting delivers the current values through `changeset` before returning.
#[unsafe(no_mangle)]
pub extern "C" fn keel_observe(handle_lo: u32, handle_hi: u32, signal_id: u32, on: u32) {
    api::observe(join(handle_lo, handle_hi), signal_id, on != 0);
}

/// `keel_release(handle_lo, handle_hi)`: releases an object handle.
#[unsafe(no_mangle)]
pub extern "C" fn keel_release(handle_lo: u32, handle_hi: u32) {
    api::release(join(handle_lo, handle_hi));
}

/// `keel_port_reply(ptr, len)`: a `PortReply` payload. Also legal from inside the `port_call`
/// import, which is how a host answers a synchronous port call (`port_call` then returns `0`).
///
/// # Safety
///
/// `ptr` must be valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_port_reply(ptr: *const u8, len: u32) {
    PORT_REPLIES.fetch_add(1, Ordering::AcqRel);
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    api::port_reply(unsafe { bytes(ptr, len) });
}

/// `keel_event(port_id, method_id, ptr, len)`: a host-to-core event of an event port.
///
/// # Safety
///
/// `ptr` must be valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_event(port_id: u32, method_id: u32, ptr: *const u8, len: u32) {
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    api::event(port_id, method_id, unsafe { bytes(ptr, len) });
}

/// `keel_timer_fired(timer_id)`: the timer requested through `timer_set` came due.
#[unsafe(no_mangle)]
pub extern "C" fn keel_timer_fired(timer_id: u32) {
    api::timer_fired(timer_id);
}

/// `keel_poll()`: drives the executor one turn; the host calls it (on the next microtask) after
/// the `schedule` import. The runtime asks for another turn itself while tasks remain.
#[unsafe(no_mangle)]
pub extern "C" fn keel_poll() {
    api::poll();
}

/// `keel_snapshot() -> i32`: pointer to a `KeelBuf` holding a `Snapshot` payload.
#[unsafe(no_mangle)]
pub extern "C" fn keel_snapshot() -> *mut KeelBuf {
    boxed(api::snapshot())
}

/// `keel_restore(ptr, len) -> i32`: `0` on success, otherwise a
/// [`restore_code`](crate::restore_code); a failed restore leaves the core unchanged.
///
/// # Safety
///
/// `ptr` must be valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_restore(ptr: *const u8, len: u32) -> u32 {
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    api::restore(unsafe { bytes(ptr, len) })
}

/// `keel_buf_free(buf_ptr)`: releases a `KeelBuf*` returned by `keel_schema_json`,
/// `keel_call_sync`, `keel_snapshot` or `keel_stats_json`, and the bytes it owns.
///
/// # Safety
///
/// `buf` must be null or exactly a pointer this module returned, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_buf_free(buf: *mut KeelBuf) {
    if buf.is_null() {
        return;
    }
    // SAFETY: by the caller's contract `buf` came from `boxed`, i.e. `Box::into_raw`.
    let inner = *unsafe { Box::from_raw(buf) };
    // SAFETY: the inner buffer was built by `KeelBuf::from_vec` and is freed exactly once here.
    unsafe { inner.free() };
}

/// `keel_stats_json() -> i32`: pointer to a `KeelBuf` holding the statistics JSON.
#[unsafe(no_mangle)]
pub extern "C" fn keel_stats_json() -> *mut KeelBuf {
    boxed(api::stats_json().into_bytes())
}
