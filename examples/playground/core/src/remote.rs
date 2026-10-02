//! The remote to-do lists: a query, mutations and optimistic commands, all over the `Http` port.
//!
//! This is the data-layer demo. The app tells the core where the server is ([`configure_remote`]
//! with a [`RemoteConfig`]); the core never opens a socket itself: every request goes through the
//! `Http` port, which is `URLSession`, OkHttp or `fetch` in an app and a scripted fake in a test
//! (`undra::ports::fakes::FakeHttp`). An app that has no server can answer the port itself, which
//! is what the playground apps do to work offline.
//!
//! * [`remote_todos`] is a query (`GET {base}/lists/{list}/todos`): cached per list, fresh for 30
//!   seconds, persisted and shared by every observer. A platform watches it through the generated
//!   `RemoteTodosQueryHandle`.
//! * [`post_remote_todo`] and [`patch_remote_todo`] are mutations. The first is `idempotent`, so
//!   while the device is offline it is queued and replayed when the network returns.
//! * [`create_remote_todo`] and [`set_remote_done`] are what the UI calls: they run the mutation
//!   with an optimistic update of the cached list, which is rolled back if the server says no.
//!
//! The server speaks JSON: `[{"id":1,"title":"Buy milk","done":false}]` for a list, a single
//! object for one item, `{"title":".."}` to create and `{"done":true}` to change. Every function
//! takes the name of the list (`inbox`, say), which is part of the cache key: two lists are two
//! independent cache entries.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};
use undra::ports::{HttpError, HttpMethod, HttpRequest};
use undra::prelude::*;

/// Where the server is: what an app supplies once, at start-up.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteConfig {
    /// The server's address without a trailing slash, such as `https://api.example.com`.
    pub base_url: String,
}

/// One to-do item on the server.
#[undra::api]
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteTodo {
    /// The server's identity of the item. An item that is only shown optimistically, and that the
    /// server has not answered for yet, has an identity counting down from `u32::MAX`.
    pub id: u32,
    /// What has to be done.
    pub title: String,
    /// Whether it is finished.
    pub done: bool,
}

/// Why a request to the server failed.
#[undra::error]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteError {
    /// [`configure_remote`] was not called.
    #[error("the remote server is not configured")]
    NotConfigured,
    /// The `Http` port failed: no network, a timeout, a bad URL, or a cancelled request.
    #[error("network: {0}")]
    Http(#[from] HttpError),
    /// The server answered with a status that is not a success.
    #[error("the server answered {code}")]
    Status {
        /// The HTTP status code.
        code: u16,
    },
    /// The server's answer is not the JSON this core expects.
    #[error("the response is not what was expected: {0}")]
    BadBody(String),
}

/// What the remote to-do functions share in one runtime.
#[derive(Default)]
struct RemoteState {
    /// The server address, once configured.
    base_url: Mutex<Option<String>>,
    /// How many optimistic placeholders were made; their identities count down from `u32::MAX`.
    placeholders: AtomicU32,
}

/// The state of the runtime `ctx` belongs to.
fn state(ctx: &Ctx) -> &RemoteState {
    ctx.runtime().extension::<RemoteState>()
}

/// Tells the core where the server is. Call it at start-up, before anything observes
/// [`remote_todos`], and again whenever the core is a new one that kept its stores (after a dev
/// reload or a web crash restart): the address lives in the core outside any store, so a snapshot
/// does not carry it. Calling it again points the core elsewhere (cached data stays until it goes
/// stale).
#[undra::api]
pub fn configure_remote(ctx: &Ctx, config: RemoteConfig) {
    let base = config.base_url.trim_end_matches('/').to_owned();
    *state(ctx)
        .base_url
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(base);
}

/// The URL of `path` on the configured server.
pub(crate) fn endpoint(ctx: &Ctx, path: &str) -> Result<String, RemoteError> {
    let base = state(ctx)
        .base_url
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match base {
        Some(base) => Ok(format!("{base}{path}")),
        None => Err(RemoteError::NotConfigured),
    }
}

