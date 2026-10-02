//! Handle generations (ADR-022): one monotonically increasing counter, carried across snapshots.
//! Regression tests for review finding H1 (a restore re-issued generations, so a stale handle
//! aliased a new object) and its edges (T8: the last generation; counter exhaustion, L8).

mod common;

use common::*;
use undra_runtime::testing::TestRuntime;
use undra_runtime::{Handle, RestoreError, Runtime, RuntimeConfig};
use undra_wire::payload::{ReplyStatus, Snapshot};
use undra_wire::{Reader, Writer};

fn floor_of(snapshot: &[u8]) -> u64 {
    Snapshot::decode(&mut Reader::new(snapshot))
        .expect("a snapshot")
        .generation_floor
}

/// A snapshot of nothing, carrying only a generation floor.
fn empty_snapshot_with_floor(generation_floor: u64) -> Vec<u8> {
    let mut w = Writer::new();
    Snapshot {
        generation_floor,
        ..Snapshot::default()
    }
    .encode(&mut w);
    w.into_vec()
}

fn status(t: &TestRuntime, handle: Handle, call_id: u32) -> ReplyStatus {
    call_counter(t, handle, GET, call_id, &[]).status
}

/// T1 of the review: a slot's generation was rolled back by the restore, so the next release
/// re-issued one the host still held.
#[test]
fn h1_a_stale_handle_never_aliases_a_new_object_after_a_restore() {
    let t = TestRuntime::new();
    let rt = t.runtime().clone();
    let a = new_counter(&t, 1, "a"); // (0, 1)
    let snapshot = rt.snapshot();
    rt.release(a.0);
    let b = new_counter(&t, 2, "b"); // the host keeps a wrapper for b
    assert_eq!(b.index(), a.index());
    assert!(b.generation() > a.generation());

    rt.restore(&snapshot).unwrap(); // a is back at its handle; b is stale, as designed
    assert_eq!(status(&t, b, 10), ReplyStatus::BadRequest);
    assert_eq!(status(&t, a, 11), ReplyStatus::Ok);

    // Release the restored object and create another one in the same slot. Before the fix the
    // slot's generation was one past the *snapshot's*, which is exactly b's.
    rt.release(a.0);
    let c = new_counter(&t, 3, "c");
    assert_eq!(c.index(), b.index(), "the slot is reused");
    assert_ne!(c, b, "the stale handle must not name the new object");
    assert!(c.generation() > b.generation());

    // The host finalising b's wrapper releases nothing, and c is untouched.
    assert_eq!(status(&t, b, 12), ReplyStatus::BadRequest);
    rt.release(b.0);
    assert_eq!(decode_body::<i32>(&call_counter(&t, c, GET, 13, &[])), 3);
}

/// Restoring into a fresh runtime (a crash): the old process may have cycled slots past the
/// snapshot's generations, and the new one must not start below what a host could still hold.
#[test]
fn h1_a_fresh_runtime_resumes_above_the_snapshot_floor() {
    let old = TestRuntime::new();
    // Churn slot 0 well past the generation of the store that ends up in the snapshot.
    for round in 0..6 {
        let h = new_counter(&old, round, "churn");
        old.runtime().release(h.0);
    }
    let kept = new_counter(&old, 42, "kept"); // (0, 7)
    let snapshot = old.runtime().snapshot();
    let floor = floor_of(&snapshot);
    assert!(floor >= kept.generation());
    // Two more handles are issued and dropped after the snapshot; the host may hold them.
    let late1 = new_counter(&old, 1, "late");
    let late2 = new_counter(&old, 2, "late");
    assert!(late1.generation() > floor && late2.generation() > floor);

    let fresh = TestRuntime::new();
    fresh.runtime().restore(&snapshot).unwrap();
    assert_eq!(
        decode_body::<i32>(&call_counter(&fresh, kept, GET, 1, &[])),
        42
    );
    // The restored store's slot is 0; objects created now never get a generation at or below
    // the floor, so nothing the old core issued before the snapshot can name them.
    let newer = new_counter(&fresh, 9, "new");
    assert!(newer.generation() > floor, "{newer:?} vs floor {floor}");
    // A handle the old core issued after the snapshot can still name an object the fresh one
    // would create only if generations were reused from the floor; they start above it, so
    // the first new one is exactly floor + 1 and the next floor + 2.
    assert_eq!(newer.generation(), floor + 1);
    // And releasing the restored store does not hand its slot a generation back.
    fresh.runtime().release(kept.0);
    let after = new_counter(&fresh, 10, "after");
    assert!(after.generation() > newer.generation());
    assert_ne!(after, kept);
}

