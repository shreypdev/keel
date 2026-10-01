//! The C ABI as a host reaches it since version 2 (ADR-044): one [`UndraApi`] table per core.
//!
//! [`api`] is this test image's table, built exactly as `export_core!` builds a core's (so its
//! `abi_version`, `size`, `schema_hash` and `name_space` are what a host reads first), and the
//! functions below are the version 1 entry names as one-line forwards through it. The scenarios
//! keep their host-shaped spelling, and every call crosses the function pointers a C, Swift or C++
//! host calls, not a Rust function of `undra-ffi`.
#![allow(dead_code)]

use core::ffi::c_void;
use std::sync::OnceLock;

use undra_ffi::{UndraApi, UndraBuf, UndraChangesetCb, UndraPortCb, UndraReplyCb, UndraStreamCb};

/// The namespace of the test image's table.
pub const NAMESPACE: &core::ffi::CStr = c"undra_test";

/// This image's table, built on first use like the one `export_core!` exports.
pub fn api() -> &'static UndraApi {
    static API: OnceLock<UndraApi> = OnceLock::new();
    API.get_or_init(|| UndraApi::new(NAMESPACE))
}

/// The table's `abi_version` (version 1's `undra_abi_version()`).
pub fn undra_abi_version() -> u32 {
    api().abi_version
}

/// The table's `schema_hash` (version 1's `undra_schema_hash()`).
pub fn undra_schema_hash() -> u64 {
    api().schema_hash
}

/// `api->schema_json()`.
pub fn undra_schema_json() -> UndraBuf {
    (api().schema_json)()
}

/// `api->init(..)`.
///
/// # Safety
///
/// As the table's `init`: `cfg` valid for `len` bytes, callbacks and `user` valid until shutdown.
pub unsafe fn undra_init(
    cfg: *const u8,
    len: u32,
    reply: Option<UndraReplyCb>,
    changes: Option<UndraChangesetCb>,
    stream: Option<UndraStreamCb>,
    user: *mut c_void,
) -> u32 {
    // SAFETY: forwarded unchanged; the caller upholds `init`'s contract.
    unsafe { (api().init)(cfg, len, reply, changes, stream, user) }
}

/// `api->shutdown()`.
pub fn undra_shutdown() {
    (api().shutdown)();
}

/// `api->call(..)`.
///
/// # Safety
///
/// `ptr` null or valid for `len` bytes.
pub unsafe fn undra_call(ptr: *const u8, len: u32) -> u32 {
    // SAFETY: forwarded unchanged; the caller upholds `call`'s contract.
    unsafe { (api().call)(ptr, len) }
}

/// `api->call_sync(..)`.
///
/// # Safety
///
/// `ptr` null or valid for `len` bytes.
pub unsafe fn undra_call_sync(ptr: *const u8, len: u32) -> UndraBuf {
    // SAFETY: forwarded unchanged; the caller upholds `call_sync`'s contract.
    unsafe { (api().call_sync)(ptr, len) }
}

/// `api->cancel(..)`.
pub fn undra_cancel(call_id: u32) {
    (api().cancel)(call_id);
}

/// `api->stream_credit(..)`.
pub fn undra_stream_credit(call_id: u32, credit: u32) {
    (api().stream_credit)(call_id, credit);
}

/// `api->observe(..)`.
pub fn undra_observe(handle: u64, signal_id: u32, on: u8) {
    (api().observe)(handle, signal_id, on);
}

/// `api->release(..)`.
pub fn undra_release(handle: u64) {
    (api().release)(handle);
}

/// `api->port_register(..)`.
///
/// # Safety
///
/// As the table's `port_register`: `cb` and `user` valid until the registration is removed.
pub unsafe fn undra_port_register(port_id: u32, cb: Option<UndraPortCb>, user: *mut c_void) {
    // SAFETY: forwarded unchanged; the caller upholds `port_register`'s contract.
    unsafe { (api().port_register)(port_id, cb, user) }
}

/// `api->port_reply(..)`.
///
/// # Safety
///
/// `ptr` null or valid for `len` bytes.
pub unsafe fn undra_port_reply(ptr: *const u8, len: u32) {
    // SAFETY: forwarded unchanged; the caller upholds `port_reply`'s contract.
    unsafe { (api().port_reply)(ptr, len) }
}

/// `api->event(..)`.
///
/// # Safety
///
/// `ptr` null or valid for `len` bytes.
pub unsafe fn undra_event(port_id: u32, method_id: u32, ptr: *const u8, len: u32) {
    // SAFETY: forwarded unchanged; the caller upholds `event`'s contract.
    unsafe { (api().event)(port_id, method_id, ptr, len) }
}

/// `api->timer_fired(..)`.
pub fn undra_timer_fired(timer_id: u32) {
    (api().timer_fired)(timer_id);
}

/// `api->snapshot()`.
pub fn undra_snapshot() -> UndraBuf {
    (api().snapshot)()
}

/// `api->restore(..)`.
///
/// # Safety
///
/// `ptr` null or valid for `len` bytes.
pub unsafe fn undra_restore(ptr: *const u8, len: u32) -> u32 {
    // SAFETY: forwarded unchanged; the caller upholds `restore`'s contract.
    unsafe { (api().restore)(ptr, len) }
}

/// `api->stats_json()`.
pub fn undra_stats_json() -> UndraBuf {
    (api().stats_json)()
}

/// `api->buf_free(..)`.
///
/// # Safety
///
/// `buf` came from this table and is freed once.
pub unsafe fn undra_buf_free(buf: UndraBuf) {
    // SAFETY: forwarded unchanged; the caller upholds `buf_free`'s contract.
    unsafe { (api().buf_free)(buf) }
}
