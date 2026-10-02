//! Undra attribute macros (`docs/SPEC.md` section 4).
//!
//! The macros are re-exported by the `undra` facade as `undra::api`, `undra::error`,
//! `undra::store`, `undra::port`, `undra::query`, `undra::mutation` and `undra::migrate`. Users
//! normally never depend on this crate directly.
//!
//! | Macro | Applies to | Generates |
//! |---|---|---|
//! | `#[undra::api]` | struct | `Encode`, `Decode`, `UNDRA_TYPE_ID`, `RecordMeta` + registration |
//! | `#[undra::api]` | enum | the same with a `u16` variant index; `EnumMeta` |
//! | `#[undra::api]` | struct with one unnamed field | a newtype: a transparent record that crosses as its inner type (ADR-042) |
//! | `#[undra::api(generic)]` | struct or enum with type parameters | a template: the item, generic codecs, a hidden `macro_rules!`; registers nothing |
//! | `#[undra::api]` | `type TodoPage = Page<Todo>;` | a named instantiation of a template: `RecordMeta` / `EnumMeta` for `TodoPage` |
//! | `#[undra::error]` | enum | as an enum, plus `Display`, `Error`, `From` for `#[from]` |
//! | `#[undra::api]` | `impl Type { .. }` | `UndraObject`, the dispatcher, `ObjectMeta` |
//! | `#[undra::api]` | free `fn` | the dispatcher, `FunctionMeta` |
//! | `#[undra::store]` | struct | `StoreObject`, signal table, restore, `StoreMeta` |
//! | `#[undra::port]` | trait | `Port`, the proxy, the accessor, the Rust-side dispatcher, `PortMeta` |
//! | `#[undra::callback]` | trait | `Port`, `CallbackInterface`, the proxy over a host instance, `PortMeta` (ADR-041) |
//! | `#[undra::query]` / `#[undra::mutation]` | `async fn` | `<Name>Query` / `<Name>Mutation`, `QueryMeta` |
//! | `#[undra::migrate]` | free `fn` | a `persist::Migration` registration (ADR-037) |
//!
//! Generated code names its dependencies through `::undra::{wire, meta, runtime, signals,
//! query}` (SPEC 16.3). `#[undra(crate = "path")]` on the item, or `crate = "path"` in the
//! macro arguments, replaces `::undra`.
//!
//! Every rejection is a diagnostic with a stable code (`error[undra::E0001]: ..`, SPEC 12):
//! what is wrong, why the rule exists, how to fix it and a docs link. After a diagnostic the
//! original item is still emitted (with its helper attributes removed), so the user sees the
//! Undra errors and no cascade; a failed `#[undra::store]` also keeps the hidden field and the
//! members its impl block uses, and a failed record, enum or error keeps behaviour-free
//! `Encode`/`Decode`/`UNDRA_TYPE_ID` impls so its users do not fail as well.
//!
//! What the macros cannot see from syntax, they check at compile time in the user's crate: that a
//! spelled type is the type the schema names (`Bytes`, `use a::Item as Todo`; E0060, E0061), that
//! the error side of a `Result` is a `#[undra::error]` enum, and that an object is not used as a
//! value (E0064). See `impl_::check`.
//!
//! # Things the macros do that the SPEC leaves open
//!
//! * `Box<T>` is transparent in the schema (`T`), and `Self` in a record or enum field means the
//!   type itself, so recursive types can be written: `children: Vec<Self>`.
//! * A store's impl block is marked `#[undra::api(store)]`; the struct gets a hidden
//!   `__undra_cell` field and struct literals of the type inside that impl block get it added.
//!   Stores with `Computed` fields name a rebuild function with
//!   `#[undra::store(restore = "Self::rebuild")]`. See [`store`].
//! * **Newtypes** (ADR-042): a tuple struct of exactly one field is a transparent record: it crosses as
//!   its inner type and may be a map key when that is one. A unit struct or a tuple struct of two or
//!   more fields stays E0007.
//! * **Generic data types** (ADR-042): `#[undra::api(generic)]` on a struct or enum with type parameters
//!   is a template that registers nothing; `#[undra::api] pub type TodoPage = Page<Todo>;` registers the
//!   instantiation `TodoPage`. Signatures spell the alias; `Page<Todo>` is E0002.
//! * `Lazy<T>` (a lazily paged list) is rejected with E0001 in v1, like in `undra-bindgen`.
//! * `#[undra::error]` derives `Debug` unless the enum already does.
//! * `async fn`s of a port trait become methods returning boxed futures (`async fn` in traits
//!   is not dyn compatible); `#[undra::port]` on `impl Trait for Type` blocks rewrites them back
//!   for the implementor. See [`port`].
//! * A returned `impl Stream<Item = T>` gets `+ 'static` added; a returned stream cannot borrow
//!   from the object.
//! * `#[cfg]` on fields, variants, parameters and public methods is rejected (E0008): the schema
//!   is hashed and must not depend on the build. A `cfg_attr` that only switches documentation
//!   or lint attributes (`#[cfg_attr(docsrs, doc(cfg(..)))]`) is fine.
//! * Documentation comes from `///` lines. `#[doc = include_str!(..)]` and other non-literal
//!   docs cannot be evaluated by a macro and are skipped (their sibling lines are kept). A
//!   store's docs are its struct's docs followed by its impl block's; a plain object's struct
//!   has no Undra attribute, so its docs are the impl block's.
//! * A method of a port that returns `Result<T, E>` reports an unavailable port as
//!   `E::from(PortError)`; one without an error channel panics with a message that names the
//!   port and how to bind it (E0062).
//!
//! One `#[undra::api] impl` block per type: the object impl and the `__UNDRA_IS_OBJECT` marker are
//! defined on the type, so a second block would define them twice. A macro cannot see another
//! block, so `rustc` finds the second one and the expansion makes its error read as the rule
//! (`_undra_error_E0007_<Type>_has_two_undra_api_impl_blocks_merge_them_into_one`). The same goes for
//! a query or mutation written inside an `impl` block that is not `#[undra::api]`: the items it adds
//! are declared through a `macro_rules!` named after the rule (see `impl_::query`). A function whose
//! signature names `Self` is reported directly. `SPEC.md` section 16.3 lists these.
//!
//! The code table (`impl_::diag`), SPEC section 12, the emitting sites and the goldens are audited
//! against each other by the integration test `tests/catalogue.rs`.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

