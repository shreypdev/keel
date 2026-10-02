//! The safe Rust layer under every shim (C ABI, JNI, wasm).
//!
//! Each function is one boundary entry expressed in plain Rust types: it finds the process
//! runtime, forwards to it under the [panic guard](crate::guard) and turns "no runtime yet" and
//! panics into typed values (a status 5 or 2 reply, an error code, or nothing for `void`
//! entries). The shims only convert raw pointers, Java arrays and wasm memory to slices and
//! back, so this is where the semantics live and where the unit tests reach without any
//! `unsafe`.

use std::sync::Arc;

#[cfg(any(target_family = "wasm", test))]
use undra_runtime::undra_wire::payload::PortReply;
use undra_runtime::undra_wire::payload::{Call, Reply, ReplyStatus};
use undra_runtime::undra_wire::{Decode, Reader, Writer};
use undra_runtime::{InitError, RestoreError, Runtime, RuntimeConfig};

use crate::guard::guarded;

/// The native C ABI version this crate implements: the `abi_version` of every [`UndraApi`] table and
/// what the JNI shim's `abiVersion()` returns. Version 2 is the function table of ADR-044 (version 1
/// was 19 global `undra_*` symbols); the wire and the schema hash did not change with it.
///
/// [`UndraApi`]: crate::UndraApi
pub const ABI_VERSION: u32 = 2;

/// The wasm ABI version (SPEC 7, `undra_abi_version` of a wasm core). The wasm ABI did not change
/// with the native table (ADR-044: a wasm module is its own namespace already).
pub const WASM_ABI_VERSION: u32 = 1;

/// Result codes of `undra_init` (`0` is success).
pub mod init_code {
    /// The runtime is up.
    pub const OK: u32 = 0;
    /// A runtime is already running with a different embedder (or one that was not started by
    /// this crate); `undra_init` with the *same* callbacks again is a no-op that returns
    /// [`OK`].
    pub const ALREADY_INITIALIZED: u32 = 1;
    /// The `RuntimeConfig` bytes do not decode, or `mode` is neither `"inproc"` nor `"dev"`.
    pub const BAD_CONFIG: u32 = 2;
    /// A required callback is null.
    pub const BAD_ARGUMENT: u32 = 3;
    /// The core thread could not be started.
    pub const START_FAILED: u32 = 4;
    /// A panic was contained inside `undra_init`.
    pub const PANICKED: u32 = 5;
}

/// Result codes of `undra_restore` (`0` is success). A failed restore leaves the core unchanged.
pub mod restore_code {
    /// Every store was rebuilt.
    pub const OK: u32 = 0;
    /// A store's restore function panicked (contained).
    pub const PANICKED: u32 = 2;
    /// The snapshot is malformed, names an unknown store type, has a null or duplicate handle,
    /// or a store rejected its values (SPEC 3.4 status 5, `bad_request`).
    pub const BAD_SNAPSHOT: u32 = 5;
    /// There is no running runtime, it is shut down, or `undra_restore` was called from inside a
    /// host callback.
    pub const UNAVAILABLE: u32 = 6;
    /// A store's persisted values cannot become this build's types: they neither migrate
    /// structurally nor through a `#[undra::migrate]` hook (ADR-037, `RestoreError::Incompatible`).
    /// The reason is in the ERROR record the host's Log port received.
    pub const INCOMPATIBLE: u32 = 7;
}

/// Decodes a `RuntimeConfig`, strictly (trailing bytes are an error).
pub(crate) fn parse_config(bytes: &[u8]) -> Option<RuntimeConfig> {
    let mut reader = Reader::new(bytes);
    let config = RuntimeConfig::decode(&mut reader).ok()?;
    reader.finish().ok()?;
    Some(config)
}

/// Maps the runtime's init failure to an `undra_init` code.
pub(crate) fn init_error_code(error: &InitError) -> u32 {
    match error {
        InitError::AlreadyInitialized => init_code::ALREADY_INITIALIZED,
        InitError::InvalidMode(_) => init_code::BAD_CONFIG,
        InitError::Spawn(_) => init_code::START_FAILED,
    }
}

/// The running runtime, if any.
pub(crate) fn runtime() -> Option<Arc<Runtime>> {
    Runtime::global()
}

/// The schema hash of this core, also before `undra_init`.
pub(crate) fn schema_hash() -> u64 {
    match runtime() {
        Some(rt) => rt.schema_hash(),
        None => undra_runtime::undra_meta::collect_schema("undra-core").hash(),
    }
}

