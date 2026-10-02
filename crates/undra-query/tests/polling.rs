//! Interval polling (ADR-043 decision 1): `#[undra::query(interval = "..")]` through the facade,
//! under the fake clock and the fake lifecycle and connectivity ports.
//!
//! Every poll is seen: the tests advance time and count the requests that reach the server (and
//! when). The semantics under test are TanStack's `refetchInterval`: the next poll is scheduled an
//! interval after the **end** of the previous fetch, background and offline pause it, the last
//! observer's release cancels it, and each observer may poll faster than the query does.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{API, Harness, TodoError};
use parking_lot::Mutex;
use undra::meta::{TypeRef, collect_schema};
use undra::prelude::*;
use undra::runtime::testing::TestRuntime;
use undra::wire::payload::{CallTarget, ChangeOp, ReplyStatus};
use undra_ports::fakes::Matcher;
use undra_ports::{AppState, Clock, HttpError, HttpRequest, HttpResponse, NetKind, Rng};
use undra_query::{
    MAX_POLL_INTERVAL_MS, MIN_POLL_INTERVAL_MS, QueryDef, QueryHandle, QueryStatus,
    SET_POLL_INTERVAL_METHOD_ID, backoff_ms,
};
use undra_wire::{Decode, Encode, Handle};

async fn ping(ctx: &Ctx, path: &str) -> Result<u32, TodoError> {
    let response = ctx
        .http()
        .request(HttpRequest::get(format!("{API}/{path}")))
        .await?;
    Ok(u32::from(response.status))
}

/// Polls every five seconds while the app is in front.
#[undra::query(key = "tick", interval = "5s", retry = 0)]
pub async fn tick(ctx: &Ctx) -> Result<u32, TodoError> {
    ping(ctx, "tick").await
}

/// The same, fresh for an hour once fetched: a resume has nothing to refetch.
#[undra::query(key = "fresh_tick", interval = "5s", stale = "1h", retry = 0)]
pub async fn fresh_tick(ctx: &Ctx) -> Result<u32, TodoError> {
    ping(ctx, "fresh_tick").await
}

/// Keeps polling in the background.
#[undra::query(key = "bg_tick", interval = "5s", poll_in_background, retry = 0)]
pub async fn bg_tick(ctx: &Ctx) -> Result<u32, TodoError> {
    ping(ctx, "bg_tick").await
}

/// No interval of its own: only an observer can poll it.
#[undra::query(key = "plain", retry = 0)]
pub async fn plain(ctx: &Ctx) -> Result<u32, TodoError> {
    ping(ctx, "plain").await
}

/// No interval and fresh for an hour: observers can join it without fetching.
#[undra::query(key = "fresh_plain", stale = "1h", retry = 0)]
pub async fn fresh_plain(ctx: &Ctx) -> Result<u32, TodoError> {
    ping(ctx, "fresh_plain").await
}

/// Polls every two seconds, and every fetch takes three.
#[undra::query(key = "slow_tick", interval = "2s", retry = 0)]
pub async fn slow_tick(ctx: &Ctx) -> Result<u32, TodoError> {
    let status = ping(ctx, "slow_tick").await?;
    ctx.sleep(Duration::from_secs(3)).await;
    Ok(status)
}

/// Polls every five seconds and retries a failed fetch twice.
#[undra::query(key = "flaky_tick", interval = "5s", retry = 2)]
pub async fn flaky_tick(ctx: &Ctx) -> Result<u32, TodoError> {
    ping(ctx, "flaky_tick").await
}

fn serve(h: &Harness, path: &str) {
    h.fakes
        .http
        .respond(format!("{API}/{path}"), HttpResponse::new(200, Vec::new()));
}

/// The requests that reached the server for `path` so far.
fn calls(h: &Harness, path: &str) -> usize {
    h.fakes
        .http
        .calls()
        .iter()
        .filter(|r| r.url == format!("{API}/{path}"))
        .count()
}

