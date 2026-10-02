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
| `api` on a struct | `impl Encode`, `impl Decode`, `UNDRA_TYPE_ID`, the hidden field-name list a keyed list looks its key up in (`__UNDRA_FIELDS`), `static __UNDRA_META_<T>: RecordMeta`, registration |
| `api` on a tuple struct of one field | a **newtype** (ADR-042): the same, as a transparent record whose `Encode`/`Decode` delegate to `.0`, plus the hidden `Newtype` impl the `MapKey` check reads; no `From`, `Deref` or accessor |
| `api` / `error` on an enum | the same with a `u16` variant index; `error` adds `Display`, `Error`, `From` for `#[from]` |
| `api(generic)` on a struct or enum with type parameters | a **template** (ADR-042): the item, `impl<T: Encode> Encode` / `impl<T: Decode> Decode`, and a hidden `#[macro_export] macro_rules! __undra_template_<Name>` (re-exported as `<Name>`); registers nothing |
| `api` on `type TodoPage = Page<Todo>;` | an **instantiation**: the alias, and the template's macro, which calls the hidden `undra::__instantiate!` to register `TodoPage` (`RecordMeta`/`EnumMeta`) and add `impl TodoPage { UNDRA_TYPE_ID, .. }` |
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

The checks are of three kinds (ADR-042). Scalars and `String` and the `Vec`/`Option`/map constructors
are compared with the canonical type (E0060). A wire leaf (`Bytes`, `Uuid`, `Timestamp`, `Duration`,
`Decimal`) is a **membership** check, `Spelled: WireLeaf<kind>`: the canonical type qualifies, and so does
the type of each opt-in `undra` feature (`uuid::Uuid`, `chrono::DateTime<Utc>`, `time::OffsetDateTime`,
`rust_decimal::Decimal`, `bytes::Bytes`, ..). Every map key must be a `MapKey` (E0006): `String`, `bool`,
the integers, `Uuid` and every newtype of one of those, which is how `HashMap<UserId, User>` compiles and
`HashMap<Price, User>` with `struct Price(Decimal)` does not. A type of an opt-in crate that only that
crate has (`DateTime<Utc>`, `OffsetDateTime`, `TimeDelta`) spelled without its feature is one E0001 that
names the feature: this crate has a mirror of each feature (`uuid`, `chrono`, `time`, `rust_decimal`,
`bytes`) that `undra` turns on together with its own, because a macro cannot ask a crate what it
enabled. A bare `Uuid`, `Decimal`, `Bytes` or `Duration` may be the built-in type or the one of a crate, so
without the feature that case is the compiler's three errors (E0060 and the two codec bounds), which agree
on the fix.

**Generic data types** (`#[undra::api(generic)]`, see `src/impl_/generic.rs`) keep names where they resolve:
the template's own types are checked once, in the template's module, and an instantiation checks only its
type arguments, because a `macro_rules!` resolves names at the place it is invoked. An alias of one
instantiation twice, or outside the crate of its template, is E0070. The template's macro calls
`<root>::__instantiate!`, so a `crate = ".."` path has to reach a crate that re-exports it (the `undra`
facade does).

**Persisted state.** A newtype has the bytes of its inner type, so wrapping an existing field in a newtype
(`id: Uuid` becoming `id: UserId`), or unwrapping it, migrates without a hook (the structural step `T`
<-> `Newtype(T)` of ADR-037/042); a generic instantiation is a record like any other, and renaming its
alias is a rename of the type.

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
* `--test newtypes_generics`: newtypes, named instantiations and `Decimal` compiled and run: codecs, the
  schema they register, a keyed list whose key field is a newtype, map keys.
* `tests/ui-leaf-off/` (in `compile_fail`, only while no leaf feature is on): the types of other crates
  without their feature. With the features on, `cargo test -p undra --features
  uuid,chrono,time,rust_decimal,bytes` runs `crates/undra/tests/leaf_types.rs`.

`crates/undra/tests/e2e_todo.rs` drives generated code through `TestRuntime` and asserts on the
decoded wire payloads.
