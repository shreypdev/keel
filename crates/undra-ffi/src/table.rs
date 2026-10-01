//! The C ABI, version 2 (SPEC 6, ADR-044): a core is reached through **one** exported function that
//! returns its [`UndraApi`] table.
//!
//! A Rust static library exports every Rust symbol it contains, so two cores linked into one binary
//! either collide or, worse, silently merge their registries and runtimes. A core is therefore a
//! self-contained image (a cdylib, or on iOS a prelinked object) whose only global symbol is
//! `<namespace>_undra_api` (plus `JNI_OnLoad` with the `jni` feature). The generated shim exports
//! it with [`export_core!`](crate::export_core), the only place an export attribute is written; the
//! entry points themselves are plain `extern "C"` functions of this crate that the table points at.
//!
//! ```c
//! const UndraApi *api = acme_pay_undra_api();
//! if (api->abi_version != UNDRA_ABI_VERSION || api->schema_hash != EXPECTED) { /* refuse */ }
//! api->init(cfg, len, on_reply, on_changes, on_stream, user);
//! ```
//!
//! The table is immutable once built (on the first call of the entry): a host may keep the pointer
//! for the life of the process. Fields are only ever appended, and `size` says how many bytes this
//! core's table has, so a host built against a later header can tell which entries exist.

use core::ffi::CStr;
#[cfg(not(target_family = "wasm"))]
use core::ffi::{c_char, c_void};

#[cfg(not(target_family = "wasm"))]
use crate::api;
#[cfg(not(target_family = "wasm"))]
use crate::buf::UndraBuf;
#[cfg(not(target_family = "wasm"))]
use crate::guard::guarded;
#[cfg(not(target_family = "wasm"))]
use crate::native::{self, UndraChangesetCb, UndraPortCb, UndraReplyCb, UndraStreamCb};

/// The longest namespace a core may have, in bytes (ADR-044).
pub const MAX_NAMESPACE_LEN: usize = 32;

/// One core's C ABI (SPEC 6): two constants, its namespace and the 17 operations, in SPEC 6 order.
///
/// `#[repr(C)]`; the C declaration is `UndraApi` in `undra.h` (version 2). Every function keeps the
/// signature and the host contract its `undra_<name>` had in version 1; `abi_version` and
/// `schema_hash` are data instead of functions, so a host can check both before it calls anything.
/// Get one from the core's `<namespace>_undra_api()` export, or in Rust from [`UndraApi::new`].
#[cfg(not(target_family = "wasm"))]
#[repr(C)]
#[derive(Debug)]
pub struct UndraApi {
    /// The C ABI version, [`ABI_VERSION`](crate::ABI_VERSION) (`2`). Check it before anything else.
    pub abi_version: u32,
    /// `sizeof(UndraApi)` as this core was built. Fields are only ever appended.
    pub size: u32,
    /// The schema hash the core was built with (`fnv1a64` of the canonical schema JSON, SPEC 2.3).
    pub schema_hash: u64,
    /// The core's namespace (`[core] namespace` of `undra.toml`), NUL-terminated, static.
    pub name_space: *const c_char,
    /// `UndraBuf schema_json(void)`: the whole schema as JSON (SPEC 2.3), freed with `buf_free`.
    pub schema_json: extern "C" fn() -> UndraBuf,
    /// `uint32_t init(cfg, len, reply, changes, stream, user)`: starts the core (SPEC 6).
    pub init: unsafe extern "C" fn(
        cfg: *const u8,
        len: u32,
        reply: Option<UndraReplyCb>,
        changes: Option<UndraChangesetCb>,
        stream: Option<UndraStreamCb>,
        user: *mut c_void,
    ) -> u32,
    /// `void shutdown(void)`: stops the core and waits for its threads and port callbacks.
    pub shutdown: extern "C" fn(),
    /// `uint32_t call(ptr, len)`: submits a `Call` payload; the reply comes through `reply`.
    pub call: unsafe extern "C" fn(ptr: *const u8, len: u32) -> u32,
    /// `UndraBuf call_sync(ptr, len)`: runs a synchronous method and returns its `Reply` payload.
    pub call_sync: unsafe extern "C" fn(ptr: *const u8, len: u32) -> UndraBuf,
    /// `void cancel(call_id)`.
    pub cancel: extern "C" fn(call_id: u32),
    /// `void stream_credit(call_id, credit)`; never takes the core lock.
    pub stream_credit: extern "C" fn(call_id: u32, credit: u32),
    /// `void observe(handle, signal_id, on)`.
    pub observe: extern "C" fn(handle: u64, signal_id: u32, on: u8),
    /// `void release(handle)`.
    pub release: extern "C" fn(handle: u64),
    /// `void port_register(port_id, cb, user)`; a null `cb` removes the registration.
    pub port_register:
        unsafe extern "C" fn(port_id: u32, cb: Option<UndraPortCb>, user: *mut c_void),
    /// `void port_reply(ptr, len)`: answers a deferred port call; never takes the core lock.
    pub port_reply: unsafe extern "C" fn(ptr: *const u8, len: u32),
    /// `void event(port_id, method_id, ptr, len)`: a host-to-core event.
    pub event: unsafe extern "C" fn(port_id: u32, method_id: u32, ptr: *const u8, len: u32),
    /// `void timer_fired(timer_id)`; never takes the core lock.
    pub timer_fired: extern "C" fn(timer_id: u32),
    /// `UndraBuf snapshot(void)`: every store as a `Snapshot` payload (SPEC 5.9).
    pub snapshot: extern "C" fn() -> UndraBuf,
    /// `uint32_t restore(ptr, len)`: rebuilds the stores from a snapshot.
    pub restore: unsafe extern "C" fn(ptr: *const u8, len: u32) -> u32,
    /// `UndraBuf stats_json(void)`: the core's counters as JSON.
    pub stats_json: extern "C" fn() -> UndraBuf,
    /// `void buf_free(buf)`: releases a buffer the core returned.
    pub buf_free: unsafe extern "C" fn(buf: UndraBuf),
}

