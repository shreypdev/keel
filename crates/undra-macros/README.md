# undra-macros

The attribute macros of Undra: `#[undra::api]`, `#[undra::error]`, `#[undra::store]`,
`#[undra::port]`, `#[undra::query]` and `#[undra::mutation]`. Users get them from the `undra`
facade; this crate is the implementation (`docs/SPEC.md` section 4).

```rust
use undra::prelude::*;

// Records, enums and errors: encoding, decoding and the schema come from the type.
#[undra::api]
#[derive(Clone)]
pub struct Todo { pub id: Uuid, pub title: String, #[undra(default)] pub done: bool }

#[undra::api]
pub enum Filter { All, Active, Done }

#[undra::error]
pub enum TodoError {
    #[error("title cannot be empty")] EmptyTitle,
    #[error(transparent)] Http(#[from] HttpError),
}

// A store: signals the platforms mirror, commands they call.
#[undra::store(restore = "Self::assemble")]
pub struct Todos {
    ctx: Ctx,
    todos: Signal<Vec<Todo>>,
    filter: Signal<Filter>,
    visible: Computed<Vec<Todo>>,
}

#[undra::api(store)]
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
#[undra::port]
pub trait Http { async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>; }

#[undra::query(key = "todos:{page}", stale = "30s", persist)]
pub async fn todos(ctx: &Ctx, page: u32) -> Result<Vec<Todo>, HttpError> { /* .. */ }
```

## What is generated

| Macro | Generated (besides the item as written) |
|---|---|
| `api` on a struct | `impl Encode`, `impl Decode`, `UNDRA_TYPE_ID`, the hidden field table a keyed list looks its key up in (`__UNDRA_FIELDS`, `__undra_encode_field`), `static __UNDRA_META_<T>: RecordMeta`, registration |
| `api` / `error` on an enum | the same with a `u16` variant index; `error` adds `Display`, `Error`, `From` for `#[from]` |
| `api` on `impl T` | `impl UndraObject`, `fn __undra_dispatch_<T>` and `ObjectMeta` + registration (private to an anonymous `const _`, so a second block for the type does not define them twice) |
| `api` on a `fn` | `fn __undra_dispatch_fn_<name>`, `FunctionMeta` + registration |
| `store` on a struct | hidden `CellSlot` field, hidden `cell`/`restore` members (`impl StoreObject` is written by the `api(store)` impl block), a builder that attaches every signal (`attach`, `attach_keyed` with a typed key fn, `attach_computed`, `set_no_coalesce`), `StoreMeta`, `StoreRestorer` registration |
| `port` on a trait | `impl Port for dyn T`, `<T>Proxy`, accessor `fn <t>(ctx)`, `__undra_port_dispatch_<T>`, `PortMeta` |
| `query` / `mutation` | `<Name>Query` / `<Name>Mutation` with the ids and settings, `QueryDef` / `MutationDef`, `QueryMeta`, and a `QueryRegistration` / `MutationRegistration` (how `undra-query` finds the definition by id) |

Every expansion also carries compile-time checks that the types the schema records are the types
written in the code (`src/impl_/check.rs`): a user type called `Bytes`, an alias or a renamed import
(`use m::Item as Todo`) is an error in the user's crate (E0060, E0061), not a schema that disagrees
with its own wire layout. The proxy of a port method returning `Result<T, E>` reports an unavailable
port as `E::from(PortError)` (E0033 if `E` lacks the impl) instead of panicking; a method without an
error channel panics with a message that says how to bind the port (E0062). See ADR-025.

Generated code names its dependencies as `::undra::{wire, meta, runtime, signals, query}`;
`#[undra(crate = "path")]` on the item (or `crate = "path"` in the macro arguments) replaces
`::undra`. The exact paths and shapes are pinned by the snapshots in `tests/snapshots/`.

## Diagnostics

Every rejection is `error[undra::E00NN]: what`, with a note (why), a help (the fix) and a docs
link; the catalogue is in `docs/SPEC.md` section 12 and `src/impl_/diag.rs`. Each code has a
compile-fail test in `tests/ui/` or a message golden (`crates/*/tests/golden/diagnostics/`; the
`h1_`..`m5_` and `l3_`/`l5_` files are named after the findings of
`.10x/reviews/2026-09-30-undra-macros-review.md`, the `d1_` files after the diagnostic polish that
followed it). `tests/catalogue.rs` audits all of it: SPEC, the code table, the emitting sites, the
goldens and the docs links agree, and every message has what, why, fix and the link of its code.
After a diagnostic the item is still emitted, and a failed `#[undra::store]` still defines the
members its impl block uses, so the real error is the only one.

What a macro cannot see it hands to `rustc` in a form that names the rule: a second
`#[undra::api] impl` for a type is reported as the redefinition of
`_undra_error_E0007_<Type>_has_two_undra_api_impl_blocks_merge_them_into_one`; a query in an
`impl` block that is not `#[undra::api]` as an unknown macro called
`_undra_error_E0007_a_query_is_a_free_function_move_it_out_of_the_impl_block`; a non-`Send` future
(E0022) is `rustc`'s own error, and its last note names the assertion
`_undra_error_E0022_the_future_of_an_async_method_must_be_Send`. A key that names no field of the
list's items is checked in the user's crate (it needs the item type) and reported on the string, with
the fields there are. Everything else `rustc` reports about a type the user wrote (a struct without
`#[undra::api]` has no `Encode`) carries the Undra message too, through
`#[diagnostic::on_unimplemented]`.

## Tests

* `cargo test -p undra-macros --lib`: type mapper, attribute parsing, expansion snapshots
  (`UPDATE_SNAPSHOTS=1` rewrites `tests/snapshots/*.rs`) and a table test of every diagnostic.
* `cargo test -p undra-macros --test wire_types --test objects --test stores --test ports --test queries --test edge_cases --test review`:
  the generated code is compiled against the real `undra` facade (`undra-runtime`, `undra-signals`)
  and *run*: dispatchers found in the registry and called through a real runtime
  (`tests/support`), proxies, restore functions, keyed patches, event subscriptions.
* `--test compile_fail`: trybuild expectations for the diagnostics
  (`TRYBUILD=overwrite` regenerates them).
* `--test catalogue`: the audit of the diagnostic catalogue (SPEC section 12, the code table, the
  emitting sites, the goldens of every crate, the docs links); it fails on a code without a
  constant, an emitter or a golden, and on a message without what, why, fix or the link of its code.
* `--test ui_runtime`: compile-pass files written as a user writes a core
  (`tests/ui-runtime/`), checked against the real runtime and signals.

`crates/undra/tests/e2e_todo.rs` drives generated code through `TestRuntime` and asserts on the
decoded wire payloads.
