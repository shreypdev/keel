//! Every port, called the way core code calls it, against its fake, inside a `TestRuntime`.
//!
//! Each request/reply port is exercised twice:
//!
//! * through its **proxy** (`HttpProxy::new(ctx)`), which encodes the call, sends it through the
//!   runtime's port table, lets the fake's generated dispatcher decode it and encodes the reply
//!   back: the whole boundary path, with the fake standing in for the platform;
//! * through the **accessor** (`undra_ports::http(&ctx)`), which hands core code the fake itself.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use undra_ports::fakes::{self, FakeClock, Fakes, Matcher};
use undra_ports::{
    AppState, Clock, ClockProxy, Connectivity, Fs, FsError, FsProxy, Http, HttpError, HttpMethod,
    HttpProxy, HttpRequest, HttpResponse, Kv, KvProxy, Lifecycle, Log, LogProxy, NetKind, Rng,
    RngProxy, SecureStore, SecureStoreProxy, Timer, TimerProxy, on_connectivity_changed,
    on_lifecycle_changed,
};
use undra_runtime::Port;
use undra_runtime::testing::TestRuntime;
use undra_wire::Bytes;

fn rig() -> (TestRuntime, Fakes) {
    let t = TestRuntime::new();
    let fakes = fakes::install(&t);
    (t, fakes)
}

// ---- Clock, Rng, Log ---------------------------------------------------------------------------

#[test]
fn clock_through_the_proxy_reads_the_fake() {
    let (t, fakes) = rig();
    let clock = ClockProxy::new(t.ctx());
    assert_eq!(clock.now_ms(), FakeClock::DEFAULT_NOW_MS);
    assert_eq!(clock.monotonic_ns(), 0);
    fakes.clock.set_now_ms(1_234);
    fakes.clock.advance(Duration::from_millis(5));
    assert_eq!(clock.now_ms(), 1_239);
    assert_eq!(clock.monotonic_ns(), 5_000_000);
    // The accessor is the fake itself.
    assert_eq!(undra_ports::clock(&t.ctx()).now_ms(), 1_239);
}

#[test]
fn rng_through_the_proxy_is_the_seeded_sequence() {
    let (t, _fakes) = rig();
    let expected = undra_ports::fakes::SeededRng::default();
    let rng = RngProxy::new(t.ctx());
    assert_eq!(rng.fill(16), expected.fill(16));
    assert_eq!(rng.fill(0), Bytes(vec![]));
    assert_eq!(rng.fill(5), expected.fill(5));
    assert_eq!(undra_ports::rng(&t.ctx()).fill(3), expected.fill(3));
}

#[test]
fn a_seed_makes_a_whole_run_reproducible() {
    let run = |seed| {
        let t = TestRuntime::new();
        let fakes = Fakes::with_seed(seed);
        fakes.install(t.runtime());
        RngProxy::new(t.ctx()).fill(32)
    };
    assert_eq!(run(7), run(7));
    assert_ne!(run(7), run(8));
}

#[test]
fn log_through_the_proxy_is_captured() {
    let (t, fakes) = rig();
    LogProxy::new(t.ctx()).log(4, "sync".into(), "boom".into());
    undra_ports::log(&t.ctx()).log(2, "ui".into(), "hello".into());
    let entries = fakes.log.entries();
    assert_eq!(entries.len(), 2);
    assert_eq!(
        (
            entries[0].level,
            entries[0].target.as_str(),
            entries[0].message.as_str()
        ),
        (4, "sync", "boom")
    );
    assert_eq!(entries[1].message, "hello");
}

// ---- Http --------------------------------------------------------------------------------------

#[test]
fn http_through_the_proxy_returns_scripted_responses_and_records_requests() {
    let (t, fakes) = rig();
    fakes.http.respond(
        Matcher::post("https://api.test/todos"),
        HttpResponse::new(201, b"{\"id\":1}".to_vec())
            .with_header("Content-Type", "application/json"),
    );
    fakes
        .http
        .respond("https://api.test/gone", HttpResponse::new(404, Vec::new()));
    let http = HttpProxy::new(t.ctx());

    let request = HttpRequest::post("https://api.test/todos", b"{}".to_vec())
        .with_header("Accept", "application/json")
        .with_timeout_ms(2_500);
    let response = t.run_until(http.request(request.clone())).unwrap();
    assert_eq!(response.status, 201);
    assert_eq!(response.header("content-type"), Some("application/json"));
    assert_eq!(response.body.0, b"{\"id\":1}");

    // A 4xx is a response, not an error.
    let gone = t
        .run_until(http.request(HttpRequest::get("https://api.test/gone")))
        .unwrap();
    assert_eq!(gone.status, 404);
    assert!(!gone.is_success());

    // What the fake recorded is exactly what the core sent (through encode and decode).
    let calls = fakes.http.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], request);
    assert_eq!(calls[1].method, HttpMethod::Get);
}

