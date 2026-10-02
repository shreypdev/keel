//! Dispatch layers (`DispatchLayer`) and transient stores: what `undra-query` builds on.
//!
//! The layer under test serves ids that no registration names, exactly as `undra-query` serves
//! the handle type and the mutation function of every user query.

use std::any::Any;
use std::sync::Arc;

use undra_meta::{DispatchCall, DispatchOutcome, ids};
use undra_runtime::testing::TestRuntime;
use undra_runtime::{
    AnyObject, DispatchLayer, DispatchResult, Handle, Runtime, RuntimeConfig, UndraObjectDyn,
    inventory,
};
use undra_signals::{Signal, StoreCell};
use undra_wire::payload::{CallTarget, ChangeOp, ReplyStatus};
use undra_wire::{Decode, Encode, Reader};

/// A function the layer serves: doubles a `u32`.
const DOUBLE: u32 = ids::function_id("layer_double");
/// A function whose dispatcher panics.
const BOOM: u32 = ids::function_id("layer_boom");
/// A function the layer refuses with a reason.
const REFUSE: u32 = ids::function_id("layer_refuse");
/// A function the layer answers with a failure after it took its arguments (no unwinding).
const FAIL: u32 = ids::function_id("layer_fail");
/// The type id of the object the layer constructs (nothing registers it statically).
const GADGET: u32 = ids::type_id("LayerGadget");
/// A method of the gadget: reads its counter.
const READ: u32 = ids::method_id("LayerGadget", "read");

/// A store with one signal whose type has no static registration. `transient` decides whether
/// snapshots leave it out.
struct Gadget {
    cell: Arc<StoreCell>,
    count: Signal<u32>,
    transient: bool,
}

struct GadgetObject(Arc<Gadget>);

impl UndraObjectDyn for GadgetObject {
    fn undra_type_id(&self) -> u32 {
        GADGET
    }

    fn undra_type_name(&self) -> &'static str {
        "LayerGadget"
    }

    fn as_store(&self) -> Option<&Arc<StoreCell>> {
        Some(&self.0.cell)
    }

    fn transient(&self) -> bool {
        self.0.transient
    }
}

impl AnyObject for GadgetObject {
    fn shared(&self) -> Arc<dyn Any + Send + Sync> {
        self.0.clone()
    }
}

fn dispatch(rt: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
    let Some(rt) = rt.downcast_ref::<Runtime>() else {
        return DispatchOutcome::new(DispatchResult::Unknown);
    };
    DispatchOutcome::new(serve(rt, call))
}

fn serve(rt: &Runtime, call: DispatchCall<'_>) -> DispatchResult {
    match (call.handle, call.method_id) {
        (0, DOUBLE) => match u32::decode_exact(call.args) {
            Ok(n) => DispatchResult::Sync(Ok((n * 2).encode_to_vec())),
            Err(e) => DispatchResult::BadRequest(format!("bad arguments: {e}")),
        },
        (0, BOOM) => panic!("the layer panicked"),
        (0, REFUSE) => DispatchResult::BadRequest("the layer refuses".to_owned()),
        (0, FAIL) => DispatchResult::Failed("the layer took its arguments and failed".to_owned()),
        // The constructor: one argument, whether the gadget is transient.
        (0, GADGET) => {
            let Ok(transient) = bool::decode_exact(call.args) else {
                return DispatchResult::BadRequest("bad arguments".to_owned());
            };
            let cell = StoreCell::new(GADGET);
            let count = Signal::new(7_u32);
            if let Err(e) = cell.attach(&count, 0) {
                return DispatchResult::BadRequest(e.to_string());
            }
            let gadget = Arc::new(Gadget {
                cell,
                count,
                transient,
            });
            let handle = rt.insert(Arc::new(GadgetObject(gadget)));
            DispatchResult::Sync(Ok(handle.encode_to_vec()))
        }
        (handle, READ) if handle != 0 => match rt.object::<Gadget>(handle) {
            Ok(gadget) => DispatchResult::Sync(Ok(gadget.count.get().encode_to_vec())),
            Err(e) => DispatchResult::BadRequest(e.to_string()),
        },
        _ => DispatchResult::Unknown,
    }
}