/// Sends `request` through the `Http` port and returns the body of a successful answer.
pub(crate) async fn send(ctx: &Ctx, request: HttpRequest) -> Result<Vec<u8>, RemoteError> {
    let response = ctx.http().request(request).await?;
    if response.is_success() {
        Ok(response.body.0)
    } else {
        Err(RemoteError::Status {
            code: response.status,
        })
    }
}

/// Parses the JSON body of an answer.
fn parse<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, RemoteError> {
    serde_json::from_slice(body).map_err(|e| RemoteError::BadBody(e.to_string()))
}

/// The items of the server's list `list`: `GET {base}/lists/{list}/todos`.
#[undra::query(key = "remote-todos:{list}", stale = "30s", persist, retry = 1)]
pub async fn remote_todos(ctx: &Ctx, list: String) -> Result<Vec<RemoteTodo>, RemoteError> {
    let url = endpoint(ctx, &format!("/lists/{list}/todos"))?;
    parse(&send(ctx, HttpRequest::get(url)).await?)
}

/// Creates an item in `list`: `POST {base}/lists/{list}/todos`. Idempotent: the request carries an `Idempotency-Key`
/// header that stays the same across retries and offline replays, so the server can tell a repeat
/// from a second item.
#[undra::mutation(key = "remote-todos:{list}", idempotent)]
pub async fn post_remote_todo(
    ctx: &Ctx,
    list: String,
    title: String,
) -> Result<RemoteTodo, RemoteError> {
    let url = endpoint(ctx, &format!("/lists/{list}/todos"))?;
    let body = serde_json::json!({ "title": title }).to_string();
    let mut request =
        HttpRequest::post(url, body.into_bytes()).with_header("Content-Type", "application/json");
    if let Some(key) = undra::query::idempotency_key() {
        request = request.with_header("Idempotency-Key", key.to_string());
    }
    parse(&send(ctx, request).await?)
}

/// Marks an item of `list` finished or not: `PATCH {base}/lists/{list}/todos/{id}`.
#[undra::mutation(key = "remote-todos:{list}")]
pub async fn patch_remote_todo(
    ctx: &Ctx,
    list: String,
    id: u32,
    done: bool,
) -> Result<RemoteTodo, RemoteError> {
    let url = endpoint(ctx, &format!("/lists/{list}/todos/{id}"))?;
    let body = serde_json::json!({ "done": done }).to_string();
    let request = HttpRequest::new(HttpMethod::Patch, url)
        .with_body(body.into_bytes())
        .with_header("Content-Type", "application/json");
    parse(&send(ctx, request).await?)
}

/// Adds an item to `list` the way a UI wants it: the list shows it at once (an optimistic placeholder), the
/// server is asked, and the placeholder is taken back if the server refuses. On success the cached
/// list is refetched, so the placeholder gives way to the server's item.
///
/// While the device is offline the request is queued (the mutation is idempotent) and the call
/// keeps waiting; the placeholder stays visible until the network returns and the request is
/// replayed.
#[undra::api]
pub async fn create_remote_todo(
    ctx: &Ctx,
    list: String,
    title: String,
) -> Result<RemoteTodo, RemoteError> {
    let id = u32::MAX - state(ctx).placeholders.fetch_add(1, Ordering::Relaxed);
    let placeholder = RemoteTodo {
        id,
        title: title.clone(),
        done: false,
    };
    let cached = list.clone();
    ctx.mutate::<PostRemoteTodoMutation>((list, title))
        .optimistic(move |cache| {
            cache.update::<RemoteTodosQuery>((cached,), |items| items.push(placeholder));
        })
        .await
}

