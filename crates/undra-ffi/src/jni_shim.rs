//! The JNI shim (SPEC 6.1), feature `jni`.
//!
//! Kotlin talks to a core through a class of the core's own (ADR-044): the generated
//! `<kotlin package>.UndraCoreNative`, an `object` implementing the runtime's `NativeApi` with
//! `external` members. [`export_core!`](crate::export_core) exports `JNI_OnLoad`, which registers
//! the natives on the class it names with `RegisterNatives` ([`on_load`]); nothing is exported under
//! a `Java_*` name, so two cores in one JVM (each `System.loadLibrary`ed, each running its own
//! `JNI_OnLoad`) never bind one another's natives. A native may be declared static or as a member:
//! the second JNI argument (the class or the receiver) is not used.
//!
//! Callbacks reach Kotlin through the `dev.undra.runtime.NativeCallbacks` object given to `init`. Payloads
//! travel as **direct `ByteBuffer`s over core memory**, valid only while the callback runs (the
//! Kotlin runtime copies immediately); byte arrays coming from Kotlin are copied once with
//! `GetByteArrayRegion`. Callback threads are attached to the JVM as daemon threads, so an idle
//! `undra-core` thread never keeps a JVM from exiting, and a Java exception escaping a callback
//! is described, cleared and treated as "unavailable" (native code must never run with one
//! pending).
//!
//! The `jni` crate wraps the raw JNI function table; every `unsafe` block left here is one the
//! wrapper cannot make safe (direct buffers over borrowed memory, unchecked method calls with
//! pre-resolved method ids).

use core::ffi::c_void;
use std::any::Any;
use std::ptr;
use std::sync::Arc;

use jni::errors::Result as JniResult;
use jni::objects::{GlobalRef, JByteArray, JByteBuffer, JMethodID, JObject, JValue};
use jni::signature::{Primitive, ReturnType};
use jni::sys::{JNI_ERR, JNI_VERSION_1_6, jboolean, jbyteArray, jint, jlong, jstring};
use jni::{JNIEnv, JavaVM, NativeMethod};
use undra_runtime::PortCallOutcome;

use crate::api::{self, init_code};
use crate::guard::guarded;
use crate::session::{self, Sink};

/// The callbacks interface, shared by every core (it declares no natives).
const CALLBACKS_CLASS: &str = "dev/undra/runtime/NativeCallbacks";
/// The JNI descriptor of `init`, whose second parameter is a [`CALLBACKS_CLASS`].
const INIT_DESCRIPTOR: &str = "([BLdev/undra/runtime/NativeCallbacks;)I";

/// The `NativeCallbacks` methods, resolved once in `init`.
#[derive(Clone, Copy)]
struct MethodIds {
    on_reply: JMethodID,
    on_change_set: JMethodID,
    on_stream: JMethodID,
    on_port_call: JMethodID,
    port_sync_reply: JMethodID,
}

/// The Kotlin embedder: the `Callbacks` object and how to reach it from any thread.
struct JniSink {
    vm: JavaVM,
    callbacks: GlobalRef,
    ids: MethodIds,
}

/// A direct buffer over `payload`, valid until `payload` is dropped or moved.
fn direct<'l>(env: &mut JNIEnv<'l>, payload: &[u8]) -> JniResult<JByteBuffer<'l>> {
    // SAFETY: `payload` is valid for `payload.len()` bytes for as long as the caller keeps the
    // slice borrowed, which is the whole callback: the Java side must not retain the buffer
    // (documented on `NativeCallbacks`). The buffer is only read on the Java side; the
    // `*mut` is what the JNI signature asks for.
    unsafe { env.new_direct_byte_buffer(payload.as_ptr().cast_mut(), payload.len()) }
}

impl JniSink {
    fn new(env: &mut JNIEnv<'_>, callbacks: &JObject<'_>) -> JniResult<JniSink> {
        let vm = env.get_java_vm()?;
        let callbacks = env.new_global_ref(callbacks)?;
        let class = env.find_class(CALLBACKS_CLASS)?;
        let ids = MethodIds {
            on_reply: env.get_method_id(&class, "onReply", "(ILjava/nio/ByteBuffer;)V")?,
            on_change_set: env.get_method_id(&class, "onChangeSet", "(Ljava/nio/ByteBuffer;)V")?,
            on_stream: env.get_method_id(&class, "onStream", "(ILjava/nio/ByteBuffer;)V")?,
            on_port_call: env.get_method_id(&class, "onPortCall", "(IIILjava/nio/ByteBuffer;)I")?,
            port_sync_reply: env.get_method_id(&class, "portSyncReply", "()[B")?,
        };
        Ok(JniSink { vm, callbacks, ids })
    }

