//! Pagination and infinite lists: a keyed list fed page by page, a view derived from it, and the
//! cursor in a signal.
//!
//! * `posts` is the list the pages are appended to. It is keyed by `id` and written with the
//!   recorded list operations (`push`, `clear`), so a page of twenty rows crosses the boundary as
//!   twenty one-row inserts in one change-set, however long the list already is (SPEC 3.8).
//! * `visible` is a [`DerivedList`] of it: the rows whose title matches `topic`. It is kept current
//!   from the same operations (ADR-039), so typing in a search box sends the rows that entered or
//!   left the view, not the list.
//! * `cursor` is the server's opaque "next page" token. It is a signal, so it survives a restore
//!   (a reloaded web page continues where it was) and a screen can show "end of list" from
//!   `exhausted` without asking.
//! * `load_more` is safe to call from a scroll view on every frame: it does nothing while a page
//!   is in flight or when the list is exhausted. A call that is cancelled mid-flight, because the
//!   screen went away, does not leave the store stuck "loading".

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use undra::ports::HttpRequest;
use undra::prelude::*;

use crate::net::{self, NetError};

/// One row of the feed.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Post {
    /// The server's identity of the post; the list is updated by key, so the UI diffs by it.
    pub id: u32,
    /// The headline.
    pub title: String,
    /// Who wrote it.
    pub author: String,
}

/// What the server answers: a page and where the next one starts (`null` on the last page).
#[derive(Deserialize)]
struct PageBody {
    items: Vec<Post>,
    next: Option<String>,
}

/// Clears `loading` when a page request ends, unless a refresh started a newer generation.
struct Loading {
    flag: Signal<bool>,
    generation: Arc<AtomicU64>,
    mine: u64,
}

impl Drop for Loading {
    fn drop(&mut self) {
        // Only a write that changes something: every write is a change-set for the platforms.
        if self.generation.load(Ordering::SeqCst) == self.mine && self.flag.get() {
            self.flag.set(false);
        }
    }
}

/// The feed screen's store.
#[undra::store(restore = "Self::assemble")]
pub struct Feed {
    ctx: WeakCtx,
    /// Bumped by `refresh`: a page that comes back for an older generation is dropped.
    generation: Arc<AtomicU64>,
    #[undra(key = "id")]
    posts: Signal<Vec<Post>>,
    cursor: Signal<Option<String>>,
    exhausted: Signal<bool>,
    loading: Signal<bool>,
    topic: Signal<String>,
    #[undra(key = "id")]
    visible: DerivedList<Post>,
}

#[undra::api(store)]
impl Feed {
    /// An empty feed. The first `load_more` fetches the first page.
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(
            ctx,
            Signal::new(vec![]),
            Signal::new(None),
            Signal::new(false),
            Signal::new(false),
            Signal::new(String::new()),
        )
    }

    // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot.
    fn assemble(
        ctx: Ctx,
        posts: Signal<Vec<Post>>,
        cursor: Signal<Option<String>>,
        exhausted: Signal<bool>,
        loading: Signal<bool>,
        topic: Signal<String>,
    ) -> Self {
        // A request that was in flight when the snapshot was taken is not running any more.
        if loading.get() {
            loading.set(false);
        }
        let visible = posts
            .derive()
            .filter_with(&topic, |topic, post: &Post| matches(topic, post))
            .build();
        Self {
            ctx: ctx.downgrade(),
            generation: Arc::new(AtomicU64::new(0)),
            posts,
            cursor,
            exhausted,
            loading,
            topic,
            visible,
        }
    }

    /// Fetches the next page and appends it. Returns how many rows were new. Does nothing (and
    /// returns 0) while a page is in flight or after the last page.
    pub async fn load_more(&self) -> Result<u32, NetError> {
        if self.loading.get() || self.exhausted.get() {
            return Ok(0);
        }
        let ctx = self.ctx.upgrade().map_err(|_| NetError::Closed)?;
        let mine = self.generation.load(Ordering::SeqCst);
        self.loading.set(true);
        let _loading = Loading {
            flag: self.loading.clone(),
            generation: self.generation.clone(),
            mine,
        };

        let path = match self.cursor.get() {
            Some(cursor) => format!("/feed?cursor={}", percent_encode(&cursor)),
            None => "/feed".to_owned(),
        };
        let url = net::url(&ctx, &path)?;
        let page: PageBody = net::json(&net::ok(net::send(&ctx, HttpRequest::get(url)).await?)?)?;
        if self.generation.load(Ordering::SeqCst) != mine {
            return Ok(0); // a refresh started while this page was on its way
        }

        // The server can repeat a row when the list changes between two pages: keep one.
        let fresh: Vec<Post> = self.posts.with(|have| {
            page.items
                .into_iter()
                .filter(|post| !have.iter().any(|h| h.id == post.id))
                .collect()
        });
        let added = u32::try_from(fresh.len()).unwrap_or(u32::MAX);
        // One transaction: the rows, the cursor and the flags reach the platforms together.
        txn(|| {
            for post in fresh {
                self.posts.push(post);
            }
            self.exhausted.set(page.next.is_none());
            self.cursor.set(page.next);
            self.loading.set(false);
        });
        Ok(added)
    }

    /// Starts again from the first page: the list and the cursor are reset and page one is
    /// fetched. A page still on its way from before is dropped when it arrives.
    pub async fn refresh(&self) -> Result<u32, NetError> {
        self.generation.fetch_add(1, Ordering::SeqCst);
        txn(|| {
            self.posts.clear();
            self.cursor.set(None);
            self.exhausted.set(false);
            self.loading.set(false);
        });
        self.load_more().await
    }

    /// Shows only the posts whose title contains `topic` (any case); an empty text shows all.
    pub fn set_topic(&self, topic: String) {
        self.topic.set(topic);
    }
}

