//! The safe Rust layer under every shim (C ABI, JNI, wasm).
//!
//! Each function is one boundary entry expressed in plain Rust types: it finds the process
//! runtime, forwards to it under the [panic guard](crate::guard) and turns "no runtime yet" and
//! panics into typed values (a status 5 or 2 reply, an error code, or nothing for `void`
//! entries). The shims only convert raw pointers, Java arrays and wasm memory to slices and
//! back, so this is where the semantics live and where the unit tests reach without any
//! `unsafe`.

use std::sync::Arc;

use keel_runtime::keel_wire::payload::{Call, Reply, ReplyStatus};
use keel_runtime::keel_wire::{Reader, Writer};
use keel_runtime::{RestoreError, Runtime};

use crate::guard::guarded;

/// The ABI version this crate implements (`keel_abi_version`).
pub const ABI_VERSION: u32 = 1;

/// Result codes of `keel_init` (`0` is success).
pub mod init_code {
    /// The runtime is up.
    pub const OK: u32 = 0;
    /// A runtime is already running with a different embedder (or one that was not started by
    /// this crate); `keel_init` with the *same* callbacks again is a no-op that returns
    /// [`OK`].
    pub const ALREADY_INITIALIZED: u32 = 1;
    /// The `RuntimeConfig` bytes do not decode, or `mode` is neither `"inproc"` nor `"dev"`.
    pub const BAD_CONFIG: u32 = 2;
    /// A required callback is null.
    pub const BAD_ARGUMENT: u32 = 3;
    /// The core thread could not be started.
    pub const START_FAILED: u32 = 4;
    /// A panic was contained inside `keel_init`.
    pub const PANICKED: u32 = 5;
}

/// Result codes of `keel_restore` (`0` is success). A failed restore leaves the core unchanged.
pub mod restore_code {
    /// Every store was rebuilt.
    pub const OK: u32 = 0;
    /// A store's restore function panicked (contained).
    pub const PANICKED: u32 = 2;
    /// The snapshot is malformed, names an unknown store type, has a null or duplicate handle,
    /// or a store rejected its values (SPEC 3.4 status 5, `bad_request`).
    pub const BAD_SNAPSHOT: u32 = 5;
    /// There is no running runtime, it is shut down, or `keel_restore` was called from inside a
    /// host callback.
    pub const UNAVAILABLE: u32 = 6;
}

/// The running runtime, if any.
pub(crate) fn runtime() -> Option<Arc<Runtime>> {
    Runtime::global()
}

/// The schema hash of this core, also before `keel_init`.
pub(crate) fn schema_hash() -> u64 {
    match runtime() {
        Some(rt) => rt.schema_hash(),
        None => keel_runtime::keel_meta::collect_schema("keel-core").hash(),
    }
}

/// The canonical schema JSON of this core (what the hash covers), also before `keel_init`.
pub(crate) fn schema_json() -> Vec<u8> {
    let json = match runtime() {
        Some(rt) => rt.schema().canonical_json(),
        None => keel_runtime::keel_meta::collect_schema("keel-core").canonical_json(),
    };
    json.into_bytes()
}

fn string_body(text: &str) -> Vec<u8> {
    let mut w = Writer::with_capacity(4 + text.len());
    w.write_str(text);
    w.into_vec()
}

fn reply_bytes(call_id: u32, status: ReplyStatus, body: &[u8]) -> Vec<u8> {
    let mut w = Writer::with_capacity(5 + body.len());
    Reply {
        call_id,
        status,
        body,
    }
    .encode(&mut w);
    w.into_vec()
}

/// The id of the call in `payload`, if it decodes; `0` (never a valid id) otherwise.
fn call_id_of(payload: &[u8]) -> u32 {
    Call::decode(&mut Reader::new(payload)).map_or(0, |call| call.call_id)
}

/// A status 5 reply for a call the shim could not hand to a runtime.
fn bad_request(payload: &[u8], reason: &str) -> Vec<u8> {
    reply_bytes(
        call_id_of(payload),
        ReplyStatus::BadRequest,
        &string_body(reason),
    )
}

/// `keel_call`: `0` accepted (a reply follows through the host), `5` rejected.
pub(crate) fn call(payload: &[u8]) -> u32 {
    guarded(
        "keel_call",
        |_| 5,
        || runtime().map_or(5, |rt| rt.call(payload)),
    )
}

/// `keel_call_sync`: the whole `Reply` payload.
pub(crate) fn call_sync(payload: &[u8]) -> Vec<u8> {
    guarded(
        "keel_call_sync",
        |message| {
            let mut body = string_body(message);
            body.extend_from_slice(&string_body("panicked in keel-ffi"));
            reply_bytes(call_id_of(payload), ReplyStatus::Panic, &body)
        },
        || match runtime() {
            Some(rt) => rt.call_sync(payload),
            None => bad_request(payload, "the Keel core is not initialized"),
        },
    )
}

/// `keel_cancel`.
pub(crate) fn cancel(call_id: u32) {
    guarded(
        "keel_cancel",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.cancel(call_id);
            }
        },
    );
}