/// Marks an item of `list` finished or not, showing the change at once and taking it back if the server
/// refuses.
#[undra::api]
pub async fn set_remote_done(
    ctx: &Ctx,
    list: String,
    id: u32,
    done: bool,
) -> Result<RemoteTodo, RemoteError> {
    let cached = list.clone();
    ctx.mutate::<PatchRemoteTodoMutation>((list, id, done))
        .optimistic(move |cache| {
            cache.update::<RemoteTodosQuery>((cached,), |items| {
                if let Some(todo) = items.iter_mut().find(|todo| todo.id == id) {
                    todo.done = done;
                }
            });
        })
        .await
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use undra::ports::fakes::{self, Fakes, Matcher};
    use undra::ports::{HttpResponse, NetKind};
    use undra::query::{CtxQuery, QueryHandle, QueryStatus};
    use undra::runtime::testing::TestRuntime;

    use super::*;

    const BASE: &str = "https://playground.test";
    const LIST: &str = "inbox";

    /// A runtime like a started app: every port faked, the server configured.
    struct App {
        t: TestRuntime,
        fakes: Fakes,
    }

    impl App {
        fn new() -> App {
            let t = TestRuntime::new();
            let fakes = fakes::install(&t);
            t.run_init_hooks();
            let app = App { t, fakes };
            app.configure();
            app.t.run_pending();
            app
        }

        fn configure(&self) {
            self.t.run_until(std::future::ready(()));
            let _scope = self.t.ctx().enter();
            // The function a platform calls, called directly.
            configure_remote(
                &self.t.ctx(),
                RemoteConfig {
                    base_url: format!("{BASE}/"),
                },
            );
        }

        fn serve_list(&self, todos: &[RemoteTodo]) {
            self.fakes.http.respond(
                Matcher::get(format!("{BASE}/lists/{LIST}/todos")),
                json(200, todos),
            );
        }

        fn observe(&self) -> QueryHandle<RemoteTodosQuery> {
            let handle = self
                .t
                .ctx()
                .query()
                .observe::<RemoteTodosQuery>((LIST.to_owned(),));
            self.t.run_pending();
            handle
        }

        fn advance(&self, ms: u64) {
            self.fakes.advance(&self.t, Duration::from_millis(ms));
        }
    }

    fn json<T: Serialize + ?Sized>(status: u16, value: &T) -> HttpResponse {
        HttpResponse::new(status, serde_json::to_vec(value).unwrap())
    }

    fn todo(id: u32, title: &str, done: bool) -> RemoteTodo {
        RemoteTodo {
            id,
            title: title.to_owned(),
            done,
        }
    }

    #[test]
    fn the_query_fetches_and_parses_the_servers_list() {
        let app = App::new();
        app.serve_list(&[todo(1, "Buy milk", false), todo(2, "Walk", true)]);
        let handle = app.observe();
        assert_eq!(handle.status().get(), QueryStatus::Success);
        assert_eq!(
            handle.data().get(),
            Some(vec![todo(1, "Buy milk", false), todo(2, "Walk", true)])
        );
        // The trailing slash of the configured URL was dropped.
        assert_eq!(
            app.fakes.http.last_call().unwrap().url,
            format!("{BASE}/lists/{LIST}/todos")
        );
    }

    #[test]
    fn an_unconfigured_core_says_so() {
        let t = TestRuntime::new();
        let _fakes = fakes::install(&t);
        t.run_init_hooks();
        let handle = t
            .ctx()
            .query()
            .observe::<RemoteTodosQuery>((LIST.to_owned(),));
        // The fetch retries once after a backoff before it reports the error.
        t.run_pending();
        let fakes = _fakes;
        fakes.advance(&t, Duration::from_secs(2));
        assert_eq!(handle.status().get(), QueryStatus::Error);
        assert_eq!(handle.error().get(), Some(RemoteError::NotConfigured));
    }

    #[test]
    fn a_status_and_a_bad_body_are_typed_errors() {
        let app = App::new();
        app.fakes.http.respond(
            Matcher::get(format!("{BASE}/lists/{LIST}/todos")),
            HttpResponse::new(503, b"down".to_vec()),
        );
        let handle = app.observe();
        app.advance(2_000);
        assert_eq!(
            handle.error().get(),
            Some(RemoteError::Status { code: 503 })
        );

        let app = App::new();
        app.fakes.http.respond(
            Matcher::get(format!("{BASE}/lists/{LIST}/todos")),
            HttpResponse::new(200, b"not json".to_vec()),
        );
        let handle = app.observe();
        app.advance(2_000);
        assert!(matches!(
            handle.error().get(),
            Some(RemoteError::BadBody(_))
        ));
    }

    /// Records every value `handle.data()` takes from now on.
    fn record(
        handle: &QueryHandle<RemoteTodosQuery>,
    ) -> (Effect, Arc<Mutex<Vec<Vec<RemoteTodo>>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let effect = Effect::new(handle.data(), move |data| {
            if let Some(list) = data {
                sink.lock().unwrap().push(list.clone());
            }
        });
        (effect, seen)
    }

    #[test]
    fn creating_shows_a_placeholder_then_the_servers_item() {
        let app = App::new();
        app.serve_list(&[todo(1, "Buy milk", false)]);
        let handle = app.observe();
        let (_effect, seen) = record(&handle);
        // The first scripted reply that matches wins, so start the script over: the server now
        // accepts the new item and will list it (the mutation refetches the list).
        app.fakes.http.reset();
        app.fakes.http.respond(
            Matcher::post(format!("{BASE}/lists/{LIST}/todos")),
            json(201, &todo(2, "Walk", false)),
        );
        app.serve_list(&[todo(1, "Buy milk", false), todo(2, "Walk", false)]);

        let ctx = app.t.ctx();
        let created = app.t.run_until(async move {
            create_remote_todo(&ctx, LIST.to_owned(), "Walk".to_owned()).await
        });
        assert_eq!(created, Ok(todo(2, "Walk", false)));
        app.t.run_pending();

        let seen = seen.lock().unwrap().clone();
        // First the optimistic list (the placeholder counts down from u32::MAX), then the
        // server's own list after the refetch.
        assert_eq!(
            seen,
            [
                vec![todo(1, "Buy milk", false), todo(u32::MAX, "Walk", false)],
                vec![todo(1, "Buy milk", false), todo(2, "Walk", false)],
            ]
        );

        let post = app
            .fakes
            .http
            .calls()
            .into_iter()
            .find(|request| request.method == HttpMethod::Post)
            .expect("the POST");
        assert_eq!(
            post.body.as_ref().map(|b| b.0.as_slice()),
            Some(br#"{"title":"Walk"}"#.as_slice())
        );
        assert!(
            post.header("Idempotency-Key").is_some(),
            "idempotent mutations carry a key"
        );
    }

    #[test]
    fn a_refused_creation_is_rolled_back() {
        let app = App::new();
        app.serve_list(&[todo(1, "Buy milk", false)]);
        let handle = app.observe();
        let before = handle.data().get();
        app.fakes.http.respond(
            Matcher::post(format!("{BASE}/lists/{LIST}/todos")),
            HttpResponse::new(422, b"no".to_vec()),
        );

        let ctx = app.t.ctx();
        let result = app.t.run_until(async move {
            create_remote_todo(&ctx, LIST.to_owned(), "Walk".to_owned()).await
        });
        assert_eq!(result, Err(RemoteError::Status { code: 422 }));
        assert_eq!(handle.data().get(), before, "the placeholder is gone again");
        assert_eq!(handle.status().get(), QueryStatus::Success);
    }

    #[test]
    fn toggling_is_optimistic_and_rolls_back() {
        let app = App::new();
        app.serve_list(&[todo(1, "Buy milk", false)]);
        let handle = app.observe();
        app.fakes.http.respond(
            Matcher::custom(|r| r.method == HttpMethod::Patch),
            HttpResponse::new(500, b"boom".to_vec()),
        );
        let ctx = app.t.ctx();
        let result = app
            .t
            .run_until(async move { set_remote_done(&ctx, LIST.to_owned(), 1, true).await });
        assert_eq!(result, Err(RemoteError::Status { code: 500 }));
        assert_eq!(handle.data().get(), Some(vec![todo(1, "Buy milk", false)]));
        let patch = app
            .fakes
            .http
            .calls()
            .into_iter()
            .find(|request| request.method == HttpMethod::Patch)
            .expect("the PATCH");
        assert_eq!(patch.url, format!("{BASE}/lists/{LIST}/todos/1"));
    }

    #[test]
    fn a_creation_made_offline_is_queued_and_replayed_when_the_network_returns() {
        let app = App::new();
        app.serve_list(&[]);
        let handle = app.observe();
        app.fakes.connectivity.go_offline();
        app.t.run_pending();
        app.fakes.http.fail(
            Matcher::post(format!("{BASE}/lists/{LIST}/todos")),
            HttpError::Network("offline".into()),
        );

        let ctx = app.t.ctx();
        let created = std::sync::Arc::new(Mutex::new(None));
        let slot = created.clone();
        ctx.spawn(async move {
            let result =
                create_remote_todo(&Ctx::current(), LIST.to_owned(), "Walk".to_owned()).await;
            *slot.lock().unwrap() = Some(result);
        });
        app.t.run_pending();
        assert!(
            created.lock().unwrap().is_none(),
            "queued: the caller keeps waiting"
        );
        assert_eq!(
            handle.data().get().map(|list| list.len()),
            Some(1),
            "the placeholder shows"
        );

        // The network returns and the server accepts the replay.
        app.fakes.http.reset();
        app.fakes.http.respond(
            Matcher::post(format!("{BASE}/lists/{LIST}/todos")),
            json(201, &todo(7, "Walk", false)),
        );
        app.serve_list(&[todo(7, "Walk", false)]);
        app.fakes.connectivity.go_online(NetKind::Wifi);
        app.t.run_pending();
        app.advance(2_000);
        assert_eq!(
            created.lock().unwrap().clone(),
            Some(Ok(todo(7, "Walk", false)))
        );
        assert_eq!(handle.data().get(), Some(vec![todo(7, "Walk", false)]));
    }

    #[test]
    fn the_list_is_fresh_for_thirty_seconds() {
        let app = App::new();
        app.serve_list(&[todo(1, "Buy milk", false)]);
        let first = app.observe();
        assert_eq!(app.fakes.http.call_count(), 1);
        // A second observer inside the window shares the cached entry: no request.
        let second = app.observe();
        assert_eq!(app.fakes.http.call_count(), 1);
        assert_eq!(second.data().get(), first.data().get());
        drop((first, second));
        // Past the window, observing fetches again.
        app.advance(31_000);
        let _third = app.observe();
        assert_eq!(app.fakes.http.call_count(), 2);
    }

    #[test]
    fn two_lists_are_two_cache_entries() {
        let app = App::new();
        app.serve_list(&[todo(1, "Buy milk", false)]);
        app.fakes.http.respond(
            Matcher::get(format!("{BASE}/lists/work/todos")),
            json(200, &[todo(5, "Ship it", false)]),
        );
        let inbox = app.observe();
        let work = app
            .t
            .ctx()
            .query()
            .observe::<RemoteTodosQuery>(("work".to_owned(),));
        app.t.run_pending();
        assert_eq!(inbox.data().get(), Some(vec![todo(1, "Buy milk", false)]));
        assert_eq!(work.data().get(), Some(vec![todo(5, "Ship it", false)]));
        assert_eq!(app.fakes.http.call_count(), 2);
    }
}
