//! Objects that are built again after a restore (ADR-059), at the runtime's level: a test reviver
//! over a layer object stands in for what `undra-query` does for a query handle, so every rule of
//! the restore is pinned without a query.
//!
//! The object under test, a `RecWidget`, is a transient store with one signal. A record says what it
//! was made of (`n`); its `bump` method changes its value, which a record does not carry, so
//! "rebuilt from the record" and "left alone" are told apart by the value it shows.

mod common;

use std::any::Any;
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use undra_meta::{DispatchCall, DispatchOutcome, ids};
use undra_runtime::testing::{ReplyRecord, TestRuntime};
use undra_runtime::{
    AnyObject, DispatchLayer, DispatchResult, Handle, RestoreError, RestoreReport, Reviver,
    Runtime, UndraObjectDyn, inventory,
};
use undra_signals::{Signal, StoreCell};
use undra_wire::payload::{CallTarget, ChangeSet, ReplyStatus, Snapshot, StoreSnapshot};
use undra_wire::{Decode, Encode, Reader, Writer};

const WIDGET: u32 = ids::type_id("RecWidget");
const READ: u32 = ids::method_id("RecWidget", "read");
const BUMP: u32 = ids::method_id("RecWidget", "bump");
/// An asynchronous method that never finishes (a call in flight on a widget).
const HOLD: u32 = ids::method_id("RecWidget", "hold");
/// A stream method (`0..n`).
const TICKS: u32 = ids::method_id("RecWidget", "ticks");
/// A type no reviver claims.
const GHOST: u32 = ids::type_id("RecGhost");

/// The widgets whose record is this `n` misbehave.
const CHECK_PANICS: u32 = 1_000;
const REVIVE_FAILS: u32 = 2_000;
const REVIVE_PANICS: u32 = 3_000;

const ALL: u32 = u32::MAX;

/// Per runtime: how often a widget was built by the reviver, and the fingerprint it reports.
struct Counts {
    builds: AtomicUsize,
    fingerprint: AtomicU64,
}

impl Default for Counts {
    fn default() -> Counts {
        Counts {
            builds: AtomicUsize::new(0),
            fingerprint: AtomicU64::new(0xF1),
        }
    }
}

fn record_of(n: u32) -> Vec<u8> {
    let mut record = vec![1_u8];
    record.extend_from_slice(&n.to_le_bytes());
    record
}

fn n_of(record: &[u8]) -> Result<u32, String> {
    match record {
        [1, a, b, c, d] => Ok(u32::from_le_bytes([*a, *b, *c, *d])),
        other => Err(format!(
            "the record does not decode ({} bytes)",
            other.len()
        )),
    }
}

struct Widget {
    cell: Arc<StoreCell>,
    value: Signal<u32>,
    made_from: u32,
    /// Called inside `recreation()`: lets a test hold a snapshot there.
    gate: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Widget {
    fn new(n: u32) -> Arc<Widget> {
        let cell = StoreCell::new(WIDGET);
        let value = Signal::new(n);
        cell.attach(&value, 0).unwrap();
        Arc::new(Widget {
            cell,
            value,
            made_from: n,
            gate: Mutex::new(None),
        })
    }
}

struct WidgetObject(Arc<Widget>);

impl UndraObjectDyn for WidgetObject {
    fn undra_type_id(&self) -> u32 {
        WIDGET
    }

    fn undra_type_name(&self) -> &'static str {
        "RecWidget"
    }

    fn as_store(&self) -> Option<&Arc<StoreCell>> {
        Some(&self.0.cell)
    }

    fn transient(&self) -> bool {
        true
    }

    fn recreation(&self) -> Option<Vec<u8>> {
        let gate = self.0.gate.lock().unwrap().clone();
        if let Some(gate) = gate {
            gate();
        }
        Some(record_of(self.0.made_from))
    }
}

impl AnyObject for WidgetObject {
    fn shared(&self) -> Arc<dyn Any + Send + Sync> {
        self.0.clone()
    }
}

const REVIVER: Reviver = Reviver {
    name: "rec-widgets",
    object_name: "RecWidget",
    fingerprint: |rt, type_id| {
        (type_id == WIDGET).then(|| rt.extension::<Counts>().fingerprint.load(Ordering::Relaxed))
    },
    check: |_, _, record| {
        let n = n_of(record)?;
        assert!(n != CHECK_PANICS, "check kaboom");
        Ok(())
    },
    revive: |rt, _, record| {
        let n = n_of(record)?;
        rt.extension::<Counts>()
            .builds
            .fetch_add(1, Ordering::Relaxed);
        assert!(n != REVIVE_PANICS, "revive kaboom");
        if n == REVIVE_FAILS {
            return Err("the widget cannot be built".to_owned());
        }
        Ok(Arc::new(WidgetObject(Widget::new(n))))
    },
};

fn dispatch(rt: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
    let Some(rt) = rt.downcast_ref::<Runtime>() else {
        return DispatchOutcome::new(DispatchResult::Unknown);
    };
    DispatchOutcome::new(serve(rt, call))
}