fn observe<Q: QueryDef>(h: &Harness, params: Q::Params) -> QueryHandle<Q> {
    let handle = h.query().observe::<Q>(params);
    h.t.run_pending();
    handle
}

// ----- the interval ---------------------------------------------------------------------------

#[test]
fn the_attribute_is_in_the_schema_and_the_constants() {
    assert_eq!(TickQuery::INTERVAL_MS, Some(5_000));
    assert!(!TickQuery::POLL_IN_BACKGROUND);
    assert_eq!(BgTickQuery::INTERVAL_MS, Some(5_000));
    assert!(BgTickQuery::POLL_IN_BACKGROUND);
    assert_eq!(PlainQuery::INTERVAL_MS, None);
    assert_eq!(<TickQuery as QueryDef>::INTERVAL_MS, Some(5_000));
    let schema = collect_schema("t");
    let find = |name: &str| schema.queries.iter().find(|q| q.name == name).unwrap();
    assert_eq!(find("tick").interval_ms, Some(5_000));
    assert!(!find("tick").poll_in_background);
    assert!(find("bg_tick").poll_in_background);
    assert_eq!(find("plain").interval_ms, None);
    assert_eq!(find("tick").infinite, None);
    assert_eq!(
        find("tick").returns,
        TypeRef::result(TypeRef::U32, TypeRef::named("TodoError"))
    );
    assert_eq!(schema.validate(), Ok(()));
}

#[test]
fn an_observed_entry_polls_an_interval_after_each_fetch() {
    let h = Harness::new();
    serve(&h, "tick");
    let tick = observe::<TickQuery>(&h, ());
    assert_eq!(calls(&h, "tick"), 1);
    assert_eq!(tick.data().get(), Some(200));

    h.advance_ms(4_999);
    assert_eq!(calls(&h, "tick"), 1, "not yet");
    h.advance_ms(1);
    assert_eq!(calls(&h, "tick"), 2, "five seconds after the fetch ended");
    assert!(tick.fetching().get() || tick.status().get() == QueryStatus::Success);
    h.advance_ms(5_000);
    assert_eq!(calls(&h, "tick"), 3);
    h.advance_ms(10_000);
    assert_eq!(calls(&h, "tick"), 5);
    assert_eq!(h.http_calls(), 5, "nothing else was requested");
}

#[test]
fn every_poll_is_seen_at_the_moment_it_is_due_under_the_fake_clock() {
    // The same schedule on two runs, to the millisecond: time is the Clock port, delays the Timer.
    let run = || {
        let h = Harness::new();
        let times: Arc<Mutex<Vec<i64>>> = Arc::default();
        let (clock, log) = (h.fakes.clock.clone(), times.clone());
        h.fakes
            .http
            .respond_with(Matcher::url_prefix(format!("{API}/tick")), move |_| {
                log.lock().push(clock.now_ms());
                Ok(HttpResponse::new(200, Vec::new()))
            });
        let start = h.fakes.clock.now_ms();
        let _tick = observe::<TickQuery>(&h, ());
        h.advance_ms(21_000);
        let seen: Vec<i64> = times.lock().iter().map(|t| t - start).collect();
        seen
    };
    assert_eq!(run(), [0, 5_000, 10_000, 15_000, 20_000]);
    assert_eq!(run(), run());
}

