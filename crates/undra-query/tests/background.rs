//! Background runs (ADR-046 decision 3) through the query runtime's three tasks: replay the
//! offline queue, refetch stale queries, flush pending persistence, driven the way a platform
//! drives them, with the standard function `run_background` and the fake clock.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::*;
use undra::background::{BackgroundFuture, BackgroundOutcome, Deadline};
use undra_ports::fakes::{FailOn, Fakes};
use undra_ports::{
    AppState, BackgroundReport, Clock, Http, HttpError, HttpRequest, HttpResponse, NetKind,
    StorageError,
};
use undra_query::{CtxQuery, QUEUE_KEY};
use undra_runtime::Port;

fn post_todos() -> undra_ports::fakes::Matcher {
    undra_ports::fakes::Matcher::post(format!("{API}/todos"))
}

/// An `Http` that answers after `delay` of fake time, then like the scripted fake.
struct SlowHttp {
    inner: Arc<undra_ports::fakes::FakeHttp>,
    ctx: undra_runtime::WeakCtx,
    delay: Mutex<Duration>,
}

#[undra::port]
impl Http for SlowHttp {
    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
        let delay = *self.delay.lock().unwrap();
        let sleep = self.ctx.sleep(delay);
        // The request reaches the server at once (the fake records it) and the answer takes `delay`.
        let answer = self.inner.request(req).await;
        let _ = sleep.await;
        answer
    }
}

impl SlowHttp {
    fn arm(&self, delay: Duration) {
        *self.delay.lock().unwrap() = delay;
    }
}

/// A harness whose `Http` answers late once [`SlowHttp::arm`]ed (not before: the offline attempt
/// that parks a mutation must fail at once).
fn slow_harness(delay: Duration) -> (Harness, Arc<SlowHttp>) {
    let h = Harness::new();
    let slow = Arc::new(SlowHttp {
        inner: h.fakes.http.clone(),
        ctx: h.ctx().downgrade(),
        delay: Mutex::new(Duration::ZERO),
    });
    let _ = delay;
    h.t.runtime().bind_dyn_port_with::<dyn Http>(
        <dyn Http as Port>::PORT_ID,
        slow.clone(),
        &undra_ports::HTTP_DISPATCHER,
    );
    h.settle();
    (h, slow)
}

fn queue_one_offline(h: &Harness, title: &str) -> Slot<Result<Todo, TodoError>> {
    h.fakes.connectivity.go_offline();
    h.fakes
        .http
        .fail(post_todos(), HttpError::Network("offline".into()));
    let (result, _) = spawn(h, h.ctx().mutate::<AddTodoMutation>((title.to_owned(),)));
    h.t.run_pending();
    assert!(take(&result).is_none(), "parked, not failed");
    assert_eq!(h.query().pending_mutations(), 1);
    result
}

fn run(h: &Harness, deadline: Duration) -> BackgroundReport {
    h.fakes.run_background(&h.t, deadline)
}

#[test]
fn with_nothing_to_do_a_run_finishes_at_once() {
    let h = Harness::new();
    h.settle();
    assert_eq!(
        run(&h, Duration::from_secs(10)),
        BackgroundReport {
            finished: true,
            replayed: 0,
            refetched: 0,
            still_pending: 0
        }
    );
    let stats = h.t.runtime().stats_json();
    assert!(stats.contains("\"runs\":1,\"finished\":1"), "{stats}");
}

#[test]
fn offline_a_run_does_not_hold_the_window_and_reports_the_work_still_pending() {
    let h = Harness::new();
    h.settle();
    let _queued = queue_one_offline(&h, "milk");
    let started = h.fakes.clock.monotonic_ns();
    let report = run(&h, Duration::from_secs(30));
    let spent = Duration::from_nanos(h.fakes.clock.monotonic_ns() - started);
    assert_eq!(
        report,
        BackgroundReport {
            finished: false,
            replayed: 0,
            refetched: 0,
            still_pending: 1
        }
    );
    assert!(
        spent < Duration::from_secs(1),
        "ended at once, took {spent:?}"
    );
    assert_eq!(h.query().pending_mutations(), 1, "still queued");
    let stats = h.t.runtime().stats_json();
    assert!(stats.contains("\"pending\":1"), "{stats}");
}

