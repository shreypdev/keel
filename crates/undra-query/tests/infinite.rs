//! Infinite queries (ADR-043 decision 2): `#[undra::query(infinite, item_key = "..")]` through the
//! facade, as an app writes it, on a scripted keyset-paginated server.
//!
//! The Rust handle (`ctx.query().infinite::<FeedQuery>(..)`) and the platform's (a constructor
//! call, signals 0..=6, `fetch_next_page` by its pinned method id, change-sets applied to a mirror
//! list the way a host does) are both driven.

mod common;

use std::cell::Cell;
use std::sync::Arc;

use common::{API, Harness, TodoError};
use parking_lot::Mutex;
use undra::meta::{QueryKind, TypeRef, collect_schema};
use undra::prelude::*;
use undra::runtime::testing::TestRuntime;
use undra::signals::ALL_SIGNALS;
use undra::wire::payload::{CallTarget, ChangeOp, ChangeSet, ReplyStatus};
use undra_ports::fakes::Matcher;
use undra_ports::{Clock, HttpError, HttpRequest, HttpResponse};
use undra_query::{
    FETCH_NEXT_PAGE_METHOD_ID, InfiniteHandle, QueryDef, QueryStatus, REFETCH_METHOD_ID,
    SET_POLL_INTERVAL_METHOD_ID,
};
use undra_wire::{Decode, Encode, Handle, KeyedPatch, PatchOp, Reader, Writer};

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

#[undra::query(key = "feed/{tag}", infinite, item_key = "id", stale = "1m", retry = 0)]
pub async fn feed(
    ctx: &Ctx,
    tag: String,
    #[undra(cursor)] cursor: Option<String>,
) -> Result<undra::query::Page<Post, String>, TodoError> {
    let body = get_feed(ctx, &tag, cursor).await?;
    Ok(undra::query::Page::new(body.items, body.next))
}

/// Refetches only its first two pages, and persists its first two.
#[undra::query(
    key = "capped/{tag}",
    infinite,
    item_key = "id",
    stale = "1m",
    retry = 0,
    refetch_pages = 2,
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

/// Persists its first page (the default), retries a failed page twice.
#[undra::query(
    key = "saved/{tag}",
    infinite,
    item_key = "id",
    stale = "1m",
    retry = 2,
    persist
)]
pub async fn saved(
    ctx: &Ctx,
    #[undra(cursor)] cursor: Option<String>,
    tag: String,
) -> Result<undra::query::Page<Post, String>, TodoError> {
    let body = get_feed(ctx, &tag, cursor).await?;
    Ok(undra::query::Page::new(body.items, body.next))
}