#[test]
fn a_fetch_slower_than_the_interval_is_never_overlapped_and_the_next_poll_counts_from_its_end() {
    let h = Harness::new();
    serve(&h, "slow_tick");
    let slow = observe::<SlowTickQuery>(&h, ());
    assert_eq!(calls(&h, "slow_tick"), 1);
    assert!(slow.fetching().get(), "three seconds of fetching");

    // A fixed rate would fire at 2 s into the fetch; the interval only starts when it ends.
    h.advance_ms(2_000);
    assert_eq!(calls(&h, "slow_tick"), 1);
    assert!(slow.fetching().get());
    h.advance_ms(1_000); // t = 3 s: the fetch ended, the next poll is due at 5 s
    assert!(!slow.fetching().get());
    h.advance_ms(1_999);
    assert_eq!(calls(&h, "slow_tick"), 1, "t = 4.999 s");
    h.advance_ms(1);
    assert_eq!(
        calls(&h, "slow_tick"),
        2,
        "t = 5 s: two seconds after the end"
    );
    h.advance_ms(2_999); // t = 7.999 s: still inside the second fetch
    assert_eq!(calls(&h, "slow_tick"), 2);
    h.advance_ms(2_001); // t = 10 s: it ended at 8 s, the poll is due at 10 s
    assert_eq!(calls(&h, "slow_tick"), 3);
}

/// The delays a runtime whose `Rng` is `SeededRng::default()` sleeps between retries.
fn backoffs(retries: u32) -> Vec<u64> {
    let rng = undra_ports::fakes::SeededRng::default();
    (0..retries)
        .map(|attempt| {
            let bytes: [u8; 8] = rng.fill(8).0.try_into().unwrap();
            backoff_ms(attempt, u64::from_le_bytes(bytes))
        })
        .collect()
}

#[test]
fn a_failure_after_its_retries_schedules_the_next_poll_from_where_the_retries_ended() {
    let h = Harness::new();
    h.fakes
        .http
        .fail(format!("{API}/flaky_tick"), HttpError::Timeout);
    let tick = observe::<FlakyTickQuery>(&h, ());
    assert_eq!(calls(&h, "flaky_tick"), 1);
    let [first, second] = backoffs(2)[..] else {
        unreachable!()
    };
    h.advance_ms(first);
    h.advance_ms(second);
    assert_eq!(
        calls(&h, "flaky_tick"),
        3,
        "the attempt and its two retries"
    );
    assert_eq!(tick.status().get(), QueryStatus::Error);

    h.advance_ms(4_999);
    assert_eq!(
        calls(&h, "flaky_tick"),
        3,
        "five seconds after the retries ended"
    );
    h.advance_ms(1);
    assert_eq!(
        calls(&h, "flaky_tick"),
        4,
        "the poll is a new fetch, with its own retries"
    );
}

// ----- pausing and resuming -----------------------------------------------------------------------

#[test]
fn background_and_inactive_pause_polling_and_active_resumes_it_with_the_stale_refetch() {
    let h = Harness::new();
    serve(&h, "tick");
    let _tick = observe::<TickQuery>(&h, ());
    assert_eq!(calls(&h, "tick"), 1);

    h.fakes.lifecycle.set(AppState::Inactive);
    h.advance_ms(60_000);
    assert_eq!(calls(&h, "tick"), 1, "Inactive pauses");
    h.fakes.lifecycle.set(AppState::Background);
    h.advance_ms(60_000);
    assert_eq!(calls(&h, "tick"), 1, "Background pauses");

    // Active: the entry has no `stale` window, so it is stale and refetches at once ...
    h.fakes.lifecycle.set(AppState::Active);
    h.t.run_pending();
    assert_eq!(calls(&h, "tick"), 2);
    // ... and the next poll counts from the end of that fetch.
    h.advance_ms(4_999);
    assert_eq!(calls(&h, "tick"), 2);
    h.advance_ms(1);
    assert_eq!(calls(&h, "tick"), 3);
}

#[test]
fn an_entry_that_is_fresh_on_resume_gets_a_full_interval_from_the_moment_of_the_resume() {
    let h = Harness::new();
    serve(&h, "fresh_tick");
    let _tick = observe::<FreshTickQuery>(&h, ());
    assert_eq!(calls(&h, "fresh_tick"), 1);
    h.advance_ms(1_000);
    h.fakes.lifecycle.set(AppState::Background);
    h.advance_ms(30_000);
    assert_eq!(calls(&h, "fresh_tick"), 1);

    h.fakes.lifecycle.set(AppState::Active);
    h.t.run_pending();
    assert_eq!(
        calls(&h, "fresh_tick"),
        1,
        "fresh for an hour: nothing to refetch"
    );
    h.advance_ms(4_999);
    assert_eq!(calls(&h, "fresh_tick"), 1);
    h.advance_ms(1);
    assert_eq!(
        calls(&h, "fresh_tick"),
        2,
        "a full interval after the resume"
    );
}

