//! The query client through its Rust API: fetching, staleness, dedup, retry, cancellation,
//! garbage collection, invalidation and the refetch triggers, on faked ports and a fake clock.

mod common;

use std::time::Duration;

use common::*;
use keel_ports::fakes::SeededRng;
use keel_ports::{AppState, Clock, HttpError, NetKind, Rng};
use keel_query::{Invalidate, QueryStatus, backoff_ms};
use keel_wire::Uuid;

fn network_down() -> HttpError {
    HttpError::Network("down".into())
}

/// The delays a runtime whose `Rng` is `SeededRng::default()` sleeps between retries.
fn expected_delays(retries: u32) -> Vec<u64> {
    let rng = SeededRng::default();
    (0..retries)
        .map(|attempt| {
            let bytes: [u8; 8] = rng.fill(8).0.try_into().unwrap();
            backoff_ms(attempt, u64::from_le_bytes(bytes))
        })
        .collect()
}

// ----- fetching ---------------------------------------------------------------------------

#[test]
fn a_fetch_succeeds_and_the_handle_shows_it() {
    let h = Harness::new();
    h.serve_page(0, vec![todo(1, "milk")]);
    let handle = h.query().observe::<TodosQuery>((0,));

    // Observing starts the fetch: there is nothing to show yet.
    assert_eq!(handle.status().get(), QueryStatus::Fetching);
    assert!(handle.fetching().get());
    assert_eq!(handle.data().get(), None);
    assert_eq!(handle.updated_at().get(), None);

    h.t.run_pending();
    assert_eq!(handle.data().get(), Some(page(vec![todo(1, "milk")])));
    assert_eq!(handle.status().get(), QueryStatus::Success);
    assert_eq!(handle.error().get(), None);
    assert!(!handle.fetching().get());
    assert_eq!(
        handle.updated_at().get().map(|t| t.0),
        Some(h.fakes.clock.now_ms())
    );
    assert_eq!(h.http_calls(), 1);
}

#[test]
fn a_failed_fetch_shows_the_error_and_keeps_stale_data() {
    let h = Harness::new();
    h.serve_page(0, vec![todo(1, "milk")]);
    let handle = h.query().observe::<TodosQuery>((0,));
    h.t.run_pending();
    assert_eq!(handle.status().get(), QueryStatus::Success);

    // The server starts failing; a forced refetch (no retries left to hide it: the query has the
    // default three, so let them all run).
    h.fakes.http.reset();
    h.fakes
        .http
        .fail(format!("{API}/todos?page=0"), network_down());
    handle.refetch();
    h.t.run_pending();
    for delay in expected_delays(3) {
        h.advance_ms(delay);
    }
    assert_eq!(handle.status().get(), QueryStatus::Error);
    assert_eq!(handle.error().get(), Some(TodoError::Http(network_down())));
    // The data from the last success is still there for the screen to show.
    assert_eq!(handle.data().get(), Some(page(vec![todo(1, "milk")])));
    assert!(!handle.fetching().get());
}

#[test]
fn a_success_clears_the_error() {
    let h = Harness::new();
    h.fakes.http.respond_sequence(
        format!("{API}/todos?page=0"),
        [
            Err(network_down()),
            Err(network_down()),
            Err(network_down()),
            Err(network_down()),
        ],
    );
    h.serve_page(0, vec![todo(2, "eggs")]);
    let handle = h.query().observe::<TodosQuery>((0,));
    h.t.run_pending();
    for delay in expected_delays(3) {
        h.advance_ms(delay);
    }
    assert_eq!(handle.status().get(), QueryStatus::Error);
    assert!(handle.error().get().is_some());

    handle.refetch();
    h.t.run_pending();
    assert_eq!(handle.status().get(), QueryStatus::Success);
    assert_eq!(handle.error().get(), None);
    assert_eq!(handle.data().get(), Some(page(vec![todo(2, "eggs")])));
}

