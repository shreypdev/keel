//! Query handles across a restore (ADR-059), driven the way a platform drives them (a constructor
//! call, `Observe`, `refetch` by method id, `Release`) with no host code in between: a dev reload
//! (a fresh runtime, the old snapshot), a web crash restart (the same), a time travel and an app's
//! own `restore` on a live core (the same runtime).

mod common;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use common::*;
use parking_lot::Mutex;
use undra::meta::ids;
use undra::prelude::*;
use undra::runtime::Runtime;
use undra::runtime::testing::{ReplyRecord, TestRuntime};
use undra::signals::ALL_SIGNALS;
use undra::wire::payload::{CallTarget, ChangeOp, ChangeSet, ReplyStatus, Snapshot, SnapshotType};
use undra_ports::fakes::{Fakes, Matcher};
use undra_ports::{HttpRequest, HttpResponse};
use undra_query::{
    FETCH_NEXT_PAGE_METHOD_ID, QueryDef, REFETCH_METHOD_ID, SET_POLL_INTERVAL_METHOD_ID,
};
use undra_wire::{Decode, Encode, Handle, KeyedPatch, Reader, Writer};

const DATA: u32 = 0;
const STATUS: u32 = 1;
const FETCHING: u32 = 3;
const HAS_NEXT_PAGE: u32 = 5;

// ----- queries of this test core ----------------------------------------------------------------

/// A query that polls: the core fetches it again a second after the last fetch ended.
#[undra::query(key = "tick", interval = "1s", retry = 0)]
pub async fn tick(ctx: &Ctx) -> Result<u32, TodoError> {
    let response = ctx
        .http()
        .request(HttpRequest::get(format!("{API}/tick")))
        .await?;
    u32::decode_exact(&response.body.0).map_err(|_| TodoError::BadResponse)
}

#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Post {
    pub id: u64,
    pub title: String,
}

#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub struct FeedBody {
    pub items: Vec<Post>,
    pub next: Option<String>,
}

async fn get_feed(ctx: &Ctx, tag: &str, cursor: Option<String>) -> Result<FeedBody, TodoError> {
    let after = cursor.map(|c| format!("&after={c}")).unwrap_or_default();
    let response = ctx
        .http()
        .request(HttpRequest::get(format!("{API}/feed?tag={tag}{after}")))
        .await?;
    FeedBody::decode_exact(&response.body.0).map_err(|_| TodoError::BadResponse)
}

