//! Panic reports (ADR-046 decision 4), end to end in a `TestRuntime`: a call, an async call, a
//! detached task and a computed that panic each reach the `Diagnostics` port exactly once, as the
//! record `undra_ports::PanicReport` declares (the runtime encodes it by hand, so this decodes it
//! with the type), and the caller still gets status 2.

use undra_meta::ids;
use undra_ports::fakes;
use undra_ports::{Diagnostics, PanicReport};
use undra_runtime::CoreIdentity;
use undra_runtime::testing::TestRuntime;
use undra_wire::payload::{CallTarget, ReplyStatus};
use undra_wire::{Decode, Encode, Writer};

mod root {
    pub use undra_meta as meta;
    pub use undra_runtime as runtime;
    pub use undra_wire as wire;
}

undra_meta::inventory::submit! {
    CoreIdentity { namespace: "test_core", version: "9.8.7" }
}

/// A function that panics, to have a call to contain.
#[undra_macros::api]
#[undra(crate = "crate::root")]
pub fn explode(message: String) -> u32 {
    panic!("{message}")
}

/// An async function that panics.
#[undra_macros::api]
#[undra(crate = "crate::root")]
pub async fn explode_later(message: String) -> u32 {
    panic!("{message}")
}

fn function(name: &str) -> CallTarget {
    CallTarget::Function {
        method_id: ids::function_id(name),
    }
}

fn string_args(text: &str) -> Vec<u8> {
    let mut w = Writer::new();
    w.write_str(text);
    w.into_vec()
}

fn rig() -> (TestRuntime, fakes::Fakes) {
    let t = TestRuntime::new();
    let fakes = fakes::install(&t);
    (t, fakes)
}

#[test]
fn a_panicking_call_answers_status_2_and_reports_once() {
    let (t, fakes) = rig();
    let reply = t.call_sync(function("explode"), 1, &string_args("kaboom"));
    assert_eq!(reply.status, ReplyStatus::Panic, "{reply:?}");
    let reports = fakes.diagnostics.take();
    assert_eq!(reports.len(), 1, "{reports:#?}");
    let report = &reports[0];
    assert_eq!(report.message, "kaboom");
    assert_eq!(report.operation, "explode");
    assert!(
        report.location.contains("diagnostics.rs:"),
        "{}",
        report.location
    );
    assert!(!report.thread.is_empty());
    assert_eq!(
        (report.namespace.as_str(), report.core_version.as_str()),
        ("test_core", "9.8.7")
    );
    assert_eq!(report.schema_hash, t.runtime().schema_hash());
    // Frames: a debug build names them from the backtrace text; a release one has no frame source
    // in a test runtime (undra-ffi installs one), so no addresses.
    if cfg!(debug_assertions) {
        assert!(
            report.frames.iter().any(|f| f.symbol.is_some()),
            "{report:#?}"
        );
    }
    let stats = t.runtime().stats_json();
    assert!(stats.contains("\"panic_reports\":1"), "{stats}");
    assert!(stats.contains("\"panics\":1"), "{stats}");
}

#[test]
fn a_panicking_async_call_and_a_detached_task_report_too() {
    let (t, fakes) = rig();
    t.call(function("explode_later"), 2, &string_args("later"));
    t.run_pending();
    let reply = t.take_replies().pop().expect("the call was answered");
    assert_eq!(reply.status, ReplyStatus::Panic);
    t.ctx().spawn(async { panic!("detached") });
    t.run_pending();
    let reports = fakes.diagnostics.take();
    let seen: Vec<(&str, &str)> = reports
        .iter()
        .map(|r| (r.message.as_str(), r.operation.as_str()))
        .collect();
    assert_eq!(seen, [("later", "explode_later"), ("detached", "task")]);
}

#[test]
fn the_report_is_the_record_the_port_declares() {
    // What the runtime hands the port is the wire form of `PanicReport`: decoding it with the
    // type and encoding it again gives the same bytes (a layout drift between the runtime's
    // hand-written encoder and the record fails here).
    let t = TestRuntime::new();
    let fakes = fakes::Fakes::new();
    fakes.install_test(&t);
    // The platform's way: no Rust binding, so the report goes to the host as a port call.
    t.runtime()
        .bind_foreign_port(<dyn Diagnostics as undra_runtime::Port>::PORT_ID);
    t.call_sync(function("explode"), 1, &string_args("raw"));
    let calls: Vec<_> = t
        .host()
        .take_port_calls()
        .into_iter()
        .filter(|c| c.port_id == <dyn Diagnostics as undra_runtime::Port>::PORT_ID)
        .collect();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].method_id,
        ids::port_method_id("Diagnostics", "panicked")
    );
    assert_eq!(
        calls[0].port_call_id, 0,
        "fire and forget, like a log record"
    );
    let report = PanicReport::decode_exact(&calls[0].args).expect("the args are one PanicReport");
    assert_eq!(report.message, "raw");
    assert_eq!(report.encode_to_vec(), calls[0].args);
}

#[test]
fn a_report_that_panics_is_not_reported_again() {
    struct Bomb;
    impl Diagnostics for Bomb {
        fn panicked(&self, _report: PanicReport) {
            panic!("the reporter panicked");
        }
    }
    let t = TestRuntime::new();
    let _fakes = fakes::install(&t);
    t.runtime().bind_dyn_port_with::<dyn Diagnostics>(
        <dyn Diagnostics as undra_runtime::Port>::PORT_ID,
        std::sync::Arc::new(Bomb),
        &undra_ports::DIAGNOSTICS_DISPATCHER,
    );
    let reply = t.call_sync(function("explode"), 1, &string_args("first"));
    assert_eq!(
        reply.status,
        ReplyStatus::Panic,
        "the caller still gets its answer"
    );
}

/// What a contained panic costs, for `bench/RESULTS.md` (the cold path: no row is budgeted, the hot
/// path is untouched, ADR-046). `cargo test --release -p undra-ports --test diagnostics -- --ignored --nocapture panic_cost`.
#[test]
#[ignore = "a measurement, not a test"]
fn panic_cost() {
    let measure = |report: bool| {
        let t = TestRuntime::new();
        let fakes = fakes::install(&t);
        if !report {
            t.runtime()
                .unbind_port(<dyn Diagnostics as undra_runtime::Port>::PORT_ID);
        }
        let mut times = Vec::new();
        for i in 0..400_u32 {
            let started = std::time::Instant::now();
            let reply = t.call_sync(function("explode"), i + 1, &string_args("cost"));
            times.push(started.elapsed());
            assert_eq!(reply.status, ReplyStatus::Panic);
            fakes.diagnostics.clear();
        }
        times.sort();
        times[times.len() / 2]
    };
    println!(
        "contained panic, status 2 reply and a report: {:?} (p50 of 400)",
        measure(true)
    );
    println!(
        "contained panic, status 2 reply, port unbound: {:?} (p50 of 400)",
        measure(false)
    );
}