/// Every restore leaves the counter where it was or higher: a floor below the current value
/// changes nothing (the same process restoring an older snapshot).
#[test]
fn h1_a_floor_below_the_current_counter_is_ignored() {
    let t = TestRuntime::new();
    let first = new_counter(&t, 1, "a");
    let old = t.runtime().snapshot();
    for n in 0..10 {
        let h = new_counter(&t, n, "x");
        t.runtime().release(h.0);
    }
    let before = t.runtime().snapshot();
    assert!(floor_of(&before) > floor_of(&old));
    t.runtime().restore(&old).unwrap();
    assert!(
        floor_of(&t.runtime().snapshot()) >= floor_of(&before),
        "restoring an older snapshot must not lower the counter"
    );
    let next = new_counter(&t, 2, "b");
    assert!(next.generation() > floor_of(&before));
    assert_eq!(status(&t, first, 5), ReplyStatus::Ok);
}

/// T8: a snapshot whose handle carries the last generation used to saturate the floor, so the
/// first release wrapped the slot back to generation 1.
#[test]
fn h1_a_snapshot_with_generation_max_is_refused_instead_of_wrapping() {
    let t = TestRuntime::new();
    let a = new_counter(&t, 1, "a"); // (0, 1)
    let mut decoded = Snapshot::decode(&mut Reader::new(&t.runtime().snapshot())).unwrap();
    decoded.stores[0].handle = Handle::new(0, Handle::MAX_GENERATION);
    let mut w = Writer::new();
    decoded.encode(&mut w);
    let snapshot = w.into_vec();
    assert_eq!(
        t.runtime().restore(&snapshot),
        Err(RestoreError::BadHandle {
            handle: Handle::new(0, Handle::MAX_GENERATION).0
        })
    );
    // Refused means untouched: the original store is still there under its handle.
    assert_eq!(status(&t, a, 2), ReplyStatus::Ok);
}

#[test]
fn h1_a_floor_that_leaves_nothing_to_issue_is_refused() {
    let t = TestRuntime::new();
    const MAX: u64 = Handle::MAX_GENERATION;
    assert_eq!(
        t.runtime().restore(&empty_snapshot_with_floor(MAX)),
        Err(RestoreError::GenerationFloor { floor: MAX })
    );
    // Anything at or above the ceiling is refused too (re-review NF1): the counter is shared
    // by the whole process and crash recovery replays the same snapshot every launch. A floor
    // the 40-bit field cannot even hold is refused as well.
    const CEILING: u64 = MAX - (1 << 36);
    for floor in [u64::MAX, MAX + 1, MAX - 1, CEILING + 1, CEILING] {
        assert_eq!(
            t.runtime().restore(&empty_snapshot_with_floor(floor)),
            Err(RestoreError::GenerationFloor { floor })
        );
    }
    // Just under the ceiling is accepted, with 2^36 generations of headroom left.
    t.runtime()
        .restore(&empty_snapshot_with_floor(CEILING - 1))
        .unwrap();
}