#[test]
fn online_the_run_drains_the_queue_and_counts_it() {
    let (h, slow) = slow_harness(Duration::from_millis(400));
    let result = queue_one_offline(&h, "milk");
    slow.arm(Duration::from_millis(400));
    h.fakes.http.reset();
    h.fakes.http.respond(post_todos(), ok(&todo(9, "milk")));
    // The platform reports the network back; the client's own replay starts, and the run that
    // follows joins it.
    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.t.run_pending();
    let report = run(&h, Duration::from_secs(10));
    assert_eq!(
        report,
        BackgroundReport {
            finished: true,
            replayed: 1,
            refetched: report.refetched,
            still_pending: 0
        }
    );
    assert_eq!(take(&result).unwrap().unwrap().title, "milk");
    assert_eq!(h.query().pending_mutations(), 0);
    let posts = h
        .fakes
        .http
        .calls()
        .iter()
        .filter(|r| r.method == undra_ports::HttpMethod::Post)
        .count();
    assert_eq!(posts, 1, "sent once");
}

#[test]
fn a_run_that_drains_the_queue_waits_for_the_refetch_the_replay_caused() {
    // An observed, persisted list: the replayed mutation invalidates it, so it is fetched again
    // and written. The run is finished when all of that is, not when the queue is empty.
    let (h, slow) = slow_harness(Duration::from_millis(400));
    h.serve_page(0, vec![todo(1, "milk")]);
    let _observed = h.query().observe::<TodosQuery>((0,));
    h.settle();
    h.advance_ms(1_000);
    let result = queue_one_offline(&h, "eggs");
    slow.arm(Duration::from_millis(400));
    h.fakes.http.reset();
    h.serve_page(0, vec![todo(1, "milk"), todo(2, "eggs")]);
    h.fakes.http.respond(post_todos(), ok(&todo(2, "eggs")));
    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.t.run_pending();
    let report = run(&h, Duration::from_secs(10));
    // (Coming back online refetches the observed list, and the replay's invalidation does again:
    // both are fetches this run waited for.)
    assert!(
        report.finished && report.replayed == 1 && report.still_pending == 0,
        "{report:?}"
    );
    assert!(report.refetched >= 1, "{report:?}");
    assert_eq!(take(&result).unwrap().unwrap().title, "eggs");
    assert!(
        h.fakes
            .kv
            .keys()
            .iter()
            .any(|k| k.starts_with("undra.query.cache2."))
    );
}

#[test]
fn a_run_cut_at_its_deadline_leaves_the_replay_and_the_queue_alone() {
    let (h, slow) = slow_harness(Duration::from_secs(5));
    let result = queue_one_offline(&h, "slow");
    slow.arm(Duration::from_secs(5));
    h.fakes.http.reset();
    h.fakes.http.respond(post_todos(), ok(&todo(9, "slow")));
    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.t.run_pending();

    let started = h.fakes.clock.monotonic_ns();
    let report = run(&h, Duration::from_secs(1));
    let spent = Duration::from_nanos(h.fakes.clock.monotonic_ns() - started);
    assert_eq!(
        report,
        BackgroundReport {
            finished: false,
            replayed: 0,
            refetched: 0,
            still_pending: 1
        }
    );
    assert!(
        spent >= Duration::from_millis(450) && spent <= Duration::from_millis(900),
        "returned at the deadline less the host's half second, took {spent:?}"
    );
    // The replay in flight is the client's, not the run's: the item is still queued and nothing
    // was sent twice.
    assert_eq!(h.query().pending_mutations(), 1);
    assert!(h.fakes.kv.value(QUEUE_KEY).is_some(), "still persisted");
    assert!(take(&result).is_none());
    h.advance_ms(6_000);
    assert_eq!(take(&result).unwrap().unwrap().title, "slow");
    assert_eq!(h.query().pending_mutations(), 0);
    let posts = h
        .fakes
        .http
        .calls()
        .iter()
        .filter(|r| r.method == undra_ports::HttpMethod::Post)
        .count();
    assert_eq!(posts, 1, "one POST in all: the run did not resend it");
}

#[test]
fn a_host_that_cancels_the_call_cancels_the_run_and_loses_nothing() {
    let (h, slow) = slow_harness(Duration::from_secs(3));
    let result = queue_one_offline(&h, "held");
    slow.arm(Duration::from_secs(3));
    h.fakes.http.reset();
    h.fakes.http.respond(post_todos(), ok(&todo(9, "held")));
    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.t.run_pending();

    let function = undra_wire::payload::CallTarget::Function {
        method_id: undra_meta::ids::function_id("run_background"),
    };
    assert_eq!(h.t.call(function, 77, &30_000_u64.to_le_bytes()), 0);
    h.advance_ms(100);
    assert!(
        h.t.take_replies().is_empty(),
        "the run is waiting for the replay"
    );
    h.t.runtime().cancel(77);
    let replies = h.t.take_replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(
        replies[0].status,
        undra_wire::payload::ReplyStatus::Cancelled
    );
    // The core keeps working and the queued mutation still completes.
    assert_eq!(h.query().pending_mutations(), 1);
    h.advance_ms(4_000);
    assert_eq!(take(&result).unwrap().unwrap().title, "held");
}