    /// Runs `body` on this thread, attached to the JVM as a daemon, inside a local reference
    /// frame (so a long-lived native thread does not leak local references). A pending Java
    /// exception is described and cleared, and turns the result into `None`.
    fn on_java<R>(&self, body: impl FnOnce(&mut JNIEnv<'_>) -> JniResult<R>) -> Option<R> {
        let mut env = self.vm.attach_current_thread_as_daemon().ok()?;
        let outcome = env.with_local_frame(8, body);
        if env.exception_check().unwrap_or(true) {
            let _ = env.exception_describe();
            let _ = env.exception_clear();
            return None;
        }
        outcome.ok()
    }

    /// Calls a `void` callback taking `([int callId,] ByteBuffer payload)`.
    fn deliver(&self, method: JMethodID, call_id: Option<u32>, payload: &[u8]) {
        let _ = self.on_java(|env| {
            let buffer = direct(env, payload)?;
            let buffer_arg = JValue::Object(&buffer).as_jni();
            let with_id;
            let without_id;
            let args = match call_id {
                Some(call_id) => {
                    with_id = [JValue::Int(call_id as jint).as_jni(), buffer_arg];
                    &with_id[..]
                }
                None => {
                    without_id = [buffer_arg];
                    &without_id[..]
                }
            };
            // SAFETY: `method` was resolved on `NativeCallbacks` with a descriptor whose
            // parameters are exactly `args` (an optional `int` and the `ByteBuffer`), returning
            // `void`; `self.callbacks` implements that interface.
            unsafe {
                env.call_method_unchecked(
                    &self.callbacks,
                    method,
                    ReturnType::Primitive(Primitive::Void),
                    args,
                )
            }
            .map(|_| ())
        });
    }
}

impl Sink for JniSink {
    fn reply(&self, call_id: u32, payload: &[u8]) {
        self.deliver(self.ids.on_reply, Some(call_id), payload);
    }

    fn change_set(&self, payload: &[u8]) {
        self.deliver(self.ids.on_change_set, None, payload);
    }

    fn stream_item(&self, call_id: u32, payload: &[u8]) {
        self.deliver(self.ids.on_stream, Some(call_id), payload);
    }

    fn port_call(
        &self,
        port_id: u32,
        method_id: u32,
        port_call_id: u32,
        args: &[u8],
    ) -> PortCallOutcome {
        self.on_java(|env| {
            let buffer = direct(env, args)?;
            let jargs = [
                JValue::Int(port_id as jint).as_jni(),
                JValue::Int(method_id as jint).as_jni(),
                JValue::Int(port_call_id as jint).as_jni(),
                JValue::Object(&buffer).as_jni(),
            ];
            // SAFETY: `on_port_call` was resolved with the descriptor `(IIILByteBuffer;)I`, which
            // `jargs` matches, and the return type is `int`.
            let answer = unsafe {
                env.call_method_unchecked(
                    &self.callbacks,
                    self.ids.on_port_call,
                    ReturnType::Primitive(Primitive::Int),
                    &jargs,
                )
            }?
            .i()?;
            Ok(match answer {
                0 => {
                    // SAFETY: `port_sync_reply` was resolved with the descriptor `()[B`; it takes
                    // no arguments and returns an array object.
                    let reply = unsafe {
                        env.call_method_unchecked(
                            &self.callbacks,
                            self.ids.port_sync_reply,
                            ReturnType::Array,
                            &[],
                        )
                    }?
                    .l()?;
                    let bytes = env.convert_byte_array(JByteArray::from(reply))?;
                    if bytes.is_empty() {
                        PortCallOutcome::Unavailable
                    } else {
                        PortCallOutcome::Sync(bytes)
                    }
                }
                1 => PortCallOutcome::Async,
                _ => PortCallOutcome::Unavailable,
            })
        })
        .unwrap_or(PortCallOutcome::Unavailable)
    }

