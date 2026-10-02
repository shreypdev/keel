//! A store-and-signal migration hook whose return type is not the signal's (ADR-037 decision 6).
//!
//! The compiler cannot see a store's signals from a free function, so such a hook compiles and the
//! runtime reports it with an E0066 ERROR when it starts. Its bytes must never be spliced into the
//! restore: an `i32` hook for an `f32` signal would decode as a wrong `f32` and the restore would
//! answer `Ok`, the very misdecode ADR-037 exists to prevent. The restore is refused instead, typed,
//! and the core is unchanged. (Kept in its own test binary: the other migration tests assert that
//! every registered hook is well-formed.)

use undra::meta::{TypeRef, ids};
use undra::prelude::*;
use undra::runtime::RestoreError;
use undra::runtime::testing::TestRuntime;
use undra::wire::payload::{CallTarget, ReplyStatus, Snapshot, SnapshotType, StoreSnapshot};
use undra::wire::{Decode, Encode, Writer};

#[undra::store]
pub struct Gauge {
    level: Signal<f32>,
}

#[undra::api(store)]
impl Gauge {
    #[allow(clippy::new_without_default)] // a constructor the platforms call
    pub fn new() -> Self {
        Gauge {
            level: Signal::new(2.5),
        }
    }
}

/// Wrong: `Gauge.level` is an `f32`, this returns an `i32` (E0066 at start-up).
#[undra::migrate(store = "Gauge", signal = "level")]
fn gauge_level(old: Option<&DynValue>) -> Result<i32, MigrateError> {
    Ok(old.and_then(DynValue::as_i64).unwrap_or(0) as i32)
}

#[test]
fn a_hook_returning_another_type_than_its_signal_refuses_the_restore() {
    let t = TestRuntime::new();
    let problems = undra::persist::check_migrations(t.runtime().schema());
    assert!(
        problems
            .iter()
            .any(|p| p.contains("E0066") && p.contains("gauge_level")),
        "the start-up check names the hook: {problems:?}"
    );

    let reply = t.call_sync(
        CallTarget::Constructor {
            type_id: ids::type_id("Gauge"),
            method_id: ids::method_id("Gauge", "new"),
        },
        1,
        &[],
    );
    assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    let handle = Handle(u64::decode_exact(&reply.body).unwrap());

    // The older build stored `level` as an `i64` (not structural to `f32`: the hook runs).
    let mut old = t.runtime().schema().clone();
    let gauge = old
        .objects
        .iter_mut()
        .find(|o| o.name == "Gauge")
        .and_then(|o| o.store.as_mut())
        .unwrap();
    gauge.signals[0].ty = TypeRef::I64;
    let type_id = ids::type_id("Gauge");
    let snapshot = Snapshot {
        generation_floor: 1,
        schema_hash: old.hash(),
        types: vec![SnapshotType {
            type_id,
            fingerprint: old.store_fingerprint(type_id).unwrap(),
        }],
        description: old.stores_closure(&[type_id]).canonical_json(),
        stores: vec![StoreSnapshot {
            handle,
            type_id,
            signals: vec![(0, 1_084_227_584_i64.encode_to_vec())],
        }],
    };
    let mut w = Writer::new();
    snapshot.encode(&mut w);
    let before = t.runtime().snapshot();

    match t.runtime().restore(w.as_slice()) {
        Err(RestoreError::Incompatible {
            store,
            signal,
            reason,
            ..
        }) => {
            assert_eq!((store.as_str(), signal.as_str()), ("Gauge", "level"));
            assert!(reason.contains("gauge_level"), "{reason}");
        }
        other => panic!(
            "a mistyped hook's bytes must not be restored: {other:?}, level = {}",
            t.runtime().object::<Gauge>(handle.0).unwrap().level.get()
        ),
    }
    assert_eq!(
        t.runtime().snapshot(),
        before,
        "a refused restore changes nothing"
    );
}
