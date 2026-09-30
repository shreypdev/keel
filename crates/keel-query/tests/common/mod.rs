//! The application core the integration tests run against: real `#[keel::query]` and
//! `#[keel::mutation]` functions (through the facade, as an app writes them) that talk to a
//! scripted `FakeHttp`, on a `TestRuntime` with every port faked.
//!
//! The queries and mutations named `todos`, `todo_by_id`, `add_todo` and `clear_todos` are the
//! ones `keel-bindgen`'s `queries` golden schema describes, so `tests/wire.rs` can prove the ids
//! and shapes the platforms were generated against are the ones this crate serves.
#![allow(dead_code)]

use std::time::Duration;

use keel::prelude::*;
use keel::runtime::testing::TestRuntime;
use keel_ports::fakes::{self, Fakes};
use keel_ports::{CtxPorts, HttpError, HttpRequest, HttpResponse};
use keel_wire::{Decode, Encode};

pub const API: &str = "https://api.test";

#[keel::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Todo {
    pub id: Uuid,
    pub title: String,
}

#[keel::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Page {
    pub items: Vec<Todo>,
    pub total: u64,
}

#[keel::error]
#[derive(Clone, Debug, PartialEq)]
pub enum TodoError {
    #[error("offline")]
    Offline,
    #[error("network: {0}")]
    Http(#[from] HttpError),
    #[error("the server said no: {0}")]
    Rejected(String),
    #[error("the response did not decode")]
    BadResponse,
}

/// Performs `request` and decodes a 2xx body as `T`; a 4xx body is the server's reason.
async fn call<T: Decode>(ctx: &Ctx, request: HttpRequest) -> Result<T, TodoError> {
    let response = ctx.http().request(request).await?;
    if response.status >= 400 {
        return Err(TodoError::Rejected(
            String::from_utf8_lossy(&response.body.0).into_owned(),
        ));
    }
    T::decode_exact(&response.body.0).map_err(|_| TodoError::BadResponse)
}

#[keel::query(key = "todos:{page}", stale = "30s", persist)]
pub async fn todos(ctx: &Ctx, page: u32) -> Result<Page, TodoError> {
    call(ctx, HttpRequest::get(format!("{API}/todos?page={page}"))).await
}

#[keel::query(key = "todo:{id}", stale = "5s")]
pub async fn todo_by_id(ctx: &Ctx, id: Uuid, fresh: bool) -> Result<Todo, TodoError> {
    let fresh = if fresh { "?fresh=1" } else { "" };
    call(ctx, HttpRequest::get(format!("{API}/todos/{id}{fresh}"))).await
}

#[keel::mutation(key = "todos", idempotent)]
pub async fn add_todo(ctx: &Ctx, title: String) -> Result<Todo, TodoError> {
    let mut request = HttpRequest::post(format!("{API}/todos"), title.into_bytes());
    if let Some(key) = keel_query::idempotency_key() {
        request = request.with_header("Idempotency-Key", key.to_string());
    }
    call(ctx, request).await
}

#[keel::mutation(key = "todos")]
pub async fn clear_todos(ctx: &Ctx) -> Result<(), TodoError> {
    let response = ctx
        .http()
        .request(HttpRequest::new(
            keel_ports::HttpMethod::Delete,
            format!("{API}/todos"),
        ))
        .await?;
    if response.status >= 400 {
        return Err(TodoError::Rejected("no".into()));
    }
    Ok(())
}

/// A query with no staleness window (always stale), no retries and a server-side latency of
/// `ms` milliseconds: what the in-flight tests need.
#[keel::query(key = "slow:{ms}", retry = 0)]
pub async fn slow(ctx: &Ctx, ms: u32) -> Result<u32, TodoError> {
    let answer: u32 = call(ctx, HttpRequest::get(format!("{API}/slow?ms={ms}"))).await?;
    ctx.sleep(Duration::from_millis(u64::from(ms))).await;
    Ok(answer)
}

/// A persisted query that is fresh for an hour.
#[keel::query(key = "settings", stale = "1h", persist, retry = 0)]
pub async fn settings(ctx: &Ctx) -> Result<String, TodoError> {
    call(ctx, HttpRequest::get(format!("{API}/settings"))).await
}

/// A query with the default retries and no staleness window.
#[keel::query(key = "greeting:{name}")]
pub async fn greeting(ctx: &Ctx, name: String) -> Result<String, TodoError> {
    call(ctx, HttpRequest::get(format!("{API}/hello/{name}"))).await
}

#[keel::query(key = "explode", retry = 0)]
pub async fn explode(ctx: &Ctx) -> Result<u32, TodoError> {
    let _ = ctx;
    panic!("the query blew up");
}

/// An idempotent mutation with two retries.
#[keel::mutation(key = "todos", idempotent, retry = 2)]
pub async fn flaky_add(ctx: &Ctx, title: String) -> Result<Todo, TodoError> {
    let mut request = HttpRequest::post(format!("{API}/flaky"), title.into_bytes());
    if let Some(key) = keel_query::idempotency_key() {
        request = request.with_header("Idempotency-Key", key.to_string());
    }
    call(ctx, request).await
}

/// A mutation whose key names its parameter: it invalidates one todo, not all of them.
#[keel::mutation(key = "todo:{id}")]
pub async fn rename_todo(ctx: &Ctx, id: Uuid, title: String) -> Result<(), TodoError> {
    let response = ctx
        .http()
        .request(HttpRequest::post(
            format!("{API}/todos/{id}"),
            title.into_bytes(),
        ))
        .await?;
    if response.status >= 400 {
        return Err(TodoError::Rejected("no".into()));
    }
    Ok(())
}

/// A mutation that takes `ms` milliseconds before it talks to the server: something to be
/// in flight while a test looks at the optimistic state.
#[keel::mutation(key = "todos")]
pub async fn slow_add(ctx: &Ctx, title: String, ms: u32) -> Result<Todo, TodoError> {
    ctx.sleep(Duration::from_millis(u64::from(ms))).await;
    call(
        ctx,
        HttpRequest::post(format!("{API}/todos"), title.into_bytes()),
    )
    .await
}

/// Reports whether an idempotency key is visible to the mutation body (it is only for
/// `idempotent` mutations).
#[keel::mutation]
pub async fn key_probe(ctx: &Ctx) -> Result<bool, TodoError> {
    let _ = ctx;
    Ok(keel_query::idempotency_key().is_some())
}

/// The idempotent twin of [`key_probe`]: fails `retry` times so the key can be compared.
#[keel::mutation(idempotent, retry = 1)]
pub async fn key_probe_idem(ctx: &Ctx) -> Result<String, TodoError> {
    let key = keel_query::idempotency_key()
        .map(|k| k.to_string())
        .unwrap_or_default();
    ctx.http()
        .request(HttpRequest::get(format!("{API}/probe?key={key}")))
        .await?;
    Ok(key)
}

/// An idempotent mutation whose own error type is `HttpError`, not a wrapper around it.
#[keel::mutation(idempotent)]
pub async fn ping(ctx: &Ctx) -> Result<(), HttpError> {
    ctx.http()
        .request(HttpRequest::get(format!("{API}/ping")))
        .await
        .map(|_| ())
}

/// A mutation that panics.
#[keel::mutation]
pub async fn boom(ctx: &Ctx) -> Result<(), TodoError> {
    let _ = ctx;
    panic!("the mutation blew up");
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

pub type Slot<T> = std::sync::Arc<std::sync::Mutex<Option<T>>>;

/// Runs a future (a prepared mutation, say) as a task of the runtime; its result lands in the
/// slot. The task id lets a test cancel it.
pub fn spawn<T, F>(h: &Harness, future: F) -> (Slot<T>, keel::runtime::executor::TaskId)
where
    T: Send + 'static,
    F: std::future::IntoFuture<Output = T> + Send + 'static,
    F::IntoFuture: Send,
{
    let slot: Slot<T> = Slot::default();
    let out = slot.clone();
    let task = h.ctx().spawn(async move {
        let result = future.await;
        *out.lock().unwrap() = Some(result);
    });
    (slot, task)
}

/// Takes the result out of a slot, if the task has finished.
pub fn take<T>(slot: &Slot<T>) -> Option<T> {
    slot.lock().unwrap().take()
}

// ---------------------------------------------------------------------------------------------
// The harness
// ---------------------------------------------------------------------------------------------

/// A test runtime with every port faked, and the fake server helpers the tests use.
pub struct Harness {
    pub t: TestRuntime,
    pub fakes: Fakes,
}

pub fn todo(n: u8, title: &str) -> Todo {
    Todo {
        id: Uuid([n; 16]),
        title: title.to_owned(),
    }
}

pub fn page(items: Vec<Todo>) -> Page {
    let total = items.len() as u64;
    Page { items, total }
}

pub fn ok<T: Encode>(value: &T) -> HttpResponse {
    HttpResponse::new(200, value.encode_to_vec())
}

impl Harness {
    /// A runtime like a started app: fakes bound, init hooks run (the query client subscribes to
    /// connectivity and lifecycle events and starts reading its persisted state).
    pub fn new() -> Harness {
        let t = TestRuntime::new();
        let fakes = fakes::install(&t);
        t.run_init_hooks();
        Harness { t, fakes }
    }

    /// A runtime whose fakes are `fakes` (already holding whatever the test preloaded), started
    /// like [`Harness::new`]. Its executor has not run yet: call `t.run_pending()`, or
    /// [`Harness::settle`], to let the hydration finish.
    pub fn with_fakes(fakes: Fakes) -> Harness {
        let t = TestRuntime::new();
        fakes.install_test(&t);
        t.run_init_hooks();
        Harness { t, fakes }
    }

    /// Lets hydration and everything else that is ready finish.
    pub fn settle(&self) {
        self.t.run_pending();
    }

    pub fn ctx(&self) -> Ctx {
        self.t.ctx()
    }

    pub fn query(&self) -> keel_query::QueryClient {
        keel_query::CtxQuery::query(&self.ctx())
    }

    /// Serves `page` for `GET /todos?page=<n>`.
    pub fn serve_page(&self, n: u32, items: Vec<Todo>) {
        self.fakes
            .http
            .respond(format!("{API}/todos?page={n}"), ok(&page(items)));
    }

    /// Runs the executor and moves the fake clock forward `ms` milliseconds.
    pub fn advance_ms(&self, ms: u64) -> usize {
        self.fakes.advance(&self.t, Duration::from_millis(ms))
    }

    pub fn http_calls(&self) -> usize {
        self.fakes.http.call_count()
    }
}