/// Whether `post` passes the topic filter.
fn matches(topic: &str, post: &Post) -> bool {
    topic.is_empty() || post.title.to_lowercase().contains(&topic.to_lowercase())
}

/// Percent-encodes everything but the unreserved characters, for a cursor in a query string.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use undra::meta::ids;
    use undra::ports::fakes::Matcher;
    use undra::ports::{HttpError, HttpResponse};
    use undra::signals::ALL_SIGNALS;
    use undra::wire::payload::{CallTarget, ChangeOp, ChangeSet, ReplyStatus};
    use undra::wire::{Decode, KeyedPatch, PatchOp, Reader};

    use super::*;
    use crate::net::testing::{App, BASE, json_response};

    fn post(id: u32, title: &str) -> Post {
        Post {
            id,
            title: title.into(),
            author: "ada".into(),
        }
    }

    /// Six posts in three pages of two; the cursor of a page is the id it ends at.
    fn serve_pages(app: &App) {
        let page = |first: u32, next: Option<&str>| {
            json_response(
                200,
                serde_json::json!({
                    "items": [post_json(first), post_json(first + 1)],
                    "next": next,
                }),
            )
        };
        app.fakes.http.respond(
            Matcher::get(format!("{BASE}/feed")),
            page(1, Some("after 2")),
        );
        app.fakes.http.respond(
            Matcher::get(format!("{BASE}/feed?cursor=after%202")),
            page(3, Some("after 4")),
        );
        app.fakes.http.respond(
            Matcher::get(format!("{BASE}/feed?cursor=after%204")),
            page(5, None),
        );
    }

    fn post_json(id: u32) -> serde_json::Value {
        serde_json::json!({"id": id, "title": format!("Post {id}"), "author": "ada"})
    }

    #[test]
    fn pages_are_appended_until_the_server_says_there_are_no_more() {
        let app = App::new();
        serve_pages(&app);
        let feed = Feed::new(app.ctx());
        assert_eq!(app.run(feed.load_more()), Ok(2));
        assert_eq!(feed.cursor.get().as_deref(), Some("after 2"));
        assert_eq!(app.run(feed.load_more()), Ok(2));
        assert_eq!(app.run(feed.load_more()), Ok(2));
        assert_eq!(feed.posts.with(Vec::len), 6);
        assert_eq!(feed.cursor.get(), None);
        assert!(feed.exhausted.get());
        assert!(!feed.loading.get());

        let requests = app.fakes.http.call_count();
        assert_eq!(
            app.run(feed.load_more()),
            Ok(0),
            "a scroll view may ask again"
        );
        assert_eq!(app.fakes.http.call_count(), requests, "and nothing is sent");
    }

    #[test]
    fn a_failed_page_keeps_the_cursor_so_the_retry_continues_from_the_same_place() {
        let app = App::new();
        serve_pages(&app);
        let feed = Feed::new(app.ctx());
        app.run(feed.load_more()).unwrap();
        // Page two fails once, then works.
        app.fakes.http.reset();
        app.fakes.http.respond_sequence(
            Matcher::get(format!("{BASE}/feed?cursor=after%202")),
            [Err(HttpError::Timeout)],
        );
        serve_pages(&app);
        assert_eq!(
            app.run(feed.load_more()),
            Err(NetError::Http(HttpError::Timeout))
        );
        assert!(!feed.loading.get(), "the store is not stuck loading");
        assert_eq!(feed.cursor.get().as_deref(), Some("after 2"));
        assert_eq!(app.run(feed.load_more()), Ok(2));
        assert_eq!(
            feed.posts
                .with(|l| l.iter().map(|p| p.id).collect::<Vec<_>>()),
            [1, 2, 3, 4]
        );
    }

    #[test]
    fn a_call_that_is_dropped_mid_flight_does_not_leave_the_store_loading() {
        let app = App::new();
        serve_pages(&app);
        let feed = Feed::new(app.ctx());
        // Poll the future once and drop it, as a cancelled call does.
        let mut future = Box::pin(feed.load_more());
        let _ = futures_poll_once(&mut future);
        drop(future);
        assert!(!feed.loading.get());
    }

    /// Polls `future` once with a waker that does nothing.
    fn futures_poll_once<F: std::future::Future>(
        future: &mut std::pin::Pin<Box<F>>,
    ) -> std::task::Poll<F::Output> {
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        future.as_mut().poll(&mut cx)
    }

    #[test]
    fn the_topic_filters_the_view_and_new_pages_join_it() {
        let app = App::new();
        serve_pages(&app);
        let feed = Feed::new(app.ctx());
        app.run(feed.load_more()).unwrap();
        feed.set_topic("post 2".into());
        assert_eq!(feed.visible.get(), [post(2, "Post 2")]);
        app.run(feed.load_more()).unwrap();
        app.run(feed.load_more()).unwrap();
        assert_eq!(feed.visible.get(), [post(2, "Post 2")]);
        feed.set_topic("post".into());
        assert_eq!(feed.visible.with(Vec::len), 6);
        feed.set_topic(String::new());
        assert_eq!(feed.visible.with(Vec::len), 6);
    }

    #[test]
    fn a_refresh_starts_over() {
        let app = App::new();
        serve_pages(&app);
        let feed = Feed::new(app.ctx());
        app.run(feed.load_more()).unwrap();
        app.run(feed.load_more()).unwrap();
        assert_eq!(app.run(feed.refresh()), Ok(2));
        assert_eq!(feed.posts.with(Vec::len), 2);
        assert_eq!(feed.cursor.get().as_deref(), Some("after 2"));
        assert!(!feed.exhausted.get());
    }

    #[test]
    fn a_page_that_repeats_a_row_adds_it_once() {
        let app = App::new();
        app.fakes.http.respond(
            Matcher::get(format!("{BASE}/feed")),
            json_response(
                200,
                serde_json::json!({"items": [post_json(1), post_json(2)], "next": "c"}),
            ),
        );
        app.fakes.http.respond(
            Matcher::get(format!("{BASE}/feed?cursor=c")),
            json_response(
                200,
                serde_json::json!({"items": [post_json(2), post_json(3)], "next": null}),
            ),
        );
        let feed = Feed::new(app.ctx());
        app.run(feed.load_more()).unwrap();
        assert_eq!(app.run(feed.load_more()), Ok(1));
        assert_eq!(feed.posts.with(Vec::len), 3);
    }

    #[test]
    fn a_bad_answer_is_a_typed_error() {
        let app = App::new();
        app.fakes
            .http
            .respond(Matcher::any(), HttpResponse::new(503, b"down".to_vec()));
        let feed = Feed::new(app.ctx());
        assert_eq!(
            app.run(feed.load_more()),
            Err(NetError::Status { code: 503 })
        );
        app.fakes.http.reset();
        app.fakes
            .http
            .respond(Matcher::any(), HttpResponse::new(200, b"not json".to_vec()));
        assert!(matches!(
            app.run(feed.load_more()),
            Err(NetError::BadBody(_))
        ));
    }

    // ----- what a platform receives -----------------------------------------------------------

    const POSTS: u32 = 0;
    const VISIBLE: u32 = 5;

    fn ops(cs: &ChangeSet, signal: u32) -> Vec<PatchOp<Post>> {
        let entry = cs
            .entries
            .iter()
            .find(|e| e.signal_id == signal)
            .expect("an entry");
        assert_eq!(entry.op, ChangeOp::KeyedPatch, "{cs:?}");
        let mut reader = Reader::new(&entry.value);
        let patch = KeyedPatch::<Post>::decode(&mut reader).unwrap();
        reader.finish().unwrap();
        patch.ops
    }

    #[test]
    fn a_page_reaches_the_platforms_as_one_change_set_of_inserts() {
        let app = App::new();
        serve_pages(&app);
        let reply = app.t.call_sync(
            CallTarget::Constructor {
                type_id: ids::type_id("Feed"),
                method_id: ids::method_id("Feed", "new"),
            },
            1,
            &[],
        );
        assert_eq!(reply.status, ReplyStatus::Ok);
        let store = Handle::decode_exact(&reply.body).unwrap();
        app.t.take_change_sets();
        app.t.runtime().observe(store.0, ALL_SIGNALS, true);
        app.t.host().take_decoded_change_sets();

        let target = CallTarget::Method {
            handle: store,
            method_id: ids::method_id("Feed", "load_more"),
        };
        assert_eq!(app.t.call(target, 2, &[]), 0);
        app.t.run_pending();
        let sets = app.t.host().take_decoded_change_sets();
        // `loading` going up is one transaction; the page, the cursor and `loading` going down are
        // one more.
        assert_eq!(sets.len(), 2, "{sets:?}");
        assert_eq!(sets[0].entries.len(), 1);
        let inserts = vec![
            PatchOp::Insert {
                index: 0,
                item: post(1, "Post 1"),
            },
            PatchOp::Insert {
                index: 1,
                item: post(2, "Post 2"),
            },
        ];
        assert_eq!(ops(&sets[1], POSTS), inserts);
        assert_eq!(
            ops(&sets[1], VISIBLE),
            inserts,
            "the view is patched, not sent"
        );
    }
}
