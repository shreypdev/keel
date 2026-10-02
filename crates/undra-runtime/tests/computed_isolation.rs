//! The ADR-019 amendment through the runtime (gap PC-1): a computed that panics on its current
//! inputs poisons only itself. The audit's probe showed the opposite: every write to the store
//! failed with status 2 and no change-set was delivered until the computed recovered.

use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use undra_meta::{DispatchCall, DispatchOutcome, ObjectMeta, Registration, ids};
use undra_runtime::testing::{ReplyRecord, TestRuntime};
use undra_runtime::{Ctx, DispatchResult, Runtime, StoreObject, UndraObject};
use undra_signals::{ALL_SIGNALS, Computed, Signal, StoreCell};
use undra_wire::payload::{CallTarget, ReplyStatus};
use undra_wire::{Decode, Encode, Handle, Reader};

/// A store whose computed `ratio = 100 / level` panics while `level` is 0.
struct Gauge {
    cell: Arc<StoreCell>,
    level: Signal<i32>,
    label: Signal<String>,
    ratio: Computed<i32>,
    /// How often `ratio`'s closure ran.
    evaluations: Arc<AtomicUsize>,
}

impl Gauge {
    fn new(level: i32) -> Gauge {
        let cell = StoreCell::new(<Gauge as UndraObject>::TYPE_ID);
        let level = Signal::new(level);
        let label = Signal::new(String::from("gauge"));
        let evaluations = Arc::new(AtomicUsize::new(0));
        let counted = evaluations.clone();
        let ratio = Computed::new(&level, move |level: &i32| {
            counted.fetch_add(1, Ordering::SeqCst);
            100 / level
        });
        cell.attach(&level, LEVEL).unwrap();
        cell.attach(&label, LABEL).unwrap();
        cell.attach_computed(&ratio, RATIO).unwrap();
        Gauge {
            cell,
            level,
            label,
            ratio,
            evaluations,
        }
    }
}

const LEVEL: u32 = 0;
const LABEL: u32 = 1;
const RATIO: u32 = 2;

impl UndraObject for Gauge {
    const TYPE_ID: u32 = ids::type_id("Gauge");
    const NAME: &'static str = "Gauge";
}

impl StoreObject for Gauge {
    fn cell(&self) -> &Arc<StoreCell> {
        &self.cell
    }

    fn restore(_: Ctx, _: &mut Reader<'_>) -> Result<Self, undra_wire::WireError> {
        Ok(Gauge::new(1))
    }
}

const SET_LEVEL: u32 = ids::method_id("Gauge", "set_level");
const SET_LABEL: u32 = ids::method_id("Gauge", "set_label");
const RATIO_NOW: u32 = ids::method_id("Gauge", "ratio_now");

fn gauge_dispatch(rt: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
    let Some(rt) = rt.downcast_ref::<Runtime>() else {
        return DispatchOutcome::new(DispatchResult::Unknown);
    };
    let gauge = match rt.object::<Gauge>(call.handle) {
        Ok(gauge) => gauge,
        Err(e) => return DispatchOutcome::new(DispatchResult::BadRequest(e.to_string())),
    };
    let mut args = Reader::new(call.args);
    DispatchOutcome::new(match call.method_id {
        SET_LEVEL => {
            gauge.level.set(i32::decode(&mut args).unwrap());
            DispatchResult::Sync(Ok(Vec::new()))
        }
        SET_LABEL => {
            gauge.label.set(String::decode(&mut args).unwrap());
            DispatchResult::Sync(Ok(Vec::new()))
        }
        // Reads the computed: panics (status 2) while it cannot be evaluated.
        RATIO_NOW => DispatchResult::Sync(Ok(gauge.ratio.get().encode_to_vec())),
        _ => DispatchResult::Unknown,
    })
}

static GAUGE_META: ObjectMeta = ObjectMeta {
    name: "Gauge",
    type_id: ids::type_id("Gauge"),
    constructors: &[],
    methods: &[],
    store: None,
    docs: "",
    dispatch: gauge_dispatch,
};
undra_meta::inventory::submit! { Registration::Object(&GAUGE_META) }