#[test]
fn http_typed_errors_survive_the_boundary() {
    let (t, fakes) = rig();
    fakes.http.fail("https://a", HttpError::Timeout);
    fakes.http.fail("https://b", HttpError::Cancelled);
    fakes
        .http
        .fail("https://c", HttpError::InvalidUrl("no host".into()));
    fakes
        .http
        .fail("https://d", HttpError::Network("dns".into()));
    let http = HttpProxy::new(t.ctx());
    for (url, expected) in [
        ("https://a", HttpError::Timeout),
        ("https://b", HttpError::Cancelled),
        ("https://c", HttpError::InvalidUrl("no host".into())),
        ("https://d", HttpError::Network("dns".into())),
    ] {
        assert_eq!(
            t.run_until(http.request(HttpRequest::get(url))),
            Err(expected),
            "{url}"
        );
    }
    // Nothing scripted for this one: a network error that names the request.
    let Err(HttpError::Network(message)) =
        t.run_until(http.request(HttpRequest::get("https://unscripted")))
    else {
        panic!("an unscripted request must fail with a network error");
    };
    assert!(message.contains("https://unscripted"), "{message}");
}

#[test]
fn http_retry_script_is_consumed_in_order_through_the_accessor() {
    let (t, fakes) = rig();
    fakes.http.respond_sequence(
        "https://flaky",
        [
            Err(HttpError::Timeout),
            Ok(HttpResponse::new(503, Vec::new())),
        ],
    );
    fakes
        .http
        .respond("https://flaky", HttpResponse::new(200, Vec::new()));
    let http = undra_ports::http(&t.ctx());
    let mut outcomes = Vec::new();
    for _ in 0..4 {
        outcomes.push(
            t.run_until(http.request(HttpRequest::get("https://flaky")))
                .map(|r| r.status),
        );
    }
    assert_eq!(
        outcomes,
        [Err(HttpError::Timeout), Ok(503), Ok(200), Ok(200)]
    );
    assert_eq!(fakes.http.call_count(), 4);
}

// ---- Kv and SecureStore ------------------------------------------------------------------------

#[test]
fn kv_through_the_proxy_stores_in_the_fake() {
    let (t, fakes) = rig();
    let kv = KvProxy::new(t.ctx());
    assert_eq!(t.run_until(kv.get("missing".into())), None);
    t.run_until(kv.set("todos/2".into(), Bytes(vec![2])));
    t.run_until(kv.set("todos/1".into(), Bytes(vec![1])));
    t.run_until(kv.set("other".into(), Bytes(vec![])));
    assert_eq!(t.run_until(kv.get("todos/1".into())), Some(Bytes(vec![1])));
    assert_eq!(
        t.run_until(kv.get("other".into())),
        Some(Bytes(vec![])),
        "an empty value is not a missing key"
    );
    assert_eq!(
        t.run_until(kv.list("todos/".into())),
        ["todos/1", "todos/2"]
    );
    t.run_until(kv.delete("todos/1".into()));
    t.run_until(kv.delete("never-there".into()));
    assert_eq!(t.run_until(kv.list("".into())), ["other", "todos/2"]);
    assert_eq!(fakes.kv.keys(), ["other", "todos/2"]);
    assert_eq!(fakes.kv.value("todos/2"), Some(vec![2]));
    assert!(
        fakes.secure_store.is_empty(),
        "Kv and SecureStore are separate"
    );
}

