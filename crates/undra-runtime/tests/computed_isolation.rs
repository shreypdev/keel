//! The ADR-019 amendment through the runtime (gap PC-1): a computed that panics on its current
//! inputs poisons only itself. The audit's probe showed the opposite: every write to the store
//! failed with status 2 and no change-set was delivered until the computed recovered.

use std::any::Any;
use std::sync::Arc;

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
}

impl Gauge {
    fn new(level: i32) -> Gauge {
        let cell = StoreCell::new(<Gauge as UndraObject>::TYPE_ID);
        let level = Signal::new(level);
        let label = Signal::new(String::from("gauge"));
        let ratio = Computed::new(&level, |level: &i32| 100 / level);
        cell.attach(&level, LEVEL).unwrap();
        cell.attach(&label, LABEL).unwrap();
        cell.attach_computed(&ratio, RATIO).unwrap();
        Gauge {
            cell,
            level,
            label,
            ratio,
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