/// `keel_stream_credit`.
pub(crate) fn stream_credit(call_id: u32, credit: u32) {
    guarded(
        "keel_stream_credit",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.stream_credit(call_id, credit);
            }
        },
    );
}

/// `keel_observe`.
pub(crate) fn observe(handle: u64, signal_id: u32, on: bool) {
    guarded(
        "keel_observe",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.observe(handle, signal_id, on);
            }
        },
    );
}

/// `keel_release`.
pub(crate) fn release(handle: u64) {
    guarded(
        "keel_release",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.release(handle);
            }
        },
    );
}

/// `keel_port_reply`.
pub(crate) fn port_reply(payload: &[u8]) {
    guarded(
        "keel_port_reply",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.port_reply(payload);
            }
        },
    );
}

/// `keel_event`.
pub(crate) fn event(port_id: u32, method_id: u32, payload: &[u8]) {
    guarded(
        "keel_event",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.event(port_id, method_id, payload);
            }
        },
    );
}

/// `keel_timer_fired`.
pub(crate) fn timer_fired(timer_id: u32) {
    guarded(
        "keel_timer_fired",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.timer_fired(timer_id);
            }
        },
    );
}

/// `keel_poll` (wasm): drives the executor.
#[cfg(target_family = "wasm")]
pub(crate) fn poll() {
    guarded(
        "keel_poll",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.poll();
            }
        },
    );
}

/// `keel_snapshot`: a `Snapshot` payload; an empty one (no stores) before init.
pub(crate) fn snapshot() -> Vec<u8> {
    guarded(
        "keel_snapshot",
        |_| 0_u32.to_le_bytes().to_vec(),
        || match runtime() {
            Some(rt) => rt.snapshot(),
            None => 0_u32.to_le_bytes().to_vec(),
        },
    )
}

/// Maps the runtime's restore result to a `keel_restore` code.
pub(crate) fn restore_code(result: &Result<(), RestoreError>) -> u32 {
    match result {
        Ok(()) => restore_code::OK,
        Err(RestoreError::Panicked { .. }) => restore_code::PANICKED,
        Err(RestoreError::ShutDown | RestoreError::Reentrant) => restore_code::UNAVAILABLE,
        Err(
            RestoreError::Decode(_)
            | RestoreError::UnknownStoreType { .. }
            | RestoreError::Store { .. }
            | RestoreError::BadHandle { .. },
        ) => restore_code::BAD_SNAPSHOT,
    }
}

/// `keel_restore`: `0` on success, otherwise a [`restore_code`].
pub(crate) fn restore(payload: &[u8]) -> u32 {
    guarded(
        "keel_restore",
        |_| restore_code::PANICKED,
        || match runtime() {
            Some(rt) => restore_code(&rt.restore(payload)),
            None => restore_code::UNAVAILABLE,
        },
    )
}

/// `keel_stats_json`.
pub(crate) fn stats_json() -> String {
    guarded(
        "keel_stats_json",
        |_| "{\"initialized\":false,\"panicked\":true}".to_owned(),
        || match runtime() {
            Some(rt) => rt.stats_json(),
            None => "{\"initialized\":false,\"live_handles\":0}".to_owned(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_runtime::keel_wire::Reader;

    fn decode(reply: &[u8]) -> (u32, ReplyStatus, Vec<u8>) {
        let r = Reply::decode(&mut Reader::new(reply)).expect("a reply payload");
        (r.call_id, r.status, r.body.to_vec())
    }

    #[test]
    fn restore_codes_cover_every_error() {
        use keel_runtime::keel_wire::WireError;
        assert_eq!(restore_code(&Ok(())), 0);
        assert_eq!(
            restore_code(&Err(RestoreError::Panicked {
                type_id: 1,
                message: String::new()
            })),
            restore_code::PANICKED
        );
        assert_eq!(
            restore_code(&Err(RestoreError::ShutDown)),
            restore_code::UNAVAILABLE
        );
        assert_eq!(
            restore_code(&Err(RestoreError::Reentrant)),
            restore_code::UNAVAILABLE
        );
        for e in [
            RestoreError::Decode(WireError::BadMagic),
            RestoreError::UnknownStoreType { type_id: 1 },
            RestoreError::BadHandle { handle: 0 },
            RestoreError::Store {
                type_id: 1,
                source: WireError::BadMagic,
            },
        ] {
            assert_eq!(restore_code(&Err(e)), restore_code::BAD_SNAPSHOT);
        }
    }

    #[test]
    fn bad_request_echoes_the_call_id_when_the_payload_decodes() {
        let mut w = Writer::new();
        Call {
            target: keel_runtime::keel_wire::payload::CallTarget::Function { method_id: 7 },
            call_id: 42,
            args: &[],
        }
        .encode(&mut w);
        let (id, status, body) = decode(&bad_request(w.as_slice(), "why"));
        assert_eq!((id, status), (42, ReplyStatus::BadRequest));
        assert_eq!(body, [3, 0, 0, 0, b'w', b'h', b'y']);
        let (id, status, _) = decode(&bad_request(&[0xff], "why"));
        assert_eq!((id, status), (0, ReplyStatus::BadRequest));
    }
}