#[cfg(not(target_family = "wasm"))]
// SAFETY: every field is immutable once the table is built: integers, function pointers to this
// crate's entry points (which are themselves safe to call from any thread, SPEC 6), and
// `name_space`, which points at a `'static` NUL-terminated string that nothing writes. Sharing or
// sending the table between threads therefore cannot race.
unsafe impl Sync for UndraApi {}
#[cfg(not(target_family = "wasm"))]
// SAFETY: as for `Sync`: the table owns nothing and holds no thread-affine state.
unsafe impl Send for UndraApi {}

#[cfg(not(target_family = "wasm"))]
impl UndraApi {
    /// The table of the core linked into this image, under `name_space`.
    ///
    /// Reads the schema hash once (it needs no running core), so build it after the image's
    /// static constructors ran: [`export_core!`](crate::export_core) builds it on the first call of
    /// the entry point, which a host makes long after loading. A contained panic while reading the
    /// hash leaves it `0`, which no host accepts (R6: nothing unwinds out of the entry).
    #[must_use]
    pub fn new(name_space: &'static CStr) -> UndraApi {
        UndraApi {
            abi_version: api::ABI_VERSION,
            size: core::mem::size_of::<UndraApi>() as u32,
            schema_hash: guarded("undra_api", |_| 0, api::schema_hash),
            name_space: name_space.as_ptr(),
            schema_json: native::undra_schema_json,
            init: native::undra_init,
            shutdown: native::undra_shutdown,
            call: native::undra_call,
            call_sync: native::undra_call_sync,
            cancel: native::undra_cancel,
            stream_credit: native::undra_stream_credit,
            observe: native::undra_observe,
            release: native::undra_release,
            port_register: native::undra_port_register,
            port_reply: native::undra_port_reply,
            event: native::undra_event,
            timer_fired: native::undra_timer_fired,
            snapshot: native::undra_snapshot,
            restore: native::undra_restore,
            stats_json: native::undra_stats_json,
            buf_free: native::undra_buf_free,
        }
    }
}

/// Whether `name` is a valid core namespace: a C identifier (ASCII letter or `_` first, then
/// letters, digits and `_`) of 1 to [`MAX_NAMESPACE_LEN`] bytes. [`export_core!`](crate::export_core)
/// checks it at compile time.
#[must_use]
pub const fn is_valid_namespace(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > MAX_NAMESPACE_LEN {
        return false;
    }
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        let ok = b == b'_' || b.is_ascii_alphabetic() || (i > 0 && b.is_ascii_digit());
        if !ok {
            return false;
        }
        i += 1;
    }
    true
}