/// The schema JSON of this core, also before `undra_init`: the whole schema (`Schema::to_json`),
/// doc comments and labels included, not the canonical form the hash is computed over. The hash
/// does not cover docs or labels, so `Schema::from_json(..).hash()` of these bytes is
/// [`schema_hash`]; hosts that want the hash read it from `undra_schema_hash`.
pub(crate) fn schema_json() -> Vec<u8> {
    let json = match runtime() {
        Some(rt) => rt.schema().to_json(),
        None => undra_runtime::undra_meta::collect_schema("undra-core").to_json(),
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

/// `undra_call`: `0` accepted (a reply follows through the host), `5` rejected.
pub(crate) fn call(payload: &[u8]) -> u32 {
    guarded(
        "undra_call",
        |_| 5,
        || runtime().map_or(5, |rt| rt.call(payload)),
    )
}

/// `undra_call_sync`: the whole `Reply` payload.
pub(crate) fn call_sync(payload: &[u8]) -> Vec<u8> {
    guarded(
        "undra_call_sync",
        |message| {
            let mut body = string_body(message);
            body.extend_from_slice(&string_body("panicked in undra-ffi"));
            reply_bytes(call_id_of(payload), ReplyStatus::Panic, &body)
        },
        || match runtime() {
            Some(rt) => rt.call_sync(payload),
            None => bad_request(payload, "the Undra core is not initialized"),
        },
    )
}

/// `undra_cancel`.
pub(crate) fn cancel(call_id: u32) {
    guarded(
        "undra_cancel",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.cancel(call_id);
            }
        },
    );
}

/// `undra_stream_credit`.
pub(crate) fn stream_credit(call_id: u32, credit: u32) {
    guarded(
        "undra_stream_credit",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.stream_credit(call_id, credit);
            }
        },
    );
}

/// `undra_observe`.
pub(crate) fn observe(handle: u64, signal_id: u32, on: bool) {
    guarded(
        "undra_observe",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.observe(handle, signal_id, on);
            }
        },
    );
}

/// `undra_release`.
pub(crate) fn release(handle: u64) {
    guarded(
        "undra_release",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.release(handle);
            }
        },
    );
}

/// The port call id of fire-and-forget calls: the core sends the host's `Log` port its log
/// records under it and never waits for an answer. Real port calls are numbered from 1.
pub(crate) const FIRE_AND_FORGET_PORT_CALL: u32 = 0;

/// Whether `payload` is a well-formed `PortReply` header answering a fire-and-forget call.
fn answers_a_fire_and_forget_call(payload: &[u8]) -> bool {
    payload.len() >= 5 && payload[..4] == FIRE_AND_FORGET_PORT_CALL.to_le_bytes()
}

/// The port call a well-formed `PortReply` payload answers (`None` for a malformed one, which the
/// runtime rejects too, so it answers nothing).
#[cfg(any(target_family = "wasm", test))]
pub(crate) fn port_reply_call_id(payload: &[u8]) -> Option<u32> {
    PortReply::decode(&mut Reader::new(payload))
        .ok()
        .map(|reply| reply.port_call_id)
}

/// `undra_port_reply`. An answer to a fire-and-forget call is dropped silently: nothing waits for
/// it, and logging "no port call 0 is pending" would be one more Log call, which a host that
/// answers Log asynchronously would answer again, without end.
pub(crate) fn port_reply(payload: &[u8]) {
    guarded(
        "undra_port_reply",
        |_| (),
        || {
            if answers_a_fire_and_forget_call(payload) {
                return;
            }
            if let Some(rt) = runtime() {
                rt.port_reply(payload);
            }
        },
    );
}

/// `undra_event`.
pub(crate) fn event(port_id: u32, method_id: u32, payload: &[u8]) {
    guarded(
        "undra_event",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.event(port_id, method_id, payload);
            }
        },
    );
}

/// `undra_timer_fired`.
pub(crate) fn timer_fired(timer_id: u32) {
    guarded(
        "undra_timer_fired",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.timer_fired(timer_id);
            }
        },
    );
}

/// `undra_poll` (wasm): drives the executor.
#[cfg(target_family = "wasm")]
pub(crate) fn poll() {
    guarded(
        "undra_poll",
        |_| (),
        || {
            if let Some(rt) = runtime() {
                rt.poll();
            }
        },
    );
}

/// A snapshot of nothing: `count u32 = 0, generation_floor u32` (SPEC 5.9), the floor being the
/// process-wide generation counter: it outlives `undra_shutdown`, so a host that snapshots
/// between a shutdown and the next init keeps ADR-022's guarantee that a generation it may still
/// hold is never issued again in this process.
fn empty_snapshot() -> Vec<u8> {
    // Layout 2 (ADR-037): no stores, the floor, this core's schema hash, no types, and the
    // description of no store types.
    let description = undra_runtime::undra_meta::StoresClosure {
        stores: Vec::new(),
        records: Vec::new(),
        enums: Vec::new(),
    }
    .canonical_json();
    let mut w = Writer::with_capacity(24 + description.len());
    w.write_u32(0);
    w.write_u32(undra_runtime::object_table::process_generation_floor());
    w.write_u64(schema_hash());
    w.write_u32(0);
    w.write_str(&description);
    w.into_vec()
}

/// `undra_snapshot`: a `Snapshot` payload; with no runtime, an empty one carrying the
/// process-wide generation floor.
pub(crate) fn snapshot() -> Vec<u8> {
    guarded(
        "undra_snapshot",
        |_| empty_snapshot(),
        || match runtime() {
            Some(rt) => rt.snapshot(),
            None => empty_snapshot(),
        },
    )
}

