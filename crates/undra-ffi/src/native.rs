//! The native C ABI (SPEC 6): every `undra_*` function, exported with `#[unsafe(no_mangle)]`.
//!
//! The C prototypes are in `runtimes/swift/UndraRuntime/Sources/UndraFFI/include/undra.h`; each
//! function documents its own. Conventions:
//!
//! * every `ptr, len` pair is borrowed for the duration of the call (a null `ptr` is an empty
//!   payload);
//! * buffers the core returns are [`UndraBuf`]s, owned by the caller until
//!   [`undra_buf_free`];
//! * the reply, change-set, stream and port callbacks run on the core thread, a blocking thread
//!   or the caller's thread, possibly while the core lock is held, **concurrently** with each
//!   other, and must be thread-safe, must not unwind and must not call back into the core
//!   (SPEC 5.1) except the entries that never take the core lock: [`undra_buf_free`],
//!   [`undra_port_reply`], [`undra_stream_credit`], [`undra_timer_fired`], [`undra_stats_json`] and
//!   the read-only [`undra_abi_version`], [`undra_schema_hash`] and [`undra_schema_json`]; the
//!   others are refused (`E_REENTRANT`) or must not be called from a callback at all. The
//!   complete host contract is the header comment of `undra.h` and SPEC 6;
//! * the callbacks and their `user` pointers stay valid until [`undra_shutdown`] returns, a port
//!   callback until [`undra_port_register`] has removed or replaced it (which waits for its running
//!   invocations, so the host may free `user` when it returns);
//! * nothing unwinds out of any function (constitution R6): a contained panic becomes a status 2
//!   reply, an error code, or nothing for `void` entries.

use core::ffi::c_void;
use std::sync::Arc;

use undra_runtime::{PortCallOutcome, Runtime};

use crate::api::{self, init_code};
use crate::buf::UndraBuf;
use crate::guard::guarded;
use crate::registry::{PORTS, UserPtr};
use crate::session::{self, Sink};

/// `void (*undra_reply_cb)(void *user, uint32_t call_id, const uint8_t *ptr, uint32_t len)`:
/// a `Reply` payload (SPEC 3.4) for an asynchronous call. `ptr` is valid only during the call.
pub type UndraReplyCb =
    unsafe extern "C" fn(user: *mut c_void, call_id: u32, ptr: *const u8, len: u32);

/// `void (*undra_changeset_cb)(void *user, const uint8_t *ptr, uint32_t len)`: a `ChangeSet`
/// payload (SPEC 3.5). `ptr` is valid only during the call.
pub type UndraChangesetCb = unsafe extern "C" fn(user: *mut c_void, ptr: *const u8, len: u32);

/// `void (*undra_stream_cb)(void *user, uint32_t call_id, const uint8_t *ptr, uint32_t len)`:
/// a `StreamItem` payload (SPEC 3.7). `ptr` is valid only during the call.
pub type UndraStreamCb =
    unsafe extern "C" fn(user: *mut c_void, call_id: u32, ptr: *const u8, len: u32);

/// `uint8_t (*undra_port_cb)(void *user, uint32_t port_id, uint32_t method_id,
/// uint32_t port_call_id, const uint8_t *ptr, uint32_t len, UndraBuf *out_reply)`:
/// the core calls a platform port (SPEC 6.3). `ptr, len` are the encoded arguments, valid only
/// during the call. The return value is
///
/// * `0`: answered synchronously, `*out_reply` holds a complete `PortReply` payload
///   (`port_call_id u32, status u8, body`);
/// * `1`: the answer comes later through `undra_port_reply`;
/// * `2` (or anything else): the port is unavailable.
///
/// **Host reply memory rule** (the one place where the host, not the core, allocates a
/// buffer): on `0` the host stores in `*out_reply` a block from the C allocator (`malloc(n)`)
/// with `len = n`. **The block is always released with `free`**, by the core, once it has copied
/// the bytes: never with `undra_buf_free`, never as a Rust `Vec`, whatever the other fields say.
/// `cap` is reserved and ignored: set it to `0`. (An earlier draft let a non-zero `cap` mark a
/// Rust-allocated buffer; a host that filled `cap` with the natural meaning of the word then
/// made the core free a C block with Rust's allocator, which is undefined behaviour, so the
/// branch is gone.) For `1` and `2` the host leaves `*out_reply` untouched.
pub type UndraPortCb = unsafe extern "C" fn(
    user: *mut c_void,
    port_id: u32,
    method_id: u32,
    port_call_id: u32,
    ptr: *const u8,
    len: u32,
    out_reply: *mut UndraBuf,
) -> u8;

