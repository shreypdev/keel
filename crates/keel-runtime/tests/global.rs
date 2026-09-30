//! The process-global runtime (`Runtime::init` / `Runtime::global`), init hooks and runtime
//! extensions. One test function: the global slot is process-wide state.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use common::*;
use keel_meta::ids::ALL_SIGNALS;
use keel_runtime::testing::{RecordingHost, TestRuntime};
use keel_runtime::{InitError, InitHook, Runtime, RuntimeConfig};

static HOOK_RUNS: AtomicU32 = AtomicU32::new(0);
static HOOK_SAW_CTX: AtomicU32 = AtomicU32::new(0);

keel_meta::inventory::submit! {
    InitHook {
        name: "test.count-runs",
        run: |ctx| {
            HOOK_RUNS.fetch_add(1, Ordering::SeqCst);
            if keel_runtime::Ctx::try_current().is_some() && ctx.runtime().id() != 0 {
                HOOK_SAW_CTX.fetch_add(1, Ordering::SeqCst);
            }
            // Hooks may spawn (query hydration is async).
            ctx.spawn(async {});
        },
    }
}

#[derive(Default)]
struct Cache {
    hits: AtomicU32,
}

fn config() -> RuntimeConfig {
    RuntimeConfig {
        platform: "test".into(),
        log_level: 0,
        core_threads: 0,
        ..RuntimeConfig::default()
    }
}

#[test]
fn the_global_runtime_lifecycle() {
    assert!(Runtime::global().is_none());

    // `TestRuntime` leaves hooks to the test; `Runtime::new` and `init` run them.
    let t = TestRuntime::new();
    assert_eq!(HOOK_RUNS.load(Ordering::SeqCst), 0);
    t.run_init_hooks();
    assert_eq!(HOOK_RUNS.load(Ordering::SeqCst), 1);
    assert_eq!(t.run_pending(), 1, "the hook's spawned task ran");
    drop(t);

    let host = Arc::new(RecordingHost::new());
    let rt = Runtime::init(config(), host.clone()).unwrap();
    assert_eq!(HOOK_RUNS.load(Ordering::SeqCst), 2, "init runs the hooks");
    assert_eq!(
        HOOK_SAW_CTX.load(Ordering::SeqCst),
        2,
        "with the runtime current"
    );
    let global = Runtime::global().expect("init registered the runtime");
    assert!(Arc::ptr_eq(&global, &rt));

    // A second init is refused while the first is alive.
    let err = Runtime::init(config(), Arc::new(RecordingHost::new())).err();
    assert_eq!(err, Some(InitError::AlreadyInitialized));
    assert_eq!(
        HOOK_RUNS.load(Ordering::SeqCst),
        2,
        "the refused init ran nothing"
    );
    // Runtime::new is not global and coexists.
    let other = Runtime::new(config(), Arc::new(RecordingHost::new())).unwrap();
    assert_eq!(
        HOOK_RUNS.load(Ordering::SeqCst),
        3,
        "Runtime::new runs the hooks too"
    );
    assert!(!Arc::ptr_eq(&Runtime::global().unwrap(), &other));
    other.shutdown();
    assert!(
        Runtime::global().is_some(),
        "shutting down another runtime leaves the global alone"
    );

    // A signal written on a thread that is inside no runtime reaches the global runtime, where
    // the write-context check is not in the way (release builds; debug builds refuse such a
    // write, ADR-023, which the second half of this block checks).
    let h = new_counter_rt(&rt, 1, "");
    rt.observe(h.0, ALL_SIGNALS, true);
    host.take_change_sets();
    let counter = rt.object::<Counter>(h.0).unwrap();
    let unscoped = counter.clone();
    std::thread::spawn(move || keel_runtime::testing::unchecked_writes(|| unscoped.count.set(2)))
        .join()
        .unwrap();
    assert_eq!(
        host.take_decoded_change_sets().len(),
        1,
        "routed to the global runtime's host"
    );
    if cfg!(debug_assertions) {
        let refused = std::thread::spawn(move || counter.count.set(3)).join();
        assert!(refused.is_err(), "debug builds refuse a write off the core");
        assert_eq!(host.take_decoded_change_sets().len(), 0);
    }

    // So do log records emitted outside any call.
    host.take_logs();
    keel_runtime::keel_info!("outside a call");
    assert_eq!(host.take_logs()[0].message, "outside a call");

    // Extensions: one value per type, shared, alive as long as the runtime.
    let a: &Cache = rt.extension();
    a.hits.fetch_add(1, Ordering::SeqCst);
    let ctx = rt.ctx();
    let b: &Cache = ctx.runtime().extension();
    assert_eq!(b.hits.load(Ordering::SeqCst), 1);
    assert!(std::ptr::eq(a, b));
    let with: &String = rt.extension_with(|| "made once".to_owned());
    assert_eq!(rt.extension_with(|| "ignored".to_owned()), with);

    // Shutdown releases the slot, and init works again.
    rt.shutdown();
    assert!(Runtime::global().is_none());
    let again = Runtime::init(config(), Arc::new(RecordingHost::new())).unwrap();
    assert!(Arc::ptr_eq(&Runtime::global().unwrap(), &again));
    assert_eq!(HOOK_RUNS.load(Ordering::SeqCst), 4);
    again.shutdown();
    assert!(Runtime::global().is_none());
    // The global slot holds a strong reference: dropping your own `Arc` does not stop the
    // runtime; `shutdown` does.
    let kept = Runtime::init(config(), Arc::new(RecordingHost::new())).unwrap();
    let id = kept.id();
    drop(kept);
    assert_eq!(Runtime::global().map(|r| r.id()), Some(id));
    Runtime::global().unwrap().shutdown();
    assert!(Runtime::global().is_none());
}