/// Maps the runtime's restore result to an `undra_restore` code.
pub(crate) fn restore_code(result: &Result<(), RestoreError>) -> u32 {
    match result {
        Ok(()) => restore_code::OK,
        Err(RestoreError::Panicked { .. }) => restore_code::PANICKED,
        Err(RestoreError::ShutDown | RestoreError::Reentrant) => restore_code::UNAVAILABLE,
        Err(RestoreError::Incompatible { .. }) => restore_code::INCOMPATIBLE,
        Err(
            RestoreError::Decode(_)
            | RestoreError::UnknownStoreType { .. }
            | RestoreError::Store { .. }
            | RestoreError::BadHandle { .. }
            | RestoreError::GenerationFloor { .. },
        ) => restore_code::BAD_SNAPSHOT,
    }
}

/// `undra_restore`: `0` on success, otherwise a [`restore_code`].
pub(crate) fn restore(payload: &[u8]) -> u32 {
    guarded(
        "undra_restore",
        |_| restore_code::PANICKED,
        || match runtime() {
            Some(rt) => restore_code(&rt.restore(payload)),
            None => restore_code::UNAVAILABLE,
        },
    )
}

/// `undra_stats_json`. With no runtime the document also says how many threads started by
/// `undra-runtime` are still running in the process (`runtime_threads`): `0` once a shutdown has
/// joined them, which is how a host checks that closing ended the core's work (ADR-034) rather
/// than only detaching from it.
pub(crate) fn stats_json() -> String {
    guarded(
        "undra_stats_json",
        |_| "{\"initialized\":false,\"panicked\":true}".to_owned(),
        || match runtime() {
            Some(rt) => rt.stats_json(),
            None => format!(
                "{{\"initialized\":false,\"live_handles\":0,\"runtime_threads\":{}}}",
                undra_runtime::testing::live_threads()
            ),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use undra_runtime::undra_wire::Reader;

    fn decode(reply: &[u8]) -> (u32, ReplyStatus, Vec<u8>) {
        let r = Reply::decode(&mut Reader::new(reply)).expect("a reply payload");
        (r.call_id, r.status, r.body.to_vec())
    }

    #[test]
    fn a_port_reply_names_its_call_only_when_well_formed() {
        assert_eq!(port_reply_call_id(&[7, 0, 0, 0, 0]), Some(7));
        assert_eq!(port_reply_call_id(&[7, 0, 0, 0, 0, 1, 2]), Some(7));
        assert_eq!(port_reply_call_id(&[7, 0, 0, 0, 9]), None, "no such status");
        assert_eq!(port_reply_call_id(&[7, 0, 0]), None);
    }

    #[test]
    fn the_empty_snapshot_is_count_zero_then_the_process_floor() {
        let floor = undra_runtime::object_table::process_generation_floor();
        let bytes = empty_snapshot();
        assert_eq!(bytes[..4], [0; 4]);
        let carried = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
        // Layout 2 (ADR-037): it decodes, with this core's hash and no types.
        let snapshot = undra_runtime::undra_wire::payload::Snapshot::decode(
            &mut undra_runtime::undra_wire::Reader::new(&bytes),
        )
        .unwrap();
        assert_eq!(snapshot.schema_hash, schema_hash());
        assert!(snapshot.types.is_empty() && snapshot.stores.is_empty());
        assert!(
            carried >= floor,
            "{carried} < {floor}: the floor never goes down"
        );
    }

    #[test]
    fn only_a_whole_port_reply_header_with_id_zero_is_fire_and_forget() {
        assert!(answers_a_fire_and_forget_call(&[0, 0, 0, 0, 0]));
        assert!(answers_a_fire_and_forget_call(&[0, 0, 0, 0, 2, 9, 9]));
        assert!(!answers_a_fire_and_forget_call(&[0, 0, 0, 0]), "truncated");
        assert!(!answers_a_fire_and_forget_call(&[1, 0, 0, 0, 0]));
        assert!(!answers_a_fire_and_forget_call(&[0, 0, 0, 1, 0]));
        assert!(!answers_a_fire_and_forget_call(&[]));
    }

    #[test]
    fn restore_codes_cover_every_error() {
        use undra_runtime::undra_wire::WireError;
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
        assert_eq!(
            restore_code(&Err(RestoreError::Incompatible {
                type_id: 1,
                store: "Profile".into(),
                signal: "age".into(),
                reason: "i32 cannot become f32".into(),
            })),
            restore_code::INCOMPATIBLE
        );
        for e in [
            RestoreError::Decode(WireError::BadMagic),
            RestoreError::UnknownStoreType { type_id: 1 },
            RestoreError::BadHandle { handle: 0 },
            RestoreError::GenerationFloor { floor: u32::MAX },
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
            target: undra_runtime::undra_wire::payload::CallTarget::Function { method_id: 7 },
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