#[test]
fn poll_in_background_keeps_polling_while_the_app_is_away() {
    let h = Harness::new();
    serve(&h, "bg_tick");
    let _tick = observe::<BgTickQuery>(&h, ());
    h.fakes.lifecycle.set(AppState::Background);
    h.advance_ms(5_000);
    assert_eq!(calls(&h, "bg_tick"), 2);
    h.fakes.lifecycle.set(AppState::Inactive);
    h.advance_ms(5_000);
    assert_eq!(calls(&h, "bg_tick"), 3);
    // Coming to the front changes nothing about when the next one is due.
    h.advance_ms(2_000);
    h.fakes.lifecycle.set(AppState::Active);
    h.t.run_pending();
    assert_eq!(
        calls(&h, "bg_tick"),
        4,
        "always stale: the resume refetches (a trigger)"
    );
}

#[test]
fn offline_pauses_even_a_background_poll_and_online_refetches_then_polls_again() {
    let h = Harness::new();
    serve(&h, "bg_tick");
    serve(&h, "tick");
    let _bg = observe::<BgTickQuery>(&h, ());
    let _tick = observe::<TickQuery>(&h, ());
    h.fakes.http.take_calls();

    h.fakes.connectivity.go_offline();
    h.advance_ms(60_000);
    assert_eq!(h.http_calls(), 0, "offline: no poll of either");

    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.t.run_pending();
    assert_eq!(
        h.http_calls(),
        2,
        "going online refetches everything observed"
    );
    h.advance_ms(4_999);
    assert_eq!(h.http_calls(), 2);
    h.advance_ms(1);
    assert_eq!(
        h.http_calls(),
        4,
        "and both poll again from the end of that fetch"
    );
}

#[test]
fn polling_started_offline_waits_for_the_network() {
    let h = Harness::new();
    serve(&h, "tick");
    h.fakes.connectivity.go_offline();
    h.t.run_pending();
    let _tick = observe::<TickQuery>(&h, ());
    assert_eq!(
        calls(&h, "tick"),
        1,
        "observing still fetches (the query client does not refuse)"
    );
    h.advance_ms(30_000);
    assert_eq!(calls(&h, "tick"), 1, "but nothing polls while offline");
}

#[test]
fn releasing_the_last_observer_cancels_the_poll() {
    let h = Harness::new();
    serve(&h, "fresh_tick");
    let first = observe::<FreshTickQuery>(&h, ());
    let second = h.query().observe::<FreshTickQuery>(());
    h.t.run_pending();
    assert_eq!(calls(&h, "fresh_tick"), 1, "one fetch for two observers");

    drop(first);
    h.advance_ms(5_000);
    assert_eq!(
        calls(&h, "fresh_tick"),
        2,
        "one observer left: still polled, once for the entry"
    );
    drop(second);
    h.advance_ms(60_000);
    assert_eq!(calls(&h, "fresh_tick"), 2, "nobody watches: nothing polls");

    // Observed again before the entry is collected: it is fresh (no fetch), and polls again.
    let _again = h.query().observe::<FreshTickQuery>(());
    h.t.run_pending();
    assert_eq!(calls(&h, "fresh_tick"), 2);
    h.advance_ms(5_000);
    assert_eq!(calls(&h, "fresh_tick"), 3);
}

// ----- the per-observer override -------------------------------------------------------------------