unsafe extern "C" {
    /// The C allocator's `free`, for host-allocated port replies (always `malloc`ed).
    fn free(ptr: *mut c_void);
}

/// The embedder of a C host: its three callbacks and its `user` pointer.
struct CSink {
    reply: UndraReplyCb,
    changes: UndraChangesetCb,
    stream: UndraStreamCb,
    user: UserPtr,
}

impl CSink {
    /// The identity of the embedder, for the idempotence check.
    fn identity(&self) -> (usize, usize, usize, *mut c_void) {
        (
            self.reply as usize,
            self.changes as usize,
            self.stream as usize,
            self.user.0,
        )
    }
}

/// `Some(len)` when a payload fits the C ABI's `u32` lengths.
fn c_len(payload: &[u8]) -> Option<u32> {
    u32::try_from(payload.len()).ok()
}

/// Reads and releases the reply a synchronous port callback left in `out`.
///
/// The block is the host's `malloc`ed memory and is released with `free`, always: `out.cap` is
/// reserved and never looked at (see [`UndraPortCb`]).
///
/// # Safety
///
/// `out` must be what the host stored on returning `0` from the port callback, following the
/// memory rule of [`UndraPortCb`]: null, or `len` readable bytes in a block from the C allocator
/// that the host hands over.
unsafe fn take_host_reply(out: UndraBuf) -> Option<Vec<u8>> {
    if out.ptr.is_null() {
        return None;
    }
    let bytes = if out.len == 0 {
        None
    } else {
        // SAFETY: the host promised `len` readable bytes at `ptr` and keeps them alive until the
        // callback's caller (us) releases the block below.
        Some(unsafe { core::slice::from_raw_parts(out.ptr, out.len as usize) }.to_vec())
    };
    // SAFETY: the memory rule makes `ptr` a block from the C allocator that the host handed over.
    unsafe { free(out.ptr.cast()) };
    bytes
}

impl Sink for CSink {
    fn reply(&self, call_id: u32, payload: &[u8]) {
        let Some(len) = c_len(payload) else { return };
        // SAFETY: `reply` and `user` were supplied by the host in `undra_init`, which requires
        // them to stay valid until `undra_shutdown`; `payload` is valid for `len` bytes for the
        // duration of the call.
        unsafe { (self.reply)(self.user.0, call_id, payload.as_ptr(), len) };
    }

    fn change_set(&self, payload: &[u8]) {
        let Some(len) = c_len(payload) else { return };
        // SAFETY: as in `reply`.
        unsafe { (self.changes)(self.user.0, payload.as_ptr(), len) };
    }

    fn stream_item(&self, call_id: u32, payload: &[u8]) {
        let Some(len) = c_len(payload) else { return };
        // SAFETY: as in `reply`.
        unsafe { (self.stream)(self.user.0, call_id, payload.as_ptr(), len) };
    }

    fn port_call(
        &self,
        port_id: u32,
        method_id: u32,
        port_call_id: u32,
        args: &[u8],
    ) -> PortCallOutcome {
        let Some(len) = c_len(args) else {
            return PortCallOutcome::Unavailable;
        };
        // Held until this call is done with the host's memory: `undra_port_register` (replace or
        // remove) and `undra_shutdown` wait for it, so the host may free `user` when they return.
        let Some(invocation) = PORTS.enter(port_id) else {
            return PortCallOutcome::Unavailable;
        };
        let mut out = UndraBuf::EMPTY;
        // SAFETY: `callback` and `user` come from `undra_port_register`; the invocation keeps that
        // registration alive, because removing it waits until the invocation is dropped (after
        // the reply below was read and released). `args` is valid for `len` bytes and `out` is a
        // live `UndraBuf` the callback may fill.
        let answer = unsafe {
            (invocation.callback())(
                invocation.user(),
                port_id,
                method_id,
                port_call_id,
                args.as_ptr(),
                len,
                &raw mut out,
            )
        };
        match answer {
            0 => {
                // SAFETY: the host returned 0, so `out` follows the UndraPortCb memory rule.
                match unsafe { take_host_reply(out) } {
                    Some(reply) => PortCallOutcome::Sync(reply),
                    None => PortCallOutcome::Unavailable,
                }
            }
            1 => PortCallOutcome::Async,
            _ => PortCallOutcome::Unavailable,
        }
    }

    fn same_embedder(&self, other: &dyn Sink) -> bool {
        other
            .as_any()
            .downcast_ref::<CSink>()
            .is_some_and(|other| self.identity() == other.identity())
    }

    fn as_any(&self) -> &dyn core::any::Any {
        self
    }
}

