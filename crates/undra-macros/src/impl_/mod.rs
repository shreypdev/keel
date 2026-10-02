//! The implementation of the Undra attribute macros.
//!
//! `proc-macro` crates cannot export ordinary functions, and macros cannot be unit tested
//! through the compiler, so every macro is a thin wrapper in `lib.rs` around one of the
//! `expand_*` functions here. They map token streams to token streams using `proc_macro2`
//! only, which is what lets the snapshot tests in `src/tests` run them on fixture inputs.
//!
//! Error handling: a macro that finds a problem emits the diagnostics as `compile_error!`
//! *and* the original item with its helper attributes stripped, so the user sees exactly
//! the Undra diagnostics and no cascade of "cannot find type" or "unknown attribute" errors.

use proc_macro2::TokenStream;
use quote::quote;
use syn::visit_mut::VisitMut;

pub(crate) mod attrs;
pub(crate) mod check;
pub(crate) mod common;
pub(crate) mod diag;
pub(crate) mod error;
pub(crate) mod migrate;
pub(crate) mod naming;
pub(crate) mod object;
pub(crate) mod paths;
pub(crate) mod port;
pub(crate) mod query;
pub(crate) mod record;
pub(crate) mod store;
pub(crate) mod types;

use attrs::{StripHelpers, flag, parse_args, root_arg};
use diag::{Diag, code};
use paths::Root;
use record::Mode;

/// Parses `item` as an [`syn::Item`] and runs `f` on it. On failure, emits the errors next to
/// the item with `undra` helper attributes (and those in `strip_also`) removed.
fn run(
    item: TokenStream,
    strip_also: &[&str],
    f: impl FnOnce(syn::Item) -> syn::Result<TokenStream>,
) -> TokenStream {
    run_recovering(item, strip_also, |_| TokenStream::new(), f)
}

/// Like [`run`], with a `recover` step for macros whose item is not self-contained: code written
/// next to it (the impl block of a store) uses members the macro adds to it. `recover` runs on the
/// original item, before the helper attributes are stripped, and may change it (the hidden field)
/// and return items to emit after it (the members the impl block expects), so the one real error
/// is not followed by errors about members that only exist because the macro failed.
fn run_recovering(
    item: TokenStream,
    strip_also: &[&str],
    recover: impl FnOnce(&mut syn::Item) -> TokenStream,
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
            let extra = recover(&mut fallback);
            StripHelpers::new(strip_also).visit_item_mut(&mut fallback);
            let errors = error.to_compile_error();
            quote! { #errors #fallback #extra }
        }
    }
}

fn wrong_item(macro_name: &str, expected: &str, item: &syn::Item) -> syn::Error {
    Diag::new(
        code::E0007,
        format!(
            "`#[undra::{macro_name}]` cannot be applied to {}",
            item_kind(item)
        ),
        format!("`#[undra::{macro_name}]` applies to {expected}"),
        format!("move `#[undra::{macro_name}]` onto {expected}, or remove it"),
    )
    .on(&Pointer::of(item))
}

/// Where a diagnostic about a whole item points: its name if it has one (the rest of a `struct`
/// or an `enum` is not what is wrong with it), else the item.
struct Pointer<'a>(&'a syn::Item);

impl<'a> Pointer<'a> {
    fn of(item: &'a syn::Item) -> Pointer<'a> {
        Pointer(item)
    }
}

impl quote::ToTokens for Pointer<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        use syn::Item;
        match self.0 {
            Item::Const(i) => i.ident.to_tokens(tokens),
            Item::Enum(i) => i.ident.to_tokens(tokens),
            Item::Fn(i) => i.sig.ident.to_tokens(tokens),
            Item::Mod(i) => i.ident.to_tokens(tokens),
            Item::Static(i) => i.ident.to_tokens(tokens),
            Item::Struct(i) => i.ident.to_tokens(tokens),
            Item::Trait(i) => i.ident.to_tokens(tokens),
            Item::TraitAlias(i) => i.ident.to_tokens(tokens),
            Item::Type(i) => i.ident.to_tokens(tokens),
            Item::Union(i) => i.ident.to_tokens(tokens),
            other => other.to_tokens(tokens),
        }
    }
}