/// An infinite query that is not persisted.
#[undra::query(key = "feed/{tag}", infinite, item_key = "id", stale = "1m", retry = 0)]
pub async fn feed(
    ctx: &Ctx,
    tag: String,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<undra::query::Page<Post, String>, TodoError> {
    let body = get_feed(ctx, &tag, cursor).await?;
    Ok(undra::query::Page::new(body.items, body.next))
}

/// An infinite query that persists its first two pages.
#[undra::query(
    key = "capped/{tag}",
    infinite,
    item_key = "id",
    stale = "1m",
    retry = 0,
    persist,
    persist_pages = 2
)]
pub async fn capped(
    ctx: &Ctx,
    tag: String,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<undra::query::Page<Post, String>, TodoError> {
    let body = get_feed(ctx, &tag, cursor).await?;
    Ok(undra::query::Page::new(body.items, body.next))
}

/// Posts newest first; a page is the posts with an id below the cursor, `page` of them.
fn serve_feed(h: &Harness, n: u64, page: usize) {
    let posts: Arc<Vec<Post>> = Arc::new(
        (1..=n)
            .rev()
            .map(|id| Post {
                id,
                title: format!("post {id}"),
            })
            .collect(),
    );
    h.fakes
        .http
        .respond_with(Matcher::url_prefix(format!("{API}/feed")), move |request| {
            let after = request
                .url
                .split("after=")
                .nth(1)
                .and_then(|rest| rest.split('&').next())
                .and_then(|id| id.parse::<u64>().ok());
            let rest: Vec<Post> = posts
                .iter()
                .filter(|p| after.is_none_or(|id| p.id < id))
                .cloned()
                .collect();
            let items: Vec<Post> = rest.iter().take(page).cloned().collect();
            let next = (rest.len() > items.len())
                .then(|| items.last().map(|p| p.id.to_string()))
                .flatten();
            Ok(HttpResponse::new(
                200,
                FeedBody { items, next }.encode_to_vec(),
            ))
        });
}

// ----- the platform ---------------------------------------------------------------------------------

/// What a host mirrors: the last value of every signal it was sent, and the rows of an infinite
/// handle's list (patched as a host patches them).
#[derive(Default)]
struct Mirror {
    values: BTreeMap<(u64, u32), Vec<u8>>,
    rows: BTreeMap<u64, Vec<Post>>,
}

struct Platform {
    h: Harness,
    next_call: Cell<u32>,
    mirror: Mutex<Mirror>,
}

impl Platform {
    fn new() -> Platform {
        Platform::on(Harness::new())
    }

    fn on(h: Harness) -> Platform {
        Platform {
            h,
            next_call: Cell::new(1),
            mirror: Mutex::new(Mirror::default()),
        }
    }

    fn t(&self) -> &TestRuntime {
        &self.h.t
    }

    fn rt(&self) -> &Runtime {
        self.h.t.runtime()
    }

    fn call_id(&self) -> u32 {
        let id = self.next_call.get();
        self.next_call.set(id + 1);
        id
    }

    fn construct<Q: QueryDef>(&self, params: &impl Encode) -> Handle {
        let reply = self.t().call_sync(
            CallTarget::Constructor {
                type_id: Q::ID,
                method_id: Q::ID,
            },
            self.call_id(),
            &params.encode_to_vec(),
        );
        assert_eq!(reply.status, ReplyStatus::Ok, "constructor: {reply:?}");
        Handle::decode_exact(&reply.body).unwrap()
    }

    fn method(&self, handle: Handle, method_id: u32, args: &[u8]) -> ReplyRecord {
        self.t().call_sync(
            CallTarget::Method { handle, method_id },
            self.call_id(),
            args,
        )
    }

    /// Observes every signal of `handle`; returns the change-sets that took.
    fn observe(&self, handle: Handle) -> Vec<ChangeSet> {
        self.take();
        self.rt().observe(handle.0, ALL_SIGNALS, true);
        self.take()
    }

    /// The change-sets delivered since, applied to the mirror.
    fn take(&self) -> Vec<ChangeSet> {
        let sets = self.t().host().take_decoded_change_sets();
        let mut mirror = self.mirror.lock();
        for set in &sets {
            for entry in &set.entries {
                let key = (entry.handle.0, entry.signal_id);
                match entry.op {
                    ChangeOp::Full if entry.signal_id == DATA && entry.value.len() > 4 => {
                        // The rows of an infinite handle, when they decode as such; a todos page
                        // is kept as bytes below.
                        if let Ok(rows) = Vec::<Post>::decode_exact(&entry.value) {
                            mirror.rows.insert(entry.handle.0, rows);
                        }
                        mirror.values.insert(key, entry.value.clone());
                    }
                    ChangeOp::Full => {
                        if entry.signal_id == DATA {
                            mirror.rows.insert(entry.handle.0, Vec::new());
                        }
                        mirror.values.insert(key, entry.value.clone());
                    }
                    ChangeOp::KeyedPatch => {
                        let mut r = Reader::new(&entry.value);
                        let patch = KeyedPatch::<Post>::decode(&mut r).unwrap();
                        patch
                            .apply(mirror.rows.entry(entry.handle.0).or_default())
                            .unwrap();
                    }
                    _ => {}
                }
            }
        }
        sets
    }

    fn value(&self, handle: Handle, signal: u32) -> Option<Vec<u8>> {
        self.mirror.lock().values.get(&(handle.0, signal)).cloned()
    }

    fn rows(&self, handle: Handle) -> Vec<u64> {
        self.mirror
            .lock()
            .rows
            .get(&handle.0)
            .map(|rows| rows.iter().map(|p| p.id).collect())
            .unwrap_or_default()
    }

    fn stat(&self, key: &str) -> u64 {
        let stats: serde_json::Value = serde_json::from_str(&self.rt().stats_json()).unwrap();
        stats[key]
            .as_u64()
            .unwrap_or_else(|| panic!("{key}: {stats}"))
    }

    fn snapshot(&self) -> Vec<u8> {
        self.rt().snapshot()
    }

    /// Whether the last `status` the host was sent says "fetching" (`1`).
    fn fetching(&self, handle: Handle) -> Option<bool> {
        self.value(handle, FETCHING).map(|v| v == [1])
    }
}

fn status_of(sets: &[ChangeSet], handle: Handle) -> Option<Vec<u8>> {
    sets.iter()
        .flat_map(|cs| cs.entries.iter())
        .filter(|e| e.handle == handle && e.signal_id == STATUS)
        .map(|e| e.value.clone())
        .next_back()
}

fn titles(bytes: &[u8]) -> Vec<String> {
    Option::<Page>::decode_exact(bytes)
        .unwrap()
        .map(|p| p.items.into_iter().map(|t| t.title).collect())
        .unwrap_or_default()
}

fn titles_of(p: &Platform, handle: Handle) -> Option<Vec<String>> {
    p.value(handle, DATA).map(|bytes| titles(&bytes))
}

fn records(snapshot: &[u8]) -> Vec<(Handle, u32)> {
    let decoded = Snapshot::decode(&mut Reader::new(snapshot)).unwrap();
    decoded
        .stores
        .iter()
        .filter(|s| s.recreation().is_some())
        .map(|s| (s.handle, s.type_id))
        .collect()
}

/// `snapshot` with its records re-encoded by `edit`.
fn edit_snapshot(snapshot: &[u8], edit: impl FnOnce(&mut Snapshot)) -> Vec<u8> {
    let mut decoded = Snapshot::decode(&mut Reader::new(snapshot)).unwrap();
    edit(&mut decoded);
    let mut w = Writer::new();
    decoded.encode(&mut w);
    w.into_vec()
}

// ----- a fresh runtime: a dev reload, a web crash restart, a cold start -------------------------------

/// The Remote tab across a dev reload or a web crash restart: a new runtime, the old snapshot, the
/// same handle value, and a client that only observes again.
#[test]
fn a_fresh_runtime_re_issues_the_handle_and_builds_it_when_the_host_first_uses_it() {
    let old = Platform::new();
    old.h.serve_page(1, vec![todo(1, "milk")]);
    let handle = old.construct::<TodosQuery>(&1_u32);
    old.observe(handle);
    old.h.settle();
    old.take();
    assert_eq!(titles_of(&old, handle).unwrap(), ["milk"]);
    let snapshot = old.snapshot();
    assert_eq!(records(&snapshot), [(handle, TodosQuery::ID)]);

    // The new process: nothing in memory, the snapshot restored before any client attaches, and
    // the server now answers differently (the rebuilt code).
    let new = Platform::new();
    new.h.settle();
    new.h.serve_page(1, vec![todo(1, "milk"), todo(2, "eggs")]);
    let report = new.rt().restore_with_report(&snapshot).unwrap();
    assert_eq!(
        (report.restored, report.reissued, report.refused.len()),
        (0, 1, 0)
    );
    new.h.settle();
    assert_eq!(new.stat("dormant_handles"), 1);
    assert_eq!(new.stat("live_handles"), 1);
    assert_eq!(
        new.h.http_calls(),
        0,
        "a re-issued handle nobody uses does nothing"
    );

    // The client is back: it observes what it observed, as after any reconnect (ADR-051).
    let sets = new.observe(handle);
    assert_eq!(new.stat("dormant_handles"), 0, "built on first use");
    assert_eq!(
        status_of(&sets, handle),
        Some(vec![1, 0]),
        "the observe answers with the handle's values: fetching, nothing cached ({sets:?})"
    );
    assert_eq!(new.fetching(handle), Some(true));
    new.h.settle();
    new.take();
    assert_eq!(
        titles_of(&new, handle).unwrap(),
        ["milk", "eggs"],
        "fetched by the new core, delivered to the same handle"
    );
    assert_eq!(new.h.http_calls(), 1);

    // `refetch` is accepted (it was status 5 before ADR-059).
    assert_eq!(
        new.method(handle, REFETCH_METHOD_ID, &[]).status,
        ReplyStatus::Ok
    );
    new.h.settle();
    assert_eq!(new.h.http_calls(), 2);

    // Releasing it removes the observer, as before.
    let live = new.stat("live_handles");
    new.rt().release(handle.0);
    assert_eq!(new.stat("live_handles"), live - 1);
    assert_eq!(
        new.method(handle, REFETCH_METHOD_ID, &[]).status,
        ReplyStatus::BadRequest
    );
}

#[test]
fn the_observers_own_polling_interval_comes_back_with_the_handle() {
    let old = Platform::new();
    old.h.serve_page(1, vec![todo(1, "milk")]);
    let handle = old.construct::<TodosQuery>(&1_u32);
    old.observe(handle);
    old.h.settle();
    // This observer polls every 5 s (the query declares no interval).
    let five_seconds = Some(Duration::from_secs(5)).encode_to_vec();
    assert_eq!(
        old.method(handle, SET_POLL_INTERVAL_METHOD_ID, &five_seconds)
            .status,
        ReplyStatus::Ok
    );
    let snapshot = old.snapshot();

    let new = Platform::new();
    new.h.serve_page(1, vec![todo(1, "milk")]);
    new.rt().restore(&snapshot).unwrap();
    new.observe(handle);
    new.h.settle();
    assert_eq!(new.h.http_calls(), 1);
    new.h.advance_ms(4_000);
    assert_eq!(new.h.http_calls(), 1);
    new.h.advance_ms(1_100);
    assert_eq!(
        new.h.http_calls(),
        2,
        "polling continues at the observer's interval, 5 s after the last fetch ended"
    );
}

#[test]
fn the_polling_a_query_declares_starts_with_the_first_fetch_after_the_handle_is_built() {
    let old = Platform::new();
    old.h.fakes.http.respond(format!("{API}/tick"), ok(&1_u32));
    let handle = old.construct::<TickQuery>(&());
    old.observe(handle);
    old.h.settle();
    let snapshot = old.snapshot();

    let new = Platform::new();
    new.h.fakes.http.respond(format!("{API}/tick"), ok(&10_u32));
    new.rt().restore(&snapshot).unwrap();
    new.h.advance_ms(5_000);
    assert_eq!(new.h.http_calls(), 0, "a dormant handle polls nothing");
    new.observe(handle);
    new.h.settle();
    assert_eq!(new.h.http_calls(), 1);
    new.h.advance_ms(1_100);
    assert_eq!(
        new.h.http_calls(),
        2,
        "and then it polls, as the query says"
    );
    new.take();
    assert_eq!(new.value(handle, DATA), Some(Some(10_u32).encode_to_vec()));
}

/// A handle the host released after the snapshot was taken: the restore re-issues it (it cannot
/// know), nobody uses it, and it never fetches or polls.
#[test]
fn a_handle_released_since_the_snapshot_stays_dormant_and_costs_nothing() {
    let p = Platform::new();
    p.h.serve_page(1, vec![todo(1, "milk")]);
    let handle = p.construct::<TodosQuery>(&1_u32);
    p.observe(handle);
    p.h.settle();
    let snapshot = p.snapshot();
    p.rt().release(handle.0);
    let calls = p.h.http_calls();

    p.rt().restore(&snapshot).unwrap();
    assert_eq!(p.stat("dormant_handles"), 1);
    p.h.advance_ms(120_000);
    assert_eq!(
        p.h.http_calls(),
        calls,
        "no observer was registered: nothing is fetched"
    );

    // The next snapshot carries the record on (a second reload before the client came back).
    assert_eq!(records(&p.snapshot()), [(handle, TodosQuery::ID)]);

    // A release of a dormant handle forgets it without ever building the object.
    p.rt().release(handle.0);
    assert_eq!(p.stat("dormant_handles"), 0);
    assert_eq!(p.h.http_calls(), calls);
    assert!(records(&p.snapshot()).is_empty());
}

// ----- the same runtime: a time travel, an app's own `restore` ------------------------------------------

/// The handle is not state, so the restore leaves it alone: no refetch, no blank value, polling,
/// pages and the observer untouched, also when the snapshot is older than the handle.
#[test]
fn a_runtime_that_holds_a_handle_keeps_it_through_any_restore() {
    let p = Platform::new();
    p.h.serve_page(1, vec![todo(1, "milk")]);
    // A snapshot from before the handle existed (a time travel to an earlier step).
    let before = p.snapshot();
    let handle = p.construct::<TodosQuery>(&1_u32);
    p.observe(handle);
    p.h.settle();
    p.take();
    let with = p.snapshot();
    assert_eq!(p.h.http_calls(), 1);
    let live = p.stat("live_handles");

    for snapshot in [&with, &with, &before] {
        let report = p.rt().restore_with_report(snapshot).unwrap();
        assert_eq!((report.reissued, report.refused.len()), (1, 0));
        p.h.settle();
        assert!(
            p.take().is_empty(),
            "the restore sends nothing for the handle: the mirror already has its values"
        );
        assert_eq!(p.h.http_calls(), 1, "and fetches nothing");
        assert_eq!(p.stat("live_handles"), live);
        assert_eq!(p.stat("dormant_handles"), 0);
    }
    assert_eq!(titles_of(&p, handle).unwrap(), ["milk"]);

    // It is still observed: a refetch is accepted and what it changes reaches the mirror.
    p.h.fakes.http.reset();
    p.h.serve_page(1, vec![todo(1, "milk"), todo(2, "eggs")]);
    assert_eq!(
        p.method(handle, REFETCH_METHOD_ID, &[]).status,
        ReplyStatus::Ok
    );
    p.h.settle();
    p.take();
    assert_eq!(
        p.h.http_calls(),
        1,
        "(the reset above emptied the count): one request"
    );
    assert_eq!(titles_of(&p, handle).unwrap(), ["milk", "eggs"]);
}

#[test]
fn a_restore_does_not_reset_the_polling_of_a_live_handle() {
    let p = Platform::new();
    p.h.fakes.http.respond(format!("{API}/tick"), ok(&1_u32));
    let handle = p.construct::<TickQuery>(&());
    p.observe(handle);
    p.h.settle();
    let snapshot = p.snapshot();
    assert_eq!(p.h.http_calls(), 1);
    p.h.advance_ms(600);
    p.rt().restore(&snapshot).unwrap();
    p.h.advance_ms(500);
    assert_eq!(
        p.h.http_calls(),
        2,
        "the timer armed 1 s after the first fetch ended fires on time: the restore did not touch it"
    );
}

#[test]
fn a_fetch_in_flight_carries_on_across_a_restore_into_the_same_runtime() {
    let p = Platform::new();
    p.h.fakes
        .http
        .respond(format!("{API}/slow?ms=1000"), ok(&7_u32));
    let handle = p.construct::<SlowQuery>(&1_000_u32);
    p.observe(handle);
    p.h.settle();
    assert_eq!(p.fetching(handle), Some(true));
    let snapshot = p.snapshot();
    p.h.advance_ms(400);
    p.rt().restore(&snapshot).unwrap();
    p.h.advance_ms(700);
    p.take();
    assert_eq!(
        p.value(handle, DATA),
        Some(Some(7_u32).encode_to_vec()),
        "the answer of the fetch that was in flight reaches the handle"
    );
    assert_eq!(p.h.http_calls(), 1, "and it was not started again");
}

// ----- infinite queries -------------------------------------------------------------------------------------

#[test]
fn an_infinite_handle_keeps_all_its_pages_in_the_same_runtime() {
    let p = Platform::new();
    serve_feed(&p.h, 45, 10);
    let handle = p.construct::<FeedQuery>(&("rust".to_owned(),));
    p.observe(handle);
    p.h.settle();
    p.take();
    assert_eq!(
        p.method(handle, FETCH_NEXT_PAGE_METHOD_ID, &[]).status,
        ReplyStatus::Ok
    );
    p.h.settle();
    p.take();
    assert_eq!(p.rows(handle).len(), 20);
    let snapshot = p.snapshot();
    p.method(handle, FETCH_NEXT_PAGE_METHOD_ID, &[]);
    p.h.settle();
    p.take();
    assert_eq!(p.rows(handle).len(), 30);
    let calls = p.h.http_calls();

    // A time travel to when it had two pages: it still has three, and nothing was fetched.
    p.rt().restore(&snapshot).unwrap();
    p.h.settle();
    assert!(p.take().is_empty());
    assert_eq!(p.h.http_calls(), calls);
    p.method(handle, FETCH_NEXT_PAGE_METHOD_ID, &[]);
    p.h.settle();
    p.take();
    assert_eq!(
        p.rows(handle).len(),
        40,
        "the fourth page follows the third"
    );
}

#[test]
fn an_infinite_handle_comes_back_with_its_first_page_and_pages_on_demand() {
    let old = Platform::new();
    serve_feed(&old.h, 45, 10);
    let handle = old.construct::<FeedQuery>(&("rust".to_owned(),));
    old.observe(handle);
    old.h.settle();
    old.method(handle, FETCH_NEXT_PAGE_METHOD_ID, &[]);
    old.h.settle();
    old.take();
    assert_eq!(old.rows(handle).len(), 20);
    let snapshot = old.snapshot();

    let new = Platform::new();
    serve_feed(&new.h, 45, 10);
    new.rt().restore(&snapshot).unwrap();
    assert_eq!(new.h.http_calls(), 0);
    let sets = new.observe(handle);
    assert!(
        status_of(&sets, handle).is_some(),
        "answered with the handle's values: {sets:?}"
    );
    assert!(
        new.rows(handle).is_empty(),
        "nothing is cached: an empty list, not an absent one"
    );
    new.h.settle();
    new.take();
    assert_eq!(
        new.rows(handle).len(),
        10,
        "the first page, not back to the old depth"
    );
    assert_eq!(new.h.http_calls(), 1);
    assert_eq!(new.value(handle, HAS_NEXT_PAGE), Some(vec![1]));
    // Further pages are loaded on demand.
    assert_eq!(
        new.method(handle, FETCH_NEXT_PAGE_METHOD_ID, &[]).status,
        ReplyStatus::Ok
    );
    new.h.settle();
    new.take();
    assert_eq!(new.rows(handle).len(), 20);
}

/// The persisted pages of an infinite query (`persist_pages`) are shown at once, from `Kv`, and
/// `stale` decides whether a fetch starts.
#[test]
fn a_persisted_infinite_handle_comes_back_with_its_persisted_pages_at_once() {
    let old = Platform::new();
    serve_feed(&old.h, 45, 10);
    let handle = old.construct::<CappedQuery>(&("rust".to_owned(),));
    old.observe(handle);
    old.h.settle();
    for _ in 0..2 {
        old.method(handle, FETCH_NEXT_PAGE_METHOD_ID, &[]);
        old.h.settle();
    }
    old.take();
    assert_eq!(old.rows(handle).len(), 30);
    old.h.advance_ms(600); // the entry is written 250 ms after the last fetch
    let snapshot = old.snapshot();

    // The new process starts with the app's own `Kv`, as a dev reload does.
    let fakes = Fakes::new();
    for key in old.h.fakes.kv.keys() {
        fakes
            .kv
            .insert(key.clone(), old.h.fakes.kv.value(&key).unwrap());
    }
    let new = Platform::on(Harness::with_fakes(fakes));
    serve_feed(&new.h, 45, 10);
    new.h.settle(); // hydration
    new.rt().restore(&snapshot).unwrap();
    let sets = new.observe(handle);
    assert!(status_of(&sets, handle).is_some());
    assert_eq!(
        new.rows(handle).len(),
        20,
        "the two persisted pages, at once: {sets:?}"
    );
    new.h.settle();
    new.take();
    assert_eq!(
        new.h.http_calls(),
        0,
        "fresh for a minute: `stale` decides, no fetch"
    );
    // The third page is loaded on demand, from the persisted cursor.
    new.method(handle, FETCH_NEXT_PAGE_METHOD_ID, &[]);
    new.h.settle();
    new.take();
    assert_eq!(new.rows(handle).len(), 30);
    assert_eq!(new.h.http_calls(), 1);
}

// ----- a persisted entry ----------------------------------------------------------------------------------------

/// What constructing a handle answers is what an observe after a restore answers: one rule. A
/// persisted entry shows its data at once, and `stale` decides whether a fetch starts.
#[test]
fn a_persisted_entry_is_shown_at_once_and_stale_decides_whether_a_fetch_starts() {
    let old = Platform::new();
    old.h.serve_page(1, vec![todo(1, "milk")]);
    let todos = old.construct::<TodosQuery>(&1_u32); // stale = 30 s
    old.h
        .fakes
        .http
        .respond(format!("{API}/settings"), ok(&"dark".to_owned()));
    let settings = old.construct::<SettingsQuery>(&()); // stale = 1 h
    old.observe(todos);
    old.observe(settings);
    old.h.settle();
    old.h.advance_ms(600); // persisted 250 ms after each fetch
    let snapshot = old.snapshot();
    assert_eq!(old.h.http_calls(), 2);

    let carry = |from: &Platform| {
        let fakes = Fakes::new();
        for key in from.h.fakes.kv.keys() {
            fakes
                .kv
                .insert(key.clone(), from.h.fakes.kv.value(&key).unwrap());
        }
        fakes
    };

    // Within both windows: the data at once, no fetch.
    let soon = Platform::on(Harness::with_fakes(carry(&old)));
    soon.h.settle();
    soon.rt().restore(&snapshot).unwrap();
    soon.observe(todos);
    soon.observe(settings);
    assert_eq!(titles_of(&soon, todos).unwrap(), ["milk"], "shown at once");
    assert_eq!(
        soon.value(settings, DATA),
        Some(Some("dark".to_owned()).encode_to_vec())
    );
    soon.h.settle();
    assert_eq!(soon.h.http_calls(), 0, "both are fresh");

    // Past the 30 s window of `todos`, within the hour of `settings`: `todos` shows its data at
    // once and refetches; `settings` does not.
    let later = Platform::on(Harness::with_fakes(carry(&old)));
    later.h.settle();
    later
        .h
        .serve_page(1, vec![todo(1, "milk"), todo(2, "eggs")]);
    later.h.fakes.clock.advance(Duration::from_secs(31));
    later.rt().restore(&snapshot).unwrap();
    later.observe(todos);
    later.observe(settings);
    assert_eq!(
        titles_of(&later, todos).unwrap(),
        ["milk"],
        "the stored data is shown while it refetches"
    );
    assert_eq!(later.fetching(todos), Some(true));
    later.h.settle();
    later.take();
    assert_eq!(later.h.http_calls(), 1, "only the stale one");
    assert_eq!(titles_of(&later, todos).unwrap(), ["milk", "eggs"]);
}

// ----- what cannot be honoured ---------------------------------------------------------------------------------

/// A record this build cannot honour is left out, counted and reported, and everything else is
/// restored.
#[test]
fn a_record_that_cannot_be_honoured_is_refused_and_counted_and_the_rest_is_restored() {
    let old = Platform::new();
    old.h.serve_page(1, vec![]);
    old.h.serve_page(2, vec![]);
    let good = old.construct::<TodosQuery>(&1_u32);
    let changed = old.construct::<SettingsQuery>(&());
    let snapshot = old.snapshot();
    let g = good.generation();
    let unknown = Handle::new(7, g + 10);
    let broken = Handle::new(8, g + 11);
    let forged = edit_snapshot(&snapshot, |s| {
        // The settings query was built with other parameter types: its recorded fingerprint is
        // not today's.
        s.types
            .iter_mut()
            .filter(|t| t.type_id == SettingsQuery::ID)
            .for_each(|t| t.fingerprint ^= 1);
        // A query this build does not have.
        let mut gone = s.stores.iter().find(|r| r.handle == good).unwrap().clone();
        gone.handle = unknown;
        gone.type_id = 0xdead_beef;
        s.types.push(SnapshotType {
            type_id: 0xdead_beef,
            fingerprint: 1,
        });
        s.stores.push(gone);
        // A record whose bytes do not decode, and one whose parameters do not decode as the
        // query's.
        let mut bad = s.stores.iter().find(|r| r.handle == good).unwrap().clone();
        bad.handle = broken;
        bad.signals[0].1 = vec![0xff];
        s.stores.push(bad);
        s.generation_floor = s.generation_floor.max(g + 11);
    });

    let new = Platform::new();
    let report = new.rt().restore_with_report(&forged).unwrap();
    assert_eq!((report.restored, report.reissued), (0, 1), "{report:?}");
    let refused: Vec<u64> = report.refused.iter().map(|r| r.handle).collect();
    assert_eq!(
        refused,
        [changed.0, unknown.0, broken.0],
        "{:?}",
        report.refused
    );
    assert!(
        report.refused[0].reason.contains("changed"),
        "{:?}",
        report.refused[0]
    );
    assert!(
        report.refused[1].reason.contains("nothing that builds"),
        "{:?}",
        report.refused[1]
    );
    assert!(
        report.refused[2].reason.contains("does not decode"),
        "{:?}",
        report.refused[2]
    );
    for handle in [changed, unknown, broken] {
        assert_eq!(
            new.method(handle, REFETCH_METHOD_ID, &[]).status,
            ReplyStatus::BadRequest,
            "a refused record's handle is stale, as today"
        );
    }
    let warns: Vec<String> = new
        .t()
        .host()
        .take_logs()
        .into_iter()
        .filter(|l| l.level == 3)
        .map(|l| l.message)
        .collect();
    for handle in [changed, unknown, broken] {
        assert!(
            warns.iter().any(|w| w.contains(&format!("{handle:?}"))),
            "{handle:?}: {warns:?}"
        );
    }
    new.h.serve_page(1, vec![]);
    assert_eq!(
        new.method(good, REFETCH_METHOD_ID, &[]).status,
        ReplyStatus::Ok,
        "the good one is re-issued, and a `refetch` on it, never observed, builds it"
    );
}

/// A record whose parameters were encoded for other types does not decode as this build's.
#[test]
fn parameters_that_no_longer_decode_refuse_the_record() {
    let old = Platform::new();
    old.h.serve_page(1, vec![]);
    let handle = old.construct::<TodosQuery>(&1_u32);
    let snapshot = old.snapshot();
    // The recorded parameters are three bytes where a `u32` needs four.
    let forged = edit_snapshot(&snapshot, |s| {
        let mut record = vec![1_u8, 0]; // format 1
        record.extend(3_u32.to_le_bytes());
        record.extend([1, 0, 0]);
        record.push(0); // no polling interval
        s.stores[0].signals[0].1 = record;
    });
    let new = Platform::new();
    let report = new.rt().restore_with_report(&forged).unwrap();
    assert_eq!(
        (report.reissued, report.refused.len()),
        (0, 1),
        "{report:?}"
    );
    assert!(
        report.refused[0].reason.contains("no longer decode"),
        "{:?}",
        report.refused[0]
    );
    assert_eq!(report.refused[0].handle, handle.0);
}

/// The fingerprint a snapshot compares is the closure of the query's parameters by name and type.
#[test]
fn the_fingerprint_of_a_query_is_the_closure_of_its_parameters() {
    let p = Platform::new();
    p.h.serve_page(1, vec![]);
    let handle = p.construct::<TodosQuery>(&1_u32);
    let snapshot = p.snapshot();
    let decoded = Snapshot::decode(&mut Reader::new(&snapshot)).unwrap();
    let schema = p.rt().schema();
    let query = schema
        .queries
        .iter()
        .find(|q| q.query_id == TodosQuery::ID)
        .unwrap();
    assert_eq!(
        decoded.fingerprint(TodosQuery::ID),
        Some(schema.closure_of_params(&query.params).fingerprint())
    );
    // Another parameter type is another fingerprint (what a rebuild that changes it records).
    let mut changed = query.params.clone();
    changed[0].ty = undra::meta::TypeRef::String;
    assert_ne!(
        decoded.fingerprint(TodosQuery::ID),
        Some(schema.closure_of_params(&changed).fingerprint())
    );
    let _ = handle;
    let _ = ids::fnv1a32("query.todos");
}

// ----- containment -----------------------------------------------------------------------------------------------

/// A snapshot taken from inside the change-set callback of a handle's own commit: the record is
/// asked for with no lock of the query cache held.
#[test]
fn a_snapshot_taken_inside_a_change_set_callback_of_a_commit_does_not_deadlock() {
    struct SnapshotInside {
        inner: Arc<undra::runtime::testing::RecordingHost>,
        runtime: Mutex<Option<std::sync::Weak<Runtime>>>,
        taken: Mutex<Vec<Vec<u8>>>,
    }
    impl undra::runtime::Host for SnapshotInside {
        fn change_set(&self, payload: &[u8]) {
            self.inner.change_set(payload);
            let rt = self
                .runtime
                .lock()
                .as_ref()
                .and_then(std::sync::Weak::upgrade);
            if let Some(rt) = rt {
                self.taken.lock().push(rt.snapshot());
            }
        }
        fn reply(&self, call_id: u32, payload: &[u8]) {
            self.inner.reply(call_id, payload);
        }
        fn stream_item(&self, call_id: u32, payload: &[u8]) {
            self.inner.stream_item(call_id, payload);
        }
        fn log(&self, level: u8, target: &str, message: &str) {
            self.inner.log(level, target, message);
        }
        fn port_call(
            &self,
            port_id: u32,
            method_id: u32,
            port_call_id: u32,
            args: &[u8],
        ) -> undra::runtime::PortCallOutcome {
            self.inner.port_call(port_id, method_id, port_call_id, args)
        }
        fn schedule(&self) {
            self.inner.schedule();
        }
        fn timer_set(&self, timer_id: u32, delay_ms: u64) -> bool {
            self.inner.timer_set(timer_id, delay_ms)
        }
    }
    let hook = Arc::new(SnapshotInside {
        inner: Arc::new(undra::runtime::testing::RecordingHost::new()),
        runtime: Mutex::new(None),
        taken: Mutex::new(Vec::new()),
    });
    let host = hook.clone();
    let t = TestRuntime::with_host(
        undra::runtime::RuntimeConfig {
            platform: "test".to_owned(),
            mode: "inproc".to_owned(),
            core_threads: 0,
            blocking_threads: 0,
            log_level: 0,
        },
        move |_| host,
    );
    let fakes = undra_ports::fakes::install(&t);
    t.run_init_hooks();
    *hook.runtime.lock() = Some(Arc::downgrade(t.runtime()));
    let h = Harness { t, fakes };
    h.serve_page(1, vec![todo(1, "milk")]);
    let p = Platform::on(h);
    let handle = p.construct::<TodosQuery>(&1_u32);
    p.rt().observe(handle.0, ALL_SIGNALS, true);
    p.h.settle();
    let taken = hook.taken.lock().clone();
    assert!(!taken.is_empty(), "the fetch's commit reached the host");
    assert!(
        taken
            .iter()
            .all(|s| records(s) == [(handle, TodosQuery::ID)]),
        "the snapshot taken inside the callback carries the handle's record"
    );
}