fn serve(rt: &Runtime, call: DispatchCall<'_>) -> DispatchResult {
    if call.handle == 0 && call.method_id == WIDGET {
        let Ok(n) = u32::decode_exact(call.args) else {
            return DispatchResult::BadRequest("bad arguments".to_owned());
        };
        let handle = rt.insert(Arc::new(WidgetObject(Widget::new(n))));
        return DispatchResult::Sync(Ok(handle.encode_to_vec()));
    }
    if call.handle == 0 {
        return DispatchResult::Unknown;
    }
    let widget = match rt.object::<Widget>(call.handle) {
        Ok(widget) => widget,
        Err(e) => return DispatchResult::BadRequest(e.to_string()),
    };
    match call.method_id {
        READ => DispatchResult::Sync(Ok(widget.value.get().encode_to_vec())),
        BUMP => {
            widget.value.update(|v| *v += 1);
            DispatchResult::Sync(Ok(widget.value.get().encode_to_vec()))
        }
        HOLD => DispatchResult::Async(Box::pin(async move {
            let _keep = widget;
            std::future::pending::<()>().await;
            Ok(Vec::new())
        })),
        TICKS => DispatchResult::Stream(Box::pin(Ticks(0))),
        _ => DispatchResult::Unknown,
    }
}

/// `0, 1, 2` and then the end.
struct Ticks(u32);

impl futures_core::Stream for Ticks {
    type Item = undra_runtime::DispatchBytes;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        if self.0 >= 3 {
            return std::task::Poll::Ready(None);
        }
        let item = self.0.encode_to_vec();
        self.0 += 1;
        std::task::Poll::Ready(Some(Ok(item)))
    }
}

inventory::submit! {
    DispatchLayer { name: "rec-layer", dispatch }
}

// ----- the rig ------------------------------------------------------------------------------

/// A runtime with the test reviver, as a runtime whose client crate added one.
fn runtime() -> TestRuntime {
    let t = TestRuntime::new();
    t.runtime().add_reviver(REVIVER);
    t
}

fn builds(t: &TestRuntime) -> usize {
    t.runtime()
        .extension::<Counts>()
        .builds
        .load(Ordering::Relaxed)
}

fn widget(t: &TestRuntime, n: u32) -> Handle {
    let reply = t.call_sync(
        CallTarget::Constructor {
            type_id: WIDGET,
            method_id: WIDGET,
        },
        1,
        &n.encode_to_vec(),
    );
    assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    Handle::decode_exact(&reply.body).unwrap()
}

fn call(t: &TestRuntime, handle: Handle, method_id: u32) -> ReplyRecord {
    t.call_sync(CallTarget::Method { handle, method_id }, 7, &[])
}

fn read(t: &TestRuntime, handle: Handle) -> u32 {
    let reply = call(t, handle, READ);
    assert_eq!(reply.status, ReplyStatus::Ok, "read: {reply:?}");
    u32::decode_exact(&reply.body).unwrap()
}

fn bump(t: &TestRuntime, handle: Handle) {
    assert_eq!(call(t, handle, BUMP).status, ReplyStatus::Ok);
}

fn observe(t: &TestRuntime, handle: Handle) -> Vec<ChangeSet> {
    t.host().take_decoded_change_sets();
    t.runtime().observe(handle.0, ALL, true);
    t.host().take_decoded_change_sets()
}

fn stat(t: &TestRuntime, key: &str) -> u64 {
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    stats[key]
        .as_u64()
        .unwrap_or_else(|| panic!("{key}: {stats}"))
}

fn logs_at(t: &TestRuntime, level: u8) -> Vec<String> {
    t.host()
        .take_logs()
        .into_iter()
        .filter(|record| record.level == level)
        .map(|record| record.message)
        .collect()
}

/// The records of a snapshot that are recreation records, as `(handle, n)`.
fn records_of(snapshot: &[u8]) -> Vec<(Handle, u32)> {
    let decoded = Snapshot::decode(&mut Reader::new(snapshot)).unwrap();
    decoded
        .stores
        .iter()
        .filter_map(|s| Some((s.handle, n_of(s.recreation()?).unwrap())))
        .collect()
}

fn restore(t: &TestRuntime, snapshot: &[u8]) -> RestoreReport {
    t.runtime().restore_with_report(snapshot).unwrap()
}