inventory::submit! {
    DispatchLayer { name: "test-layer", dispatch }
}

fn construct(t: &TestRuntime, transient: bool) -> Handle {
    let reply = t.call_sync(
        CallTarget::Constructor {
            type_id: GADGET,
            method_id: GADGET,
        },
        1,
        &transient.encode_to_vec(),
    );
    assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    Handle::decode_exact(&reply.body).unwrap()
}

#[test]
fn a_layer_serves_functions_that_have_no_registration() {
    let t = TestRuntime::new();
    let reply = t.call_sync(
        CallTarget::Function { method_id: DOUBLE },
        1,
        &21_u32.encode_to_vec(),
    );
    assert_eq!(reply.status, ReplyStatus::Ok);
    assert_eq!(u32::decode_exact(&reply.body).unwrap(), 42);
}

#[test]
fn an_id_no_layer_claims_is_still_unknown() {
    let t = TestRuntime::new();
    let reply = t.call_sync(
        CallTarget::Function {
            method_id: ids::function_id("nobody_serves_this"),
        },
        1,
        &[],
    );
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    let mut r = Reader::new(&reply.body);
    let reason = r.read_str().unwrap();
    assert!(reason.starts_with("unknown function"), "{reason}");

    let reply = t.call_sync(
        CallTarget::Constructor {
            type_id: ids::type_id("NoSuchType"),
            method_id: 1,
        },
        2,
        &[],
    );
    assert_eq!(reply.status, ReplyStatus::BadRequest);
}

/// `DispatchResult::Failed` is status 2 with the reason as the message and no unwinding: the call
/// took what it was handed (a refusal, status 5, says it owns nothing), on both reply paths.
#[test]
fn a_failed_call_is_status_two_with_its_reason_and_the_runtime_carries_on() {
    let t = TestRuntime::new();
    let sync = t.call_sync(CallTarget::Function { method_id: FAIL }, 1, &[]);
    assert_eq!(sync.status, ReplyStatus::Panic);
    assert_eq!(
        Reader::new(&sync.body).read_str().unwrap(),
        "the layer took its arguments and failed"
    );
    assert_eq!(t.call(CallTarget::Function { method_id: FAIL }, 2, &[]), 0);
    let replies = t.take_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(
        (replies[0].call_id, replies[0].status),
        (2, ReplyStatus::Panic)
    );
    assert_eq!(
        Reader::new(&replies[0].body).read_str().unwrap(),
        "the layer took its arguments and failed"
    );
    let ok = t.call_sync(
        CallTarget::Function { method_id: DOUBLE },
        3,
        &1_u32.encode_to_vec(),
    );
    assert_eq!(ok.status, ReplyStatus::Ok);
}

#[test]
fn a_layer_can_refuse_with_a_reason_and_a_panicking_layer_is_contained() {
    let t = TestRuntime::new();
    let refused = t.call_sync(CallTarget::Function { method_id: REFUSE }, 1, &[]);
    assert_eq!(refused.status, ReplyStatus::BadRequest);
    assert_eq!(
        Reader::new(&refused.body).read_str().unwrap(),
        "the layer refuses"
    );
    let bad = t.call_sync(CallTarget::Function { method_id: DOUBLE }, 2, &[1]);
    assert_eq!(bad.status, ReplyStatus::BadRequest);

    let boom = t.call_sync(CallTarget::Function { method_id: BOOM }, 3, &[]);
    assert_eq!(boom.status, ReplyStatus::Panic);
    // The runtime carries on.
    let ok = t.call_sync(
        CallTarget::Function { method_id: DOUBLE },
        4,
        &1_u32.encode_to_vec(),
    );
    assert_eq!(ok.status, ReplyStatus::Ok);
}

