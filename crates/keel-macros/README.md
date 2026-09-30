# keel-macros

The attribute macros of Keel: `#[keel::api]`, `#[keel::error]`, `#[keel::store]`,
`#[keel::port]`, `#[keel::query]` and `#[keel::mutation]`. Users get them from the `keel`
facade; this crate is the implementation (`docs/SPEC.md` section 4).

```rust
use keel::prelude::*;

// Records, enums and errors: encoding, decoding and the schema come from the type.
#[keel::api]
#[derive(Clone)]
pub struct Todo { pub id: Uuid, pub title: String, #[keel(default)] pub done: bool }

#[keel::api]
pub enum Filter { All, Active, Done }

#[keel::error]
pub enum TodoError {
    #[error("title cannot be empty")] EmptyTitle,
    #[error(transparent)] Http(#[from] HttpError),
}

// A store: signals the platforms mirror, commands they call.
#[keel::store(restore = "Self::assemble")]
pub struct Todos {
    ctx: Ctx,
    todos: Signal<Vec<Todo>>,
    filter: Signal<Filter>,
    visible: Computed<Vec<Todo>>,
}

#[keel::api(store)]
impl Todos {
    pub fn new(ctx: Ctx) -> Self { Self::assemble(ctx, Signal::new(vec![]), Signal::new(Filter::All)) }

    fn assemble(ctx: Ctx, todos: Signal<Vec<Todo>>, filter: Signal<Filter>) -> Self {
        let visible = Computed::new((&todos, &filter), |(t, f)| {
            t.iter().filter(|x| f.matches(x)).cloned().collect()
        });
        Self { ctx, todos, filter, visible }
    }

    pub fn set_filter(&self, f: Filter) { self.filter.set(f) }

    pub async fn add(&self, title: String) -> Result<Todo, TodoError> { /* .. */ }
}

// A port the platform implements, and a cached query over it.
#[keel::port]
pub trait Http { async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>; }

#[keel::query(key = "todos:{page}", stale = "30s", persist)]
pub async fn todos(ctx: &Ctx, page: u32) -> Result<Vec<Todo>, HttpError> { /* .. */ }
```

## What is generated

| Macro | Generated (besides the item as written) |
|---|---|
| `api` on a struct | `impl Encode`, `impl Decode`, `KEEL_TYPE_ID`, `static __KEEL_META_<T>: RecordMeta`, registration |
| `api` / `error` on an enum | the same with a `u16` variant index; `error` adds `Display`, `Error`, `From` for `#[from]` |
| `api` on `impl T` | `impl KeelObject`, `fn __keel_dispatch_<T>`, `ObjectMeta` + registration |
| `api` on a `fn` | `fn __keel_dispatch_fn_<name>`, `FunctionMeta` + registration |
| `store` on a struct | hidden `CellSlot` field, hidden `cell`/`restore` members (`impl StoreObject` is written by the `api(store)` impl block), a builder that attaches every signal (`attach`, `attach_keyed` with a typed key fn, `attach_computed`, `set_no_coalesce`), `StoreMeta`, `StoreRestorer` registration |
| `port` on a trait | `impl Port for dyn T`, `<T>Proxy`, accessor `fn <t>(ctx)`, `__keel_port_dispatch_<T>`, `PortMeta` |
| `query` / `mutation` | `<Name>Query` / `<Name>Mutation` with the ids and settings, `QueryDef` / `MutationDef`, `QueryMeta`, and a `QueryRegistration` / `MutationRegistration` (how `keel-query` finds the definition by id) |

Every expansion also carries compile-time checks that the types the schema records are the types
written in the code (`src/impl_/check.rs`): a user type called `Bytes`, an alias or a renamed import
(`use m::Item as Todo`) is an error in the user's crate (E0060, E0061), not a schema that disagrees
with its own wire layout. The proxy of a port method returning `Result<T, E>` reports an unavailable
port as `E::from(PortError)` (E0033 if `E` lacks the impl) instead of panicking; a method without an
error channel panics with a message that says how to bind the port (E0062). See ADR-025.

Generated code names its dependencies as `::keel::{wire, meta, runtime, signals, query}`;
`#[keel(crate = "path")]` on the item (or `crate = "path"` in the macro arguments) replaces
`::keel`. The exact paths and shapes are pinned by the snapshots in `tests/snapshots/`.

## Diagnostics

Every rejection is `error[keel::E00NN]: what`, with a note (why), a help (the fix) and a docs
link; the catalogue is in `docs/SPEC.md` section 12 and `src/impl_/diag.rs`. Each code has a
compile-fail test in `tests/ui/` (the `h1_`..`m5_` and `l3_`/`l5_` files are named after the findings
of `.10x/reviews/2026-09-30-keel-macros-review.md`). After a diagnostic the item is still emitted, and
a failed `#[keel::store]` still defines the members its impl block uses, so the real error is the
only one. What `rustc` reports by itself (a duplicate `#[keel::api] impl`, a query inside a plain
`impl`) is named after the rule where the macro can manage it.

## Tests

* `cargo test -p keel-macros --lib`: type mapper, attribute parsing, expansion snapshots
  (`UPDATE_SNAPSHOTS=1` rewrites `tests/snapshots/*.rs`) and a table test of every diagnostic.
* `cargo test -p keel-macros --test wire_types --test objects --test stores --test ports --test queries --test edge_cases --test review`:
  the generated code is compiled against the real `keel` facade (`keel-runtime`, `keel-signals`)
  and *run*: dispatchers found in the registry and called through a real runtime
  (`tests/support`), proxies, restore functions, keyed patches, event subscriptions.
* `--test compile_fail`: trybuild expectations for the diagnostics
  (`TRYBUILD=overwrite` regenerates them).
* `--test ui_runtime`: compile-pass files written as a user writes a core
  (`tests/ui-runtime/`), checked against the real runtime and signals.

`crates/keel/tests/e2e_todo.rs` drives generated code through `TestRuntime` and asserts on the
decoded wire payloads.