/// One table entry as a test sees it: the handle, its type id and name, the host references.
type TableRow = (u64, Option<(u32, &'static str)>, Option<u32>);

/// Every table entry's identity, for comparing two states: handle, type, references.
fn table_of(t: &TestRuntime, handles: &[Handle]) -> Vec<TableRow> {
    handles
        .iter()
        .map(|h| {
            (
                h.0,
                t.runtime().objects().type_of(*h).ok(),
                t.runtime().objects().host_refs_of(*h),
            )
        })
        .collect()
}

// ----- a fresh runtime (a dev reload, a web crash restart, a cold start) -----------------------

#[test]
fn a_fresh_runtime_re_issues_the_handle_and_builds_the_object_when_it_is_first_called() {
    let old = runtime();
    let handle = widget(&old, 5);
    bump(&old, handle); // 6 now; the record says what it was made of: 5
    let snapshot = old.runtime().snapshot();
    assert_eq!(records_of(&snapshot), [(handle, 5)]);

    let new = runtime();
    let report = restore(&new, &snapshot);
    assert_eq!((report.restored, report.reissued), (0, 1), "{report:?}");
    assert!(report.refused.is_empty() && report.displaced.is_empty() && report.dropped.is_empty());
    // Re-issued, not built: the handle is live and routes, nothing ran, no port was called.
    assert_eq!(builds(&new), 0);
    assert_eq!(stat(&new, "dormant_handles"), 1);
    assert_eq!(stat(&new, "live_handles"), 1);
    assert_eq!(stat(&new, "host_refs"), 1, "the host owns the reference");
    assert_eq!(
        new.runtime().objects().type_of(handle).unwrap(),
        (WIDGET, "RecWidget"),
        "a dormant handle reports the record's type, as its object will"
    );

    // The first call builds it, in the same entry, from the record.
    assert_eq!(read(&new, handle), 5);
    assert_eq!(builds(&new), 1);
    assert_eq!(stat(&new, "dormant_handles"), 0);
    assert_eq!(
        stat(&new, "live_handles"),
        1,
        "the same entry: nothing was added"
    );
    assert_eq!(new.runtime().objects().host_refs_of(handle), Some(1));
    // Once built it is an ordinary object: no second build, and it keeps its own state.
    bump(&new, handle);
    assert_eq!(read(&new, handle), 6);
    assert_eq!(builds(&new), 1);
}

#[test]
fn observing_a_dormant_handle_builds_it_and_answers_with_its_values() {
    let old = runtime();
    let handle = widget(&old, 5);
    let snapshot = old.runtime().snapshot();

    let new = runtime();
    restore(&new, &snapshot);
    assert!(new.host().take_decoded_change_sets().is_empty());
    let sets = observe(&new, handle);
    assert_eq!(builds(&new), 1);
    assert_eq!(sets.len(), 1, "{sets:?}");
    assert_eq!(sets[0].entries[0].handle, handle);
    assert_eq!(sets[0].entries[0].value, 5_u32.encode_to_vec());
    assert_eq!(stat(&new, "dormant_handles"), 0);
    // It is observed now: a change reaches the host.
    bump(&new, handle);
    let sets = new.host().take_decoded_change_sets();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].entries[0].value, 6_u32.encode_to_vec());
}

#[test]
fn a_host_that_stops_observing_a_handle_it_never_observed_builds_nothing() {
    let old = runtime();
    let handle = widget(&old, 5);
    let snapshot = old.runtime().snapshot();
    let new = runtime();
    restore(&new, &snapshot);
    new.runtime().observe(handle.0, ALL, false);
    assert_eq!(builds(&new), 0);
    assert_eq!(stat(&new, "dormant_handles"), 1);
    assert!(
        logs_at(&new, 3).is_empty(),
        "and says nothing: it is not a mistake"
    );
}

#[test]
fn releasing_a_dormant_handle_forgets_its_record_without_building_it() {
    let old = runtime();
    let handle = widget(&old, 5);
    let snapshot = old.runtime().snapshot();
    let new = runtime();
    restore(&new, &snapshot);
    new.runtime().release(handle.0);
    assert_eq!(builds(&new), 0);
    assert_eq!(
        (stat(&new, "dormant_handles"), stat(&new, "live_handles")),
        (0, 0)
    );
    assert_eq!(call(&new, handle, READ).status, ReplyStatus::BadRequest);
    // And the next snapshot has no record of it.
    assert!(records_of(&new.runtime().snapshot()).is_empty());
}

#[test]
fn a_dormant_handle_is_carried_into_the_next_snapshot() {
    let old = runtime();
    let handle = widget(&old, 5);
    let first = old.runtime().snapshot();
    let new = runtime();
    restore(&new, &first);
    // A second reload before the client came back: the record goes on, unchanged.
    let second = new.runtime().snapshot();
    assert_eq!(records_of(&second), [(handle, 5)]);
    let third = runtime();
    assert_eq!(restore(&third, &second).reissued, 1);
    assert_eq!(read(&third, handle), 5);
    // A snapshot round trip through a fresh runtime is the identity.
    assert_eq!(second, first);
}

#[test]
fn every_entry_point_that_resolves_a_handle_builds_a_dormant_one_and_nothing_else_does() {
    let old = runtime();
    let handles: Vec<Handle> = (0..6).map(|n| widget(&old, n)).collect();
    let snapshot = old.runtime().snapshot();
    let new = runtime();
    restore(&new, &snapshot);
    assert_eq!(stat(&new, "dormant_handles"), 6);

    // `call_sync` and `call` (a sync method served on the core).
    assert_eq!(read(&new, handles[0]), 0);
    let id = new.call(
        CallTarget::Method {
            handle: handles[1],
            method_id: READ,
        },
        21,
        &[],
    );
    assert_eq!(id, 0);
    let replies = new.take_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].status, ReplyStatus::Ok);
    assert_eq!(u32::decode_exact(&replies[0].body).unwrap(), 1);
    // A stream (it opens, then flows as the host grants credit).
    new.call(
        CallTarget::Method {
            handle: handles[2],
            method_id: TICKS,
        },
        22,
        &[],
    );
    new.runtime().stream_credit(22, 10);
    new.run_pending();
    assert_eq!(
        new.host().take_stream_items().len(),
        4,
        "three items and the end"
    );
    // An asynchronous call.
    new.call(
        CallTarget::Method {
            handle: handles[3],
            method_id: HOLD,
        },
        23,
        &[],
    );
    new.run_pending();
    assert_eq!(builds(&new), 4);
    // `observe` builds; `observe(.., false)` and `release` do not.
    new.runtime().observe(handles[4].0, ALL, false);
    assert_eq!(builds(&new), 4);
    new.runtime().observe(handles[4].0, ALL, true);
    assert_eq!(builds(&new), 5);
    new.runtime().release(handles[5].0);
    assert_eq!(builds(&new), 5);
    assert_eq!(
        stat(&new, "dormant_handles"),
        0,
        "five built, one forgotten"
    );
    new.runtime().cancel(23);
}

