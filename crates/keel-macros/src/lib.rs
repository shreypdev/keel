//! Keel attribute macros (`docs/SPEC.md` section 4).
//!
//! The macros are re-exported by the `keel` facade as `keel::api`, `keel::error`,
//! `keel::store`, `keel::port`, `keel::query` and `keel::mutation`. Users normally never
//! depend on this crate directly.
//!
//! | Macro | Applies to | Generates |
//! |---|---|---|
//! | `#[keel::api]` | struct | `Encode`, `Decode`, `KEEL_TYPE_ID`, `RecordMeta` + registration |
//! | `#[keel::api]` | enum | the same with a `u16` variant index; `EnumMeta` |
//! | `#[keel::error]` | enum | as an enum, plus `Display`, `Error`, `From` for `#[from]` |
//! | `#[keel::api]` | `impl Type { .. }` | `KeelObject`, the dispatcher, `ObjectMeta` |
//! | `#[keel::api]` | free `fn` | the dispatcher, `FunctionMeta` |
//! | `#[keel::store]` | struct | `StoreObject`, signal table, restore, `StoreMeta` |
//! | `#[keel::port]` | trait | `Port`, the proxy, the accessor, the Rust-side dispatcher, `PortMeta` |
//! | `#[keel::query]` / `#[keel::mutation]` | `async fn` | `<Name>Query` / `<Name>Mutation`, `QueryMeta` |
//!
//! Generated code names its dependencies through `::keel::{wire, meta, runtime, signals,
//! query}` (SPEC 16.3). `#[keel(crate = "path")]` on the item, or `crate = "path"` in the
//! macro arguments, replaces `::keel`.
//!
//! Every rejection is a diagnostic with a stable code (`error[keel::E0001]: ..`, SPEC 12):
//! what is wrong, why the rule exists, how to fix it and a docs link. After a diagnostic the
//! original item is still emitted (with its helper attributes removed), so the user sees the
//! Keel errors and no cascade.
//!
//! # Things the macros do that the SPEC leaves open
//!
//! * `Box<T>` is transparent in the schema (`T`), and `Self` in a record or enum field means the
//!   type itself, so recursive types can be written: `children: Vec<Self>`.
//! * A store's impl block is marked `#[keel::api(store)]`; the struct gets a hidden
//!   `__keel_cell` field and struct literals of the type inside that impl block get it added.
//!   Stores with `Computed`/`Lazy` fields name a rebuild function with
//!   `#[keel::store(restore = "Self::rebuild")]`. See [`store`].
//! * `#[keel::error]` derives `Debug` unless the enum already does.
//! * `async fn`s of a port trait become methods returning boxed futures (`async fn` in traits
//!   is not dyn compatible); `#[keel::port]` on `impl Trait for Type` blocks rewrites them back
//!   for the implementor. See [`port`].
//! * A returned `impl Stream<Item = T>` gets `+ 'static` added; a returned stream cannot borrow
//!   from the object.
//! * `#[cfg]` on fields, variants, parameters and public methods is rejected (E0008): the schema
//!   is hashed and must not depend on the build.
//!
//! One `#[keel::api] impl` block per type: the registration and the dispatcher are named after
//! the type, so a second block would define them twice.

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
///   (`#[keel::api(store)]` for the impl block of a `#[keel::store]` struct);
/// * on a free `fn`: a function.
///
/// `#[keel(default)]` on a field marks it as having a default in generated constructors.
#[proc_macro_attribute]
pub fn api(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_api(attr.into(), item.into()).into()
}

/// Marks an error enum: everything `#[keel::api]` does for an enum, plus `Display` from
/// `#[error("..")]`, `std::error::Error` and `From` for `#[from]` fields.
#[proc_macro_attribute]
pub fn error(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_error(attr.into(), item.into()).into()
}

/// Marks an `async fn(ctx: &Ctx, ..params) -> Result<T, E>` as a cached, keyed read managed by
/// `keel-query`.
///
/// Arguments: `key = "todos:{page}"` (required; `{param}` placeholders name parameters),
/// `stale = "30s"`, `persist`, `retry = 3` and `idempotent`. Generates `pub struct <Name>Query`
/// implementing `::keel::query::QueryDef`, plus `QueryMeta`.
#[proc_macro_attribute]
pub fn query(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_query(impl_::query::Flavor::Query, attr.into(), item.into()).into()
}

/// Marks an `async fn(ctx: &Ctx, ..params) -> Result<T, E>` as a mutation managed by
/// `keel-query`: optimistic patches, invalidation, retry and the offline queue.
///
/// Arguments: `retry = N`, `idempotent`, and an optional `key`; `stale` and `persist` are
/// rejected. Generates `pub struct <Name>Mutation` implementing `::keel::query::MutationDef`,
/// plus `QueryMeta`.
#[proc_macro_attribute]
pub fn mutation(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_query(impl_::query::Flavor::Mutation, attr.into(), item.into()).into()
}

/// Marks a trait as a port: an interface the platform (or a Rust fake) implements and the core
/// calls.
///
/// `#[keel::port(sync)]` asserts every method is synchronous; `#[keel::port(event)]` makes it a
/// host-to-core event port (methods return `()`). The trait gains `Send + Sync` supertraits and
/// its `async fn`s become methods returning boxed futures (`async fn` in traits is not dyn
/// compatible); apply `#[keel::port]` to `impl Trait for Type` blocks to keep writing `async fn`
/// in implementations.
///
/// Generated: `impl Port for dyn Trait`, `<Trait>Proxy`, an accessor `fn <trait_snake>(ctx)`, a
/// Rust-side dispatcher and `PortMeta` (event ports: `on_<trait>_<method>` subscriptions and
/// `encode_<trait>_<method>_event` payload encoders instead).
#[proc_macro_attribute]
pub fn port(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_port(attr.into(), item.into()).into()
}

/// Marks a struct as a store: an object whose `Signal<T>`, `Computed<T>` and `Lazy<T>` fields
/// the platforms mirror.
///
/// The macro appends a hidden `__keel_cell` field to the struct; struct literals of the type
/// inside its `#[keel::api(store)]` impl block get it added automatically. See the
/// `store` module of the implementation for the full contract, including
/// `#[keel::store(restore = "Self::rebuild")]`.
///
/// `#[keel(key = "id")]` on a `Signal<Vec<T>>` enables keyed patches, `#[keel(no_coalesce)]`
/// delivers every commit of a signal.
#[proc_macro_attribute]
pub fn store(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_store(attr.into(), item.into()).into()
}