/// Borrows `ptr, len` as a slice; a null `ptr` or `len == 0` is the empty slice.
///
/// # Safety
///
/// A non-null `ptr` must be valid for reads of `len` bytes for the returned lifetime.
unsafe fn bytes<'a>(ptr: *const u8, len: u32) -> &'a [u8] {
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        // SAFETY: the caller guarantees validity; the pointer is non-null and `len > 0`.
        unsafe { core::slice::from_raw_parts(ptr, len as usize) }
    }
}

/// `uint32_t undra_abi_version(void)`: the C ABI version this library implements, `1`.
#[unsafe(no_mangle)]
pub extern "C" fn undra_abi_version() -> u32 {
    api::ABI_VERSION
}

/// `uint64_t undra_schema_hash(void)`: the hash of the schema this core was built with
/// (`fnv1a64` of the canonical schema JSON, SPEC 2.3). Works before `undra_init`.
#[unsafe(no_mangle)]
pub extern "C" fn undra_schema_hash() -> u64 {
    guarded("undra_schema_hash", |_| 0, api::schema_hash)
}

/// `UndraBuf undra_schema_json(void)`: an owned copy of the canonical schema JSON (UTF-8). Free it
/// with [`undra_buf_free`]. Works before `undra_init`; `undra-cli` calls it to run bindgen.
#[unsafe(no_mangle)]
pub extern "C" fn undra_schema_json() -> UndraBuf {
    guarded(
        "undra_schema_json",
        |_| UndraBuf::EMPTY,
        || UndraBuf::from_vec(api::schema_json()),
    )
}

/// `uint32_t undra_init(const uint8_t *cfg, uint32_t len, undra_reply_cb reply,
/// undra_changeset_cb changes, undra_stream_cb stream, void *user)`.
///
/// Starts the process-global runtime. `cfg, len` is an encoded `RuntimeConfig`
/// (`platform String, mode String, core_threads u8, blocking_threads u8, log_level u8`); the
/// three callbacks and `user` are how the core reaches the host. The native ABI has no
/// `undra_poll`, so `core_threads == 0` is treated as `1`: the `undra-core` thread always runs.
///
/// Returns `0` on success, otherwise an [`init_code`]. **Idempotent per process**: calling it
/// again with the same callbacks and `user` while the runtime is up does nothing and returns `0`;
/// with different ones it returns `ALREADY_INITIALIZED` and changes nothing. After
/// [`undra_shutdown`] it can be called again.
///
/// # Safety
///
/// `cfg` must be null or valid for `len` bytes. The callbacks and `user` must stay valid, and
/// the callbacks must be thread-safe (they run concurrently on arbitrary threads) and must not
/// unwind, until [`undra_shutdown`] returns. See the host contract in `undra.h`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn undra_init(
    cfg: *const u8,
    len: u32,
    reply: Option<UndraReplyCb>,
    changes: Option<UndraChangesetCb>,
    stream: Option<UndraStreamCb>,
    user: *mut c_void,
) -> u32 {
    let (Some(reply), Some(changes), Some(stream)) = (reply, changes, stream) else {
        return init_code::BAD_ARGUMENT;
    };
    // SAFETY: the caller guarantees `cfg` is valid for `len` bytes.
    let config = unsafe { bytes(cfg, len) };
    let sink = Arc::new(CSink {
        reply,
        changes,
        stream,
        user: UserPtr(user),
    });
    session::start(config, sink, |runtime: &Runtime| {
        // A host that registered a port takes it over from any default Rust binding.
        for id in PORTS.ids() {
            runtime.bind_foreign_port(id);
        }
    })
}

/// `void undra_shutdown(void)`: stops the runtime (idempotent), joins its threads, drops every
/// object and forgets the callbacks and port registrations. Calls made afterwards fail with
/// status 5 until `undra_init` runs again.
///
/// **Blocking:** it waits for the port callbacks still running on other threads (after the
/// runtime has stopped), so when it returns no callback is running or will start and the host may
/// free every `user` pointer. It does all this inside the critical section that serialises
/// `undra_init`, so an init on another thread waits for it. The host must not call it from inside
/// a callback (debug builds assert; release builds skip the waits that could never finish) nor
/// while holding a lock a port callback needs.
#[unsafe(no_mangle)]
pub extern "C" fn undra_shutdown() {
    session::stop();
}