extern crate proc_macro;

mod impl_;

#[cfg(test)]
mod tests;

use proc_macro::TokenStream;

/// Marks a public type or function of the core.
///
/// * on a `struct`: a record, encoded field by field;
/// * on an `enum`: an enum encoded as a `u16` variant index plus fields;
/// * on an `impl Type { .. }` block: an object whose `pub fn`s become methods
///   (`#[undra::api(store)]` for the impl block of a `#[undra::store]` struct);
/// * on a free `fn`: a function.
///
/// `#[undra(default)]` on a field marks it as having a default in generated constructors (and lets a
/// migration fill it when an older value lacks it, ADR-037).
#[proc_macro_attribute]
pub fn api(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_api(attr.into(), item.into()).into()
}

/// What the hidden `macro_rules!` of a `#[undra::api(generic)]` type calls to instantiate it
/// (ADR-042): not written by hand.
///
/// Takes the template's definition with the type arguments substituted, under the name of the
/// alias (`#[undra::api] pub type TodoPage = Page<Todo>;`), and registers that record or enum.
#[doc(hidden)]
#[proc_macro]
pub fn __instantiate(input: TokenStream) -> TokenStream {
    impl_::expand_instantiate(input.into()).into()
}

/// Marks an error enum: everything `#[undra::api]` does for an enum, plus `Display` from
/// `#[error("..")]`, `std::error::Error` and `From` for `#[from]` fields.
#[proc_macro_attribute]
pub fn error(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_error(attr.into(), item.into()).into()
}

/// Marks an `async fn(ctx: &Ctx, ..params) -> Result<T, E>` as a cached, keyed read managed by
/// `undra-query`.
///
/// Arguments: `key = "todos:{page}"` (required; `{param}` placeholders name parameters),
/// `stale = "30s"`, `persist`, `retry = 3` and `idempotent`. Generates `pub struct <Name>Query`
/// implementing `::undra::query::QueryDef`, plus `QueryMeta`.
#[proc_macro_attribute]
pub fn query(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_query(impl_::query::Flavor::Query, attr.into(), item.into()).into()
}

