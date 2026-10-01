//! A host that does nothing but count, and the helpers that build a runtime around it.
//!
//! The runtime's own `RecordingHost` keeps every payload it is handed, which is what a test
//! wants and what a benchmark must not do (a few million change-sets would be gigabytes). The
//! [`CountingHost`] is what a platform runtime looks like from the core's side at its cheapest:
//! it is told about each reply and change-set and keeps two integers.
#![allow(missing_docs, dead_code)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use undra::meta::ids;
use undra::runtime::testing::{call_payload, decode_reply};
use undra::runtime::{Host, PortCallOutcome};
use undra::runtime::{Runtime, RuntimeConfig};
use undra::wire::payload::{CallTarget, ReplyStatus};
use undra::wire::{Decode, Handle};

#[derive(Default)]
pub struct CountingHost {
    pub replies: AtomicU64,
    pub reply_bytes: AtomicU64,
    pub change_sets: AtomicU64,
    pub change_set_bytes: AtomicU64,
    /// Log records at warn or above: a benchmark that provokes one is measuring a failure.
    pub warnings: AtomicU64,
}

impl CountingHost {
    pub fn change_sets(&self) -> u64 {
        self.change_sets.load(Ordering::Relaxed)
    }

    pub fn change_set_bytes(&self) -> u64 {
        self.change_set_bytes.load(Ordering::Relaxed)
    }

    pub fn replies(&self) -> u64 {
        self.replies.load(Ordering::Relaxed)
    }

    pub fn warnings(&self) -> u64 {
        self.warnings.load(Ordering::Relaxed)
    }
}

impl Host for CountingHost {
    fn reply(&self, _call_id: u32, payload: &[u8]) {
        self.replies.fetch_add(1, Ordering::Relaxed);
        self.reply_bytes
            .fetch_add(payload.len() as u64, Ordering::Relaxed);
    }

    fn change_set(&self, payload: &[u8]) {
        self.change_sets.fetch_add(1, Ordering::Relaxed);
        self.change_set_bytes
            .fetch_add(payload.len() as u64, Ordering::Relaxed);
    }

    fn stream_item(&self, _call_id: u32, _payload: &[u8]) {}

    fn port_call(&self, _: u32, _: u32, _: u32, _: &[u8]) -> PortCallOutcome {
        PortCallOutcome::Unavailable
    }

    fn log(&self, level: u8, _target: &str, _message: &str) {
        if level >= 3 {
            self.warnings.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// A runtime that is shut down when it is dropped.
///
/// `Runtime::new` hands out an `Arc<Runtime>` whose last drop does **not** stop it: the
/// query client (an extension) holds a `Ctx`, a reference cycle, so a runtime that is merely
/// dropped keeps itself, and its `undra-core` thread, alive for the life of the process (see
/// `bench/RESULTS.md`, findings). `shutdown()` is what releases it, so the harness calls it, and
/// a benchmark that builds hundreds of runtimes does not accumulate threads.
pub struct Core(Arc<Runtime>);

impl Core {
    /// Builds a runtime for `config` around `host`.
    pub fn new(config: RuntimeConfig, host: Arc<CountingHost>) -> Core {
        Core(Runtime::new(config, host).expect("a runtime"))
    }
}

impl std::ops::Deref for Core {
    type Target = Runtime;

    fn deref(&self) -> &Runtime {
        &self.0
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

/// A runtime with no `undra-core` thread (the host drives the executor with `run_pending`, as the
/// wasm shell does), so the numbers do not include a thread hop; `bench/RESULTS.md` has the
/// `undra_call` numbers that do.
pub fn runtime() -> (Arc<Core>, Arc<CountingHost>) {
    let host = Arc::new(CountingHost::default());
    let config = RuntimeConfig {
        platform: "bench".to_owned(),
        core_threads: 0,
        ..RuntimeConfig::default()
    };
    (Arc::new(Core::new(config, host.clone())), host)
}

/// Calls `Type::new(args)` and returns the new object's handle.
pub fn construct(rt: &Runtime, type_name: &str, args: &[u8]) -> Handle {
    let payload = call_payload(
        CallTarget::Constructor {
            type_id: ids::type_id(type_name),
            method_id: ids::method_id(type_name, "new"),
        },
        1,
        args,
    );
    let reply = decode_reply(&rt.call_sync(&payload));
    assert_eq!(
        reply.status,
        ReplyStatus::Ok,
        "{type_name}::new answered {reply:?}"
    );
    Handle::decode_exact(&reply.body).expect("a constructor answers a handle")
}

/// The prebuilt `Call` payload for a method call on `handle`.
pub fn method_call(
    handle: Handle,
    type_name: &str,
    method: &str,
    call_id: u32,
    args: &[u8],
) -> Vec<u8> {
    call_payload(
        CallTarget::Method {
            handle,
            method_id: ids::method_id(type_name, method),
        },
        call_id,
        args,
    )
}

/// Runs a prebuilt sync call and panics unless it succeeded.
pub fn call_ok(rt: &Runtime, payload: &[u8]) -> Vec<u8> {
    let reply = rt.call_sync(payload);
    let status = decode_reply(&reply).status;
    assert_eq!(status, ReplyStatus::Ok, "call failed: {status:?}");
    reply
}
