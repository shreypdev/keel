# keel-query

The Keel query client: a typed cache in front of your data source, with staleness, request
deduplication, retries with backoff, optimistic mutations with rollback, an offline queue and
persistence. Core code writes `#[keel::query]` and `#[keel::mutation]` functions; Swift, Kotlin
and TypeScript get an observable `<Name>QueryHandle` class per query (its `data`, `status`,
`error`, `fetching` and `updatedAt` are ordinary signals) and an async function per mutation.

```rust,ignore
use keel::prelude::*;

#[keel::query(key = "todos:{page}", stale = "30s", persist)]
async fn todos(ctx: &Ctx, page: u32) -> Result<Vec<Todo>, HttpError> {
    fetch_json(ctx, &format!("/todos?page={page}")).await
}

#[keel::mutation(key = "todos", idempotent)]
async fn add_todo(ctx: &Ctx, title: String) -> Result<Todo, HttpError> {
    post_json(ctx, "/todos", &title, keel::query::idempotency_key()).await
}

async fn add(ctx: &Ctx, title: String) -> Result<Todo, HttpError> {
    // Watch a query from the core: a handle with typed signals.
    let page = ctx.query().observe::<TodosQuery>((0,));
    let _count = Computed::new(page.data(), |data| data.as_ref().map_or(0, Vec::len));

    // Run a mutation: the placeholder is on screen at once, and is rolled back if the
    // request fails. On success every "todos..." entry that is observed refetches.
    ctx.mutate::<AddTodoMutation>((title.clone(),))
        .optimistic(move |cache| {
            cache.update::<TodosQuery>((0,), |todos| todos.push(placeholder(&title)));
        })
        .invalidates(["todos"])
        .await
}
```

Everything runs on the core loop and through the standard ports, so a test controls the network
(`FakeHttp`), storage (`MemKv`), time (`FakeClock`) and connectivity (`ScriptedConnectivity`):

```rust,ignore
let t = TestRuntime::new();
let fakes = keel_ports::fakes::install(&t);
let todos = t.ctx().query().observe::<TodosQuery>((0,));
t.run_pending();                                       // the fetch runs
fakes.advance(&t, Duration::from_secs(31));            // the entry goes stale
```

See the crate documentation for the model (keys, entries, staleness, triggers, garbage
collection), [`MutationBuilder`] for mutations and the offline queue, and `docs/SPEC.md` section 9
for the contract this crate implements.