#[test]
fn an_error_status_with_no_data_yet_shows_fetching_again_while_retrying_by_hand() {
    let h = Harness::new();
    h.fakes.http.fail(format!("{API}/settings"), network_down());
    let handle = h.query().observe::<SettingsQuery>(());
    h.t.run_pending();
    // `settings` has no retries: it failed at once.
    assert_eq!(handle.status().get(), QueryStatus::Error);
    assert_eq!(handle.data().get(), None);

    h.fakes.http.reset();
    h.fakes
        .http
        .respond(format!("{API}/settings"), ok(&"dark".to_owned()));
    handle.refetch();
    // No data yet, so the retry shows as `Fetching` (a spinner), and the old error stays until
    // the fetch settles.
    assert_eq!(handle.status().get(), QueryStatus::Fetching);
    assert!(handle.fetching().get());
    h.t.run_pending();
    assert_eq!(handle.data().get(), Some("dark".to_owned()));
    assert_eq!(handle.status().get(), QueryStatus::Success);
}

// ----- retry -------------------------------------------------------------------------------

#[test]
fn retries_back_off_with_seeded_jitter_inside_the_20_percent_envelope() {
    let h = Harness::new();
    h.fakes
        .http
        .fail(format!("{API}/hello/bob"), network_down());
    let handle = h.query().observe::<GreetingQuery>(("bob".to_owned(),));
    h.t.run_pending();
    assert_eq!(h.http_calls(), 1, "the first attempt is immediate");

    let delays = expected_delays(3);
    for (retry, (delay, nominal)) in delays.iter().zip([1_000_u64, 2_000, 4_000]).enumerate() {
        // The envelope of SPEC 9: nominal delay, plus or minus 20%.
        assert!(
            *delay >= nominal * 8 / 10 && *delay <= nominal * 12 / 10,
            "retry {retry}: {delay} ms is outside 80%..120% of {nominal}"
        );
        // Nothing happens until the delay is over, and the retry fires exactly at it.
        h.advance_ms(delay - 1);
        assert_eq!(h.http_calls(), retry + 1, "retry {retry} came early");
        assert!(handle.fetching().get());
        h.advance_ms(1);
        assert_eq!(
            h.http_calls(),
            retry + 2,
            "retry {retry} did not fire on time"
        );
    }

    // Three retries were the limit: the fourth failure is final.
    assert_eq!(h.http_calls(), 4);
    assert_eq!(handle.status().get(), QueryStatus::Error);
    assert!(!handle.fetching().get());
    h.advance_ms(120_000);
    assert_eq!(h.http_calls(), 4, "no fifth attempt");
}

#[test]
fn a_query_that_recovers_mid_retry_succeeds_without_showing_an_error() {
    let h = Harness::new();
    h.fakes.http.respond_sequence(
        format!("{API}/hello/ann"),
        [Err(network_down()), Err(network_down())],
    );
    h.fakes
        .http
        .respond(format!("{API}/hello/ann"), ok(&"hi ann".to_owned()));
    let handle = h.query().observe::<GreetingQuery>(("ann".to_owned(),));
    h.t.run_pending();
    for delay in expected_delays(2) {
        assert_eq!(
            handle.error().get(),
            None,
            "errors are only shown once the retries run out"
        );
        h.advance_ms(delay);
    }
    assert_eq!(handle.data().get(), Some("hi ann".to_owned()));
    assert_eq!(handle.status().get(), QueryStatus::Success);
    assert_eq!(h.http_calls(), 3);
}

#[test]
fn a_query_with_zero_retries_fails_at_once() {
    let h = Harness::new();
    h.fakes.http.fail(format!("{API}/settings"), network_down());
    let handle = h.query().observe::<SettingsQuery>(());
    h.t.run_pending();
    assert_eq!(handle.status().get(), QueryStatus::Error);
    assert_eq!(h.http_calls(), 1);
    h.advance_ms(60_000);
    assert_eq!(h.http_calls(), 1);
}

// ----- staleness ---------------------------------------------------------------------------

