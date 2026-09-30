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
//! what is wrong, why the rule exists, how to fix it and a docs link.

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