#[test]
fn secure_store_through_the_proxy_stores_in_its_own_fake() {
    let (t, fakes) = rig();
    let secrets = SecureStoreProxy::new(t.ctx());
    t.run_until(secrets.set("token".into(), Bytes(b"s3cr3t".to_vec())));
    assert_eq!(
        t.run_until(secrets.get("token".into())),
        Some(Bytes(b"s3cr3t".to_vec()))
    );
    assert_eq!(t.run_until(secrets.list("t".into())), ["token"]);
    assert!(fakes.kv.is_empty());
    t.run_until(secrets.delete("token".into()));
    assert_eq!(t.run_until(secrets.get("token".into())), None);
    // Through the accessor, straight to the fake.
    t.run_until(undra_ports::secure_store(&t.ctx()).set("k".into(), Bytes(vec![1])));
    assert_eq!(fakes.secure_store.value("k"), Some(vec![1]));
}

// ---- Fs ----------------------------------------------------------------------------------------

#[test]
fn fs_through_the_proxy_uses_the_fake_tree_and_returns_typed_errors() {
    let (t, fakes) = rig();
    let fs = FsProxy::new(t.ctx());
    assert_eq!(t.run_until(fs.read("nope".into())), Err(FsError::NotFound));
    assert_eq!(t.run_until(fs.read("../x".into())), Err(FsError::Denied));
    t.run_until(fs.write("cache/a/1.bin".into(), Bytes(vec![1, 2, 3])))
        .unwrap();
    assert_eq!(
        t.run_until(fs.read("cache/a/1.bin".into())),
        Ok(Bytes(vec![1, 2, 3]))
    );
    assert_eq!(
        t.run_until(fs.list("cache".into())),
        Ok(vec!["a".to_owned()])
    );
    assert_eq!(
        t.run_until(fs.read("cache".into())),
        Err(FsError::Io("is a directory".into()))
    );
    assert_eq!(fakes.fs.contents("cache/a/1.bin"), Some(vec![1, 2, 3]));
    t.run_until(fs.delete("cache".into())).unwrap();
    assert_eq!(
        t.run_until(fs.delete("cache".into())),
        Err(FsError::NotFound)
    );
    assert!(fakes.fs.file_paths().is_empty());
    // The accessor reaches the same tree.
    t.run_until(undra_ports::fs(&t.ctx()).write("x".into(), Bytes(vec![7])))
        .unwrap();
    assert_eq!(fakes.fs.contents("x"), Some(vec![7]));
}

// ---- Timer -------------------------------------------------------------------------------------

#[test]
fn timer_through_the_proxy_arms_the_fake_clock() {
    let (t, fakes) = rig();
    let timer = TimerProxy::new(t.ctx());
    timer.set(1, 300);
    timer.set(2, 100);
    undra_ports::timer(&t.ctx()).set(3, 200);
    assert_eq!(fakes.clock.pending_timer_ids(), [2, 3, 1]);
    assert_eq!(fakes.clock.advance(Duration::from_millis(250)), [2, 3]);
    assert_eq!(fakes.clock.pending_timer_ids(), [1]);
}

#[test]
fn a_fake_timer_completes_a_sleep_the_way_a_platform_timer_does() {
    // A host that owns timers (like the web host): the runtime asks it for a timer instead of
    // arming its own. The test plays the platform: it forwards the request to the Timer port
    // (`FakeClock`), and time passing fires `Runtime::timer_fired`.
    let (t, fakes) = rig();
    t.host().set_own_timers(true);
    let woke = Arc::new(Mutex::new(false));
    let flag = woke.clone();
    let ctx = t.ctx();
    t.ctx().spawn(async move {
        ctx.sleep(Duration::from_millis(250)).await;
        *flag.lock().unwrap() = true;
    });
    t.run_pending();
    let sets = t.host().take_timer_sets();
    assert_eq!(sets.len(), 1, "the runtime asked the host for one timer");
    let (timer_id, delay_ms) = sets[0];
    assert_eq!(delay_ms, 250);

    TimerProxy::new(t.ctx()).set(timer_id, delay_ms);
    fakes.clock.advance(Duration::from_millis(249));
    t.run_pending();
    assert!(!*woke.lock().unwrap(), "not due yet");
    assert_eq!(fakes.clock.advance(Duration::from_millis(1)), [timer_id]);
    t.run_pending();
    assert!(
        *woke.lock().unwrap(),
        "the fake timer fired into the runtime"
    );
}