/// `uint32_t undra_call(const uint8_t *ptr, uint32_t len)`: submits a `Call` payload (SPEC 3.3).
/// Returns `0` when accepted (the reply follows through `reply_cb`, for sync and async methods
/// alike) and `5` when rejected without a reply: an undecodable payload, `call_id == 0`, a
/// `call_id` already in flight, no runtime, or a call from inside a host callback.
///
/// # Safety
///
/// `ptr` must be null or valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn undra_call(ptr: *const u8, len: u32) -> u32 {
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    api::call(unsafe { bytes(ptr, len) })
}

/// `UndraBuf undra_call_sync(const uint8_t *ptr, uint32_t len)`: runs a synchronous method and
/// returns its whole `Reply` payload (SPEC 3.4) directly; an `async` method or a stream is
/// answered with status 5 without running. Free the buffer with [`undra_buf_free`].
///
/// # Safety
///
/// `ptr` must be null or valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn undra_call_sync(ptr: *const u8, len: u32) -> UndraBuf {
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    let payload = unsafe { bytes(ptr, len) };
    guarded(
        "undra_call_sync",
        |_| UndraBuf::EMPTY,
        || UndraBuf::from_vec(api::call_sync(payload)),
    )
}

/// `void undra_cancel(uint32_t call_id)`: cancels an in-flight call or stream. A plain call is
/// answered with status 3 exactly once; unknown ids are ignored.
#[unsafe(no_mangle)]
pub extern "C" fn undra_cancel(call_id: u32) {
    api::cancel(call_id);
}

/// `void undra_stream_credit(uint32_t call_id, uint32_t credit)`: lets the stream `call_id` send
/// `credit` more items (SPEC 3.7). Never takes the core lock.
#[unsafe(no_mangle)]
pub extern "C" fn undra_stream_credit(call_id: u32, credit: u32) {
    api::stream_credit(call_id, credit);
}

/// `void undra_observe(uint64_t handle, uint32_t signal_id, uint8_t on)`: starts (`on != 0`) or
/// stops observing a signal of a store (`signal_id == UINT32_MAX` for all). Starting delivers the
/// current values through `changeset_cb` before this returns. Unknown handles are ignored.
#[unsafe(no_mangle)]
pub extern "C" fn undra_observe(handle: u64, signal_id: u32, on: u8) {
    api::observe(handle, signal_id, on != 0);
}

/// `void undra_release(uint64_t handle)`: releases an object handle. Stale or unknown handles are
/// ignored.
#[unsafe(no_mangle)]
pub extern "C" fn undra_release(handle: u64) {
    api::release(handle);
}

/// `void undra_port_register(uint32_t port_id, undra_port_cb cb, void *user)`: routes calls to the
/// platform-implemented port `port_id` to `cb`. A null `cb` removes the registration. A port with
/// no registration behaves as unavailable (SPEC 6.3). Registering takes the port over from any
/// default Rust binding (for example the native `Timer`).
///
/// **Blocking:** removing a registration, or replacing it with another, returns only after every
/// invocation of the *old* callback running on another thread has returned, and the old callback
/// is never started again: the host may free the old `user` the moment this returns. Do not call
/// it from inside a port callback of the registration being removed (it would wait for itself:
/// debug builds assert, release builds skip that wait) nor while holding a lock a port callback
/// needs; a port callback that never returns keeps this call from returning.
///
/// # Safety
///
/// `cb` and `user` must stay valid until the registration is removed or replaced (this call
/// returns for that id) or [`undra_shutdown`] returns, and `cb` must be thread-safe (port calls
/// arrive concurrently on arbitrary threads) and must not unwind.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn undra_port_register(
    port_id: u32,
    cb: Option<UndraPortCb>,
    user: *mut c_void,
) {
    guarded(
        "undra_port_register",
        |_| (),
        || {
            match cb {
                Some(cb) => PORTS.install(port_id, cb, user),
                None => PORTS.remove(port_id),
            }
            if cb.is_some() {
                if let Some(runtime) = api::runtime() {
                    runtime.bind_foreign_port(port_id);
                }
            }
        },
    );
}

/// `void undra_port_reply(const uint8_t *ptr, uint32_t len)`: answers a port call the callback
/// deferred by returning `1` (a `PortReply` payload, SPEC 3.6). Never takes the core lock, so it
/// is safe from any thread, including from inside the port callback itself. A reply to an
/// abandoned call is discarded, and one carrying port call id `0` (the fire-and-forget id of the
/// `Log` port's calls) is ignored silently.
///
/// # Safety
///
/// `ptr` must be null or valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn undra_port_reply(ptr: *const u8, len: u32) {
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    api::port_reply(unsafe { bytes(ptr, len) });
}

