//! The implementation of the Keel attribute macros.
//!
//! `proc-macro` crates cannot export ordinary functions, and macros cannot be unit tested
//! through the compiler, so every macro is a thin wrapper in `lib.rs` around one of the
//! `expand_*` functions here. They map token streams to token streams using `proc_macro2`
//! only, which is what lets the snapshot tests in `src/tests` run them on fixture inputs.
//!
//! Error handling: a macro that finds a problem emits the diagnostics as `compile_error!`
//! *and* the original item with its helper attributes stripped, so the user sees exactly
//! the Keel diagnostics and no cascade of "cannot find type" or "unknown attribute" errors.

use proc_macro2::TokenStream;
use quote::quote;
use syn::visit_mut::VisitMut;

pub(crate) mod attrs;
pub(crate) mod common;
pub(crate) mod diag;
pub(crate) mod error;
pub(crate) mod naming;
pub(crate) mod object;
pub(crate) mod paths;
pub(crate) mod record;
pub(crate) mod types;

use attrs::{StripHelpers, parse_args, root_arg};
use diag::{Diag, code};
use paths::Root;
use record::Mode;

/// Parses `item` as an [`syn::Item`] and runs `f` on it. On failure, emits the errors next to
/// the item with `keel` helper attributes (and those in `strip_also`) removed.
fn run(
    item: TokenStream,
    strip_also: &[&str],
    f: impl FnOnce(syn::Item) -> syn::Result<TokenStream>,
) -> TokenStream {
    let item: syn::Item = match syn::parse2(item) {
        Ok(item) => item,
        Err(error) => return error.to_compile_error(),
    };
    let fallback = item.clone();
    match f(item) {
        Ok(tokens) => tokens,
        Err(error) => {
            let mut fallback = fallback;
            StripHelpers::new(strip_also).visit_item_mut(&mut fallback);
            let errors = error.to_compile_error();
            quote! { #errors #fallback }
        }
    }
}

fn wrong_item(macro_name: &str, expected: &str, item: &syn::Item) -> syn::Error {
    Diag::new(
        code::E0007,
        format!("`#[keel::{macro_name}]` cannot be applied to this item"),
        format!("`#[keel::{macro_name}]` applies to {expected}"),
        "move the attribute to a supported item",
    )
    .on(item)
}

/// `#[keel::api]`.
pub(crate) fn expand_api(attr: TokenStream, item: TokenStream) -> TokenStream {
    run(item, &[], |item| {
        let mut root: Option<Root> = None;
        let mut store = false;
        parse_args(
            attr,
            "api",
            "`crate = \"path\"`, and `store` on an impl block of a `#[keel::store]` struct",
            |meta| {
                if meta.path.is_ident("crate") {
                    root = Some(root_arg(meta)?);
                    Ok(true)
                } else if meta.path.is_ident("store") {
                    store = true;
                    Ok(true)
                } else {
                    Ok(false)
                }
            },
        )?;
        if store && !matches!(item, syn::Item::Impl(_)) {
            return Err(Diag::new(
                code::E0008,
                "`store` is only valid on an `impl` block",
                "`#[keel::api(store)]` marks the impl block of a `#[keel::store]` struct so constructors can be wired to the store's signals",
                "remove `store`, or apply the attribute to the store's impl block",
            )
            .on(&item));
        }
        match item {
            syn::Item::Struct(item) => record::expand_struct(root, item),
            syn::Item::Enum(item) => record::expand_enum(root, item, Mode::Api),
            syn::Item::Impl(item) => object::expand_impl(root, store, item),
            syn::Item::Fn(item) => object::expand_fn(root, item),
            other => Err(wrong_item(
                "api",
                "structs, enums, `impl` blocks and free functions",
                &other,
            )),
        }
    })
}

/// `#[keel::error]`.
pub(crate) fn expand_error(attr: TokenStream, item: TokenStream) -> TokenStream {
    run(item, &["error", "from", "source"], |item| {
        let mut root: Option<Root> = None;
        parse_args(attr, "error", "`crate = \"path\"`", |meta| {
            if meta.path.is_ident("crate") {
                root = Some(root_arg(meta)?);
                Ok(true)
            } else {
                Ok(false)
            }
        })?;
        match item {
            syn::Item::Enum(item) => record::expand_enum(root, item, Mode::Error),
            other => Err(wrong_item("error", "enums", &other)),
        }
    })
}