#[test]
fn a_layer_constructs_an_object_and_serves_its_methods_and_observation() {
    let t = TestRuntime::new();
    let handle = construct(&t, false);
    assert!(!handle.is_null());
    let read = t.call_sync(
        CallTarget::Method {
            handle,
            method_id: READ,
        },
        2,
        &[],
    );
    assert_eq!(read.status, ReplyStatus::Ok);
    assert_eq!(u32::decode_exact(&read.body).unwrap(), 7);

    // It is a store: observing delivers its initial value through the host.
    t.runtime().observe(handle.0, u32::MAX, true);
    let change_sets = t.host().take_decoded_change_sets();
    assert_eq!(change_sets.len(), 1);
    assert_eq!(change_sets[0].entries[0].op, ChangeOp::Full);
    assert_eq!(change_sets[0].entries[0].value, 7_u32.encode_to_vec());

    // A stale handle is a bad request, not a panic.
    t.runtime().release(handle.0);
    let stale = t.call_sync(
        CallTarget::Method {
            handle,
            method_id: READ,
        },
        3,
        &[],
    );
    assert_eq!(stale.status, ReplyStatus::BadRequest);
}

fn snapshot_store_count(t: &TestRuntime) -> u32 {
    let bytes = t.runtime().snapshot();
    Reader::new(&bytes).read_u32().unwrap()
}

#[test]
fn transient_stores_are_left_out_of_snapshots_so_restore_still_works() {
    let t = TestRuntime::new();
    let transient = construct(&t, true);
    assert_eq!(snapshot_store_count(&t), 0, "a transient store is skipped");

    let snapshot = t.runtime().snapshot();
    t.runtime().restore(&snapshot).unwrap();
    // Like every object that is not in the snapshot, its handle is stale afterwards.
    let stale = t.call_sync(
        CallTarget::Method {
            handle: transient,
            method_id: READ,
        },
        9,
        &[],
    );
    assert_eq!(stale.status, ReplyStatus::BadRequest);
}

#[test]
fn a_store_that_is_not_transient_is_snapshotted_and_without_a_restorer_is_left_out() {
    let t = TestRuntime::new();
    let kept = construct(&t, false);
    assert_eq!(snapshot_store_count(&t), 1);
    // Nothing registered a `StoreRestorer` for the gadget: to a restore it is a store type this
    // build does not have, which since ADR-037 is left out and reported instead of failing the
    // restore as a whole (query handles are transient so they are not even written).
    let snapshot = t.runtime().snapshot();
    let report = t.runtime().restore_with_report(&snapshot).unwrap();
    assert_eq!(report.restored, 0);
    assert_eq!(report.dropped.len(), 1);
    assert_eq!(report.dropped[0].handles, [kept.0]);
    let read = t.call_sync(
        CallTarget::Method {
            handle: kept,
            method_id: READ,
        },
        2,
        &[],
    );
    assert_eq!(read.status, ReplyStatus::BadRequest, "its handle is stale");
}

#[test]
fn layers_work_on_a_runtime_with_a_core_thread_too() {
    // The same table is built for every runtime; a call from a plain thread reaches the layer.
    let host = Arc::new(undra_runtime::testing::RecordingHost::new());
    let rt = Runtime::new(
        RuntimeConfig {
            platform: "test".to_owned(),
            mode: "inproc".to_owned(),
            core_threads: 1,
            blocking_threads: 1,
            log_level: 0,
        },
        host,
    )
    .unwrap();
    let payload = undra_runtime::testing::call_payload(
        CallTarget::Function { method_id: DOUBLE },
        1,
        &4_u32.encode_to_vec(),
    );
    let reply = undra_runtime::testing::decode_reply(&rt.call_sync(&payload));
    assert_eq!(reply.status, ReplyStatus::Ok);
    assert_eq!(u32::decode_exact(&reply.body).unwrap(), 8);
    rt.shutdown();
}