#[test]
fn an_unreadable_queue_is_read_again_by_a_run_and_replayed() {
    // A queue an earlier run left behind, and a store that is locked (before the first unlock).
    let first = Harness::new();
    first.settle();
    let _queued = queue_one_offline(&first, "left behind");
    let stored = first.fakes.kv.value(QUEUE_KEY).expect("persisted");

    let fakes = Fakes::new();
    fakes.kv.insert(QUEUE_KEY, stored);
    fakes.kv.fail(FailOn::Get, StorageError::Locked);
    let h = Harness::with_fakes(fakes);
    h.settle();
    h.fakes
        .http
        .respond(post_todos(), ok(&todo(9, "left behind")));
    assert_eq!(
        h.query().pending_mutations(),
        0,
        "unread: nothing to replay yet"
    );
    let stats = h.t.runtime().stats_json();
    assert!(
        stats.contains("\"pending\":1"),
        "an unreadable queue is work: {stats}"
    );

    // Unlocked: the run reads it again, finds the item and sends it.
    h.fakes.kv.heal();
    let report = run(&h, Duration::from_secs(10));
    assert_eq!(report.replayed, 1, "{report:?}");
    assert!(report.finished, "{report:?}");
    assert_eq!(h.query().pending_mutations(), 0);
}

#[test]
fn going_to_the_background_writes_what_waits_out_its_debounce_at_once() {
    let h = Harness::new();
    h.settle();
    h.serve_page(0, vec![todo(1, "milk")]);
    let _observed = h.query().observe::<TodosQuery>((0,));
    h.t.run_pending();
    assert!(
        !h.fakes
            .kv
            .keys()
            .iter()
            .any(|k| k.starts_with("undra.query.cache2.")),
        "the write is debounced"
    );
    h.fakes.lifecycle.set(AppState::Background);
    h.t.run_pending();
    assert!(
        h.fakes
            .kv
            .keys()
            .iter()
            .any(|k| k.starts_with("undra.query.cache2.")),
        "written when the app went to the background, not 250 ms later"
    );
}

#[test]
fn a_run_fetches_stale_persisted_queries_again_and_writes_them() {
    let h = Harness::new();
    h.settle();
    h.serve_page(0, vec![todo(1, "milk")]);
    {
        let _observed = h.query().observe::<TodosQuery>((0,));
        h.t.run_pending();
    }
    // 30 s later the persisted entry is stale; the server has more.
    h.advance_ms(31_000);
    h.fakes.http.reset();
    h.serve_page(0, vec![todo(1, "milk"), todo(2, "eggs")]);
    let stats = h.t.runtime().stats_json();
    assert!(
        stats.contains("\"pending\":1"),
        "a stale persisted query is work: {stats}"
    );
    let report = run(&h, Duration::from_secs(10));
    assert_eq!(
        report,
        BackgroundReport {
            finished: true,
            replayed: 0,
            refetched: 1,
            still_pending: 0
        }
    );
    assert!(
        h.fakes
            .kv
            .keys()
            .iter()
            .any(|k| k.starts_with("undra.query.cache2."))
    );
}

#[test]
fn a_task_that_panics_is_contained_reported_and_incomplete() {
    let h = Harness::new();
    h.settle();
    undra::background::register(
        &h.ctx(),
        "test.explode",
        |_| 1,
        |_ctx: &undra_runtime::Ctx, _deadline: Deadline| -> BackgroundFuture {
            Box::pin(async {
                panic!("the task blew up");
                #[allow(unreachable_code)]
                BackgroundOutcome::Done
            })
        },
    );
    let report = run(&h, Duration::from_secs(10));
    assert!(!report.finished, "{report:?}");
    assert_eq!(report.still_pending, 1);
    let reports = h.fakes.diagnostics.take();
    assert_eq!(reports.len(), 1, "{reports:#?}");
    assert_eq!(reports[0].message, "the task blew up");
    assert_eq!(reports[0].operation, "task");
}

#[test]
fn the_stats_say_how_many_tasks_there_are_and_what_the_runs_did() {
    let h = Harness::new();
    h.settle();
    let stats: serde_json::Value = serde_json::from_str(&h.t.runtime().stats_json()).unwrap();
    assert_eq!(stats["background"]["tasks"], 3, "replay, refetch, flush");
    assert_eq!(stats["background"]["pending"], 0);
    assert_eq!(stats["panic_reports"], 0);
    let _ = run(&h, Duration::from_secs(10));
    let stats: serde_json::Value = serde_json::from_str(&h.t.runtime().stats_json()).unwrap();
    assert_eq!(stats["background"]["runs"], 1);
    assert_eq!(stats["background"]["finished"], 1);
}
