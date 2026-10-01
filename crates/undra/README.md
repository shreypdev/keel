# undra

Undra owns everything under the pixels of a native app: domain logic, reactive state, the
data layer and persistence, written once in Rust. The UI stays SwiftUI, Compose and React;
Undra generates the Swift, Kotlin and TypeScript APIs they call. This crate is the one an
application core depends on: it re-exports the runtime, the signals, the wire codec, the
schema and the six attribute macros (`docs/SPEC.md` sections 4 and 16).

## A core in 60 lines

```rust
use std::sync::atomic::{AtomicU64, Ordering};

use undra::prelude::*;

// Records, enums and errors: the wire codec and the schema come from the type.
#[undra::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Todo {
    pub id: Uuid,
    pub title: String,
    pub done: bool,
}

#[undra::api]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Filter {
    All,
    Active,
    Done,
}

impl Filter {
    fn matches(self, todo: &Todo) -> bool {
        match self {
            Filter::All => true,
            Filter::Active => !todo.done,
            Filter::Done => todo.done,
        }
    }
}

#[undra::error]
#[derive(Clone, Debug, PartialEq)]
pub enum StoreError {
    #[error("the server is unreachable")]
    Offline,
}

// A port nobody registered is an ordinary outcome, not a bug: a port method that returns
// `Result<T, E>` reports it as `E`, so `E` implements `From<PortError>`.
impl From<undra::runtime::PortError> for StoreError {
    fn from(_: undra::runtime::PortError) -> Self {
        StoreError::Offline
    }
}

#[undra::error]
#[derive(Clone, Debug, PartialEq)]
pub enum TodoError {
    #[error("title cannot be empty")]
    EmptyTitle,
    #[error(transparent)]
    Store(#[from] StoreError),
}

// A port: implemented by the platform (URLSession, OkHttp, fetch) or by a Rust fake in tests.
#[undra::port]
pub trait Store {
    async fn save(&self, todo: Todo) -> Result<(), StoreError>;
}

// A store: signals the platforms mirror (`todos`, `filter`, `visible`) and commands they call.
#[undra::store(restore = "Self::assemble")]
pub struct Todos {
    ctx: Ctx,
    next: AtomicU64,
    #[undra(key = "id")]
    todos: Signal<Vec<Todo>>,
    filter: Signal<Filter>,
    visible: Computed<Vec<Todo>>,
}

#[undra::api(store)]
impl Todos {
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(vec![]), Signal::new(Filter::All))
    }

    // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot.
    fn assemble(ctx: Ctx, todos: Signal<Vec<Todo>>, filter: Signal<Filter>) -> Self {
        let visible = Computed::new((&todos, &filter), |(todos, filter)| {
            todos.iter().filter(|t| filter.matches(t)).cloned().collect()
        });
        let next = AtomicU64::new(todos.with(|list| list.len() as u64) + 1);
        Self { ctx, next, todos, filter, visible }
    }

    pub fn set_filter(&self, filter: Filter) {
        self.filter.set(filter);
    }

    pub async fn add(&self, title: String) -> Result<Todo, TodoError> {
        let title = title.trim().to_owned();
        if title.is_empty() {
            return Err(TodoError::EmptyTitle);
        }
        let mut id = [0; 16];
        id[..8].copy_from_slice(&self.next.fetch_add(1, Ordering::Relaxed).to_be_bytes());
        let todo = Todo { id: Uuid(id), title, done: false };
        store(&self.ctx).save(todo.clone()).await?; // `store(ctx)`: the port, platform or fake
        self.todos.update(|list| list.push(todo.clone()));
        Ok(todo)
    }
}

fn main() {
    // A runtime with no threads and no device, for tests: call the constructor by id.
    use undra::meta::ids;
    use undra::runtime::testing::TestRuntime;
    use undra::wire::payload::{CallTarget, ReplyStatus};

    let core = TestRuntime::new();
    let new = CallTarget::Constructor {
        type_id: ids::type_id("Todos"),
        method_id: ids::method_id("Todos", "new"),
    };
    let reply = core.call_sync(new, 1, &[]);
    assert_eq!(reply.status, ReplyStatus::Ok); // the body is the new store's handle
}
```

## What is in the crate

| Path | What |
|---|---|
| `undra::{api, error, store, port, query, mutation}` | the attribute macros |
| `undra::prelude` | `Signal`, `Computed`, `Effect`, `txn`, `Ctx`, `Bytes`, `Uuid`, `Timestamp`, `Duration`, `Handle` and the macros |
| `undra::runtime` | `undra-runtime`: `Runtime`, `Ctx`, dispatch, ports, timers, snapshots, `testing::TestRuntime` |
| `undra::signals` | `undra-signals`: `Signal`, `Computed`, `Effect`, `txn`, `StoreCell`, `CellSlot` |
| `undra::wire` | `undra-wire`: the binary codec and the message payloads |
| `undra::meta` | `undra-meta`: the schema (`collect_schema`) every language is generated from |
| `undra::query` | `undra-query`: `QueryDef` / `MutationDef` (the contract `#[undra::query]` and `#[undra::mutation]` implement), `ctx.query()` / `ctx.mutate(..)` (`CtxQuery`, in the prelude), `QueryHandle`, `MutationBuilder` |

The macros generate code that names `::undra::{wire, meta, runtime, signals, query}`, so an
application depends on this crate and nothing else. (`#[undra(crate = "path")]` points the
generated code somewhere else, for crates inside the workspace.)

The crate contains no `unsafe` and builds for `wasm32-unknown-unknown`.

## Tests

`cargo test -p undra` runs `tests/e2e_todo.rs`: real generated code (a store with a computed
signal and a keyed list, an object with sync, async, stream, panicking and failing methods, a
free function and a port with a Rust fake) driven through `TestRuntime`, asserting on the
decoded wire payloads, plus schema validation and generation of all three languages.