/// What an item is, with its article, for a sentence ("a `trait`", "an `enum`").
fn item_kind(item: &syn::Item) -> &'static str {
    match item {
        syn::Item::Const(_) => "a `const`",
        syn::Item::Enum(_) => "an `enum`",
        syn::Item::ExternCrate(_) => "an `extern crate` declaration",
        syn::Item::Fn(_) => "a `fn`",
        syn::Item::ForeignMod(_) => "an `extern` block",
        syn::Item::Impl(_) => "an `impl` block",
        syn::Item::Macro(_) => "a macro invocation",
        syn::Item::Mod(_) => "a `mod`",
        syn::Item::Static(_) => "a `static`",
        syn::Item::Struct(_) => "a `struct`",
        syn::Item::Trait(_) => "a `trait`",
        syn::Item::TraitAlias(_) => "a trait alias",
        syn::Item::Type(_) => "a type alias",
        syn::Item::Union(_) => "a `union`",
        syn::Item::Use(_) => "a `use` declaration",
        _ => "this item",
    }
}

/// The `crate = ".."` path of a macro's arguments, for a recovery step that only needs that:
/// errors in the arguments are reported by the expansion itself.
fn recovery_root(attr: TokenStream, macro_name: &str) -> Option<Root> {
    let mut root: Option<Root> = None;
    let _ = parse_args(attr, macro_name, "", |meta| {
        if meta.path.is_ident("crate") {
            root = root_arg(meta).ok();
        }
        Ok(true)
    });
    root
}

/// `#[undra::api]`.
pub(crate) fn expand_api(attr: TokenStream, item: TokenStream) -> TokenStream {
    // `error`, `from` and `source` belong to `#[undra::error]`; on a plain enum that derives no
    // `Error` they are reported (E0010) and then dropped, so `rustc` does not add "cannot find
    // attribute" on top. Where another derive (`thiserror::Error`) may own them they are left.
    let strip: &[&str] = match syn::parse2::<syn::Item>(item.clone()) {
        Ok(parsed) if record::drops_error_helpers(&parsed) => &["error", "from", "source"],
        _ => &[],
    };
    let recovery_attr = attr.clone();
    let recover = move |item: &mut syn::Item| {
        record::recover(recovery_root(recovery_attr, "api"), Mode::Api, item)
    };
    run_recovering(item, strip, recover, |item| {
        let mut root: Option<Root> = None;
        let mut store: Option<proc_macro2::Span> = None;
        parse_args(
            attr,
            "api",
            "`crate = \"path\"`, and `store` on an impl block of a `#[undra::store]` struct",
            |meta| {
                if meta.path.is_ident("crate") {
                    root = Some(root_arg(meta)?);
                    Ok(true)
                } else if meta.path.is_ident("store") {
                    flag(meta, code::E0008, "store")?;
                    store = Some(syn::spanned::Spanned::span(&meta.path));
                    Ok(true)
                } else {
                    Ok(false)
                }
            },
        )?;
        if let Some(span) = store.filter(|_| !matches!(item, syn::Item::Impl(_))) {
            return Err(Diag::new(
                code::E0008,
                "`store` is only valid on an `impl` block",
                "`#[undra::api(store)]` marks the impl block of a `#[undra::store]` struct so constructors can be wired to the store's signals",
                "remove `store`, or apply the attribute to the store's impl block",
            )
            .at(span));
        }
        match item {
            syn::Item::Struct(item) => record::expand_struct(root, item),
            syn::Item::Enum(item) => record::expand_enum(root, item, Mode::Api),
            syn::Item::Impl(item) => object::expand_impl(root, store.is_some(), item),
            syn::Item::Fn(item) => object::expand_fn(root, item),
            other => Err(wrong_item(
                "api",
                "a struct, an enum, an `impl` block or a free `fn`",
                &other,
            )),
        }
    })
}