#[test]
fn fakes_advance_serves_sleeps_from_the_fake_clock_at_their_deadlines() {
    let (t, fakes) = rig();
    let woke = Arc::new(Mutex::new(Vec::new()));
    let (ctx, sink, clock) = (t.ctx(), woke.clone(), fakes.clock.clone());
    t.ctx().spawn(async move {
        for _ in 0..3 {
            ctx.sleep(Duration::from_secs(10)).await;
            sink.lock().unwrap().push(clock.now_ms());
        }
    });
    let start = fakes.clock.now_ms();
    assert_eq!(fakes.advance(&t, Duration::from_secs(25)), 2);
    assert_eq!(
        *woke.lock().unwrap(),
        [start + 10_000, start + 20_000],
        "a task that wakes up reads its own deadline, not the end of the window"
    );
    assert_eq!(fakes.clock.now_ms(), start + 25_000);
    assert_eq!(fakes.advance(&t, Duration::from_secs(5)), 1);
    assert_eq!(woke.lock().unwrap()[2], start + 30_000);
    assert_eq!(
        fakes.advance(&t, Duration::from_secs(100)),
        0,
        "nothing left to fire"
    );
}

#[test]
fn sleeps_and_port_timers_interleave_by_deadline() {
    let (t, fakes) = rig();
    let order = Arc::new(Mutex::new(Vec::new()));
    let (ctx, sink) = (t.ctx(), order.clone());
    t.ctx().spawn(async move {
        ctx.sleep(Duration::from_millis(50)).await;
        sink.lock().unwrap().push("sleep 50");
        ctx.sleep(Duration::from_millis(100)).await;
        sink.lock().unwrap().push("sleep 50+100");
    });
    t.run_pending();
    fakes.sync_timers(&t);
    assert_eq!(
        fakes.clock.pending_timers(),
        1,
        "the sleep is armed on the fake clock"
    );
    // A timer armed straight through the Timer port, between the two sleeps.
    let fired = Arc::new(Mutex::new(Vec::new()));
    let sink = fired.clone();
    let clock = fakes.clock.clone();
    let hook_order = order.clone();
    fakes.clock.on_timer_fired({
        let runtime = Arc::downgrade(t.runtime());
        move |id| {
            sink.lock().unwrap().push((id, clock.now_ms()));
            if id == 9_999 {
                hook_order.lock().unwrap().push("port timer");
            }
            if let Some(rt) = runtime.upgrade() {
                rt.timer_fired(id);
            }
        }
    });
    Timer::set(&*fakes.clock, 9_999, 100);
    let start = fakes.clock.now_ms();
    assert_eq!(fakes.advance(&t, Duration::from_millis(500)), 3);
    assert_eq!(
        *order.lock().unwrap(),
        ["sleep 50", "port timer", "sleep 50+100"],
        "deadlines 50, 100 and 150"
    );
    let times: Vec<i64> = fired
        .lock()
        .unwrap()
        .iter()
        .map(|(_, at)| at - start)
        .collect();
    assert_eq!(times, [50, 100, 150]);
}

#[test]
fn advance_with_no_time_still_runs_ready_tasks() {
    let (t, fakes) = rig();
    let ran = Arc::new(Mutex::new(false));
    let flag = ran.clone();
    t.ctx().spawn(async move { *flag.lock().unwrap() = true });
    assert_eq!(fakes.advance(&t, Duration::ZERO), 0);
    assert!(*ran.lock().unwrap());
}

#[test]
fn a_zero_delay_port_timer_fires_at_the_end_of_advance() {
    let (t, fakes) = rig();
    Timer::set(&*fakes.clock, 5, 0);
    assert_eq!(fakes.advance(&t, Duration::ZERO), 1);
    assert_eq!(fakes.clock.pending_timers(), 0);
}

// ---- events ------------------------------------------------------------------------------------

#[test]
fn connectivity_events_reach_core_subscribers() {
    let (t, fakes) = rig();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let _sub = on_connectivity_changed(&t.ctx(), move |online, kind| {
        sink.lock().unwrap().push((online, kind));
    });
    fakes.connectivity.emit_current();
    fakes.connectivity.go_offline();
    fakes
        .connectivity
        .script([(true, NetKind::Cellular), (true, NetKind::Wifi)]);
    assert_eq!(fakes.connectivity.play(), 2);
    assert_eq!(
        *seen.lock().unwrap(),
        [
            (true, NetKind::Wifi),
            (false, NetKind::None),
            (true, NetKind::Cellular),
            (true, NetKind::Wifi)
        ]
    );
    assert_eq!(fakes.connectivity.current(), (true, NetKind::Wifi));
}