/// The namespace `name` as a static C string, for [`export_core!`](crate::export_core): `name`
/// must end in exactly one NUL and be a valid namespace before it (checked at compile time when
/// used in a `const`).
#[doc(hidden)]
#[must_use]
pub const fn __namespace(name: &'static str) -> &'static CStr {
    let bytes = name.as_bytes();
    let Some((&0, body)) = bytes.split_last() else {
        panic!("a core namespace must be NUL-terminated");
    };
    let Ok(body) = core::str::from_utf8(body) else {
        panic!("a core namespace is ASCII");
    };
    assert!(
        is_valid_namespace(body),
        "a core namespace is a C identifier of 1 to 32 bytes (undra.toml `[core] namespace`, ADR-044)"
    );
    match CStr::from_bytes_with_nul(bytes) {
        Ok(name) => name,
        Err(_) => panic!("a core namespace has no interior NUL"),
    }
}

/// Exports this image's core: `<ns>_undra_api()`, the one entry point of the C ABI (SPEC 6), and,
/// with `undra-ffi`'s `jni` feature, `JNI_OnLoad` / `JNI_OnUnload` registering the natives of
/// SPEC 6.1 on `jni_class` (a JNI class name, `dev/acme/pay/core/UndraCoreNative`).
///
/// Call it once, in the crate that is built as the core's cdylib or staticlib (`undra build`'s
/// generated shim does). The namespace is a C identifier of at most 32 bytes, checked at compile
/// time; on wasm the macro exports nothing (a wasm module is its own namespace, SPEC 7).
///
/// ```ignore
/// undra_ffi::export_core!(acme_pay, jni_class = "dev/acme/pay/core/UndraCoreNative");
/// // extern "C" fn acme_pay_undra_api() -> &'static UndraApi
/// ```
#[macro_export]
macro_rules! export_core {
    ($ns:ident $(, jni_class = $class:literal)? $(,)?) => {
        const _: &::core::ffi::CStr = $crate::__namespace(concat!(stringify!($ns), "\0"));

        #[cfg(not(target_family = "wasm"))]
        const _: () = {
            /// The core's table, built on the first call (after the image's constructors ran).
            static API: ::std::sync::OnceLock<$crate::UndraApi> = ::std::sync::OnceLock::new();

            /// `const UndraApi *<ns>_undra_api(void)`: this core's C ABI (SPEC 6, ADR-044).
            #[unsafe(export_name = concat!(stringify!($ns), "_undra_api"))]
            extern "C" fn undra_api_entry() -> &'static $crate::UndraApi {
                API.get_or_init(|| {
                    $crate::UndraApi::new($crate::__namespace(concat!(stringify!($ns), "\0")))
                })
            }
        };

        $( $crate::__export_jni!($class); )?
    };
}

/// With the `jni` feature: `JNI_OnLoad` registering the natives on the class, and `JNI_OnUnload`.
#[cfg(all(feature = "jni", not(target_family = "wasm")))]
#[doc(hidden)]
#[macro_export]
macro_rules! __export_jni {
    ($class:literal) => {
        const _: () = {
            /// Called by the JVM when `System.loadLibrary` loads this core: registers the natives of
            /// SPEC 6.1 on the core's class (ADR-044).
            #[unsafe(no_mangle)]
            extern "system" fn JNI_OnLoad(
                vm: *mut ::core::ffi::c_void,
                _reserved: *mut ::core::ffi::c_void,
            ) -> i32 {
                // SAFETY: the JVM calls `JNI_OnLoad` with a valid `JavaVM *` for the duration of the
                // call, on a thread attached to it; `$class` is a JNI class name literal.
                unsafe { $crate::__jni_on_load(vm, $class) }
            }

            /// Called by the JVM before this core is unloaded: stops the runtime.
            #[unsafe(no_mangle)]
            extern "system" fn JNI_OnUnload(
                _vm: *mut ::core::ffi::c_void,
                _reserved: *mut ::core::ffi::c_void,
            ) {
                $crate::__jni_on_unload();
            }
        };
    };
}

/// Without the `jni` feature (or on wasm) a core has no JNI entry points.
#[cfg(not(all(feature = "jni", not(target_family = "wasm"))))]
#[doc(hidden)]
#[macro_export]
macro_rules! __export_jni {
    ($class:literal) => {};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn namespaces_are_short_c_identifiers() {
        for good in [
            "acme_pay",
            "a",
            "_x",
            "playground_core",
            "A1_b2",
            &"n".repeat(32),
        ] {
            assert!(is_valid_namespace(good), "{good}");
        }
        for bad in [
            "",
            "1abc",
            "acme-pay",
            "acme pay",
            "é",
            &"n".repeat(33),
            "a.b",
        ] {
            assert!(!is_valid_namespace(bad), "{bad}");
        }
    }

    #[test]
    fn the_namespace_constant_is_the_c_string() {
        assert_eq!(__namespace("acme_pay\0"), c"acme_pay");
    }

    #[test]
    #[cfg(not(target_family = "wasm"))]
    fn a_table_says_its_version_size_namespace_and_hash() {
        let api = UndraApi::new(c"unit_test");
        assert_eq!(api.abi_version, 2);
        assert_eq!(api.size as usize, core::mem::size_of::<UndraApi>());
        // SAFETY: `new` stores the pointer of a `'static` C string.
        assert_eq!(unsafe { CStr::from_ptr(api.name_space) }, c"unit_test");
        assert_eq!(api.schema_hash, api::schema_hash());
    }

    #[test]
    #[cfg(not(target_family = "wasm"))]
    fn the_layout_is_the_c_one() {
        // undra.h: two u32, a u64, a pointer, then 17 function pointers.
        let word = core::mem::size_of::<usize>();
        assert_eq!(core::mem::offset_of!(UndraApi, schema_hash), 8);
        assert_eq!(core::mem::offset_of!(UndraApi, name_space), 16);
        assert_eq!(core::mem::offset_of!(UndraApi, schema_json), 16 + word);
        assert_eq!(core::mem::offset_of!(UndraApi, buf_free), 16 + 17 * word);
        assert_eq!(core::mem::size_of::<UndraApi>(), 16 + 18 * word);
    }
}
