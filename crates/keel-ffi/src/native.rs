//! The native C ABI (SPEC 6): every `keel_*` function, exported with `#[unsafe(no_mangle)]`.
//!
//! The C prototypes are in `runtimes/swift/KeelRuntime/Sources/KeelFFI/include/keel.h`; each
//! function documents its own. Conventions:
//!
//! * every `ptr, len` pair is borrowed for the duration of the call (a null `ptr` is an empty
//!   payload);
//! * buffers the core returns are [`KeelBuf`]s, owned by the caller until
//!   [`keel_buf_free`];
//! * the reply, change-set, stream and port callbacks run on the core thread, a blocking thread
//!   or the caller's thread, possibly while the core lock is held, and must not call back into
//!   the core (SPEC 5.1) except [`keel_buf_free`];
//! * nothing unwinds out of any function (constitution R6): a contained panic becomes a status 2
//!   reply, an error code, or nothing for `void` entries.

use core::ffi::c_void;
use std::collections::BTreeMap;
use std::sync::{Arc, PoisonError, RwLock};

use keel_runtime::{PortCallOutcome, Runtime};

use crate::api::{self, init_code};
use crate::buf::KeelBuf;
use crate::guard::guarded;
use crate::session::{self, Sink};

/// `void (*keel_reply_cb)(void *user, uint32_t call_id, const uint8_t *ptr, uint32_t len)`:
/// a `Reply` payload (SPEC 3.4) for an asynchronous call. `ptr` is valid only during the call.
pub type KeelReplyCb =
    unsafe extern "C" fn(user: *mut c_void, call_id: u32, ptr: *const u8, len: u32);

/// `void (*keel_changeset_cb)(void *user, const uint8_t *ptr, uint32_t len)`: a `ChangeSet`
/// payload (SPEC 3.5). `ptr` is valid only during the call.
pub type KeelChangesetCb = unsafe extern "C" fn(user: *mut c_void, ptr: *const u8, len: u32);

/// `void (*keel_stream_cb)(void *user, uint32_t call_id, const uint8_t *ptr, uint32_t len)`:
/// a `StreamItem` payload (SPEC 3.7). `ptr` is valid only during the call.
pub type KeelStreamCb =
    unsafe extern "C" fn(user: *mut c_void, call_id: u32, ptr: *const u8, len: u32);

/// `uint8_t (*keel_port_cb)(void *user, uint32_t port_id, uint32_t method_id,
/// uint32_t port_call_id, const uint8_t *ptr, uint32_t len, KeelBuf *out_reply)`:
/// the core calls a platform port (SPEC 6.3). `ptr, len` are the encoded arguments, valid only
/// during the call. The return value is
///
/// * `0`: answered synchronously, `*out_reply` holds a complete `PortReply` payload
///   (`port_call_id u32, status u8, body`);
/// * `1`: the answer comes later through `keel_port_reply`;
/// * `2` (or anything else): the port is unavailable.
///
/// **Host reply memory rule** (the one place where the host, not the core, allocates a
/// buffer): on `0` the host stores in `*out_reply` a block from the C allocator
/// (`malloc(n)`) with `len = n` and **`cap = 0`**. Ownership passes to the core when the callback
/// returns: it copies the bytes at once and releases the block with `free`, never with
/// `keel_buf_free`. A non-zero `cap` marks a buffer that this crate allocated (a Rust embedder
/// reusing a [`KeelBuf`]); the core then reclaims it as a `Vec`. For `1` and `2` the host leaves
/// `*out_reply` untouched.
pub type KeelPortCb = unsafe extern "C" fn(
    user: *mut c_void,
    port_id: u32,
    method_id: u32,
    port_call_id: u32,
    ptr: *const u8,
    len: u32,
    out_reply: *mut KeelBuf,
) -> u8;

unsafe extern "C" {
    /// The C allocator's `free`, for host-allocated port replies (`cap == 0`).
    fn free(ptr: *mut c_void);
}

/// An opaque host pointer handed back to its callbacks.
#[derive(Clone, Copy, PartialEq, Eq)]
struct UserPtr(*mut c_void);