#[test]
fn lifecycle_events_reach_core_subscribers() {
    let (t, fakes) = rig();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let _sub = on_lifecycle_changed(&t.ctx(), move |state| sink.lock().unwrap().push(state));
    fakes
        .lifecycle
        .script([AppState::Inactive, AppState::Background]);
    fakes.lifecycle.play();
    fakes.lifecycle.set(AppState::Active);
    assert_eq!(
        *seen.lock().unwrap(),
        [AppState::Inactive, AppState::Background, AppState::Active]
    );
}

#[test]
fn a_dropped_subscription_stops_receiving() {
    let (t, fakes) = rig();
    let count = Arc::new(Mutex::new(0));
    let counter = count.clone();
    let subscription = on_lifecycle_changed(&t.ctx(), move |_| *counter.lock().unwrap() += 1);
    fakes.lifecycle.set(AppState::Background);
    drop(subscription);
    fakes.lifecycle.set(AppState::Active);
    assert_eq!(*count.lock().unwrap(), 1);
}

// ---- installation ------------------------------------------------------------------------------

#[test]
fn install_binds_every_request_reply_port_and_both_event_sources() {
    let (t, fakes) = rig();
    let ctx = t.ctx();
    assert!(
        ctx.rust_port::<dyn Clock>(<dyn Clock as Port>::PORT_ID)
            .is_some()
    );
    assert!(
        ctx.rust_port::<dyn Timer>(<dyn Timer as Port>::PORT_ID)
            .is_some()
    );
    assert!(
        ctx.rust_port::<dyn Rng>(<dyn Rng as Port>::PORT_ID)
            .is_some()
    );
    assert!(
        ctx.rust_port::<dyn Log>(<dyn Log as Port>::PORT_ID)
            .is_some()
    );
    assert!(
        ctx.rust_port::<dyn Http>(<dyn Http as Port>::PORT_ID)
            .is_some()
    );
    assert!(ctx.rust_port::<dyn Kv>(<dyn Kv as Port>::PORT_ID).is_some());
    assert!(
        ctx.rust_port::<dyn SecureStore>(<dyn SecureStore as Port>::PORT_ID)
            .is_some()
    );
    assert!(ctx.rust_port::<dyn Fs>(<dyn Fs as Port>::PORT_ID).is_some());
    assert!(
        ctx.rust_port::<dyn Connectivity>(<dyn Connectivity as Port>::PORT_ID)
            .is_some()
    );
    assert!(
        ctx.rust_port::<dyn Lifecycle>(<dyn Lifecycle as Port>::PORT_ID)
            .is_some()
    );
    // Not installed into a bare runtime.
    let bare = TestRuntime::new();
    assert!(
        bare.ctx()
            .rust_port::<dyn Http>(<dyn Http as Port>::PORT_ID)
            .is_none()
    );
    // The same fakes can serve a second runtime.
    fakes.install(bare.runtime());
    assert!(
        bare.ctx()
            .rust_port::<dyn Http>(<dyn Http as Port>::PORT_ID)
            .is_some()
    );
}

#[test]
fn fakes_do_not_keep_a_dropped_runtime_alive() {
    // The runtime owns the fakes (through its port table); the fakes reach back to it only
    // weakly (the clock's timer hook, the event sources), so there is no cycle.
    let fakes = Fakes::new();
    let runtime = {
        let t = TestRuntime::new();
        fakes.install(t.runtime());
        Arc::downgrade(t.runtime())
    };
    assert!(runtime.upgrade().is_none(), "a fake kept the runtime alive");
    // Firing a timer or emitting an event with nobody listening is harmless.
    Timer::set(&*fakes.clock, 1, 1);
    assert_eq!(fakes.clock.advance(Duration::from_millis(1)), [1]);
    fakes.connectivity.go_offline();
    fakes.lifecycle.set(AppState::Background);
}

