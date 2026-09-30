//! Starting and stopping the process runtime for the native shims (C ABI and JNI).
//!
//! A shim describes its embedder as a [`Sink`] (where replies, change-sets, stream items and
//! port calls go); this module wraps it in the [`Host`] the runtime wants, applies the
//! idempotence rule of `keel_init` and owns the shutdown path.

use std::any::Any;
use std::cell::Cell;
use std::sync::{Arc, Mutex, PoisonError};

use keel_runtime::keel_meta::ids;
use keel_runtime::keel_wire::{Decode, Reader, Writer};
use keel_runtime::{Host, InitError, PortCallOutcome, Runtime, RuntimeConfig};

use crate::api::{self, init_code};
use crate::guard::guarded;

/// Where the runtime's outgoing traffic goes; one implementation per native shim.
pub(crate) trait Sink: Send + Sync + 'static {
    /// A `Reply` payload for `call_id`.
    fn reply(&self, call_id: u32, payload: &[u8]);
    /// A `ChangeSet` payload.
    fn change_set(&self, payload: &[u8]);
    /// A `StreamItem` payload for `call_id`.
    fn stream_item(&self, call_id: u32, payload: &[u8]);
    /// A port call; the shim answers with the embedder's outcome.
    fn port_call(
        &self,
        port_id: u32,
        method_id: u32,
        port_call_id: u32,
        args: &[u8],
    ) -> PortCallOutcome;
    /// Whether `other` is the same embedder as `self` (a repeated `keel_init` is then a no-op).
    fn same_embedder(&self, other: &dyn Sink) -> bool;
    /// For [`Sink::same_embedder`] downcasts.
    fn as_any(&self) -> &dyn Any;
}

/// Port and method ids of the standard `Log` port (SPEC 8): native hosts receive the runtime's
/// own log records as `Log.log(level, target, message)` port calls.
const LOG_PORT: u32 = ids::port_id("Log");
const LOG_METHOD: u32 = ids::port_method_id("Log", "log");

thread_local! {
    /// Set while this thread is inside the log path, so a log record produced by a failing log
    /// callback is dropped instead of recursing.
    static LOGGING: Cell<bool> = const { Cell::new(false) };
}

/// The [`Host`] the runtime sees: everything goes to the shim's [`Sink`].
struct NativeHost {
    sink: Arc<dyn Sink>,
}

impl Host for NativeHost {
    fn reply(&self, call_id: u32, payload: &[u8]) {
        self.sink.reply(call_id, payload);
    }

    fn change_set(&self, payload: &[u8]) {
        self.sink.change_set(payload);
    }

    fn stream_item(&self, call_id: u32, payload: &[u8]) {
        self.sink.stream_item(call_id, payload);
    }

    fn port_call(
        &self,
        port_id: u32,
        method_id: u32,
        port_call_id: u32,
        args: &[u8],
    ) -> PortCallOutcome {
        self.sink.port_call(port_id, method_id, port_call_id, args)
    }

    fn log(&self, level: u8, target: &str, message: &str) {
        if LOGGING.with(|flag| flag.replace(true)) {
            return;
        }
        let mut args = Writer::with_capacity(9 + target.len() + message.len());
        args.write_u8(level);
        args.write_str(target);
        args.write_str(message);
        // The answer is irrelevant: a log record is fire and forget. A synchronous reply is
        // consumed (and freed) by the sink; an asynchronous one is ignored.
        let _ = self
            .sink
            .port_call(LOG_PORT, LOG_METHOD, 0, args.as_slice());
        LOGGING.with(|flag| flag.set(false));
    }
}

/// The embedder that started the running runtime.
static INSTALLED: Mutex<Option<Arc<dyn Sink>>> = Mutex::new(None);

fn installed() -> std::sync::MutexGuard<'static, Option<Arc<dyn Sink>>> {
    INSTALLED.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Decodes a `RuntimeConfig`, strictly (trailing bytes are an error).
pub(crate) fn parse_config(bytes: &[u8]) -> Option<RuntimeConfig> {
    let mut reader = Reader::new(bytes);
    let config = RuntimeConfig::decode(&mut reader).ok()?;
    reader.finish().ok()?;
    Some(config)
}

/// Maps the runtime's init failure to a `keel_init` code.
pub(crate) fn init_error_code(error: &InitError) -> u32 {
    match error {
        InitError::AlreadyInitialized => init_code::ALREADY_INITIALIZED,
        InitError::InvalidMode(_) => init_code::BAD_CONFIG,
        InitError::Spawn(_) => init_code::START_FAILED,
    }
}

/// `keel_init`: starts the process runtime for `sink`.
///
/// Idempotent: when this crate already started a runtime for the *same* embedder the call does
/// nothing and succeeds; for a different one it fails with `ALREADY_INITIALIZED` and the running
/// runtime is untouched.
pub(crate) fn start(config: &[u8], sink: Arc<dyn Sink>, after_init: impl FnOnce(&Runtime)) -> u32 {
    guarded(
        "keel_init",
        |_| init_code::PANICKED,
        || {
            let Some(config) = parse_config(config) else {
                return init_code::BAD_CONFIG;
            };
            let mut slot = installed();
            if let Some(existing) = slot.as_ref() {
                if api::runtime().is_some() {
                    return if existing.same_embedder(&*sink) {
                        init_code::OK
                    } else {
                        init_code::ALREADY_INITIALIZED
                    };
                }
                // The runtime was shut down behind our back (through the Rust API): start over.
                *slot = None;
            }
            let host = Arc::new(NativeHost { sink: sink.clone() });
            match Runtime::init(config, host) {
                Ok(runtime) => {
                    *slot = Some(sink);
                    drop(slot);
                    after_init(&runtime);
                    init_code::OK
                }
                Err(error) => init_error_code(&error),
            }
        },
    )
}

/// `keel_shutdown`: stops the runtime (idempotent) and forgets the embedder, so a later
/// `keel_init` may install another one.
pub(crate) fn stop() {
    guarded(
        "keel_shutdown",
        |_| (),
        || {
            let mut slot = installed();
            if let Some(rt) = api::runtime() {
                rt.shutdown();
            }
            *slot = None;
        },
    );
}