/// Marks an `async fn(ctx: &Ctx, ..params) -> Result<T, E>` as a mutation managed by
/// `undra-query`: optimistic patches, invalidation, retry and the offline queue.
///
/// Arguments: `retry = N`, `idempotent`, and an optional `key`; `stale` and `persist` are
/// rejected. Generates `pub struct <Name>Mutation` implementing `::undra::query::MutationDef`,
/// plus `QueryMeta`.
#[proc_macro_attribute]
pub fn mutation(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_query(impl_::query::Flavor::Mutation, attr.into(), item.into()).into()
}

/// Registers a migration hook for persisted data an older build wrote (ADR-037): snapshot
/// signals, cached query results and queued mutations whose types changed in a way structural
/// migration (fields and variants by name, lossless widenings) cannot convert.
///
/// * `#[undra::migrate(ty = "Todo")] fn f(old: &DynValue) -> Result<Todo, MigrateError>`: any
///   persisted `Todo` whose structure changed, at any depth;
/// * `#[undra::migrate(store = "Profile", signal = "age")] fn f(old: Option<&DynValue>) ->
///   Result<f32, MigrateError>`: one signal of one store in a snapshot (`None`: the snapshot lacks
///   it);
/// * `#[undra::migrate(mutation = "add_todo")] fn f(old: &DynRecord) -> Result<DynRecord,
///   MigrateError>`: the queued input of one mutation, by parameter name.
///
/// `from = "0x.."` restricts a hook to old data with that fingerprint. The function is kept as
/// written; the runtime calls it under the panic guard. A wrong target or shape is E0066.
///
/// Many changes need no hook. Fields and variants are matched by name, integers widen, `T` becomes
/// `Option<T>`, and (ADR-042) wrapping a value in a newtype or unwrapping it is lossless, because a
/// newtype has the bytes of its inner type: `id: Uuid` becoming `id: UserId` migrates by itself.
#[proc_macro_attribute]
pub fn migrate(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_migrate(attr.into(), item.into()).into()
}

/// Marks a trait as a port: an interface the platform (or a Rust fake) implements and the core
/// calls.
///
/// `#[undra::port(sync)]` asserts every method is synchronous; `#[undra::port(event)]` makes it a
/// host-to-core event port (methods return `()`). The trait gains `Send + Sync` supertraits and
/// its `async fn`s become methods returning boxed futures (`async fn` in traits is not dyn
/// compatible); apply `#[undra::port]` to `impl Trait for Type` blocks to keep writing `async fn`
/// in implementations.
///
/// Generated: `impl Port for dyn Trait`, `<Trait>Proxy`, an accessor `fn <trait_snake>(ctx)`, a
/// Rust-side dispatcher and `PortMeta` (event ports: `on_<trait>_<method>` subscriptions and
/// `encode_<trait>_<method>_event` payload encoders instead).
#[proc_macro_attribute]
pub fn port(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_port(attr.into(), item.into()).into()
}

/// Marks a trait as a host callback interface (ADR-041): the host implements it, once per
/// instance, and passes an instance to a method as `Arc<dyn Trait>`; the core calls it back.
///
/// A method either reports (`fn m(&self, ..)`, fire-and-forget; `#[undra(coalesce)]` keeps only
/// the newest pending call of a method per instance) or is `async` and returns `Result<T, E>`
/// with an `#[undra::error]` enum that implements `From<PortError>` (anything else is E0071).
/// `#[undra::callback(background)]` makes the host run implementations on a serial executor per
/// instance instead of on the main thread through the mirror's drain.
///
/// Generated: `impl Port for dyn Trait`, `<Trait>Proxy`, `impl CallbackInterface for dyn Trait`
/// and `PortMeta` with `kind: Callback`. Rust code and tests implement the trait directly and
/// pass their own `Arc`.
#[proc_macro_attribute]
pub fn callback(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_callback(attr.into(), item.into()).into()
}

/// Marks a struct as a store: an object whose `Signal<T>` and `Computed<T>` fields the
/// platforms mirror.
///
/// The macro appends a hidden `__undra_cell` field to the struct; struct literals of the type
/// inside its `#[undra::api(store)]` impl block get it added automatically. See the
/// `store` module of the implementation for the full contract, including
/// `#[undra::store(restore = "Self::rebuild")]`.
///
/// `#[undra(key = "id")]` on a `Signal<Vec<T>>` enables keyed patches, `#[undra(no_coalesce)]`
/// delivers every commit of a signal.
#[proc_macro_attribute]
pub fn store(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_store(attr.into(), item.into()).into()
}