    fn same_embedder(&self, other: &dyn Sink) -> bool {
        let Some(other) = other.as_any().downcast_ref::<JniSink>() else {
            return false;
        };
        self.vm
            .attach_current_thread_as_daemon()
            .ok()
            .and_then(|env| env.is_same_object(&self.callbacks, &other.callbacks).ok())
            .unwrap_or(false)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Copies a Java `byte[]` (an empty vector for null or on failure).
fn bytes_of(env: &JNIEnv<'_>, array: &JByteArray<'_>) -> Vec<u8> {
    env.convert_byte_array(array).unwrap_or_default()
}

/// A new Java `byte[]` (null on failure, with the JVM's exception pending).
fn java_bytes(env: &JNIEnv<'_>, bytes: &[u8]) -> jbyteArray {
    env.byte_array_from_slice(bytes)
        .map_or(ptr::null_mut(), JByteArray::into_raw)
}

/// `int abiVersion()`: [`ABI_VERSION`](crate::ABI_VERSION), `2`.
extern "system" fn native_abi_version<'l>(_env: JNIEnv<'l>, _this: JObject<'l>) -> jint {
    api::ABI_VERSION as jint
}

/// `long schemaHash()`.
extern "system" fn native_schema_hash<'l>(_env: JNIEnv<'l>, _this: JObject<'l>) -> jlong {
    guarded("schemaHash", |_| 0, || api::schema_hash() as jlong)
}

/// `byte[] schemaJson()`.
extern "system" fn native_schema_json<'l>(env: JNIEnv<'l>, _this: JObject<'l>) -> jbyteArray {
    guarded(
        "schemaJson",
        |_| ptr::null_mut(),
        || java_bytes(&env, &api::schema_json()),
    )
}

/// `int init(byte[] cfg, NativeCallbacks cb)`.
extern "system" fn native_init<'l>(
    mut env: JNIEnv<'l>,
    _this: JObject<'l>,
    cfg: JByteArray<'l>,
    callbacks: JObject<'l>,
) -> jint {
    guarded(
        "init",
        |_| init_code::PANICKED as jint,
        || {
            let config = bytes_of(&env, &cfg);
            let sink = match JniSink::new(&mut env, &callbacks) {
                Ok(sink) => Arc::new(sink),
                Err(_) => return init_code::BAD_ARGUMENT as jint,
            };
            session::start(&config, sink, |_| {}) as jint
        },
    )
}

/// `int call(byte[] payload)`.
extern "system" fn native_call<'l>(
    env: JNIEnv<'l>,
    _this: JObject<'l>,
    payload: JByteArray<'l>,
) -> jint {
    guarded(
        "call",
        |_| 5,
        || api::call(&bytes_of(&env, &payload)) as jint,
    )
}

/// `byte[] callSync(byte[] payload)`.
extern "system" fn native_call_sync<'l>(
    env: JNIEnv<'l>,
    _this: JObject<'l>,
    payload: JByteArray<'l>,
) -> jbyteArray {
    guarded(
        "callSync",
        |_| ptr::null_mut(),
        || java_bytes(&env, &api::call_sync(&bytes_of(&env, &payload))),
    )
}

/// `void cancel(int callId)`.
extern "system" fn native_cancel<'l>(_env: JNIEnv<'l>, _this: JObject<'l>, call_id: jint) {
    api::cancel(call_id as u32);
}

/// `void streamCredit(int callId, int credit)`.
extern "system" fn native_stream_credit<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    call_id: jint,
    credit: jint,
) {
    api::stream_credit(call_id as u32, credit as u32);
}

/// `void observe(long handle, int signalId, boolean on)`.
extern "system" fn native_observe<'l>(
    _env: JNIEnv<'l>,
    _this: JObject<'l>,
    handle: jlong,
    signal_id: jint,
    on: jboolean,
) {
    api::observe(handle as u64, signal_id as u32, on != 0);
}

/// `void release(long handle)`.
extern "system" fn native_release<'l>(_env: JNIEnv<'l>, _this: JObject<'l>, handle: jlong) {
    api::release(handle as u64);
}

/// `void portReply(byte[] payload)`.
extern "system" fn native_port_reply<'l>(
    env: JNIEnv<'l>,
    _this: JObject<'l>,
    payload: JByteArray<'l>,
) {
    guarded(
        "portReply",
        |_| (),
        || api::port_reply(&bytes_of(&env, &payload)),
    );
}

/// `void event(int portId, int methodId, byte[] payload)`.
extern "system" fn native_event<'l>(
    env: JNIEnv<'l>,
    _this: JObject<'l>,
    port_id: jint,
    method_id: jint,
    payload: JByteArray<'l>,
) {
    guarded(
        "event",
        |_| (),
        || api::event(port_id as u32, method_id as u32, &bytes_of(&env, &payload)),
    );
}

/// `void timerFired(int timerId)`.
extern "system" fn native_timer_fired<'l>(_env: JNIEnv<'l>, _this: JObject<'l>, timer_id: jint) {
    api::timer_fired(timer_id as u32);
}

/// `byte[] snapshot()`.
extern "system" fn native_snapshot<'l>(env: JNIEnv<'l>, _this: JObject<'l>) -> jbyteArray {
    guarded(
        "snapshot",
        |_| ptr::null_mut(),
        || java_bytes(&env, &api::snapshot()),
    )
}