#[test]
fn a_lazy_page_call_on_a_dormant_handle_is_answered_as_the_wrong_kind_of_handle() {
    let old = runtime();
    let handle = widget(&old, 5);
    let snapshot = old.runtime().snapshot();
    let new = runtime();
    restore(&new, &snapshot);
    // A core that serves lazy lists (this one inserts one) answers the page call on the handle: it
    // is not a page server. The handle is the host's own, so naming it builds it.
    let list = new.call_sync(
        CallTarget::Function {
            method_id: MAKE_LAZY,
        },
        1,
        &3_u32.encode_to_vec(),
    );
    assert_eq!(list.status, ReplyStatus::Ok);
    let reply = new.call_sync(
        CallTarget::LazyPage {
            handle,
            offset: 0,
            limit: 2,
        },
        2,
        &[],
    );
    assert_eq!(reply.status, ReplyStatus::BadRequest, "{reply:?}");
    assert!(
        reason_of(&reply).contains("RecWidget"),
        "{}",
        reason_of(&reply)
    );
    assert_eq!(read(&new, handle), 5);
}

// ----- the same runtime (a time travel, an app's own `restore`) --------------------------------

#[test]
fn a_restore_into_the_runtime_that_holds_a_handle_leaves_it_exactly_as_it_is() {
    let t = runtime();
    let before = t.runtime().snapshot(); // from before the handle existed
    let handle = widget(&t, 5);
    bump(&t, handle);
    bump(&t, handle); // 7: its own state, which a record does not carry
    let counter = new_counter(&t, 1, "c");
    observe(&t, handle);
    let with = t.runtime().snapshot();
    assert_eq!(records_of(&with), [(handle, 5)]);
    call_counter(&t, counter, ADD, 1, &3_i32.encode_to_vec());
    let live = stat(&t, "live_handles");
    let refs = stat(&t, "host_refs");

    for (round, snapshot) in [&with, &with, &before].into_iter().enumerate() {
        t.host().take_decoded_change_sets();
        let report = restore(&t, snapshot);
        assert_eq!(report.refused.len(), 0, "round {round}");
        assert_eq!(
            report.reissued, 1,
            "round {round}: the live handle is counted"
        );
        assert_eq!(builds(&t), 0, "round {round}: nothing was built");
        assert_eq!(stat(&t, "dormant_handles"), 0);
        assert_eq!(
            stat(&t, "host_refs"),
            if round < 2 { refs } else { refs - 1 }
        );
        assert_eq!(
            read(&t, handle),
            7,
            "round {round}: its own state, not the record's"
        );
        // The stores went to the snapshot's values (the counter), the widget's mirror is
        // current: nothing was delivered for it.
        let delivered = t.host().take_decoded_change_sets();
        assert!(
            delivered
                .iter()
                .flat_map(|cs| cs.entries.iter())
                .all(|e| e.handle != handle),
            "round {round}: {delivered:?}"
        );
        let _ = live;
    }
    // Observation is intact: a change reaches the host.
    t.host().take_decoded_change_sets();
    bump(&t, handle);
    let sets = t.host().take_decoded_change_sets();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].entries[0].value, 8_u32.encode_to_vec());
}

#[test]
fn restoring_twice_is_restoring_once() {
    let t = runtime();
    let handle = widget(&t, 5);
    let counter = new_counter(&t, 1, "c");
    observe(&t, handle);
    let snapshot = t.runtime().snapshot();
    call_counter(&t, counter, ADD, 1, &3_i32.encode_to_vec());
    t.runtime().release(handle.0); // released since: dormant after the restore

    let summary = |t: &TestRuntime| {
        (
            table_of(t, &[handle, counter]),
            t.runtime().snapshot(),
            stat(t, "live_handles"),
            stat(t, "dormant_handles"),
        )
    };
    restore(&t, &snapshot);
    let once = summary(&t);
    restore(&t, &snapshot);
    assert_eq!(summary(&t), once);
}

#[test]
fn a_handle_released_since_the_snapshot_is_re_issued_dormant_and_never_does_anything() {
    let t = runtime();
    let handle = widget(&t, 5);
    let snapshot = t.runtime().snapshot();
    t.runtime().release(handle.0);
    assert_eq!(stat(&t, "live_handles"), 0);

    let report = restore(&t, &snapshot);
    assert_eq!(report.reissued, 1);
    assert_eq!((stat(&t, "dormant_handles"), builds(&t)), (1, 0));
    t.run_pending();
    assert_eq!(builds(&t), 0, "nothing builds it: nobody uses it");
    // The host that released it never calls it again; if one does, it is built then.
    assert_eq!(read(&t, handle), 5);
}

#[test]
fn a_store_that_needs_the_slot_of_a_live_re_creatable_object_displaces_it() {
    let t = runtime();
    let counter = new_counter(&t, 1, "c"); // slot 0
    let snapshot = t.runtime().snapshot();
    t.runtime().release(counter.0);
    let handle = widget(&t, 5); // the freed slot is reused: same index, a later generation
    assert_eq!(handle.index(), counter.index());
    assert!(handle.generation() > counter.generation());

    let report = restore(&t, &snapshot);
    // The snapshot's store wins its slot; the widget there is removed like any other object.
    assert_eq!(report.restored, 1);
    assert_eq!(report.displaced, [handle.0], "{report:?}");
    assert_eq!(report.reissued, 0);
    assert_eq!(call(&t, handle, READ).status, ReplyStatus::BadRequest);
    assert_eq!(
        decode_body::<i32>(&call_counter(&t, counter, GET, 3, &[])),
        1,
        "and the store is back at its handle"
    );
}