#[test]
fn observing_fresh_data_does_not_fetch_and_stale_data_refetches_while_showing_it() {
    let h = Harness::new();
    h.serve_page(0, vec![todo(1, "milk")]);
    let first = h.query().observe::<TodosQuery>((0,));
    h.t.run_pending();
    assert_eq!(h.http_calls(), 1);

    // Fresh (30 s window): a second observer sees the data at once and nothing is fetched.
    let second = h.query().observe::<TodosQuery>((0,));
    assert_eq!(second.data().get(), Some(page(vec![todo(1, "milk")])));
    assert_eq!(second.status().get(), QueryStatus::Success);
    assert!(!second.fetching().get());
    h.t.run_pending();
    h.advance_ms(29_999);
    let third = h.query().observe::<TodosQuery>((0,));
    h.t.run_pending();
    assert_eq!(
        h.http_calls(),
        1,
        "still fresh one millisecond before the window ends"
    );

    // The window is over: the next observer triggers a refetch, and the old data stays on
    // screen (`Success`, with `fetching` set) while it runs.
    h.advance_ms(1);
    h.fakes.http.reset(); // the server now has more to say (and the call count starts over)
    h.serve_page(0, vec![todo(1, "milk"), todo(2, "eggs")]);
    let fourth = h.query().observe::<TodosQuery>((0,));
    assert_eq!(fourth.status().get(), QueryStatus::Success);
    assert!(fourth.fetching().get());
    assert!(
        first.fetching().get(),
        "every observer of the entry sees the refetch"
    );
    assert_eq!(fourth.data().get(), Some(page(vec![todo(1, "milk")])));
    h.t.run_pending();
    assert_eq!(h.http_calls(), 1, "one refetch, however many observers");
    assert_eq!(first.data().get().unwrap().items.len(), 2);
    assert_eq!(third.data().get().unwrap().items.len(), 2);
    assert!(!fourth.fetching().get());
    assert_eq!(
        fourth.updated_at().get().map(|t| t.0),
        Some(h.fakes.clock.now_ms())
    );
}

#[test]
fn a_query_without_a_staleness_window_is_always_stale() {
    let h = Harness::new();
    h.fakes
        .http
        .respond(format!("{API}/hello/x"), ok(&"one".to_owned()));
    let a = h.query().observe::<GreetingQuery>(("x".to_owned(),));
    h.t.run_pending();
    let b = h.query().observe::<GreetingQuery>(("x".to_owned(),));
    h.t.run_pending();
    assert_eq!(
        h.http_calls(),
        2,
        "every new observer of an always-stale entry refetches"
    );
    drop((a, b));
}

#[test]
fn different_parameters_are_different_entries() {
    let h = Harness::new();
    h.serve_page(0, vec![todo(1, "milk")]);
    h.serve_page(1, vec![todo(2, "eggs")]);
    let query = h.query();
    let p0 = query.observe::<TodosQuery>((0,));
    let p1 = query.observe::<TodosQuery>((1,));
    h.t.run_pending();
    assert_eq!(p0.data().get(), Some(page(vec![todo(1, "milk")])));
    assert_eq!(p1.data().get(), Some(page(vec![todo(2, "eggs")])));
    assert_eq!(query.cached_entries(), 2);
    assert_eq!(
        query.get::<TodosQuery>((1,)),
        Some(page(vec![todo(2, "eggs")]))
    );
    assert_eq!(query.get::<TodosQuery>((2,)), None);
}

#[test]
fn identical_data_from_a_refetch_keeps_the_data_and_bumps_updated_at() {
    let h = Harness::new();
    h.serve_page(0, vec![todo(1, "milk")]);
    let handle = h.query().observe::<TodosQuery>((0,));
    h.t.run_pending();
    let first = handle.updated_at().get().unwrap();

    h.advance_ms(40_000);
    handle.refetch();
    h.t.run_pending();
    assert_eq!(handle.data().get(), Some(page(vec![todo(1, "milk")])));
    assert_eq!(handle.updated_at().get().unwrap().0, first.0 + 40_000);
}

// ----- dedup and cancellation --------------------------------------------------------------

#[test]
fn concurrent_observers_share_one_in_flight_fetch() {
    let h = Harness::new();
    h.fakes
        .http
        .respond(format!("{API}/slow?ms=500"), ok(&7_u32));
    let query = h.query();
    let a = query.observe::<SlowQuery>((500,));
    let b = query.observe::<SlowQuery>((500,));
    h.t.run_pending();
    assert_eq!(h.http_calls(), 1);
    h.advance_ms(200);
    // A third observer joins the fetch that is already running.
    let c = query.observe::<SlowQuery>((500,));
    h.t.run_pending();
    assert_eq!(h.http_calls(), 1, "observers share the in-flight fetch");
    for handle in [&a, &b, &c] {
        assert!(handle.fetching().get());
        assert_eq!(handle.data().get(), None);
    }
    h.advance_ms(299);
    assert_eq!(a.data().get(), None);
    h.advance_ms(1);
    for handle in [&a, &b, &c] {
        assert_eq!(handle.data().get(), Some(7));
        assert_eq!(handle.status().get(), QueryStatus::Success);
    }
    assert_eq!(h.http_calls(), 1);
}