#[test]
fn an_observer_can_poll_faster_than_the_query_and_none_takes_it_back() {
    let h = Harness::new();
    serve(&h, "tick");
    let tick = observe::<TickQuery>(&h, ());
    assert_eq!(calls(&h, "tick"), 1);

    tick.set_poll_interval(Some(Duration::from_secs(2)));
    h.advance_ms(1_999);
    assert_eq!(calls(&h, "tick"), 1);
    h.advance_ms(1);
    assert_eq!(calls(&h, "tick"), 2, "two seconds after the fetch ended");
    h.advance_ms(2_000);
    assert_eq!(calls(&h, "tick"), 3);

    // Cleared: back to the attribute's five seconds, counted from the same fetch end.
    tick.set_poll_interval(None);
    h.advance_ms(4_999);
    assert_eq!(calls(&h, "tick"), 3);
    h.advance_ms(1);
    assert_eq!(calls(&h, "tick"), 4);
}

#[test]
fn an_observers_interval_replaces_the_default_for_that_observer_even_if_longer() {
    let h = Harness::new();
    serve(&h, "tick");
    let tick = observe::<TickQuery>(&h, ());
    tick.set_poll_interval(Some(Duration::from_secs(20)));
    h.advance_ms(19_999);
    assert_eq!(calls(&h, "tick"), 1);
    h.advance_ms(1);
    assert_eq!(calls(&h, "tick"), 2);
}

#[test]
fn the_entry_polls_at_the_smallest_interval_among_its_observers() {
    let h = Harness::new();
    serve(&h, "fresh_tick");
    let a = observe::<FreshTickQuery>(&h, ());
    let b = h.query().observe::<FreshTickQuery>(());
    h.t.run_pending();
    a.set_poll_interval(Some(Duration::from_secs(2)));
    h.advance_ms(2_000);
    assert_eq!(calls(&h, "fresh_tick"), 2, "a's two seconds");
    h.advance_ms(2_000);
    assert_eq!(
        calls(&h, "fresh_tick"),
        3,
        "one entry, one fetch per poll, whoever asked for it"
    );

    // b has no override: the attribute's five seconds is still its interval, a's is shorter.
    b.set_poll_interval(Some(Duration::from_secs(1)));
    h.advance_ms(1_000);
    assert_eq!(
        calls(&h, "fresh_tick"),
        4,
        "now b's one second is the smallest"
    );
    // a goes away: b's second remains; b goes back to the default: five.
    drop(a);
    h.advance_ms(1_000);
    assert_eq!(calls(&h, "fresh_tick"), 5);
    b.set_poll_interval(None);
    h.advance_ms(4_999);
    assert_eq!(calls(&h, "fresh_tick"), 5);
    h.advance_ms(1);
    assert_eq!(calls(&h, "fresh_tick"), 6);
}

#[test]
fn a_query_without_an_interval_is_polled_by_an_observer_that_sets_one() {
    let h = Harness::new();
    serve(&h, "plain");
    let plain = observe::<PlainQuery>(&h, ());
    h.advance_ms(60_000);
    assert_eq!(calls(&h, "plain"), 1, "no attribute, no poll");

    plain.set_poll_interval(Some(Duration::from_secs(3)));
    h.advance_ms(3_000);
    assert_eq!(calls(&h, "plain"), 2);
    h.advance_ms(3_000);
    assert_eq!(calls(&h, "plain"), 3);
    plain.set_poll_interval(None);
    h.advance_ms(60_000);
    assert_eq!(calls(&h, "plain"), 3, "cleared: it stops");
}

#[test]
fn an_override_is_per_observer_and_goes_with_it() {
    let h = Harness::new();
    serve(&h, "fresh_plain");
    let a = observe::<FreshPlainQuery>(&h, ());
    let b = h.query().observe::<FreshPlainQuery>(());
    h.t.run_pending();
    a.set_poll_interval(Some(Duration::from_secs(2)));
    b.set_poll_interval(Some(Duration::from_secs(4)));
    drop(a);
    h.advance_ms(2_000);
    assert_eq!(calls(&h, "fresh_plain"), 1, "a's two seconds went with it");
    h.advance_ms(2_000);
    assert_eq!(calls(&h, "fresh_plain"), 2, "b's four");
    drop(b);
    h.advance_ms(60_000);
    assert_eq!(calls(&h, "fresh_plain"), 2);
}

