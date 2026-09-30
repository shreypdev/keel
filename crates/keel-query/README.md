# keel-query

The Keel query client: a typed cache in front of your data source, with staleness, request
deduplication, retries with backoff, optimistic mutations with rollback, an offline queue and
persistence. Core code writes `#[keel::query]` and `#[keel::mutation]` functions; Swift, Kotlin
and TypeScript get an observable `<Name>QueryHandle` class per query (its `data`, `status`,
`error`, `fetching` and `updatedAt` are ordinary signals) and an async function per mutation.

```rust
use keel::prelude::*;
use keel_ports::{CtxPorts, HttpError, HttpRequest, HttpResponse, fakes};

/// Fresh for 30 seconds, and kept across app restarts.
#[keel::query(key = "todos", stale = "30s", persist)]
async fn todos(ctx: &Ctx) -> Result<Vec<String>, HttpError> {
    let response = ctx.http().request(HttpRequest::get("https://api.test/todos")).await?;
    Ok(String::from_utf8_lossy(&response.body.0).lines().map(str::to_owned).collect())
}

/// Safe to replay, so it waits in the offline queue while the network is down.
#[keel::mutation(key = "todos", idempotent)]
async fn add_todo(ctx: &Ctx, title: String) -> Result<(), HttpError> {
    let request = HttpRequest::post("https://api.test/todos", title.into_bytes());
    ctx.http().request(request).await.map(|_| ())
}

fn main() {
    // A test runtime with every port faked: the network, storage, the clock, connectivity.
    let t = keel::runtime::testing::TestRuntime::new();
    let fakes = fakes::install(&t);
    fakes.http.respond("https://api.test/todos", HttpResponse::new(200, b"milk\neggs".to_vec()));
    let ctx = t.ctx();

    // Watch the query from the core: a handle whose signals a `Computed` (or the UI) observes.
    let list = ctx.query().observe::<TodosQuery>(());
    t.run_pending(); // the fetch runs
    assert_eq!(list.data().get(), Some(vec!["milk".to_owned(), "eggs".to_owned()]));

    // Run a mutation: the todo is on screen at once, rolls back if the request fails, and on
    // success every `todos` entry that is observed refetches.
    t.run_until(async {
        ctx.mutate::<AddTodoMutation>(("bread".to_owned(),))
            .optimistic(|cache| {
                cache.update::<TodosQuery>((), |todos| todos.push("bread".to_owned()));
            })
            .await
    })
    .unwrap();
}
```

Everything runs on the core loop and through the standard ports, so a test controls the network
(`FakeHttp`), storage (`MemKv`), time (`FakeClock`, `fakes.advance`) and connectivity
(`ScriptedConnectivity`) and gets the same behaviour every run. The platforms see a query as a
`<Name>QueryHandle` store (constructor, `refetch()`, `invalidate()`, five signals) and a mutation
as an async function; both are served through the runtime's dispatch, see `dispatch`.

See the crate documentation for the model (keys, entries, staleness, triggers, garbage
collection), `MutationBuilder` for mutations and the offline queue, and `docs/SPEC.md` section 9
for the contract this crate implements.