#[test]
fn the_last_observer_leaving_cancels_the_fetch() {
    let h = Harness::new();
    h.fakes
        .http
        .respond(format!("{API}/slow?ms=500"), ok(&7_u32));
    let query = h.query();
    let handle = query.observe::<SlowQuery>((500,));
    h.t.run_pending();
    h.advance_ms(100);
    assert_eq!(h.http_calls(), 1);

    drop(handle);
    // The fetch was cancelled: its answer never lands in the cache.
    h.advance_ms(1_000);
    assert_eq!(query.get::<SlowQuery>((500,)), None);
    assert_eq!(
        query.cached_entries(),
        1,
        "the (empty) entry waits to be garbage collected"
    );

    // Observing again starts a fresh fetch.
    let again = query.observe::<SlowQuery>((500,));
    assert_eq!(again.status().get(), QueryStatus::Fetching);
    h.t.run_pending();
    h.advance_ms(500);
    assert_eq!(h.http_calls(), 2);
    assert_eq!(again.data().get(), Some(7));
}

#[test]
fn one_observer_leaving_does_not_cancel_the_fetch_for_the_others() {
    let h = Harness::new();
    h.fakes
        .http
        .respond(format!("{API}/slow?ms=500"), ok(&7_u32));
    let query = h.query();
    let a = query.observe::<SlowQuery>((500,));
    let b = query.observe::<SlowQuery>((500,));
    h.t.run_pending();
    h.advance_ms(100);
    drop(a);
    h.advance_ms(400);
    assert_eq!(b.data().get(), Some(7));
    assert_eq!(h.http_calls(), 1);
}

// ----- garbage collection ------------------------------------------------------------------

#[test]
fn an_unobserved_entry_is_collected_after_five_minutes() {
    let h = Harness::new();
    h.fakes
        .http
        .respond(format!("{API}/settings"), ok(&"dark".to_owned()));
    let query = h.query();
    let handle = query.observe::<SettingsQuery>(());
    h.t.run_pending();
    drop(handle);
    assert_eq!(query.cached_entries(), 1);

    h.advance_ms(299_999);
    assert_eq!(
        query.cached_entries(),
        1,
        "still cached one millisecond early"
    );
    // Observing again inside the window reuses the entry: it is fresh (1 h), so no fetch, and
    // the collection timer is cancelled.
    let again = query.observe::<SettingsQuery>(());
    assert_eq!(again.data().get(), Some("dark".to_owned()));
    h.t.run_pending();
    assert_eq!(h.http_calls(), 1);
    h.advance_ms(600_000);
    assert_eq!(
        query.cached_entries(),
        1,
        "an observed entry is never collected"
    );

    // Released again, the clock starts over.
    drop(again);
    h.advance_ms(299_999);
    assert_eq!(query.cached_entries(), 1);
    h.advance_ms(1);
    assert_eq!(query.cached_entries(), 0);
    assert_eq!(query.get::<SettingsQuery>(()), None);

    // Gone from memory, so the next observer fetches again.
    let fresh = query.observe::<SettingsQuery>(());
    assert_eq!(fresh.status().get(), QueryStatus::Fetching);
    h.t.run_pending();
    assert_eq!(h.http_calls(), 2);
}

#[test]
fn the_collection_time_is_configurable() {
    let h = Harness::new();
    h.fakes
        .http
        .respond(format!("{API}/settings"), ok(&"dark".to_owned()));
    let query = h.query();
    query.set_gc_time(Duration::from_secs(2));
    let handle = query.observe::<SettingsQuery>(());
    h.t.run_pending();
    drop(handle);
    h.advance_ms(1_999);
    assert_eq!(query.cached_entries(), 1);
    h.advance_ms(1);
    assert_eq!(query.cached_entries(), 0);
}

// ----- invalidation ------------------------------------------------------------------------

/// Observes `todos` pages 0 and 1 and a greeting; returns the handles and lets them settle.
fn three_observed(
    h: &Harness,
) -> (
    keel_query::QueryHandle<TodosQuery>,
    keel_query::QueryHandle<TodosQuery>,
    keel_query::QueryHandle<GreetingQuery>,
) {
    h.serve_page(0, vec![todo(1, "milk")]);
    h.serve_page(1, vec![todo(2, "eggs")]);
    h.fakes
        .http
        .respond(format!("{API}/hello/a"), ok(&"hi".to_owned()));
    let query = h.query();
    let p0 = query.observe::<TodosQuery>((0,));
    let p1 = query.observe::<TodosQuery>((1,));
    let g = query.observe::<GreetingQuery>(("a".to_owned(),));
    h.t.run_pending();
    assert_eq!(h.http_calls(), 3);
    h.fakes.http.take_calls();
    (p0, p1, g)
}