// SAFETY: the pointer is never dereferenced here, only passed back to the host's own callbacks.
// SPEC 6 makes the host responsible for those callbacks being callable from any thread.
unsafe impl Send for UserPtr {}
// SAFETY: as above; sharing the value shares no data.
unsafe impl Sync for UserPtr {}

/// One registered port callback.
#[derive(Clone, Copy)]
struct PortReg {
    cb: KeelPortCb,
    user: UserPtr,
}

/// Port callbacks by port id. Survives between `keel_port_register` and `keel_shutdown`; a
/// registration made before `keel_init` applies once the runtime is up.
static PORTS: RwLock<BTreeMap<u32, PortReg>> = RwLock::new(BTreeMap::new());

fn port_registration(port_id: u32) -> Option<PortReg> {
    PORTS
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&port_id)
        .copied()
}

/// The embedder of a C host: its three callbacks and its `user` pointer.
struct CSink {
    reply: KeelReplyCb,
    changes: KeelChangesetCb,
    stream: KeelStreamCb,
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
/// # Safety
///
/// `out` must be what the host stored on returning `0` from the port callback, following the
/// memory rule of [`KeelPortCb`].
unsafe fn take_host_reply(out: KeelBuf) -> Option<Vec<u8>> {
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
    if out.cap == 0 {
        // SAFETY: `cap == 0` means a block from the C allocator that the host handed over.
        unsafe { free(out.ptr.cast()) };
    } else {
        // SAFETY: a non-zero `cap` means a buffer built by `KeelBuf::from_vec`, which
        // `KeelBuf::free` reclaims exactly once.
        unsafe { out.free() };
    }
    bytes
}

impl Sink for CSink {
    fn reply(&self, call_id: u32, payload: &[u8]) {
        let Some(len) = c_len(payload) else { return };
        // SAFETY: `reply` and `user` were supplied by the host in `keel_init`, which requires
        // them to stay valid until `keel_shutdown`; `payload` is valid for `len` bytes for the
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
        let Some(reg) = port_registration(port_id) else {
            return PortCallOutcome::Unavailable;
        };
        let Some(len) = c_len(args) else {
            return PortCallOutcome::Unavailable;
        };
        let mut out = KeelBuf::EMPTY;
        // SAFETY: `cb` and `user` come from `keel_port_register`, whose contract keeps them valid
        // until `keel_shutdown`; `args` is valid for `len` bytes and `out` is a live `KeelBuf`
        // the callback may fill.
        let answer = unsafe {
            (reg.cb)(
                reg.user.0,
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
                // SAFETY: the host returned 0, so `out` follows the KeelPortCb memory rule.
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

/// `uint32_t keel_abi_version(void)`: the C ABI version this library implements, `1`.
#[unsafe(no_mangle)]
pub extern "C" fn keel_abi_version() -> u32 {
    api::ABI_VERSION
}

/// `uint64_t keel_schema_hash(void)`: the hash of the schema this core was built with
/// (`fnv1a64` of the canonical schema JSON, SPEC 2.3). Works before `keel_init`.
#[unsafe(no_mangle)]
pub extern "C" fn keel_schema_hash() -> u64 {
    guarded("keel_schema_hash", |_| 0, api::schema_hash)
}

/// `KeelBuf keel_schema_json(void)`: an owned copy of the canonical schema JSON (UTF-8). Free it
/// with [`keel_buf_free`]. Works before `keel_init`; `keel-cli` calls it to run bindgen.
#[unsafe(no_mangle)]
pub extern "C" fn keel_schema_json() -> KeelBuf {
    guarded(
        "keel_schema_json",
        |_| KeelBuf::EMPTY,
        || KeelBuf::from_vec(api::schema_json()),
    )
}

/// `uint32_t keel_init(const uint8_t *cfg, uint32_t len, keel_reply_cb reply,
/// keel_changeset_cb changes, keel_stream_cb stream, void *user)`.
///
/// Starts the process-global runtime. `cfg, len` is an encoded `RuntimeConfig`
/// (`platform String, mode String, core_threads u8, blocking_threads u8, log_level u8`); the
/// three callbacks and `user` are how the core reaches the host. The native ABI has no
/// `keel_poll`, so `core_threads == 0` is treated as `1`: the `keel-core` thread always runs.
///
/// Returns `0` on success, otherwise an [`init_code`]. **Idempotent per process**: calling it
/// again with the same callbacks and `user` while the runtime is up does nothing and returns `0`;
/// with different ones it returns `ALREADY_INITIALIZED` and changes nothing. After
/// [`keel_shutdown`] it can be called again.
///
/// # Safety
///
/// `cfg` must be null or valid for `len` bytes. The callbacks and `user` must stay valid, and
/// the callbacks callable from any thread, until [`keel_shutdown`] returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_init(
    cfg: *const u8,
    len: u32,
    reply: Option<KeelReplyCb>,
    changes: Option<KeelChangesetCb>,
    stream: Option<KeelStreamCb>,
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
        let ids: Vec<u32> = PORTS
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .copied()
            .collect();
        for id in ids {
            runtime.bind_foreign_port(id);
        }
    })
}

/// `void keel_shutdown(void)`: stops the runtime (idempotent), joins its threads, drops every
/// object and forgets the callbacks and port registrations. Calls made afterwards fail with
/// status 5 until `keel_init` runs again. The host must not call it from inside a callback.
#[unsafe(no_mangle)]
pub extern "C" fn keel_shutdown() {
    session::stop();
    PORTS
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// `uint32_t keel_call(const uint8_t *ptr, uint32_t len)`: submits a `Call` payload (SPEC 3.3).
/// Returns `0` when accepted (the reply follows through `reply_cb`, for sync and async methods
/// alike) and `5` when rejected without a reply: an undecodable payload, `call_id == 0`, a
/// `call_id` already in flight, no runtime, or a call from inside a host callback.
///
/// # Safety
///
/// `ptr` must be null or valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_call(ptr: *const u8, len: u32) -> u32 {
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    api::call(unsafe { bytes(ptr, len) })
}

/// `KeelBuf keel_call_sync(const uint8_t *ptr, uint32_t len)`: runs a synchronous method and
/// returns its whole `Reply` payload (SPEC 3.4) directly; an `async` method or a stream is
/// answered with status 5 without running. Free the buffer with [`keel_buf_free`].
///
/// # Safety
///
/// `ptr` must be null or valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_call_sync(ptr: *const u8, len: u32) -> KeelBuf {
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    let payload = unsafe { bytes(ptr, len) };
    guarded(
        "keel_call_sync",
        |_| KeelBuf::EMPTY,
        || KeelBuf::from_vec(api::call_sync(payload)),
    )
}

/// `void keel_cancel(uint32_t call_id)`: cancels an in-flight call or stream. A plain call is
/// answered with status 3 exactly once; unknown ids are ignored.
#[unsafe(no_mangle)]
pub extern "C" fn keel_cancel(call_id: u32) {
    api::cancel(call_id);
}

/// `void keel_stream_credit(uint32_t call_id, uint32_t credit)`: lets the stream `call_id` send
/// `credit` more items (SPEC 3.7). Never takes the core lock.
#[unsafe(no_mangle)]
pub extern "C" fn keel_stream_credit(call_id: u32, credit: u32) {
    api::stream_credit(call_id, credit);
}

/// `void keel_observe(uint64_t handle, uint32_t signal_id, uint8_t on)`: starts (`on != 0`) or
/// stops observing a signal of a store (`signal_id == UINT32_MAX` for all). Starting delivers the
/// current values through `changeset_cb` before this returns. Unknown handles are ignored.
#[unsafe(no_mangle)]
pub extern "C" fn keel_observe(handle: u64, signal_id: u32, on: u8) {
    api::observe(handle, signal_id, on != 0);
}

/// `void keel_release(uint64_t handle)`: releases an object handle. Stale or unknown handles are
/// ignored.
#[unsafe(no_mangle)]
pub extern "C" fn keel_release(handle: u64) {
    api::release(handle);
}

/// `void keel_port_register(uint32_t port_id, keel_port_cb cb, void *user)`: routes calls to the
/// platform-implemented port `port_id` to `cb`. A null `cb` removes the registration. A port with
/// no registration behaves as unavailable (SPEC 6.3). Registering takes the port over from any
/// default Rust binding (for example the native `Timer`).
///
/// # Safety
///
/// `cb` and `user` must stay valid, and `cb` callable from any thread, until the registration is
/// removed or [`keel_shutdown`] returns.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_port_register(
    port_id: u32,
    cb: Option<KeelPortCb>,
    user: *mut c_void,
) {
    guarded(
        "keel_port_register",
        |_| (),
        || {
            {
                let mut ports = PORTS.write().unwrap_or_else(PoisonError::into_inner);
                match cb {
                    Some(cb) => {
                        ports.insert(
                            port_id,
                            PortReg {
                                cb,
                                user: UserPtr(user),
                            },
                        );
                    }
                    None => {
                        ports.remove(&port_id);
                    }
                }
            }
            if cb.is_some() {
                if let Some(runtime) = api::runtime() {
                    runtime.bind_foreign_port(port_id);
                }
            }
        },
    );
}