/// `void undra_event(uint32_t port_id, uint32_t method_id, const uint8_t *ptr, uint32_t len)`:
/// delivers a host-to-core event of an event port (`Connectivity`, `Lifecycle`, ...) to the
/// core's subscribers.
///
/// # Safety
///
/// `ptr` must be null or valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn undra_event(port_id: u32, method_id: u32, ptr: *const u8, len: u32) {
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    api::event(port_id, method_id, unsafe { bytes(ptr, len) });
}

/// `void undra_timer_fired(uint32_t timer_id)`: tells the core that timer `timer_id` came due
/// (only used with a host-owned `Timer` port). Never takes the core lock.
#[unsafe(no_mangle)]
pub extern "C" fn undra_timer_fired(timer_id: u32) {
    api::timer_fired(timer_id);
}

/// `UndraBuf undra_snapshot(void)`: every store as a `Snapshot` payload (SPEC 5.9); an empty
/// snapshot before `undra_init`. Free the buffer with [`undra_buf_free`].
#[unsafe(no_mangle)]
pub extern "C" fn undra_snapshot() -> UndraBuf {
    guarded(
        "undra_snapshot",
        |_| UndraBuf::EMPTY,
        || UndraBuf::from_vec(api::snapshot()),
    )
}

/// `uint32_t undra_restore(const uint8_t *ptr, uint32_t len)`: rebuilds the stores from a
/// snapshot, re-issuing the same handles. Returns `0` on success, otherwise a
/// [`restore_code`](crate::restore_code); a failed restore leaves the core unchanged.
///
/// # Safety
///
/// `ptr` must be null or valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn undra_restore(ptr: *const u8, len: u32) -> u32 {
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    api::restore(unsafe { bytes(ptr, len) })
}

/// `UndraBuf undra_stats_json(void)`: a JSON document with the live handle count, tasks, calls,
/// transactions and the boundary crossing counters. Free the buffer with [`undra_buf_free`].
#[unsafe(no_mangle)]
pub extern "C" fn undra_stats_json() -> UndraBuf {
    guarded(
        "undra_stats_json",
        |_| UndraBuf::EMPTY,
        || UndraBuf::from_vec(api::stats_json().into_bytes()),
    )
}

/// `void undra_buf_free(UndraBuf buf)`: releases a buffer returned by the core. Empty buffers
/// (`cap == 0`) are ignored. Callable from inside a callback (it never touches the runtime).
///
/// # Safety
///
/// `buf` must be exactly a buffer this library returned, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn undra_buf_free(buf: UndraBuf) {
    // SAFETY: the caller guarantees `buf` came from this library and is freed once.
    unsafe { buf.free() };
}

#[cfg(test)]
mod tests {
    use super::*;

    unsafe extern "C" {
        fn malloc(size: usize) -> *mut c_void;
    }

    /// A reply the way a C host leaves it: `malloc`ed, `len` set, and whatever it put in `cap`.
    fn host_reply(bytes: &[u8], cap: u32) -> UndraBuf {
        // SAFETY: `malloc` of a non-zero size; the block is filled before it is used.
        let block = unsafe { malloc(bytes.len().max(1)) }.cast::<u8>();
        assert!(!block.is_null());
        // SAFETY: `block` is valid for `bytes.len()` bytes and does not overlap `bytes`.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), block, bytes.len()) };
        UndraBuf {
            ptr: block,
            len: u32::try_from(bytes.len()).expect("small"),
            cap,
        }
    }

    /// M1: `cap` is reserved. Whatever a host writes there, the block is released with `free`
    /// (Miri: a mismatched deallocator is undefined behaviour it reports).
    #[test]
    fn a_host_reply_is_released_with_free_whatever_cap_says() {
        for cap in [0, 5, 4096, u32::MAX] {
            let reply = host_reply(&[1, 2, 3, 4, 5], cap);
            // SAFETY: `reply` follows the memory rule of `UndraPortCb`.
            let taken = unsafe { take_host_reply(reply) };
            assert_eq!(taken, Some(vec![1, 2, 3, 4, 5]), "cap = {cap}");
        }
    }

    #[test]
    fn an_empty_or_missing_host_reply_is_unavailable_and_still_freed() {
        // A block with `len == 0` (the host allocated and wrote nothing): freed, no bytes.
        let empty = UndraBuf {
            len: 0,
            ..host_reply(&[9], 7)
        };
        // SAFETY: `empty` follows the memory rule (a `malloc`ed block, handed over).
        assert_eq!(unsafe { take_host_reply(empty) }, None);
        // The host returned 0 without storing anything: `out_reply` is still the empty buffer.
        // SAFETY: a null `ptr` is always acceptable.
        assert_eq!(unsafe { take_host_reply(UndraBuf::EMPTY) }, None);
    }
}