fn urls(h: &Harness) -> Vec<String> {
    let mut urls: Vec<String> = h
        .fakes
        .http
        .take_calls()
        .into_iter()
        .map(|r| r.url)
        .collect();
    urls.sort();
    urls
}

#[test]
fn invalidating_a_prefix_refetches_every_observed_entry_it_matches() {
    let h = Harness::new();
    let (_p0, _p1, _g) = three_observed(&h);

    h.query().invalidate("todos");
    h.t.run_pending();
    assert_eq!(
        urls(&h),
        [format!("{API}/todos?page=0"), format!("{API}/todos?page=1")],
        "both pages refetch, the greeting does not"
    );

    // A longer prefix narrows it to one page (`todos:1` is the rendered key of page 1).
    h.query().invalidate("todos:1");
    h.t.run_pending();
    assert_eq!(urls(&h), [format!("{API}/todos?page=1")]);

    // The empty prefix is everything.
    h.query().invalidate("");
    h.t.run_pending();
    assert_eq!(urls(&h).len(), 3);

    // A prefix nothing has changes nothing.
    h.query().invalidate("todo-list");
    h.t.run_pending();
    assert!(urls(&h).is_empty());
}

#[test]
fn typed_invalidation_targets_a_query_or_one_entry() {
    let h = Harness::new();
    let (_p0, _p1, _g) = three_observed(&h);

    h.query().invalidate(Invalidate::query::<TodosQuery>());
    h.t.run_pending();
    assert_eq!(urls(&h).len(), 2);

    h.query().invalidate(Invalidate::exact::<TodosQuery>(&(0,)));
    h.t.run_pending();
    assert_eq!(urls(&h), [format!("{API}/todos?page=0")]);

    h.query()
        .invalidate(Invalidate::exact::<GreetingQuery>(&("a".to_owned(),)));
    h.t.run_pending();
    assert_eq!(urls(&h), [format!("{API}/hello/a")]);
}

#[test]
fn an_unobserved_entry_is_marked_stale_and_fetches_when_next_observed() {
    let h = Harness::new();
    let (p0, p1, _g) = three_observed(&h);
    drop(p1);

    h.query().invalidate("todos");
    h.t.run_pending();
    assert_eq!(
        urls(&h),
        [format!("{API}/todos?page=0")],
        "only the observed page refetches"
    );

    // Page 1 is inside its 30 s window but was invalidated, so observing it fetches.
    let again = h.query().observe::<TodosQuery>((1,));
    assert_eq!(
        again.data().get(),
        Some(page(vec![todo(2, "eggs")])),
        "the old data shows meanwhile"
    );
    assert!(again.fetching().get());
    h.t.run_pending();
    assert_eq!(urls(&h), [format!("{API}/todos?page=1")]);
    drop(p0);
}

#[test]
fn invalidation_restarts_a_fetch_that_is_in_flight() {
    let h = Harness::new();
    h.fakes
        .http
        .respond(format!("{API}/slow?ms=500"), ok(&7_u32));
    let handle = h.query().observe::<SlowQuery>((500,));
    h.t.run_pending();
    h.advance_ms(250);
    assert_eq!(h.http_calls(), 1);

    // Its answer might predate whatever changed, so the running fetch is replaced.
    h.query().invalidate("slow");
    h.t.run_pending();
    assert_eq!(h.http_calls(), 2);
    h.advance_ms(250); // the original would have finished now
    assert_eq!(handle.data().get(), None);
    h.advance_ms(250);
    assert_eq!(handle.data().get(), Some(7));
    assert_eq!(h.http_calls(), 2);
}