fn call(t: &TestRuntime, handle: Handle, method_id: u32, args: &[u8]) -> ReplyRecord {
    t.call_sync(CallTarget::Method { handle, method_id }, 1, args)
}

fn stat(t: &TestRuntime, key: &str) -> u64 {
    let stats: serde_json::Value = serde_json::from_str(&t.runtime().stats_json()).unwrap();
    stats[key].as_u64().unwrap()
}

/// The ids of the entries of every change-set delivered since the last call.
fn delivered(t: &TestRuntime) -> Vec<Vec<u32>> {
    t.host()
        .take_decoded_change_sets()
        .iter()
        .map(|set| set.entries.iter().map(|e| e.signal_id).collect())
        .collect()
}

#[test]
fn pc1_a_panicking_computed_poisons_only_itself() {
    let t = TestRuntime::new();
    let handle = t.runtime().insert_store(Arc::new(Gauge::new(1)));
    t.runtime().observe(handle.0, ALL_SIGNALS, true);
    assert_eq!(delivered(&t), vec![vec![LEVEL, LABEL, RATIO]]);
    t.host().take_logs();

    // The write that makes the computed panic succeeds; its own slot is delivered.
    let reply = call(&t, handle, SET_LEVEL, &0_i32.encode_to_vec());
    assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    assert_eq!(delivered(&t), vec![vec![LEVEL]]);
    let errors: Vec<_> = t
        .host()
        .take_logs()
        .into_iter()
        .filter(|l| l.level == undra_runtime::log::ERROR)
        .collect();
    assert_eq!(errors.len(), 1, "reported once: {errors:?}");
    assert!(
        errors[0]
            .message
            .starts_with("computed signal 2 of `Gauge`")
            && errors[0].message.contains("divide by zero"),
        "{errors:?}"
    );
    assert_eq!(stat(&t, "poisoned_signals"), 1);
    assert_eq!(stat(&t, "poisoned_stores"), 1);
    let gauge = t.runtime().object::<Gauge>(handle.0).unwrap();
    assert_eq!(
        gauge
            .cell()
            .failed_signals()
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>(),
        vec![RATIO],
        "the signal's typed poisoned state"
    );

    // The rest of the store keeps working, and the failed computed is not retried per write.
    for label in ["a", "b", "c"] {
        let reply = call(&t, handle, SET_LABEL, &label.to_owned().encode_to_vec());
        assert_eq!(reply.status, ReplyStatus::Ok);
    }
    assert_eq!(delivered(&t), vec![vec![LABEL]; 3]);
    assert!(
        t.host()
            .take_logs()
            .iter()
            .all(|l| l.level < undra_runtime::log::ERROR),
        "no new report"
    );

    // A call that reads it gets the ordinary panicked outcome (status 2) every platform maps.
    let reply = call(&t, handle, RATIO_NOW, &[]);
    assert_eq!(reply.status, ReplyStatus::Panic);

    // A change of its inputs evaluates it again: recovered, delivered in full, cleared.
    let reply = call(&t, handle, SET_LEVEL, &4_i32.encode_to_vec());
    assert_eq!(reply.status, ReplyStatus::Ok);
    let sets = t.host().take_decoded_change_sets();
    assert_eq!(sets.len(), 1);
    let ratio = sets[0]
        .entries
        .iter()
        .find(|e| e.signal_id == RATIO)
        .expect("the recovered computed is delivered");
    assert_eq!(i32::decode_exact(&ratio.value).unwrap(), 25);
    assert_eq!(stat(&t, "poisoned_signals"), 0);
    assert!(gauge.cell().failed_signals().is_empty());
    let reply = call(&t, handle, RATIO_NOW, &[]);
    assert_eq!(i32::decode_exact(&reply.body).unwrap(), 25);
}