#[test]
fn a_poll_interval_set_while_a_fetch_is_running_starts_no_second_fetch() {
    let h = Harness::new();
    serve(&h, "slow_tick");
    let slow = observe::<SlowTickQuery>(&h, ());
    assert!(slow.fetching().get());
    h.advance_ms(1_000);
    slow.set_poll_interval(Some(Duration::from_secs(1)));
    h.advance_ms(1_500);
    assert_eq!(calls(&h, "slow_tick"), 1, "still the first fetch");
    h.advance_ms(500); // t = 3 s: it ended; the interval is now one second
    h.advance_ms(999);
    assert_eq!(calls(&h, "slow_tick"), 1);
    h.advance_ms(1);
    assert_eq!(calls(&h, "slow_tick"), 2);
}

// ----- hostile intervals ---------------------------------------------------------------------------

#[test]
fn a_zero_interval_is_raised_to_the_floor_and_never_spins() {
    assert_eq!(MIN_POLL_INTERVAL_MS, 1_000);
    let h = Harness::new();
    serve(&h, "plain");
    let plain = observe::<PlainQuery>(&h, ());
    plain.set_poll_interval(Some(Duration::ZERO));
    h.advance_ms(999);
    assert_eq!(calls(&h, "plain"), 1);
    h.advance_ms(1);
    assert_eq!(calls(&h, "plain"), 2);
    // Sub-second values too, and a very small nonzero one.
    plain.set_poll_interval(Some(Duration::from_nanos(1)));
    h.advance_ms(1_000);
    assert_eq!(calls(&h, "plain"), 3);
    plain.set_poll_interval(Some(Duration::from_millis(999)));
    h.advance_ms(1_000);
    assert_eq!(calls(&h, "plain"), 4);
}

#[test]
fn a_huge_interval_is_lowered_to_the_ceiling_without_overflow() {
    assert_eq!(MAX_POLL_INTERVAL_MS, 7 * 24 * 60 * 60 * 1_000);
    let h = Harness::new();
    serve(&h, "plain");
    let plain = observe::<PlainQuery>(&h, ());
    plain.set_poll_interval(Some(Duration::MAX));
    h.advance_ms(24 * 60 * 60 * 1_000);
    assert_eq!(calls(&h, "plain"), 1, "a day in: no poll yet");
    plain.set_poll_interval(Some(Duration::from_secs(u64::MAX / 2)));
    h.advance_ms(6 * 24 * 60 * 60 * 1_000);
    assert_eq!(
        calls(&h, "plain"),
        2,
        "a week after the fetch ended: the ceiling"
    );
}

#[test]
fn a_storm_of_changes_neither_starves_nor_advances_the_poll() {
    let h = Harness::new();
    serve(&h, "tick");
    let tick = observe::<TickQuery>(&h, ());
    h.advance_ms(1_000);
    for i in 0..5_000_u64 {
        let seconds = if i % 2 == 0 { 3 } else { 7 };
        tick.set_poll_interval(Some(Duration::from_secs(seconds)));
        if i % 1_000 == 0 {
            tick.set_poll_interval(None);
        }
    }
    tick.set_poll_interval(Some(Duration::from_secs(3)));
    // The interval counts from the end of the last fetch (t = 0), not from the last change.
    h.advance_ms(1_999);
    assert_eq!(calls(&h, "tick"), 1, "t = 2.999 s");
    h.advance_ms(1);
    assert_eq!(calls(&h, "tick"), 2, "t = 3 s");
}