#[test]
fn a_record_whose_slot_was_reused_by_a_kept_handle_is_skipped() {
    let t = runtime();
    let first = widget(&t, 5);
    let snapshot = t.runtime().snapshot();
    t.runtime().release(first.0);
    let second = widget(&t, 6); // the same slot, a later generation
    assert_eq!(second.index(), first.index());

    let report = restore(&t, &snapshot);
    // The live one is kept; the record names a handle that was released, and its slot is taken.
    assert_eq!(report.reissued, 1, "{report:?}");
    assert!(report.refused.is_empty());
    assert_eq!(read(&t, second), 6);
    assert_eq!(call(&t, first, READ).status, ReplyStatus::BadRequest);
    assert_eq!(stat(&t, "dormant_handles"), 0);
    let debug = logs_at(&t, 1);
    assert!(
        debug.iter().any(|m| m.contains("its slot was reused")),
        "{debug:?}"
    );
}

#[test]
fn the_generation_counter_rises_over_the_records_of_a_snapshot_too() {
    let old = runtime();
    for n in 0..5 {
        let h = widget(&old, n);
        old.runtime().release(h.0);
    }
    let handle = widget(&old, 9);
    let snapshot = old.runtime().snapshot();
    let floor = Snapshot::decode(&mut Reader::new(&snapshot))
        .unwrap()
        .generation_floor;
    assert!(floor >= handle.generation());

    let new = runtime();
    restore(&new, &snapshot);
    let fresh = widget(&new, 1);
    assert!(
        fresh.generation() > floor,
        "{fresh:?} would collide with a handle the old core issued (floor {floor})"
    );
}

#[test]
fn a_call_in_flight_on_a_kept_object_is_not_cancelled_by_a_restore() {
    let t = runtime();
    let handle = widget(&t, 5);
    let counter = new_counter(&t, 1, "c");
    t.call(
        CallTarget::Method {
            handle,
            method_id: HOLD,
        },
        31,
        &[],
    );
    t.call(
        CallTarget::Method {
            handle: counter,
            method_id: FOREVER,
        },
        32,
        &[],
    );
    t.run_pending();
    assert_eq!(t.take_replies().len(), 0);
    let snapshot = t.runtime().snapshot();

    restore(&t, &snapshot);
    let replies = t.take_replies();
    // The call on the store, which the restore rebuilt, was cancelled; the one on the widget
    // carries on.
    assert_eq!(
        replies
            .iter()
            .map(|r| (r.call_id, r.status))
            .collect::<Vec<_>>(),
        [(32, ReplyStatus::Cancelled)],
        "{replies:?}"
    );
    t.runtime().cancel(31);
    assert_eq!(t.take_replies().len(), 1);
}

// ----- what cannot be honoured ----------------------------------------------------------------------

/// Re-encodes `snapshot` with extra recreation records (and the types they name).
fn with_records(snapshot: &[u8], extra: &[(Handle, u32, Vec<u8>, Option<u64>)]) -> Vec<u8> {
    let mut decoded = Snapshot::decode(&mut Reader::new(snapshot)).unwrap();
    for (handle, type_id, record, fingerprint) in extra {
        if let Some(fingerprint) = fingerprint {
            if decoded.fingerprint(*type_id).is_none() {
                decoded.types.push(undra_wire::payload::SnapshotType {
                    type_id: *type_id,
                    fingerprint: *fingerprint,
                });
            }
        }
        decoded.stores.push(StoreSnapshot {
            handle: *handle,
            type_id: *type_id,
            signals: vec![(undra_wire::payload::RECREATION_FIELD, record.clone())],
        });
        decoded.generation_floor = decoded.generation_floor.max(handle.generation());
    }
    let mut w = Writer::new();
    decoded.encode(&mut w);
    w.into_vec()
}

#[test]
fn a_record_this_build_cannot_honour_is_refused_and_counted_and_everything_else_restores() {
    let old = runtime();
    let good = widget(&old, 5);
    let counter = new_counter(&old, 4, "c");
    let base = old.runtime().snapshot();
    let g = good.generation();
    let unknown = Handle::new(9, g + 1);
    let garbled = Handle::new(10, g + 2);
    let panics = Handle::new(11, g + 3);
    let snapshot = with_records(
        &base,
        &[
            (unknown, GHOST, record_of(1), Some(7)),
            (garbled, WIDGET, vec![9, 9], None),
            (panics, WIDGET, record_of(CHECK_PANICS), None),
        ],
    );

    let new = runtime();
    let report = restore(&new, &snapshot);
    assert_eq!((report.restored, report.reissued), (1, 1), "{report:?}");
    let refused: Vec<(u64, u32)> = report
        .refused
        .iter()
        .map(|r| (r.handle, r.type_id))
        .collect();
    assert_eq!(
        refused,
        [(unknown.0, GHOST), (garbled.0, WIDGET), (panics.0, WIDGET)],
        "{:?}",
        report.refused
    );
    assert!(
        report.refused[0].reason.contains("nothing that builds it"),
        "{:?}",
        report.refused[0]
    );
    assert!(
        report.refused[1].reason.contains("does not decode"),
        "{:?}",
        report.refused[1]
    );
    assert!(
        report.refused[2].reason.contains("check kaboom"),
        "{:?}",
        report.refused[2]
    );
    for refused in [unknown, garbled, panics] {
        assert_eq!(
            call(&new, refused, READ).status,
            ReplyStatus::BadRequest,
            "{refused:?} is stale, as every object a restore does not carry"
        );
    }
    // The store came back, and the good record is a dormant handle.
    assert_eq!(
        decode_body::<i32>(&call_counter(&new, counter, GET, 1, &[])),
        4
    );
    assert_eq!(read(&new, good), 5);
    // One WARN per refusal, naming the handle (the panic of a check is also a reported panic).
    let warns = logs_at(&new, 3);
    for handle in [unknown, garbled, panics] {
        assert!(
            warns
                .iter()
                .any(|m| m.contains(&format!("{handle:?}")) && m.contains("is not re-issued")),
            "{handle:?}: {warns:?}"
        );
    }
    assert_eq!(stat(&new, "panics"), 1);
}