/// `#[undra::error]`.
pub(crate) fn expand_error(attr: TokenStream, item: TokenStream) -> TokenStream {
    let recovery_attr = attr.clone();
    let recover = move |item: &mut syn::Item| {
        record::recover(recovery_root(recovery_attr, "error"), Mode::Error, item)
    };
    run_recovering(item, &["error", "from", "source"], recover, |item| {
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
            other => Err(wrong_item("error", "an enum", &other)),
        }
    })
}

/// `#[undra::query]` and `#[undra::mutation]`.
pub(crate) fn expand_query(
    flavor: query::Flavor,
    attr: TokenStream,
    item: TokenStream,
) -> TokenStream {
    run(item, &[], |item| {
        let args = query::parse_query_args(attr, flavor)?;
        match item {
            syn::Item::Fn(item) => query::expand(flavor, args, item),
            other => Err(wrong_item(
                match flavor {
                    query::Flavor::Query => "query",
                    query::Flavor::Mutation => "mutation",
                },
                "an `async fn`",
                &other,
            )),
        }
    })
}

/// `#[undra::migrate]`.
pub(crate) fn expand_migrate(attr: TokenStream, item: TokenStream) -> TokenStream {
    run(item, &[], |item| match item {
        syn::Item::Fn(item) => migrate::expand(attr, item),
        other => Err(wrong_item("migrate", "a free `fn`", &other)),
    })
}

/// `#[undra::port]`.
pub(crate) fn expand_port(attr: TokenStream, item: TokenStream) -> TokenStream {
    run(item, &[], |item| match item {
        syn::Item::Trait(item) => {
            let (root, requested, dispatcher_by_use) = port::parse_port_args(attr)?;
            port::expand_trait(root, requested, dispatcher_by_use, item)
        }
        syn::Item::Impl(item) => {
            parse_args(
                attr,
                "port",
                "nothing on an `impl Trait for Type` block",
                |_| Ok(false),
            )?;
            port::expand_impl(item)
        }
        other => Err(wrong_item(
            "port",
            "a trait definition or an `impl Trait for Type` block",
            &other,
        )),
    })
}

/// `#[undra::store]`.
pub(crate) fn expand_store(attr: TokenStream, item: TokenStream) -> TokenStream {
    let recovery_attr = attr.clone();
    let recover =
        move |item: &mut syn::Item| store::recover(recovery_root(recovery_attr, "store"), item);
    run_recovering(item, &[], recover, |item| {
        let mut root: Option<Root> = None;
        let mut hook: Option<syn::Path> = None;
        parse_args(
            attr,
            "store",
            "`crate = \"path\"` and `restore = \"Self::function\"`",
            |meta| {
                if meta.path.is_ident("crate") {
                    root = Some(root_arg(meta)?);
                    Ok(true)
                } else if meta.path.is_ident("restore") {
                    let example = "restore = \"Self::rebuild\"";
                    if !meta.input.peek(syn::Token![=]) {
                        return Err(Diag::new(
                            code::E0008,
                            "`restore` needs a value",
                            "`restore` names the function that rebuilds the store from its plain signals",
                            format!("write `{example}`"),
                        )
                        .on(&meta.path));
                    }
                    let value = meta.value()?;
                    let not_a_path = |span: proc_macro2::Span| {
                        Diag::new(
                            code::E0008,
                            "`restore` must name a function by path",
                            "`restore` names the function that rebuilds the store from its plain signals",
                            format!("write a path such as `{example}`"),
                        )
                        .at(span)
                    };
                    hook = Some(if value.peek(syn::LitStr) {
                        let lit: syn::LitStr = value.parse()?;
                        lit.parse::<syn::Path>()
                            .map_err(|_| not_a_path(lit.span()))?
                    } else {
                        value
                            .parse::<syn::Path>()
                            .map_err(|e| not_a_path(e.span()))?
                    });
                    Ok(true)
                } else {
                    Ok(false)
                }
            },
        )?;
        match item {
            syn::Item::Struct(item) => store::expand_store(root, hook, item),
            other => Err(wrong_item("store", "a struct", &other)),
        }
    })
}