/// `int restore(byte[] snapshot)`.
extern "system" fn native_restore<'l>(
    env: JNIEnv<'l>,
    _this: JObject<'l>,
    snapshot: JByteArray<'l>,
) -> jint {
    guarded(
        "restore",
        |_| api::restore_code::PANICKED as jint,
        || api::restore(&bytes_of(&env, &snapshot)) as jint,
    )
}

/// `String statsJson()`.
extern "system" fn native_stats_json<'l>(env: JNIEnv<'l>, _this: JObject<'l>) -> jstring {
    guarded(
        "statsJson",
        |_| ptr::null_mut(),
        || {
            env.new_string(api::stats_json())
                .map_or(ptr::null_mut(), jni::objects::JString::into_raw)
        },
    )
}

/// `void shutdown()`: what `undra_shutdown` runs (ADR-034, SPEC 6.1). Answers
/// every call in flight (status 3) and ends every open stream, stops the core, timer and blocking
/// threads, removes the port registrations and forgets the embedder, which releases the global
/// reference to its `Callbacks` object; a later `init` starts a new core. Idempotent. The Kotlin
/// runtime calls it from `UndraCore.close()`, never from inside a callback (it would wait for the
/// thread it runs on).
extern "system" fn native_shutdown<'l>(_env: JNIEnv<'l>, _this: JObject<'l>) {
    session::stop();
}

/// Registers every native on `class` with `RegisterNatives`. The descriptors are pinned by the
/// Kotlin runtime's `NativeTests` and the generated `UndraCoreNative` (bindgen's goldens).
fn register(env: &mut JNIEnv<'_>, class: &str) -> JniResult<()> {
    fn native(name: &str, sig: &str, fn_ptr: *mut c_void) -> NativeMethod {
        NativeMethod {
            name: name.into(),
            sig: sig.into(),
            fn_ptr,
        }
    }
    let methods = [
        native("abiVersion", "()I", native_abi_version as *mut c_void),
        native("schemaHash", "()J", native_schema_hash as *mut c_void),
        native("schemaJson", "()[B", native_schema_json as *mut c_void),
        native("init", INIT_DESCRIPTOR, native_init as *mut c_void),
        native("call", "([B)I", native_call as *mut c_void),
        native("callSync", "([B)[B", native_call_sync as *mut c_void),
        native("cancel", "(I)V", native_cancel as *mut c_void),
        native("streamCredit", "(II)V", native_stream_credit as *mut c_void),
        native("observe", "(JIZ)V", native_observe as *mut c_void),
        native("release", "(J)V", native_release as *mut c_void),
        native("portReply", "([B)V", native_port_reply as *mut c_void),
        native("event", "(II[B)V", native_event as *mut c_void),
        native("timerFired", "(I)V", native_timer_fired as *mut c_void),
        native("snapshot", "()[B", native_snapshot as *mut c_void),
        native("restore", "([B)I", native_restore as *mut c_void),
        native(
            "statsJson",
            "()Ljava/lang/String;",
            native_stats_json as *mut c_void,
        ),
        native("shutdown", "()V", native_shutdown as *mut c_void),
    ];
    env.register_native_methods(class, &methods)
}

/// What the core's `JNI_OnLoad` runs ([`export_core!`](crate::export_core)): registers the natives of
/// SPEC 6.1 on `class` (`dev/acme/pay/core/UndraCoreNative`). A failure (the class is not visible to
/// the loader that loaded the library, or lacks a native) clears the exception and returns
/// `JNI_ERR`, which makes `System.loadLibrary` throw `UnsatisfiedLinkError`.
///
/// # Safety
///
/// `vm` must be the `JavaVM *` the JVM passed to `JNI_OnLoad`, called on the thread and for the
/// duration of that call.
pub unsafe fn on_load(vm: *mut c_void, class: &str) -> jint {
    guarded(
        "JNI_OnLoad",
        |_| JNI_ERR,
        || {
            // SAFETY: the caller passes the JVM's own `JavaVM *`, valid for the whole process.
            let Ok(vm) = (unsafe { JavaVM::from_raw(vm.cast()) }) else {
                return JNI_ERR;
            };
            let Ok(mut env) = vm.get_env() else {
                return JNI_ERR;
            };
            match register(&mut env, class) {
                Ok(()) => JNI_VERSION_1_6,
                Err(_) => {
                    let _ = env.exception_clear();
                    JNI_ERR
                }
            }
        },
    )
}

/// What the core's `JNI_OnUnload` runs (its class loader was collected): stops the runtime, so no
/// core thread is left running code that is about to be unmapped.
pub fn on_unload() {
    session::stop();
}