#[test]
fn many_observers_changing_and_releasing_in_turn_leave_the_right_schedule() {
    let h = Harness::new();
    serve(&h, "fresh_plain");
    let handles: Vec<_> = (0..40)
        .map(|_| h.query().observe::<FreshPlainQuery>(()))
        .collect();
    h.t.run_pending();
    for (n, handle) in handles.iter().enumerate() {
        handle.set_poll_interval(Some(Duration::from_secs(10 + n as u64)));
    }
    let mut handles = handles;
    // Release the fastest five, one by one, setting intervals on the others in between.
    for _ in 0..5 {
        let gone = handles.remove(0);
        drop(gone);
        handles[0].set_poll_interval(Some(Duration::from_secs(30)));
    }
    // 40 observers set 10 s, 11 s, .. 49 s; the first five went, each time the next in line was
    // set to 30 s: what is left has observer 5 at 30 s and observer 6 at 16 s, the smallest.
    assert_eq!(handles.len(), 35);
    h.advance_ms(15_999);
    assert_eq!(calls(&h, "fresh_plain"), 1);
    h.advance_ms(1);
    assert_eq!(calls(&h, "fresh_plain"), 2);
}

#[test]
fn a_hand_written_query_cannot_ask_for_a_zero_interval_either() {
    struct Spin;
    impl QueryDef for Spin {
        const ID: u32 = 0x5b1_0001;
        const KEY: &'static str = "spin";
        const STALE_MS: Option<u64> = None;
        const PERSIST: bool = false;
        const RETRY: u32 = 0;
        const INTERVAL_MS: Option<u64> = Some(0);
        type Params = ();
        type Output = u32;
        type Error = String;
        fn fetch(_: Ctx, _: ()) -> undra_query::BoxFuture<Result<u32, String>> {
            Box::pin(async { Ok(1) })
        }
    }
    let t = TestRuntime::new();
    let fakes = undra_ports::fakes::install(&t);
    let handle = t.ctx().query().observe::<Spin>(());
    t.run_pending();
    assert_eq!(handle.data().get(), Some(1));
    // A hot loop would never let the executor go idle; one second of fake time is one poll.
    fakes.advance(&t, Duration::from_millis(999));
    assert_eq!(handle.data().get(), Some(1));
    fakes.advance(&t, Duration::from_millis(1));
}

// ----- what a platform does ----------------------------------------------------------------------

struct Platform {
    h: Harness,
    next_call: std::cell::Cell<u32>,
}

impl Platform {
    fn new() -> Platform {
        Platform {
            h: Harness::new(),
            next_call: std::cell::Cell::new(1),
        }
    }

    fn call_id(&self) -> u32 {
        let id = self.next_call.get();
        self.next_call.set(id + 1);
        id
    }

    fn construct<Q: QueryDef>(&self) -> Handle {
        let reply = self.h.t.call_sync(
            CallTarget::Constructor {
                type_id: Q::ID,
                method_id: Q::ID,
            },
            self.call_id(),
            &[],
        );
        assert_eq!(reply.status, ReplyStatus::Ok);
        Handle::decode_exact(&reply.body).unwrap()
    }

    fn set_poll_interval(
        &self,
        handle: Handle,
        args: &[u8],
    ) -> undra::runtime::testing::ReplyRecord {
        self.h.t.call_sync(
            CallTarget::Method {
                handle,
                method_id: SET_POLL_INTERVAL_METHOD_ID,
            },
            self.call_id(),
            args,
        )
    }
}

#[test]
fn a_platform_sets_and_clears_the_interval_by_the_pinned_method_id() {
    let p = Platform::new();
    serve(&p.h, "plain");
    let handle = p.construct::<PlainQuery>();
    p.h.t.run_pending();
    assert_eq!(calls(&p.h, "plain"), 1);

    // `Some(Duration)`: option tag 1, then i64 nanoseconds.
    let some = Some(Duration::from_secs(2)).encode_to_vec();
    assert_eq!(some[0], 1);
    assert_eq!(p.set_poll_interval(handle, &some).status, ReplyStatus::Ok);
    p.h.advance_ms(2_000);
    assert_eq!(calls(&p.h, "plain"), 2);

    // `None` is one zero byte.
    let none = None::<Duration>.encode_to_vec();
    assert_eq!(none, [0]);
    assert_eq!(p.set_poll_interval(handle, &none).status, ReplyStatus::Ok);
    p.h.advance_ms(60_000);
    assert_eq!(calls(&p.h, "plain"), 2);
}