/// L8: the counter never wraps. When its 2^40 - 1 values are spent the runtime refuses to
/// create objects (status 2, a FATAL record) and everything that exists keeps working.
#[test]
fn l8_an_exhausted_generation_counter_refuses_inserts_and_keeps_serving() {
    let t = TestRuntime::new();
    let survivor = new_counter(&t, 7, "survivor");
    // Spend all but one generation. A restore refuses floors near the ceiling (NF1), so the
    // test raises this runtime's private counter directly.
    t.raise_generation_floor(Handle::MAX_GENERATION - 1);

    let last = new_counter(&t, 1, "last"); // takes the last generation
    assert_eq!(last.generation(), Handle::MAX_GENERATION);
    t.host().take_logs();

    let reply = t.call_sync(
        undra_wire::payload::CallTarget::Constructor {
            type_id: undra_meta::ids::type_id("Counter"),
            method_id: NEW,
        },
        99,
        &args(|w| {
            use undra_wire::Encode;
            2_i32.encode(w);
            "refused".encode(w);
        }),
    );
    assert_eq!(reply.status, ReplyStatus::Panic, "{reply:?}");
    let logs = t.host().take_logs();
    assert!(
        logs.iter()
            .any(|l| l.level == 5 && l.message.contains("generations")),
        "a FATAL record names the exhaustion: {logs:?}"
    );

    // The runtime is intact: existing handles still resolve, nothing was leaked into the table.
    assert_eq!(
        decode_body::<i32>(&call_counter(&t, survivor, GET, 100, &[])),
        7
    );
    assert_eq!(
        decode_body::<i32>(&call_counter(&t, last, GET, 101, &[])),
        1
    );
    assert_eq!(t.runtime().objects().live(), 2);
    // Releasing frees a slot but not a generation: still refused.
    t.runtime().release(last.0);
    let again = t.call_sync(
        undra_wire::payload::CallTarget::Constructor {
            type_id: undra_meta::ids::type_id("Counter"),
            method_id: NEW,
        },
        102,
        &args(|w| {
            use undra_wire::Encode;
            3_i32.encode(w);
            "refused".encode(w);
        }),
    );
    assert_eq!(again.status, ReplyStatus::Panic);
}

/// Real runtimes share one counter for the whole process: a handle of a runtime that was shut
/// down cannot name an object of the next one (the host's finalisers run late).
#[test]
fn h1_generations_are_unique_across_runtimes_of_one_process() {
    let make = || {
        Runtime::new(
            RuntimeConfig {
                core_threads: 0,
                ..RuntimeConfig::default()
            },
            std::sync::Arc::new(undra_runtime::testing::RecordingHost::new()),
        )
        .unwrap()
    };
    let first = make();
    let h1 = new_counter_rt(&first, 1, "one");
    first.shutdown();

    let second = make();
    let h2 = new_counter_rt(&second, 2, "two");
    assert_eq!(h1.index(), h2.index());
    assert!(h2.generation() > h1.generation(), "{h1:?} then {h2:?}");
    assert_eq!(
        call_sync_rt(&second, counter_target(h1, GET), 5, &[]).status,
        ReplyStatus::BadRequest,
        "the old runtime's handle is stale here"
    );
    assert_eq!(
        decode_body::<i32>(&call_sync_rt(&second, counter_target(h2, GET), 6, &[])),
        2
    );
    second.shutdown();
}

/// Re-review NF1: a handle generation at or above the ceiling in a snapshot's stores is a
/// `BadHandle`, not just the floor field.
#[test]
fn nf1_a_store_generation_near_the_ceiling_is_a_bad_handle() {
    let t = TestRuntime::new();
    let h = new_counter(&t, 1, "one");
    let snapshot = t.runtime().snapshot();
    let mut decoded = Snapshot::decode(&mut Reader::new(&snapshot)).unwrap();
    let index = decoded.stores[0].handle.index();
    let bad = Handle::new(index, Handle::MAX_GENERATION - 1);
    decoded.stores[0].handle = bad;
    let mut w = Writer::new();
    decoded.encode(&mut w);
    assert_eq!(
        t.runtime().restore(w.as_slice()),
        Err(RestoreError::BadHandle { handle: bad.0 })
    );
    let _ = h;
}