#[test]
fn two_runs_of_the_same_script_leave_identical_traces() {
    /// Everything observable about a run: requests, random bytes, log lines, the final time and
    /// the connectivity history.
    type Trace = (
        Vec<HttpRequest>,
        Bytes,
        Vec<String>,
        i64,
        Vec<(bool, NetKind)>,
    );
    fn run() -> Trace {
        let t = TestRuntime::new();
        let fakes = Fakes::with_seed(99);
        fakes.install(t.runtime());
        fakes
            .http
            .respond(Matcher::any(), HttpResponse::new(200, Vec::new()));
        let ctx = t.ctx();
        for i in 0..3 {
            t.run_until(HttpProxy::new(ctx.clone()).request(HttpRequest::get(format!("u{i}"))))
                .unwrap();
            LogProxy::new(ctx.clone()).log(2, "t".into(), format!("step {i}"));
            fakes.advance(&t, Duration::from_millis(100 * (i + 1)));
            fakes.connectivity.set(i % 2 == 0, NetKind::Wifi);
        }
        (
            fakes.http.calls(),
            RngProxy::new(ctx.clone()).fill(24),
            fakes.log.messages(),
            ClockProxy::new(ctx).now_ms(),
            fakes.connectivity.history(),
        )
    }
    assert_eq!(run(), run());
}

// ---- thread safety -----------------------------------------------------------------------------

#[test]
fn every_fake_is_send_and_sync() {
    fn check<T: Send + Sync + 'static>() {}
    check::<Fakes>();
    check::<undra_ports::fakes::FakeHttp>();
    check::<undra_ports::fakes::FakeClock>();
    check::<undra_ports::fakes::SeededRng>();
    check::<undra_ports::fakes::CaptureLog>();
    check::<undra_ports::fakes::MemKv>();
    check::<undra_ports::fakes::MemSecureStore>();
    check::<undra_ports::fakes::MemFs>();
    check::<undra_ports::fakes::ScriptedConnectivity>();
    check::<undra_ports::fakes::ScriptedLifecycle>();
    check::<undra_ports::fakes::Matcher>();
}

#[test]
fn fakes_tolerate_concurrent_use() {
    let fakes = Fakes::new();
    fakes
        .http
        .respond(Matcher::any(), HttpResponse::new(200, Vec::new()));
    let threads: Vec<_> = (0..8)
        .map(|worker| {
            let fakes = fakes.clone();
            std::thread::spawn(move || {
                for i in 0..50 {
                    let key = format!("k/{worker}/{i}");
                    common::ready(Kv::set(&*fakes.kv, key.clone(), Bytes(vec![1])));
                    assert!(common::ready(Kv::get(&*fakes.kv, key)).is_some());
                    common::ready(Http::request(&*fakes.http, HttpRequest::get("u"))).unwrap();
                    Log::log(&*fakes.log, 2, "t".into(), format!("{worker}/{i}"));
                    let _ = Rng::fill(&*fakes.rng, 4);
                    Timer::set(&*fakes.clock, worker * 100 + i, 1);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().expect("a worker panicked");
    }
    assert_eq!(fakes.kv.len(), 400);
    assert_eq!(fakes.http.call_count(), 400);
    assert_eq!(fakes.log.len(), 400);
    assert_eq!(fakes.clock.pending_timers(), 400);
    assert_eq!(fakes.clock.advance(Duration::from_millis(1)).len(), 400);
}

// ---- unavailable ports (review H2) -------------------------------------------------------------

#[test]
fn h2_request_reply_ports_report_an_unbound_port_as_their_typed_error() {
    // No fakes installed: nobody registered `Http` or `Fs`, which SPEC 6.3 answers "unavailable".
    let t = TestRuntime::new();
    let http = HttpProxy::new(t.ctx());
    assert_eq!(
        t.run_until(http.request(HttpRequest::get("https://api.test/x"))),
        Err(HttpError::Network(
            "the Http port has no adapter registered".into()
        ))
    );
    let fs = FsProxy::new(t.ctx());
    let expected = FsError::Io("the Fs port has no adapter registered".into());
    assert_eq!(t.run_until(fs.read("a".into())), Err(expected.clone()));
    assert_eq!(
        t.run_until(fs.write("a".into(), Bytes(vec![1]))),
        Err(expected.clone())
    );
    assert_eq!(t.run_until(fs.delete("a".into())), Err(expected.clone()));
    assert_eq!(t.run_until(fs.list("/".into())), Err(expected));
}

#[test]
fn h2_a_port_without_an_error_channel_still_panics_and_says_how_to_bind_it() {
    let t = TestRuntime::new();
    let kv = KvProxy::new(t.ctx());
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        t.run_until(kv.get("k".into()))
    }))
    .expect_err("an unbound Kv cannot answer");
    let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(
        message.contains("the `Kv` port has no adapter registered (method `get`)"),
        "{message}"
    );
    assert!(message.contains("registerPort"), "{message}");
    assert!(message.contains("errors/E0062"), "{message}");
}