#[test]
fn a_record_whose_types_changed_is_refused() {
    let old = runtime();
    let handle = widget(&old, 5);
    let snapshot = old.runtime().snapshot();
    // The new build's fingerprint of the widget's record is another one.
    let new = runtime();
    new.runtime()
        .extension::<Counts>()
        .fingerprint
        .store(0xF2, Ordering::Relaxed);
    let report = restore(&new, &snapshot);
    assert_eq!(
        (report.reissued, report.refused.len()),
        (0, 1),
        "{report:?}"
    );
    assert!(
        report.refused[0].reason.contains("changed"),
        "{:?}",
        report.refused
    );
    assert_eq!(report.refused[0].handle, handle.0);
    assert_eq!(call(&new, handle, READ).status, ReplyStatus::BadRequest);
    assert_eq!(builds(&new), 0);
}

#[test]
fn a_runtime_without_a_reviver_takes_a_record_for_a_store_type_it_does_not_have() {
    let old = runtime();
    let handle = widget(&old, 5);
    let counter = new_counter(&old, 4, "c");
    let snapshot = old.runtime().snapshot();
    assert_eq!(records_of(&snapshot).len(), 1);

    let bare = TestRuntime::new(); // no `add_reviver`: what a core without queries is
    let report = restore(&bare, &snapshot);
    assert_eq!(report.restored, 1);
    assert_eq!((report.reissued, report.refused.len()), (0, 0));
    assert_eq!(report.dropped.len(), 1);
    assert_eq!(report.dropped[0].type_id, WIDGET);
    assert_eq!(report.dropped[0].handles, [handle.0]);
    assert!(logs_at(&bare, 3).iter().any(|m| m.contains("left out")));
    assert_eq!(call(&bare, handle, READ).status, ReplyStatus::BadRequest);
    assert_eq!(
        decode_body::<i32>(&call_counter(&bare, counter, GET, 1, &[])),
        4
    );
    assert_eq!(stat(&bare, "dormant_handles"), 0);
}

#[test]
fn a_runtime_without_a_reviver_writes_no_record_for_a_re_creatable_object() {
    let bare = TestRuntime::new();
    let handle = widget(&bare, 5);
    let snapshot = bare.runtime().snapshot();
    assert_eq!(
        Reader::new(&snapshot).read_u32().unwrap(),
        0,
        "no store, no record"
    );
    assert_eq!(read(&bare, handle), 5, "the object itself works");
}

#[test]
fn a_forged_record_fails_the_restore_like_a_forged_store_and_changes_nothing() {
    let old = runtime();
    let handle = widget(&old, 5);
    let counter = new_counter(&old, 4, "c");
    let base = old.runtime().snapshot();
    let at_the_ceiling = Handle::new(20, Handle::MAX_GENERATION - 100);
    let cases: [(&str, Vec<u8>); 3] = [
        (
            "a duplicate handle",
            with_records(&base, &[(handle, WIDGET, record_of(1), None)]),
        ),
        (
            "a generation at the ceiling",
            with_records(&base, &[(at_the_ceiling, WIDGET, record_of(1), None)]),
        ),
        (
            "the null handle",
            with_records(&base, &[(Handle::NULL, WIDGET, record_of(1), None)]),
        ),
    ];
    let new = runtime();
    restore(&new, &base);
    for (what, forged) in cases {
        let live = stat(&new, "live_handles");
        let error = new.runtime().restore_with_report(&forged).unwrap_err();
        assert!(
            matches!(
                error,
                RestoreError::BadHandle { .. } | RestoreError::GenerationFloor { .. }
            ),
            "{what}: {error:?}"
        );
        assert_eq!(
            stat(&new, "live_handles"),
            live,
            "{what}: the runtime is unchanged"
        );
        assert_eq!(
            decode_body::<i32>(&call_counter(&new, counter, GET, 1, &[])),
            4
        );
    }
}

// ----- an object that cannot be built when it is first used ----------------------------------------