#[test]
fn the_handle_can_invalidate_and_refetch_itself() {
    let h = Harness::new();
    h.fakes
        .http
        .respond(format!("{API}/settings"), ok(&"dark".to_owned()));
    let handle = h.query().observe::<SettingsQuery>(());
    h.t.run_pending();
    assert_eq!(h.http_calls(), 1);

    // Fresh (an hour): `refetch` fetches anyway.
    handle.refetch();
    h.t.run_pending();
    assert_eq!(h.http_calls(), 2);
    // A refetch while one is running joins it.
    handle.refetch();
    handle.refetch();
    h.t.run_pending();
    assert_eq!(h.http_calls(), 3);

    handle.invalidate();
    h.t.run_pending();
    assert_eq!(h.http_calls(), 4);
}

// ----- rendered keys -----------------------------------------------------------------------

#[test]
fn parameters_are_spliced_into_the_key_a_prefix_is_matched_against() {
    let h = Harness::new();
    let id = Uuid([0xab; 16]);
    let text = format!("todo:{id}");
    assert_eq!(text, "todo:abababab-abab-abab-abab-abababababab");
    h.fakes.http.respond(
        keel_ports::fakes::Matcher::url_prefix(format!("{API}/todos/{id}")),
        ok(&todo(0xab, "milk")),
    );
    let fresh = h.query().observe::<TodoByIdQuery>((id, true));
    let plain = h.query().observe::<TodoByIdQuery>((id, false));
    let other = h.query().observe::<TodoByIdQuery>((Uuid([1; 16]), false));
    h.t.run_pending();
    h.fakes.http.take_calls();

    h.query().invalidate(text);
    h.t.run_pending();
    assert_eq!(
        h.fakes.http.take_calls().len(),
        2,
        "both entries of that id, but not the other todo"
    );
    drop((fresh, plain, other));
}

// ----- triggers ----------------------------------------------------------------------------

#[test]
fn becoming_active_refetches_observed_entries_that_are_stale() {
    let h = Harness::new();
    h.fakes
        .http
        .respond(format!("{API}/hello/x"), ok(&"hi".to_owned()));
    h.fakes
        .http
        .respond(format!("{API}/settings"), ok(&"dark".to_owned()));
    let query = h.query();
    let always_stale = query.observe::<GreetingQuery>(("x".to_owned(),));
    let fresh = query.observe::<SettingsQuery>(());
    h.t.run_pending();
    h.fakes.http.take_calls();

    h.fakes.lifecycle.set(AppState::Background);
    h.fakes.lifecycle.set(AppState::Inactive);
    h.t.run_pending();
    assert!(urls(&h).is_empty(), "only Active is a trigger");

    h.fakes.lifecycle.set(AppState::Active);
    h.t.run_pending();
    assert_eq!(
        urls(&h),
        [format!("{API}/hello/x")],
        "the fresh entry stays put"
    );
    drop((always_stale, fresh));
}

#[test]
fn coming_back_online_refetches_everything_observed() {
    let h = Harness::new();
    h.fakes
        .http
        .respond(format!("{API}/settings"), ok(&"dark".to_owned()));
    let query = h.query();
    let fresh = query.observe::<SettingsQuery>(());
    h.t.run_pending();
    h.fakes.http.take_calls();
    assert!(
        query.is_online(),
        "the client assumes it is online until told otherwise"
    );

    h.fakes.connectivity.go_offline();
    h.t.run_pending();
    assert!(!query.is_online());
    assert!(urls(&h).is_empty(), "going offline fetches nothing");

    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.t.run_pending();
    assert!(query.is_online());
    assert_eq!(
        urls(&h),
        [format!("{API}/settings")],
        "fresh or not, everything observed refetches"
    );

    // Moving between networks while online is not "coming online".
    h.fakes.connectivity.set(true, NetKind::Cellular);
    h.t.run_pending();
    assert!(urls(&h).is_empty());
    drop(fresh);
}

#[test]
fn unobserved_entries_are_not_refetched_by_triggers() {
    let h = Harness::new();
    h.fakes
        .http
        .respond(format!("{API}/settings"), ok(&"dark".to_owned()));
    let handle = h.query().observe::<SettingsQuery>(());
    h.t.run_pending();
    drop(handle);
    h.fakes.http.take_calls();

    h.fakes.connectivity.go_offline();
    h.fakes.connectivity.go_online(NetKind::Wifi);
    h.fakes.lifecycle.set(AppState::Active);
    h.t.run_pending();
    assert!(urls(&h).is_empty());
}

// ----- the cache as data -------------------------------------------------------------------