#[test]
fn a_platform_poll_reaches_the_host_as_an_ordinary_change_set() {
    let p = Platform::new();
    serve(&p.h, "tick");
    let handle = p.construct::<TickQuery>();
    p.h.t.run_pending();
    p.h.t.take_change_sets();
    p.h.t
        .runtime()
        .observe(handle.0, undra::signals::ALL_SIGNALS, true);
    p.h.t.host().take_decoded_change_sets();

    p.h.advance_ms(5_000);
    let sets = p.h.t.host().take_decoded_change_sets();
    // The poll started a fetch (`fetching` true) and finished it (`fetching` false): ordinary
    // writes of signals 3 and so on, in the frame-coalesced change-sets of the runtime.
    let fetching: Vec<&[u8]> = sets
        .iter()
        .flat_map(|s| s.entries.iter())
        .filter(|e| e.signal_id == 3 && e.op == ChangeOp::Full)
        .map(|e| &e.value[..])
        .collect();
    assert!(fetching.contains(&&[1][..]), "{sets:?}");
    assert!(fetching.contains(&&[0][..]), "{sets:?}");
}

#[test]
fn a_bad_argument_is_a_typed_error_and_a_released_handle_is_not_a_panic() {
    let p = Platform::new();
    serve(&p.h, "plain");
    let handle = p.construct::<PlainQuery>();
    // Too short, too long, a bad option tag, a negative duration.
    for bad in [
        &[][..],
        &[1][..],
        &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0][..],
        &[2][..],
        &[1, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff][..],
    ] {
        let reply = p.set_poll_interval(handle, bad);
        assert_ne!(reply.status, ReplyStatus::Ok, "{bad:?}");
    }
    p.h.advance_ms(60_000);
    assert_eq!(calls(&p.h, "plain"), 1, "none of them set anything");

    // The call races the release: the handle is gone.
    p.h.t.runtime().release(handle.0);
    let reply = p.set_poll_interval(handle, &Some(Duration::from_secs(1)).encode_to_vec());
    assert_ne!(reply.status, ReplyStatus::Ok);
    p.h.advance_ms(60_000);
    assert_eq!(calls(&p.h, "plain"), 1);
}

#[test]
fn set_poll_interval_calls_interleaved_with_releases_leave_nothing_behind() {
    let p = Platform::new();
    serve(&p.h, "plain");
    let handles: Vec<Handle> = (0..8).map(|_| p.construct::<PlainQuery>()).collect();
    p.h.t.run_pending();
    let every_second = Some(Duration::from_secs(1)).encode_to_vec();
    // Set, release, set (a stale handle), set again, in an order that mixes them.
    for (n, handle) in handles.iter().enumerate() {
        assert_eq!(
            p.set_poll_interval(*handle, &every_second).status,
            ReplyStatus::Ok
        );
        if n % 2 == 0 {
            p.h.t.runtime().release(handle.0);
            let _ = p.set_poll_interval(*handle, &every_second);
        }
    }
    // Four observers (the odd ones) poll at one second.
    let before = calls(&p.h, "plain");
    p.h.advance_ms(1_000);
    assert_eq!(calls(&p.h, "plain"), before + 1);
    for handle in handles.iter().skip(1).step_by(2) {
        p.h.t.runtime().release(handle.0);
    }
    p.h.advance_ms(600_000);
    assert_eq!(
        calls(&p.h, "plain"),
        before + 1,
        "every observer is gone: the last poll was the last"
    );
}