/// `void keel_port_reply(const uint8_t *ptr, uint32_t len)`: answers a port call the callback
/// deferred by returning `1` (a `PortReply` payload, SPEC 3.6). Never takes the core lock, so it
/// is safe from any thread, including from inside the port callback itself. A reply to an
/// abandoned call is discarded.
///
/// # Safety
///
/// `ptr` must be null or valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_port_reply(ptr: *const u8, len: u32) {
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    api::port_reply(unsafe { bytes(ptr, len) });
}

/// `void keel_event(uint32_t port_id, uint32_t method_id, const uint8_t *ptr, uint32_t len)`:
/// delivers a host-to-core event of an event port (`Connectivity`, `Lifecycle`, ...) to the
/// core's subscribers.
///
/// # Safety
///
/// `ptr` must be null or valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_event(port_id: u32, method_id: u32, ptr: *const u8, len: u32) {
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    api::event(port_id, method_id, unsafe { bytes(ptr, len) });
}

/// `void keel_timer_fired(uint32_t timer_id)`: tells the core that timer `timer_id` came due
/// (only used with a host-owned `Timer` port). Never takes the core lock.
#[unsafe(no_mangle)]
pub extern "C" fn keel_timer_fired(timer_id: u32) {
    api::timer_fired(timer_id);
}