#[test]
fn an_object_that_cannot_be_built_makes_its_handle_stale_and_everything_else_goes_on() {
    let old = runtime();
    let failing = widget(&old, REVIVE_FAILS);
    let panicking = widget(&old, REVIVE_PANICS);
    let fine = widget(&old, 8);
    let counter = new_counter(&old, 4, "c");
    let snapshot = old.runtime().snapshot();

    let new = runtime();
    assert_eq!(restore(&new, &snapshot).reissued, 3);
    // The use that triggered the build is answered as for any stale handle.
    assert_eq!(call(&new, failing, READ).status, ReplyStatus::BadRequest);
    assert_eq!(builds(&new), 1);
    let errors = logs_at(&new, 4);
    assert!(
        errors.iter().any(|m| m.contains(&format!("{failing:?}"))
            && m.contains("rec-widgets")
            && m.contains("the widget cannot be built")),
        "{errors:?}"
    );
    assert_eq!(stat(&new, "revive_failed"), 1);
    // ... also when the observe triggered it (it logs, it does not panic).
    new.runtime().observe(panicking.0, ALL, true);
    assert_eq!((stat(&new, "revive_failed"), stat(&new, "panics")), (2, 1));
    assert!(new.host().take_decoded_change_sets().is_empty());
    // The failed handles are gone for good (not dormant, not live), nothing else was touched.
    assert_eq!(stat(&new, "dormant_handles"), 1);
    assert_eq!(stat(&new, "live_handles"), 2);
    assert_eq!(call(&new, failing, READ).status, ReplyStatus::BadRequest);
    assert_eq!(builds(&new), 2, "it is not tried again");
    assert_eq!(read(&new, fine), 8);
    assert_eq!(
        decode_body::<i32>(&call_counter(&new, counter, GET, 1, &[])),
        4
    );
}

// ----- no lock of the table is held while an object is asked for its record ----------------------

/// Makes the widget's `recreation()` wait for the test; the table must stay usable meanwhile (a
/// query handle's takes a lock of its own, which the thread holding it may be about to need the
/// table for). Returns what says it was entered, and what lets it go.
fn hold_recreation(
    t: &TestRuntime,
    handle: Handle,
) -> (std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>) {
    let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let go_rx = Mutex::new(go_rx);
    let entered_tx = Mutex::new(entered_tx);
    let gate: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        let _ = entered_tx.lock().unwrap().send(());
        let _ = go_rx.lock().unwrap().recv_timeout(Duration::from_secs(60));
    });
    let widget = t.runtime().object::<Widget>(handle.0).unwrap();
    *widget.gate.lock().unwrap() = Some(gate);
    (entered_rx, go_tx)
}

/// Lets `recreation()` through again.
fn release_recreation(t: &TestRuntime, handle: Handle) {
    let widget = t.runtime().object::<Widget>(handle.0).unwrap();
    *widget.gate.lock().unwrap() = None;
}

#[test]
fn a_snapshot_asks_for_the_records_with_no_lock_of_the_table_held() {
    let t = Arc::new(runtime());
    let handle = widget(&t, 5);
    let (entered, go) = hold_recreation(&t, handle);
    let snapshotter = {
        let t = t.clone();
        std::thread::spawn(move || t.runtime().snapshot())
    };
    entered
        .recv_timeout(Duration::from_secs(60))
        .expect("the snapshot reached the widget's recreation");
    // While it waits there, the table takes writes (a release from another thread, an insert).
    let other = with_timeout(
        "an insert while a snapshot waits in recreation()",
        Duration::from_secs(60),
        {
            let t = t.clone();
            move || {
                t.runtime()
                    .objects()
                    .insert(undra_runtime::plain(Arc::new(Marker)))
            }
        },
    );
    assert!(!other.is_null());
    go.send(()).unwrap();
    let snapshot = snapshotter.join().unwrap();
    release_recreation(&t, handle);
    assert_eq!(records_of(&snapshot), [(handle, 5)]);
}

struct Marker;

impl undra_runtime::UndraObject for Marker {
    const TYPE_ID: u32 = 0x7a11;
    const NAME: &'static str = "Marker";
}

#[test]
fn a_restore_asks_for_the_records_with_no_lock_of_the_table_held() {
    let t = Arc::new(runtime());
    let handle = widget(&t, 5);
    let snapshot = t.runtime().snapshot();
    let (entered, go) = hold_recreation(&t, handle);
    let restorer = {
        let t = t.clone();
        std::thread::spawn(move || t.runtime().restore_with_report(&snapshot))
    };
    entered
        .recv_timeout(Duration::from_secs(60))
        .expect("the restore reached the widget's recreation");
    let other = with_timeout(
        "an insert while a restore waits in recreation()",
        Duration::from_secs(60),
        {
            let t = t.clone();
            move || {
                t.runtime()
                    .objects()
                    .insert(undra_runtime::plain(Arc::new(Marker)))
            }
        },
    );
    assert!(!other.is_null());
    go.send(()).unwrap();
    let report = restorer.join().unwrap().unwrap();
    release_recreation(&t, handle);
    assert_eq!(report.reissued, 1);
}

// ----- properties ------------------------------------------------------------------------------------

mod properties {
    use super::*;
    use proptest::prelude::*;