#[test]
fn pc1_observing_a_store_whose_computed_fails_delivers_everything_else() {
    let t = TestRuntime::new();
    let handle = t.runtime().insert_store(Arc::new(Gauge::new(0)));
    t.runtime().observe(handle.0, ALL_SIGNALS, true);
    assert_eq!(
        delivered(&t),
        vec![vec![LEVEL, LABEL]],
        "the store is observed; only the failing computed is missing"
    );
    assert_eq!(stat(&t, "poisoned_signals"), 1);
    call(&t, handle, SET_LEVEL, &5_i32.encode_to_vec());
    assert_eq!(delivered(&t), vec![vec![LEVEL, RATIO]]);
}

/// Review (runtime-lifecycle, surface 4): the amendment says a failed computed "is evaluated again
/// when one of its inputs changes". The graph has no equality check, so writing the input the value
/// it already has (a no-op write) is a change: the computed is evaluated again, still fails, stays
/// held back, and is **not** reported a second time; the input itself is delivered.
#[test]
fn a_no_op_write_to_a_failed_computeds_input_evaluates_it_again_without_a_second_report() {
    let t = TestRuntime::new();
    let handle = t.runtime().insert_store(Arc::new(Gauge::new(1)));
    t.runtime().observe(handle.0, ALL_SIGNALS, true);
    let gauge = t.runtime().object::<Gauge>(handle.0).unwrap();
    call(&t, handle, SET_LEVEL, &0_i32.encode_to_vec());
    delivered(&t);
    let errors = |t: &TestRuntime| {
        t.host()
            .take_logs()
            .into_iter()
            .filter(|l| l.level >= undra_runtime::log::ERROR)
            .count()
    };
    assert_eq!(errors(&t), 1, "the failure is reported once");
    let evaluations = gauge.evaluations.load(Ordering::SeqCst);

    // The same value again.
    let reply = call(&t, handle, SET_LEVEL, &0_i32.encode_to_vec());
    assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    assert_eq!(
        delivered(&t),
        vec![vec![LEVEL]],
        "the input is delivered, the computed is not"
    );
    assert_eq!(
        gauge.evaluations.load(Ordering::SeqCst),
        evaluations + 1,
        "a write is a change for the graph: evaluated once more"
    );
    assert!(gauge.cell().is_failed(RATIO));
    assert_eq!(errors(&t), 0, "no second report for the same failure");
    assert_eq!(stat(&t, "poisoned_signals"), 1);

    // An unrelated write does not evaluate it at all.
    call(&t, handle, SET_LABEL, &"x".to_owned().encode_to_vec());
    assert_eq!(delivered(&t), vec![vec![LABEL]]);
    assert_eq!(gauge.evaluations.load(Ordering::SeqCst), evaluations + 1);
}

#[test]
fn a_panicking_computed_reaches_the_diagnostics_port_once_with_where_it_panicked() {
    // ADR-046 (prod-ops review): the computed's panic is caught by `undra-signals`, not by a runtime
    // guard, so its report is the one the panic hook recorded on this thread (`caught_elsewhere`):
    // with the panic's location, named after the store and the signal.
    let t = TestRuntime::new();
    t.runtime()
        .bind_foreign_port(undra_runtime::DIAGNOSTICS_PORT);
    let handle = t.runtime().insert_store(Arc::new(Gauge::new(1)));
    t.runtime().observe(handle.0, ALL_SIGNALS, true);
    let reply = call(&t, handle, SET_LEVEL, &0_i32.encode_to_vec());
    assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    let reports: Vec<_> = t
        .host()
        .take_port_calls()
        .into_iter()
        .filter(|c| c.port_id == undra_runtime::DIAGNOSTICS_PORT)
        .collect();
    assert_eq!(reports.len(), 1, "{reports:?}");
    assert_eq!(reports[0].port_call_id, 0, "fire and forget");
    let mut r = Reader::new(&reports[0].args);
    let message = r.read_str().unwrap();
    let location = r.read_str().unwrap();
    let operation = r.read_str().unwrap();
    assert!(message.contains("divide by zero"), "{message}");
    assert!(
        location.contains("computed_isolation.rs:"),
        "the hook's location: {location:?}"
    );
    assert!(operation.starts_with("computed Gauge."), "{operation}");
    assert_eq!(stat(&t, "panic_reports"), 1);
}