#[test]
fn set_and_get_read_and_write_the_cache_directly() {
    let h = Harness::new();
    let query = h.query();
    assert_eq!(query.get::<SettingsQuery>(()), None);
    query.set::<SettingsQuery>((), "light".to_owned());
    assert_eq!(query.get::<SettingsQuery>(()), Some("light".to_owned()));

    // An observer of a fresh entry (an hour) sees it and does not fetch.
    let handle = query.observe::<SettingsQuery>(());
    assert_eq!(handle.data().get(), Some("light".to_owned()));
    h.t.run_pending();
    assert_eq!(h.http_calls(), 0);

    // A write reaches observers at once.
    query.set::<SettingsQuery>((), "dark".to_owned());
    assert_eq!(handle.data().get(), Some("dark".to_owned()));
}

#[test]
fn settled_resolves_when_the_fetch_is_over() {
    let h = Harness::new();
    h.fakes
        .http
        .respond(format!("{API}/hello/z"), ok(&"hi".to_owned()));
    let handle = h.query().observe::<GreetingQuery>(("z".to_owned(),));
    assert!(handle.fetching().get());
    h.t.run_until(handle.settled());
    assert_eq!(handle.data().get(), Some("hi".to_owned()));
    // Nothing in flight: it is ready at once.
    h.t.run_until(handle.settled());
}

// ----- robustness --------------------------------------------------------------------------

#[test]
fn a_panicking_query_becomes_an_error_status_and_the_runtime_carries_on() {
    let h = Harness::new();
    let handle = h.query().observe::<ExplodeQuery>(());
    h.t.run_pending();
    assert_eq!(handle.status().get(), QueryStatus::Error);
    assert_eq!(handle.error().get(), None, "a panic has no typed error");
    assert!(!handle.fetching().get(), "the handle is not stuck fetching");

    // Everything else still works, and the broken entry can be tried again.
    h.serve_page(0, vec![todo(1, "milk")]);
    let other = h.query().observe::<TodosQuery>((0,));
    h.t.run_pending();
    assert_eq!(other.status().get(), QueryStatus::Success);
    handle.refetch();
    h.t.run_pending();
    assert_eq!(handle.status().get(), QueryStatus::Error);
}

#[test]
fn dropping_a_handle_after_the_runtime_shut_down_is_harmless() {
    let h = Harness::new();
    h.fakes
        .http
        .respond(format!("{API}/settings"), ok(&"dark".to_owned()));
    let handle = h.query().observe::<SettingsQuery>(());
    h.t.runtime().shutdown();
    drop(handle);
}

#[test]
fn nothing_keeps_a_runtime_alive_once_the_app_lets_go_of_it() {
    // The cache lives in the runtime; its tasks, timers, subscriptions and handles must not
    // hold the runtime back (no reference cycle through the extension or the event table).
    let weak = {
        let h = Harness::new();
        h.fakes
            .http
            .respond(format!("{API}/settings"), ok(&"dark".to_owned()));
        h.fakes
            .http
            .respond(format!("{API}/slow?ms=500"), ok(&7_u32));
        let settings = h.query().observe::<SettingsQuery>(());
        let slow = h.query().observe::<SlowQuery>((500,));
        h.t.run_pending();
        h.advance_ms(100);
        drop(settings);
        let weak = std::sync::Arc::downgrade(h.t.runtime());
        drop(slow);
        weak
        // `h` (and with it the TestRuntime) is dropped here, with a gc timer, a persist task
        // and an event subscription still registered.
    };
    assert!(weak.upgrade().is_none(), "the runtime leaked");
}

#[test]
fn handle_signals_compose_with_computed_and_effects_in_the_core() {
    use keel::prelude::{Computed, Effect};
    use std::sync::{Arc, Mutex};

    let h = Harness::new();
    h.serve_page(0, vec![todo(1, "milk"), todo(2, "eggs")]);
    let handle = h.query().observe::<TodosQuery>((0,));
    // What a store in the core does: derive state from a query.
    let count = Computed::new(handle.data(), |page| {
        page.as_ref().map_or(0_u32, |p| p.items.len() as u32)
    });
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let _effect = Effect::new(&count, move |n| sink.lock().unwrap().push(*n));
    assert_eq!(count.get(), 0);

    h.t.run_pending();
    assert_eq!(count.get(), 2);
    assert_eq!(
        *seen.lock().unwrap(),
        [2],
        "the effect ran once, for the fetch"
    );
}