    #[derive(Clone, Debug)]
    enum Op {
        Widget(u8),
        Counter,
        Observe(usize),
        Use(usize),
        Release(usize),
        Snapshot,
        /// A restore into the runtime that holds the handles (a time travel).
        Restore(usize),
        /// A restore into a fresh runtime (a dev reload): the host keeps every handle it holds.
        Reload(usize),
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            3 => (0_u8..6).prop_map(Op::Widget),
            2 => Just(Op::Counter),
            3 => (0_usize..16).prop_map(Op::Observe),
            2 => (0_usize..16).prop_map(Op::Use),
            2 => (0_usize..16).prop_map(Op::Release),
            3 => Just(Op::Snapshot),
            3 => (0_usize..8).prop_map(Op::Restore),
            2 => (0_usize..8).prop_map(Op::Reload),
        ]
    }

    /// The table, the next snapshot, the statistics and the builds.
    type State = (Vec<TableRow>, Vec<u8>, [u64; 4], usize);

    /// Everything a test can see of a runtime after a restore: the table, the observations (what a
    /// change does or does not deliver), the statistics and the next snapshot.
    fn state(t: &TestRuntime, handles: &[Handle]) -> State {
        (
            table_of(t, handles),
            t.runtime().snapshot(),
            [
                stat(t, "live_handles"),
                stat(t, "host_refs"),
                stat(t, "dormant_handles"),
                stat(t, "live_stores"),
            ],
            builds(t),
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 96, .. ProptestConfig::default() })]

        /// For any history of constructing, observing, using, releasing, snapshotting and restoring
        /// (into the same runtime, or into a fresh one), `restore(s); restore(s)` leaves what
        /// `restore(s)` left: the same table, references, observations, statistics and next
        /// snapshot, and a restore builds nothing.
        #[test]
        fn restoring_a_snapshot_twice_is_restoring_it_once(ops in proptest::collection::vec(op(), 1..40)) {
            let mut t = runtime();
            let mut handles: Vec<Handle> = Vec::new();
            let mut snapshots: Vec<Vec<u8>> = Vec::new();
            let pick = |handles: &[Handle], i: usize| handles.get(i % handles.len().max(1)).copied();
            for op in ops {
                match op {
                    Op::Widget(n) => handles.push(widget(&t, u32::from(n))),
                    Op::Counter => handles.push(new_counter(&t, 1, "c")),
                    Op::Observe(i) => {
                        if let Some(h) = pick(&handles, i) {
                            t.runtime().observe(h.0, ALL, true);
                        }
                    }
                    Op::Use(i) => {
                        if let Some(h) = pick(&handles, i) {
                            let _ = call(&t, h, READ);
                        }
                    }
                    Op::Release(i) => {
                        if let Some(h) = pick(&handles, i) {
                            t.runtime().release(h.0);
                        }
                    }
                    Op::Snapshot => snapshots.push(t.runtime().snapshot()),
                    Op::Restore(i) => {
                        if let Some(s) = snapshots.get(i % snapshots.len().max(1)) {
                            let _ = t.runtime().restore_with_report(s);
                        }
                    }
                    Op::Reload(i) => {
                        if let Some(s) = snapshots.get(i % snapshots.len().max(1)) {
                            let fresh = runtime();
                            if fresh.runtime().restore_with_report(s).is_ok() {
                                t = fresh;
                            }
                        }
                    }
                }
            }
            let s = t.runtime().snapshot();
            let built = builds(&t);
            t.host().take_decoded_change_sets();
            let first = t.runtime().restore_with_report(&s).unwrap();
            let once = state(&t, &handles);
            prop_assert_eq!(once.3, built, "a restore builds nothing");
            let delivered_once = t.host().take_decoded_change_sets();
            let second = t.runtime().restore_with_report(&s).unwrap();
            let twice = state(&t, &handles);
            prop_assert_eq!(&once, &twice);
            prop_assert_eq!(first.reissued, second.reissued);
            prop_assert_eq!(&first.refused, &second.refused);
            // Re-observed stores are delivered again, a handle is not: whatever the second restore
            // delivers, the first delivered too (nothing new appears for a live handle).
            let handles_of = |sets: &[ChangeSet]| -> BTreeSet<u64> {
                sets.iter().flat_map(|cs| cs.entries.iter()).map(|e| e.handle.0).collect()
            };
            let delivered_twice = t.host().take_decoded_change_sets();
            prop_assert!(handles_of(&delivered_twice).is_subset(&handles_of(&delivered_once)));
        }

        /// A snapshot written by a fresh runtime that restored `s` is `s` again: the records go
        /// on through any number of reloads that nobody used the handles in.
        #[test]
        fn a_reload_nobody_used_the_handles_in_writes_the_same_snapshot(ops in proptest::collection::vec(op(), 1..30)) {
            let t = runtime();
            let mut handles: Vec<Handle> = Vec::new();
            for op in ops {
                match op {
                    Op::Widget(n) => handles.push(widget(&t, u32::from(n))),
                    Op::Counter => handles.push(new_counter(&t, 1, "c")),
                    Op::Observe(i) => {
                        if let Some(h) = handles.get(i % handles.len().max(1)) {
                            t.runtime().observe(h.0, ALL, true);
                        }
                    }
                    Op::Release(i) => {
                        if let Some(h) = handles.get(i % handles.len().max(1)) {
                            t.runtime().release(h.0);
                        }
                    }
                    _ => {}
                }
            }
            let s = t.runtime().snapshot();
            let fresh = runtime();
            fresh.runtime().restore(&s).unwrap();
            prop_assert_eq!(fresh.runtime().snapshot(), s.clone());
            let again = runtime();
            again.runtime().restore(&fresh.runtime().snapshot()).unwrap();
            prop_assert_eq!(again.runtime().snapshot(), s);
        }
    }
}