/// Renames the first post of a feed, after `ms` milliseconds.
#[undra::mutation(key = "feed/{tag}")]
pub async fn rename_first(ctx: &Ctx, tag: String, title: String, ms: u32) -> Result<(), TodoError> {
    ctx.sleep(std::time::Duration::from_millis(u64::from(ms)))
        .await;
    let response = ctx
        .http()
        .request(HttpRequest::post(
            format!("{API}/rename?tag={tag}"),
            title.into_bytes(),
        ))
        .await?;
    if response.status >= 400 {
        return Err(TodoError::Rejected("no".into()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// A keyset-paginated server
// ---------------------------------------------------------------------------------------------

/// Posts newest first; a page is the posts with an id below the cursor, `page` of them.
struct Server {
    posts: Vec<Post>,
    page: usize,
}

type Shared = Arc<Mutex<Server>>;

fn post(id: u64) -> Post {
    Post {
        id,
        title: format!("post {id}"),
    }
}

/// Serves `n` posts, ids `n` down to `1`, `page` per page.
fn serve(h: &Harness, n: u64, page: usize) -> Shared {
    let server = Arc::new(Mutex::new(Server {
        posts: (1..=n).rev().map(post).collect(),
        page,
    }));
    let state = server.clone();
    h.fakes
        .http
        .respond_with(Matcher::url_prefix(format!("{API}/feed")), move |request| {
            let server = state.lock();
            let after = request
                .url
                .split("after=")
                .nth(1)
                .and_then(|rest| rest.split('&').next())
                .and_then(|id| id.parse::<u64>().ok());
            let rest: Vec<Post> = server
                .posts
                .iter()
                .filter(|p| after.is_none_or(|id| p.id < id))
                .cloned()
                .collect();
            let items: Vec<Post> = rest.iter().take(server.page).cloned().collect();
            let next = (rest.len() > items.len())
                .then(|| items.last().map(|p| p.id.to_string()))
                .flatten();
            Ok(HttpResponse::new(
                200,
                FeedBody { items, next }.encode_to_vec(),
            ))
        });
    server
}

fn ids(rows: &[Post]) -> Vec<u64> {
    rows.iter().map(|p| p.id).collect()
}

fn feed_handle(h: &Harness, tag: &str) -> InfiniteHandle<FeedQuery> {
    h.query().infinite::<FeedQuery>((tag.to_owned(),))
}

/// The `after=` values of the requests made so far (`""` for the first page).
fn cursors(h: &Harness) -> Vec<String> {
    h.fakes
        .http
        .take_calls()
        .iter()
        .map(|r| r.url.split("after=").nth(1).unwrap_or_default().to_owned())
        .collect()
}

// ---------------------------------------------------------------------------------------------
// The Rust handle
// ---------------------------------------------------------------------------------------------

#[test]
fn the_list_is_empty_not_absent_before_the_first_page_and_page_one_is_fetched() {
    let h = Harness::new();
    serve(&h, 25, 10);
    let feed = feed_handle(&h, "rust");
    assert!(feed.data().get().is_empty());
    assert_eq!(feed.status().get(), QueryStatus::Fetching);
    assert!(feed.fetching().get());
    assert!(!feed.has_next_page().get());
    assert!(!feed.fetching_next_page().get());

    h.t.run_pending();
    assert_eq!(ids(&feed.data().get()), (16..=25).rev().collect::<Vec<_>>());
    assert_eq!(feed.status().get(), QueryStatus::Success);
    assert_eq!(feed.error().get(), None);
    assert!(!feed.fetching().get());
    assert!(feed.has_next_page().get());
    assert_eq!(
        feed.updated_at().get().map(|t| t.0),
        Some(h.fakes.clock.now_ms())
    );
    assert_eq!(cursors(&h), [""], "the first page asks for no cursor");
}

#[test]
fn fetch_next_page_appends_the_page_after_the_cursor_the_last_one_returned() {
    let h = Harness::new();
    serve(&h, 25, 10);
    let feed = feed_handle(&h, "rust");
    h.t.run_pending();
    h.fakes.http.take_calls();

    feed.fetch_next_page();
    assert!(feed.fetching_next_page().get());
    assert!(feed.fetching().get());
    assert_eq!(
        feed.status().get(),
        QueryStatus::Success,
        "the rows already shown keep it a success"
    );
    h.t.run_pending();
    assert!(!feed.fetching_next_page().get());
    assert_eq!(ids(&feed.data().get()), (6..=25).rev().collect::<Vec<_>>());
    assert!(feed.has_next_page().get());
    assert_eq!(cursors(&h), ["16"], "the cursor is the last row's id");

    feed.fetch_next_page();
    h.t.run_pending();
    assert_eq!(ids(&feed.data().get()), (1..=25).rev().collect::<Vec<_>>());
    assert!(!feed.has_next_page().get(), "the last page has no next");

    // Nothing left: no request.
    h.fakes.http.take_calls();
    feed.fetch_next_page();
    h.t.run_pending();
    assert!(cursors(&h).is_empty());
    assert!(!feed.fetching().get());
}

#[test]
fn fetch_next_page_while_a_fetch_is_in_flight_does_nothing() {
    let h = Harness::new();
    serve(&h, 25, 10);
    let feed = feed_handle(&h, "rust");
    // The first page is still being fetched: there is no next cursor yet.
    feed.fetch_next_page();
    h.t.run_pending();
    assert_eq!(cursors(&h), [""], "one request, for page one");

    feed.fetch_next_page();
    feed.fetch_next_page();
    feed.fetch_next_page();
    h.t.run_pending();
    assert_eq!(cursors(&h), ["16"], "one fetch for a burst of calls");
    assert_eq!(feed.data().get().len(), 20);
}

#[test]
fn a_failed_next_page_keeps_the_rows_shows_the_error_and_clears_the_flag() {
    let h = Harness::new();
    serve(&h, 25, 10);
    let feed = feed_handle(&h, "rust");
    h.t.run_pending();
    let first = feed.data().get();

    // The server goes away (`feed` has no retries).
    h.fakes.http.reset();
    h.fakes.http.fail(
        Matcher::url_prefix(format!("{API}/feed")),
        HttpError::Timeout,
    );
    feed.fetch_next_page();
    assert!(feed.fetching_next_page().get());
    h.t.run_pending();
    assert!(!feed.fetching_next_page().get());
    assert!(!feed.fetching().get());
    assert_eq!(feed.data().get(), first, "the rows are still there");
    assert_eq!(feed.status().get(), QueryStatus::Error);
    assert_eq!(
        feed.error().get(),
        Some(TodoError::Http(HttpError::Timeout))
    );
    assert!(
        feed.has_next_page().get(),
        "and the next page can be asked for again"
    );

    // Back up: the same cursor is asked for again, and the error goes.
    h.fakes.http.reset();
    serve(&h, 25, 10);
    feed.fetch_next_page();
    h.t.run_pending();
    assert_eq!(feed.error().get(), None);
    assert_eq!(feed.status().get(), QueryStatus::Success);
    assert_eq!(feed.data().get().len(), 20);
}

// ---------------------------------------------------------------------------------------------
// What a platform sees
// ---------------------------------------------------------------------------------------------

const DATA: u32 = 0;
const STATUS: u32 = 1;
const ERROR: u32 = 2;
const FETCHING: u32 = 3;
const UPDATED_AT: u32 = 4;
const HAS_NEXT_PAGE: u32 = 5;
const FETCHING_NEXT_PAGE: u32 = 6;

/// A platform: a runtime, its fakes, and a mirror of the `data` list that applies every change-set
/// the way a host does.
struct Platform {
    h: Harness,
    next_call: Cell<u32>,
    rows: Mutex<Vec<Post>>,
}

impl Platform {
    fn new() -> Platform {
        Platform {
            h: Harness::new(),
            next_call: Cell::new(1),
            rows: Mutex::new(Vec::new()),
        }
    }

    fn t(&self) -> &TestRuntime {
        &self.h.t
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

    fn method(
        &self,
        handle: Handle,
        method_id: u32,
        args: &[u8],
    ) -> undra::runtime::testing::ReplyRecord {
        self.t().call_sync(
            CallTarget::Method { handle, method_id },
            self.call_id(),
            args,
        )
    }

    /// Observes every signal and returns the change-sets that took (the host's first delivery).
    fn observe(&self, handle: Handle) -> Vec<ChangeSet> {
        self.t().take_change_sets();
        self.t().runtime().observe(handle.0, ALL_SIGNALS, true);
        self.take(handle)
    }

    /// The change-sets delivered since, applied to the mirror.
    fn take(&self, handle: Handle) -> Vec<ChangeSet> {
        let sets = self.t().host().take_decoded_change_sets();
        for set in &sets {
            for entry in &set.entries {
                if entry.handle == handle && entry.signal_id == DATA {
                    let mut rows = self.rows.lock();
                    match entry.op {
                        ChangeOp::Full => {
                            *rows = Vec::<Post>::decode_exact(&entry.value).unwrap();
                        }
                        ChangeOp::KeyedPatch => {
                            decode_patch(&entry.value).apply(&mut rows).unwrap();
                        }
                        ChangeOp::LazyInvalidated => panic!("a feed is not lazy"),
                    }
                }
            }
        }
        sets
    }

    fn mirror(&self) -> Vec<Post> {
        self.rows.lock().clone()
    }
}

fn signal_ids(set: &ChangeSet) -> Vec<u32> {
    set.entries.iter().map(|e| e.signal_id).collect()
}

fn entry<'a>(set: &'a ChangeSet, signal_id: u32) -> &'a undra::wire::payload::ChangeEntry {
    set.entries
        .iter()
        .find(|e| e.signal_id == signal_id)
        .unwrap_or_else(|| panic!("no entry for signal {signal_id} in {set:?}"))
}

fn patch_of(set: &ChangeSet) -> KeyedPatch<Post> {
    let data = entry(set, DATA);
    assert_eq!(data.op, ChangeOp::KeyedPatch, "{set:?}");
    decode_patch(&data.value)
}

fn decode_patch(bytes: &[u8]) -> KeyedPatch<Post> {
    let mut r = Reader::new(bytes);
    let patch = KeyedPatch::<Post>::decode(&mut r).unwrap();
    r.finish().unwrap();
    patch
}

fn encode_patch(patch: &KeyedPatch<Post>) -> Vec<u8> {
    let mut w = Writer::new();
    patch.encode(&mut w);
    w.into_vec()
}

#[test]
fn the_platform_handle_has_seven_signals_and_the_data_is_an_empty_list_first() {
    let p = Platform::new();
    serve(&p.h, 25, 10);
    let handle = p.construct::<FeedQuery>(&("rust".to_owned(),));
    let sets = p.observe(handle);
    assert_eq!(sets.len(), 1);
    assert_eq!(
        signal_ids(&sets[0]),
        [
            DATA,
            STATUS,
            ERROR,
            FETCHING,
            UPDATED_AT,
            HAS_NEXT_PAGE,
            FETCHING_NEXT_PAGE
        ]
    );
    for e in &sets[0].entries {
        assert_eq!((e.handle, e.op), (handle, ChangeOp::Full));
    }
    assert_eq!(
        entry(&sets[0], DATA).value,
        Vec::<Post>::new().encode_to_vec()
    );
    assert_eq!(
        entry(&sets[0], STATUS).value,
        QueryStatus::Fetching.encode_to_vec()
    );
    assert_eq!(entry(&sets[0], HAS_NEXT_PAGE).value, [0]);
    assert_eq!(entry(&sets[0], FETCHING_NEXT_PAGE).value, [0]);
}

#[test]
fn a_next_page_arrives_as_a_keyed_patch_of_the_appended_rows_and_nothing_else() {
    let p = Platform::new();
    serve(&p.h, 25, 10);
    let handle = p.construct::<FeedQuery>(&("rust".to_owned(),));
    p.observe(handle);
    p.t().run_pending();
    let first = p.take(handle);
    assert_eq!(first.len(), 1);
    assert_eq!(ids(&p.mirror()), (16..=25).rev().collect::<Vec<_>>());

    // fetch_next_page() by its pinned method id.
    let reply = p.method(handle, FETCH_NEXT_PAGE_METHOD_ID, &[]);
    assert_eq!(reply.status, ReplyStatus::Ok);
    let started = p.take(handle);
    assert_eq!(started.len(), 1);
    assert_eq!(signal_ids(&started[0]), [FETCHING, FETCHING_NEXT_PAGE]);
    assert_eq!(entry(&started[0], FETCHING_NEXT_PAGE).value, [1]);

    p.t().run_pending();
    let landed = p.take(handle);
    assert_eq!(landed.len(), 1, "one change-set for the page: {landed:?}");
    let set = &landed[0];
    // Exactly the new rows, appended after the ten shown: the patch is a function of the page,
    // not of the list.
    let want = KeyedPatch {
        ops: (0..10)
            .map(|i| PatchOp::Insert {
                index: 10 + i,
                item: post(15 - u64::from(i)),
            })
            .collect(),
    };
    assert_eq!(entry(set, DATA).op, ChangeOp::KeyedPatch);
    assert_eq!(entry(set, DATA).value, encode_patch(&want));
    assert_eq!(patch_of(set), want);
    assert_eq!(entry(set, FETCHING).value, [0]);
    assert_eq!(entry(set, FETCHING_NEXT_PAGE).value, [0]);
    assert!(!signal_ids(set).contains(&STATUS), "status did not change");
    assert!(
        !signal_ids(set).contains(&UPDATED_AT),
        "the head was not confirmed again"
    );
    assert_eq!(ids(&p.mirror()), (6..=25).rev().collect::<Vec<_>>());
}

#[test]
fn the_new_method_ids_are_pinned_and_fetch_next_page_exists_only_on_infinite_handles() {
    assert_eq!(
        SET_POLL_INTERVAL_METHOD_ID,
        undra::meta::ids::fnv1a32("QueryHandle.set_poll_interval")
    );
    assert_eq!(
        FETCH_NEXT_PAGE_METHOD_ID,
        undra::meta::ids::fnv1a32("QueryHandle.fetch_next_page")
    );
    let p = Platform::new();
    serve(&p.h, 5, 10);
    let plain = p.construct::<common::SettingsQuery>(&());
    let reply = p.method(plain, FETCH_NEXT_PAGE_METHOD_ID, &[]);
    assert_ne!(
        reply.status,
        ReplyStatus::Ok,
        "an ordinary handle has no such method"
    );
    let infinite = p.construct::<FeedQuery>(&("rust".to_owned(),));
    assert_eq!(
        p.method(infinite, REFETCH_METHOD_ID, &[]).status,
        ReplyStatus::Ok
    );
    assert_eq!(
        p.method(infinite, FETCH_NEXT_PAGE_METHOD_ID, &[]).status,
        ReplyStatus::Ok
    );
}

#[test]
fn the_schema_describes_the_feed_without_the_cursor() {
    let schema = collect_schema("t");
    let query = schema.queries.iter().find(|q| q.name == "feed").unwrap();
    assert_eq!(query.kind, QueryKind::Query);
    assert_eq!(query.key, "feed/{tag}");
    assert_eq!(
        query
            .params
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["tag"],
        "the cursor is not a parameter"
    );
    assert_eq!(
        query.returns,
        TypeRef::result(
            TypeRef::vec(TypeRef::named("Post")),
            TypeRef::named("TodoError")
        )
    );
    let infinite = query.infinite.as_ref().expect("infinite");
    assert_eq!(
        (infinite.cursor.clone(), infinite.item_key.as_str()),
        (TypeRef::String, "id")
    );
    assert_eq!(query.interval_ms, None);
    assert!(!query.poll_in_background);
    // The cursor comes first in `saved`, and is still not a parameter of it.
    let saved = schema.queries.iter().find(|q| q.name == "saved").unwrap();
    assert_eq!(saved.params.len(), 1);
    assert_eq!(schema.validate(), Ok(()));
}

#[test]
fn the_cursor_may_be_anywhere_among_the_parameters() {
    let h = Harness::new();
    serve(&h, 12, 5);
    let saved = h.query().infinite::<SavedQuery>(("t".to_owned(),));
    h.t.run_pending();
    saved.fetch_next_page();
    h.t.run_pending();
    assert_eq!(saved.data().get().len(), 10);
}

#[test]
fn an_empty_feed_has_no_rows_and_no_next_page() {
    let h = Harness::new();
    serve(&h, 0, 10);
    let feed = feed_handle(&h, "rust");
    h.t.run_pending();
    assert!(feed.data().get().is_empty());
    assert_eq!(feed.status().get(), QueryStatus::Success);
    assert!(!feed.has_next_page().get());
    assert_eq!(feed.error().get(), None);
}

// ---------------------------------------------------------------------------------------------
// Refetching
// ---------------------------------------------------------------------------------------------

/// Loads `pages` pages of `feed`.
fn load(h: &Harness, feed: &InfiniteHandle<FeedQuery>, pages: usize) {
    h.t.run_pending();
    for _ in 1..pages {
        feed.fetch_next_page();
        h.t.run_pending();
    }
}

#[test]
fn a_refetch_fetches_the_loaded_pages_again_in_order_with_the_cursors_the_server_now_returns() {
    let h = Harness::new();
    let server = serve(&h, 25, 10);
    let feed = feed_handle(&h, "rust");
    load(&h, &feed, 2);
    assert_eq!(cursors(&h), ["", "16"]);

    // A post arrives at the top: every page boundary moves, and so does the cursor of page one.
    server.lock().posts.insert(0, post(26));
    feed.refetch();
    assert!(feed.fetching().get());
    assert!(
        !feed.fetching_next_page().get(),
        "a refetch is not a next page"
    );
    assert_eq!(
        ids(&feed.data().get()),
        (6..=25).rev().collect::<Vec<_>>(),
        "the old rows stay visible while it fetches"
    );
    h.t.run_pending();
    assert_eq!(
        cursors(&h),
        ["", "17"],
        "page one first, then the cursor ITS response returned (not the old `16`)"
    );
    assert_eq!(ids(&feed.data().get()), (7..=26).rev().collect::<Vec<_>>());
    assert!(feed.has_next_page().get());
    assert_eq!(feed.error().get(), None);
}

#[test]
fn a_refetch_stops_where_the_server_list_now_ends() {
    let h = Harness::new();
    let server = serve(&h, 25, 10);
    let feed = feed_handle(&h, "rust");
    load(&h, &feed, 3);
    h.fakes.http.take_calls();
    // The list shrank to 12 posts: page two is the last.
    server.lock().posts.truncate(12);
    feed.refetch();
    h.t.run_pending();
    assert_eq!(
        cursors(&h),
        ["", "16"],
        "no third request: page two has no next"
    );
    assert_eq!(feed.data().get().len(), 12);
    assert!(!feed.has_next_page().get());
}

#[test]
fn refetch_pages_bounds_a_refetch_and_drops_the_rest() {
    let h = Harness::new();
    serve(&h, 45, 10);
    let capped = h.query().infinite::<CappedQuery>(("rust".to_owned(),));
    h.t.run_pending();
    for _ in 0..3 {
        capped.fetch_next_page();
        h.t.run_pending();
    }
    assert_eq!(capped.data().get().len(), 40, "four pages loaded");
    h.fakes.http.take_calls();

    capped.refetch();
    h.t.run_pending();
    assert_eq!(
        cursors(&h),
        ["", "36"],
        "`refetch_pages = 2`: two requests, not four"
    );
    assert_eq!(
        ids(&capped.data().get()),
        (26..=45).rev().collect::<Vec<_>>()
    );
    assert!(
        capped.has_next_page().get(),
        "the next page is the one after the second"
    );
    capped.fetch_next_page();
    h.t.run_pending();
    assert_eq!(cursors(&h), ["26"]);
}

#[test]
fn a_refetch_that_fails_midway_keeps_the_old_pages_and_shows_the_error() {
    let h = Harness::new();
    serve(&h, 25, 10);
    let feed = feed_handle(&h, "rust");
    load(&h, &feed, 2);
    let before = feed.data().get();

    // Page one is answered (with a different list), page two is not.
    h.fakes.http.reset();
    let first = FeedBody {
        items: (100..110).rev().map(post).collect(),
        next: Some("100".into()),
    };
    h.fakes.http.respond_sequence(
        Matcher::url_prefix(format!("{API}/feed")),
        [
            Ok(HttpResponse::new(200, first.encode_to_vec())),
            Err(HttpError::Timeout),
        ],
    );
    feed.refetch();
    h.t.run_pending();
    assert_eq!(
        feed.data().get(),
        before,
        "nothing of the half-fetched answer is shown"
    );
    assert_eq!(feed.status().get(), QueryStatus::Error);
    assert_eq!(
        feed.error().get(),
        Some(TodoError::Http(HttpError::Timeout))
    );
    assert!(!feed.fetching().get());
    assert!(feed.has_next_page().get());

    // The next refetch (the entry is stale: it failed) succeeds and clears the error.
    h.fakes.http.reset();
    serve(&h, 25, 10);
    feed.refetch();
    h.t.run_pending();
    assert_eq!(feed.error().get(), None);
    assert_eq!(feed.status().get(), QueryStatus::Success);
    assert_eq!(feed.data().get(), before);
}

#[test]
fn a_failing_page_is_retried_with_the_query_retries() {
    let h = Harness::new();
    h.fakes.http.respond_sequence(
        Matcher::url_prefix(format!("{API}/feed")),
        [Err(HttpError::Timeout), Err(HttpError::Timeout)],
    );
    let body = FeedBody {
        items: vec![post(2), post(1)],
        next: None,
    };
    h.fakes.http.respond(
        Matcher::url_prefix(format!("{API}/feed")),
        HttpResponse::new(200, body.encode_to_vec()),
    );
    // `saved` has `retry = 2`.
    let saved = h.query().infinite::<SavedQuery>(("t".to_owned(),));
    h.t.run_pending();
    assert!(
        saved.data().get().is_empty(),
        "the first attempt failed; backing off"
    );
    assert_eq!(
        saved.error().get(),
        None,
        "errors show only after the retries run out"
    );
    assert_eq!(saved.status().get(), QueryStatus::Fetching);
    h.advance_ms(1_300);
    h.advance_ms(2_600);
    assert_eq!(ids(&saved.data().get()), [2, 1]);
    assert_eq!(h.http_calls(), 3);
}

#[test]
fn an_unchanged_refetch_sends_the_platform_no_rows() {
    let p = Platform::new();
    serve(&p.h, 25, 10);
    let handle = p.construct::<FeedQuery>(&("rust".to_owned(),));
    p.observe(handle);
    p.t().run_pending();
    p.take(handle);
    p.method(handle, FETCH_NEXT_PAGE_METHOD_ID, &[]);
    p.t().run_pending();
    p.take(handle);
    assert_eq!(p.mirror().len(), 20);

    p.method(handle, REFETCH_METHOD_ID, &[]);
    p.t().run_pending();
    for set in p.take(handle) {
        assert!(
            !signal_ids(&set).contains(&DATA),
            "the list did not change, so it is not sent: {set:?}"
        );
    }
}

#[test]
fn a_refetch_sends_the_platform_only_what_changed() {
    let p = Platform::new();
    let server = serve(&p.h, 25, 10);
    let handle = p.construct::<FeedQuery>(&("rust".to_owned(),));
    p.observe(handle);
    p.t().run_pending();
    p.method(handle, FETCH_NEXT_PAGE_METHOD_ID, &[]);
    p.t().run_pending();
    p.take(handle);
    assert_eq!(ids(&p.mirror()), (6..=25).rev().collect::<Vec<_>>());

    // One post renamed in the middle of the first page.
    server.lock().posts[3].title = "renamed".into();
    p.method(handle, REFETCH_METHOD_ID, &[]);
    p.t().run_pending();
    let sets = p.take(handle);
    let patches: Vec<KeyedPatch<Post>> = sets
        .iter()
        .filter(|s| signal_ids(s).contains(&DATA))
        .map(patch_of)
        .collect();
    assert_eq!(patches.len(), 1);
    assert_eq!(
        patches[0].ops,
        [PatchOp::Update {
            index: 3,
            item: Post {
                id: 22,
                title: "renamed".into()
            }
        }],
        "one row changed, one op"
    );
    assert_eq!(p.mirror()[3].title, "renamed");
}

// ---------------------------------------------------------------------------------------------
// Edits of the list
// ---------------------------------------------------------------------------------------------

/// The server rejects a rename (or accepts it).
fn rename_responds(h: &Harness, status: u16) {
    h.fakes.http.respond(
        Matcher::url_prefix(format!("{API}/rename")),
        HttpResponse::new(status, Vec::new()),
    );
}

#[test]
fn update_items_edits_the_list_in_the_optimistic_transaction_and_a_failure_rolls_it_back() {
    let p = Platform::new();
    serve(&p.h, 25, 10);
    let handle = p.construct::<FeedQuery>(&("rust".to_owned(),));
    p.observe(handle);
    p.t().run_pending();
    p.method(handle, FETCH_NEXT_PAGE_METHOD_ID, &[]);
    p.t().run_pending();
    p.take(handle);
    let before = p.mirror();
    assert_eq!(before.len(), 20);
    rename_responds(&p.h, 500);
    let feed = feed_handle(&p.h, "rust");
    assert_eq!(feed.data().get(), before);

    let ctx = p.h.ctx();
    let (slot, _task) = common::spawn(
        &p.h,
        ctx.mutate::<RenameFirstMutation>(("rust".to_owned(), "optimistic".to_owned(), 1_000))
            .optimistic(|cache| {
                let changed = cache.update_items::<FeedQuery>(("rust".to_owned(),), |posts| {
                    posts[0].title = "optimistic".into();
                    posts.remove(5);
                });
                assert!(changed);
            }),
    );
    p.t().run_pending();
    // The edit is visible at once, to the Rust handle and, as the minimal patch, to the platform.
    assert_eq!(feed.data().get()[0].title, "optimistic");
    assert_eq!(feed.data().get().len(), 19);
    let edit = p.take(handle);
    let patches: Vec<_> = edit
        .iter()
        .filter(|s| signal_ids(s).contains(&DATA))
        .collect();
    assert_eq!(patches.len(), 1, "{edit:?}");
    assert_eq!(
        patch_of(patches[0]).ops.len(),
        2,
        "an update and a removal, not the list"
    );
    assert_eq!(p.mirror().len(), 19);

    // The server says no: the rows are back as they were.
    p.h.advance_ms(1_000);
    assert!(matches!(
        common::take(&slot),
        Some(Err(TodoError::Rejected(_)))
    ));
    assert_eq!(feed.data().get(), before);
    p.take(handle);
    assert_eq!(p.mirror(), before, "the host got the inverse patch");
}

#[test]
fn update_items_that_succeeds_is_replaced_by_the_refetch_the_mutation_invalidates() {
    let h = Harness::new();
    let server = serve(&h, 25, 10);
    let feed = feed_handle(&h, "rust");
    load(&h, &feed, 1);
    rename_responds(&h, 200);
    server.lock().posts[0].title = "from the server".into();

    let ctx = h.ctx();
    let (slot, _task) = common::spawn(
        &h,
        ctx.mutate::<RenameFirstMutation>(("rust".to_owned(), "from the server".to_owned(), 500))
            .optimistic(|cache| {
                cache.update_items::<FeedQuery>(("rust".to_owned(),), |posts| {
                    posts[0].title = "optimistic".into();
                });
            }),
    );
    h.t.run_pending();
    assert_eq!(feed.data().get()[0].title, "optimistic");
    h.advance_ms(500);
    assert!(matches!(common::take(&slot), Some(Ok(()))));
    // `key = "feed/{tag}"` invalidated the entry: it refetched and shows the server's answer.
    assert_eq!(feed.data().get()[0].title, "from the server");
    assert_eq!(h.http_calls(), 3, "page one, the mutation, page one again");
}

#[test]
fn update_items_on_an_entry_with_no_rows_does_nothing() {
    let h = Harness::new();
    let ctx = h.ctx();
    rename_responds(&h, 200);
    let (slot, _task) = common::spawn(
        &h,
        ctx.mutate::<RenameFirstMutation>(("nobody".to_owned(), "x".to_owned(), 0))
            .optimistic(|cache| {
                assert!(
                    !cache.update_items::<FeedQuery>(("nobody".to_owned(),), |posts| posts.clear())
                );
            }),
    );
    h.t.run_pending();
    assert!(matches!(common::take(&slot), Some(Ok(()))));
}

#[test]
fn set_and_get_treat_an_infinite_query_as_its_flattened_list() {
    let h = Harness::new();
    serve(&h, 25, 10);
    let query = h.query();
    assert_eq!(query.get::<FeedQuery>(("rust".to_owned(),)), None);
    let feed = feed_handle(&h, "rust");
    load(&h, &feed, 2);
    assert_eq!(
        query
            .get::<FeedQuery>(("rust".to_owned(),))
            .map(|rows| rows.len()),
        Some(20)
    );
    query.set::<FeedQuery>(("rust".to_owned(),), vec![post(99)]);
    assert_eq!(feed.data().get(), [post(99)]);
    assert!(
        feed.has_next_page().get(),
        "the cursor of the last page is kept"
    );
}

#[test]
fn items_is_a_source_for_a_derived_list() {
    let h = Harness::new();
    serve(&h, 25, 10);
    let feed = feed_handle(&h, "rust");
    let even = feed.items().derive().filter(|p| p.id % 2 == 0).build();
    assert!(even.get().is_empty());
    h.t.run_pending();
    assert_eq!(ids(&even.get()), [24, 22, 20, 18, 16]);
    feed.fetch_next_page();
    h.t.run_pending();
    assert_eq!(ids(&even.get()), [24, 22, 20, 18, 16, 14, 12, 10, 8, 6]);
    h.query()
        .set::<FeedQuery>(("rust".to_owned(),), vec![post(4), post(3)]);
    assert_eq!(ids(&even.get()), [4]);
}

// ---------------------------------------------------------------------------------------------
// Persistence (ADR-037, ADR-049)
// ---------------------------------------------------------------------------------------------

mod persisted {
    use super::*;
    use undra_meta::{ParamDef, Schema};
    use undra_ports::fakes::Fakes;
    use undra_query::{cache_key, types_key};

    fn capped_key() -> String {
        cache_key(CappedQuery::QUERY_ID, &("rust".to_owned(),).encode_to_vec())
    }

    fn saved_key() -> String {
        cache_key(SavedQuery::QUERY_ID, &("rust".to_owned(),).encode_to_vec())
    }

    /// The closure of a persisted infinite entry: `{ next: Option<C>, pages: Vec<Vec<T>> }`.
    fn closure(schema: &Schema, query: &str) -> undra_meta::TypeClosure {
        let q = schema.queries.iter().find(|q| q.name == query).unwrap();
        let TypeRef::Result(ok, _) = &q.returns else {
            panic!("a query returns a Result");
        };
        schema.closure_of_params(&[
            ParamDef {
                name: "next".into(),
                ty: TypeRef::option(q.infinite.as_ref().unwrap().cursor.clone()),
            },
            ParamDef {
                name: "pages".into(),
                ty: TypeRef::vec((**ok).clone()),
            },
        ])
    }

    /// A stored entry's parts: `(schema hash, fingerprint, updated_at, data)`.
    fn stored(fakes: &Fakes, key: &str) -> Option<(u64, u64, i64, Vec<u8>)> {
        let bytes = fakes.kv.value(key)?;
        let mut r = Reader::new(&bytes);
        assert_eq!(r.read_u16().unwrap(), 2);
        let entry = (
            r.read_u64().unwrap(),
            r.read_u64().unwrap(),
            r.read_i64().unwrap(),
            r.read_bytes().unwrap().to_vec(),
        );
        r.finish().unwrap();
        Some(entry)
    }

    fn encode_entry(fingerprint: u64, updated_at: i64, data: &[u8]) -> Vec<u8> {
        let mut w = Writer::new();
        w.write_u16(2);
        w.write_u64(0xdead_beef);
        w.write_u64(fingerprint);
        w.write_i64(updated_at);
        w.write_bytes(data);
        w.into_vec()
    }

    /// A first run that loaded three pages of `capped` (`persist_pages = 2`) and let them persist.
    fn first_run() -> Fakes {
        let h = Harness::new();
        h.settle();
        serve(&h, 45, 10);
        let capped = h.query().infinite::<CappedQuery>(("rust".to_owned(),));
        h.t.run_pending();
        h.advance_ms(250);
        for _ in 0..2 {
            capped.fetch_next_page();
            h.t.run_pending();
            h.advance_ms(250);
        }
        assert_eq!(capped.data().get().len(), 30);
        drop(capped);
        let fakes = h.fakes.clone();
        drop(h);
        fakes
    }

    #[test]
    fn the_first_persist_pages_and_the_cursor_after_them_are_written() {
        let fakes = first_run();
        let schema = Harness::new().t.runtime().schema().clone();
        let (hash, fingerprint, _, data) = stored(&fakes, &capped_key()).expect("written");
        assert_ne!(hash, 0);
        assert_eq!(fingerprint, closure(&schema, "capped").fingerprint());
        // `{ next: Option<String>, pages: Vec<Vec<Post>> }`: two pages, the cursor after the second.
        let mut r = Reader::new(&data);
        let next = Option::<String>::decode(&mut r).unwrap();
        let pages = Vec::<Vec<Post>>::decode(&mut r).unwrap();
        r.finish().unwrap();
        assert_eq!(next.as_deref(), Some("26"));
        assert_eq!(pages.len(), 2, "the third page was not stored");
        assert_eq!(ids(&pages[0]), (36..=45).rev().collect::<Vec<_>>());
        assert_eq!(ids(&pages[1]), (26..=35).rev().collect::<Vec<_>>());
        // The closure it names is stored beside it.
        assert!(fakes.kv.contains_key(&types_key(fingerprint)));
    }

    #[test]
    fn a_restarted_app_shows_the_stored_pages_at_once_and_continues_from_the_stored_cursor() {
        let fakes = first_run();
        let h = Harness::with_fakes(fakes);
        h.settle();
        serve(&h, 45, 10);
        h.fakes.http.take_calls();
        let capped = h.query().infinite::<CappedQuery>(("rust".to_owned(),));
        // Shown before anything was fetched, and fresh (a minute): no request.
        assert_eq!(
            ids(&capped.data().get()),
            (26..=45).rev().collect::<Vec<_>>()
        );
        assert_eq!(capped.status().get(), QueryStatus::Success);
        assert!(!capped.fetching().get());
        assert!(capped.has_next_page().get());
        h.t.run_pending();
        assert_eq!(h.http_calls(), 0);

        // The cursor after the stored pages is the one the page after them is asked with.
        capped.fetch_next_page();
        h.t.run_pending();
        assert_eq!(cursors(&h), ["26"]);
        assert_eq!(capped.data().get().len(), 30);
    }

    #[test]
    fn a_stale_stored_entry_refetches_the_stored_number_of_pages() {
        let fakes = first_run();
        let h = Harness::with_fakes(fakes);
        h.settle();
        h.advance_ms(61_000);
        serve(&h, 45, 10);
        h.fakes.http.take_calls();
        let capped = h.query().infinite::<CappedQuery>(("rust".to_owned(),));
        assert_eq!(
            capped.data().get().len(),
            20,
            "the stored rows show meanwhile"
        );
        assert!(capped.fetching().get());
        h.t.run_pending();
        assert_eq!(cursors(&h), ["", "36"]);
    }

    #[test]
    fn persist_pages_defaults_to_one_and_the_stored_page_has_its_cursor() {
        let h = Harness::new();
        h.settle();
        serve(&h, 25, 10);
        let saved = h.query().infinite::<SavedQuery>(("rust".to_owned(),));
        h.t.run_pending();
        saved.fetch_next_page();
        h.t.run_pending();
        h.advance_ms(250);
        let (_, _, _, data) = stored(&h.fakes, &saved_key()).expect("written");
        let mut r = Reader::new(&data);
        let next = Option::<String>::decode(&mut r).unwrap();
        let pages = Vec::<Vec<Post>>::decode(&mut r).unwrap();
        assert_eq!((next.as_deref(), pages.len()), (Some("16"), 1));

        let second = Harness::with_fakes(h.fakes.clone());
        second.settle();
        serve(&second, 25, 10);
        second.fakes.http.take_calls();
        let again = second.query().infinite::<SavedQuery>(("rust".to_owned(),));
        assert_eq!(again.data().get().len(), 10);
        again.fetch_next_page();
        second.t.run_pending();
        assert_eq!(cursors(&second), ["16"]);
    }

    #[test]
    fn an_entry_with_more_pages_than_this_build_stores_is_not_used() {
        // A build with `persist_pages = 3` wrote it; this one keeps two and could not name the
        // cursor after the second page.
        let schema = Harness::new().t.runtime().schema().clone();
        let current = closure(&schema, "capped");
        let mut w = Writer::new();
        Some("5".to_owned()).encode(&mut w);
        vec![vec![post(3)], vec![post(2)], vec![post(1)]].encode(&mut w);
        let fakes = Fakes::new();
        fakes.kv.insert(
            capped_key(),
            encode_entry(current.fingerprint(), 5, &w.into_vec()),
        );
        let h = Harness::with_fakes(fakes);
        h.settle();
        serve(&h, 5, 10);
        let capped = h.query().infinite::<CappedQuery>(("rust".to_owned(),));
        assert!(capped.data().get().is_empty());
        assert!(capped.fetching().get(), "fetched instead");
    }

    #[test]
    fn an_entry_from_an_older_build_migrates_by_name_and_is_rewritten() {
        // The build before this one: `Post` also had `likes: bool`.
        let mut old = Harness::new().t.runtime().schema().clone();
        let post_def = old.records.iter_mut().find(|r| r.name == "Post").unwrap();
        post_def.fields.push(undra_meta::FieldDef {
            name: "likes".into(),
            ty: TypeRef::Bool,
            default: false,
            docs: String::new(),
        });
        let old_closure = closure(&old, "saved");
        let current = closure(Harness::new().t.runtime().schema(), "saved");
        assert_ne!(old_closure.fingerprint(), current.fingerprint());
        let mut w = Writer::new();
        Some("7".to_owned()).encode(&mut w);
        vec![vec![
            (9_u64, "nine".to_owned(), true),
            (8, "eight".to_owned(), false),
        ]]
        .encode(&mut w);
        let fakes = Fakes::new();
        fakes.kv.insert(
            types_key(old_closure.fingerprint()),
            old_closure.canonical_json().into_bytes(),
        );
        fakes.kv.insert(
            saved_key(),
            encode_entry(old_closure.fingerprint(), 5, &w.into_vec()),
        );

        let h = Harness::with_fakes(fakes);
        h.settle();
        serve(&h, 9, 10);
        let saved = h.query().infinite::<SavedQuery>(("rust".to_owned(),));
        assert_eq!(
            saved.data().get(),
            [
                Post {
                    id: 9,
                    title: "nine".into()
                },
                Post {
                    id: 8,
                    title: "eight".into()
                }
            ]
        );
        assert!(
            saved.has_next_page().get(),
            "the cursor survived the migration"
        );
        let (_, fingerprint, _, _) = stored(&h.fakes, &saved_key()).expect("rewritten");
        assert_eq!(fingerprint, current.fingerprint(), "in today's form");
        let stats: serde_json::Value = serde_json::from_str(&h.t.runtime().stats_json()).unwrap();
        assert_eq!(stats["query"]["persist"]["migrated"], 1);
    }

    #[test]
    fn an_entry_that_does_not_migrate_or_does_not_decode_is_dropped() {
        let fakes = Fakes::new();
        // The description of the type it was written with is missing.
        fakes
            .kv
            .insert(saved_key(), encode_entry(42, 5, &[1, 2, 3]));
        let h = Harness::with_fakes(fakes);
        h.settle();
        assert!(!h.fakes.kv.contains_key(&saved_key()));
        let stats: serde_json::Value = serde_json::from_str(&h.t.runtime().stats_json()).unwrap();
        assert_eq!(stats["query"]["persist"]["dropped"], 1);

        // Today's fingerprint, bytes that are not a page list: dropped when read.
        let schema = Harness::new().t.runtime().schema().clone();
        let fakes = Fakes::new();
        fakes.kv.insert(
            saved_key(),
            encode_entry(closure(&schema, "saved").fingerprint(), 5, &[9, 9, 9]),
        );
        let h = Harness::with_fakes(fakes);
        h.settle();
        serve(&h, 5, 10);
        let saved = h.query().infinite::<SavedQuery>(("rust".to_owned(),));
        assert!(saved.data().get().is_empty(), "nothing usable was stored");
        h.t.run_pending();
        assert_eq!(saved.data().get().len(), 5);
    }
}
