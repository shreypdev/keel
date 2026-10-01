//! An init hook or a dispatch layer submitted more than once under one name counts once
//! (ADR-052). Every `#[undra::query]` and `#[undra::mutation]` submits `undra-query`'s hook and
//! layer, so that a core without queries does not link them; a core with three queries submits
//! them three times and must still hydrate once and ask the layer once per call.

use std::any::Any;
use std::sync::atomic::{AtomicU32, Ordering};

use undra_meta::{DispatchCall, DispatchOutcome, ids};
use undra_runtime::testing::TestRuntime;
use undra_runtime::{DispatchLayer, DispatchResult, InitHook, inventory};
use undra_wire::Encode;
use undra_wire::payload::{CallTarget, ReplyStatus};

static HYDRATIONS: AtomicU32 = AtomicU32::new(0);
static OTHER_HOOK: AtomicU32 = AtomicU32::new(0);
static ASKED: AtomicU32 = AtomicU32::new(0);

/// The function id only the counting layer looks at.
const COUNTED: u32 = ids::function_id("once_per_name_counted");
/// A function id the counting layer serves.
const SERVED: u32 = ids::function_id("once_per_name_served");

fn hydrate(_: &undra_runtime::Ctx) {
    HYDRATIONS.fetch_add(1, Ordering::SeqCst);
}

const HOOK: InitHook = InitHook {
    name: "test.once-per-name",
    run: hydrate,
};

// Three definitions, three submissions of the same hook (as the macros emit them).
inventory::submit! { HOOK }
inventory::submit! { HOOK }
inventory::submit! { HOOK }
// A different name is a different hook.
inventory::submit! {
    InitHook {
        name: "test.once-per-name.other",
        run: |_| {
            OTHER_HOOK.fetch_add(1, Ordering::SeqCst);
        },
    }
}

fn counting(_: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
    if call.method_id == COUNTED {
        ASKED.fetch_add(1, Ordering::SeqCst);
    }
    if call.method_id == SERVED {
        return DispatchOutcome::new(DispatchResult::Sync(Ok(7_u32.encode_to_vec())));
    }
    DispatchOutcome::new(DispatchResult::Unknown)
}

const LAYER: DispatchLayer = DispatchLayer {
    name: "test.once-per-name.layer",
    dispatch: counting,
};

inventory::submit! { LAYER }
inventory::submit! { LAYER }
inventory::submit! { LAYER }

#[test]
fn a_hook_submitted_three_times_runs_once_per_runtime() {
    let before = HYDRATIONS.load(Ordering::SeqCst);
    let other_before = OTHER_HOOK.load(Ordering::SeqCst);
    let t = TestRuntime::new();
    t.run_init_hooks();
    assert_eq!(HYDRATIONS.load(Ordering::SeqCst) - before, 1);
    assert_eq!(
        OTHER_HOOK.load(Ordering::SeqCst) - other_before,
        1,
        "another name still runs"
    );
}

#[test]
fn a_layer_submitted_three_times_is_asked_once_per_call() {
    let t = TestRuntime::new();
    let before = ASKED.load(Ordering::SeqCst);
    let reply = t.call_sync(CallTarget::Function { method_id: COUNTED }, 1, &[]);
    // No layer serves it: the miss is reported as before.
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    assert_eq!(ASKED.load(Ordering::SeqCst) - before, 1);

    // And the layer still serves what it serves.
    let reply = t.call_sync(CallTarget::Function { method_id: SERVED }, 2, &[]);
    assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    assert_eq!(reply.body, 7_u32.encode_to_vec());
}