/// `KeelBuf keel_snapshot(void)`: every store as a `Snapshot` payload (SPEC 5.9); an empty
/// snapshot before `keel_init`. Free the buffer with [`keel_buf_free`].
#[unsafe(no_mangle)]
pub extern "C" fn keel_snapshot() -> KeelBuf {
    guarded(
        "keel_snapshot",
        |_| KeelBuf::EMPTY,
        || KeelBuf::from_vec(api::snapshot()),
    )
}

/// `uint32_t keel_restore(const uint8_t *ptr, uint32_t len)`: rebuilds the stores from a
/// snapshot, re-issuing the same handles. Returns `0` on success, otherwise a
/// [`restore_code`](crate::restore_code); a failed restore leaves the core unchanged.
///
/// # Safety
///
/// `ptr` must be null or valid for `len` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_restore(ptr: *const u8, len: u32) -> u32 {
    // SAFETY: the caller guarantees `ptr` is valid for `len` bytes.
    api::restore(unsafe { bytes(ptr, len) })
}

/// `KeelBuf keel_stats_json(void)`: a JSON document with the live handle count, tasks, calls,
/// transactions and the boundary crossing counters. Free the buffer with [`keel_buf_free`].
#[unsafe(no_mangle)]
pub extern "C" fn keel_stats_json() -> KeelBuf {
    guarded(
        "keel_stats_json",
        |_| KeelBuf::EMPTY,
        || KeelBuf::from_vec(api::stats_json().into_bytes()),
    )
}

/// `void keel_buf_free(KeelBuf buf)`: releases a buffer returned by the core. Empty buffers
/// (`cap == 0`) are ignored. The only core function a callback may call.
///
/// # Safety
///
/// `buf` must be exactly a buffer this library returned, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn keel_buf_free(buf: KeelBuf) {
    // SAFETY: the caller guarantees `buf` came from this library and is freed once.
    unsafe { buf.free() };
}
