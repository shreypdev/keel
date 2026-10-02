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
pub(crate) mod generic;
pub(crate) mod generic_fn;
pub(crate) mod generic_object;
pub(crate) mod migrate;
pub(crate) mod naming;
pub(crate) mod object;
pub(crate) mod paths;
pub(crate) mod port;
pub(crate) mod query;
pub(crate) mod record;
pub(crate) mod store;
pub(crate) mod types;

use attrs::{GenericList, StripHelpers, flag, generic_lists, parse_args, root_arg};
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

/// Whether the arguments of a macro mention `name` as an argument (`generic`, `store`): for a
/// recovery step, which has to know what the expansion that failed was trying to be.
fn mentions_argument(attr: &TokenStream, name: &str) -> bool {
    attr.clone().into_iter().any(|tree| match tree {
        proc_macro2::TokenTree::Ident(ident) => ident == name,
        _ => false,
    })
}

/// What a generic object or store whose expansion failed still has to define: its template, as a
/// macro that swallows the aliases written for it, so the one error that was reported is not
/// followed by "cannot find macro `Selection`" for each of them.
fn recover_generic_object(item: &syn::Item) -> TokenStream {
    match item {
        syn::Item::Impl(item) => match &*item.self_ty {
            syn::Type::Path(path) => match path.path.segments.last() {
                Some(seg) => generic_object::stub_template(&naming::unraw(&seg.ident)),
                None => TokenStream::new(),
            },
            _ => TokenStream::new(),
        },
        syn::Item::Struct(item) => {
            let name = naming::unraw(&item.ident);
            let compose = generic_object::stub_compose(&name);
            let template = generic_object::stub_template(&name);
            quote! { #compose #template }
        }
        _ => TokenStream::new(),
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
    let generic_block = mentions_argument(&attr, "generic");
    let recover = move |item: &mut syn::Item| {
        let stubs = if generic_block && matches!(item, syn::Item::Impl(_)) {
            recover_generic_object(item)
        } else {
            TokenStream::new()
        };
        let rest = record::recover(recovery_root(recovery_attr, "api"), Mode::Api, item);
        quote! { #stubs #rest }
    };
    run_recovering(item, strip, recover, |item| {
        let mut root: Option<Root> = None;
        let mut store: Option<proc_macro2::Span> = None;
        let mut generic: Option<proc_macro2::Span> = None;
        // `generic(T = [Todo, Note])` on a function: the types it crosses the boundary for.
        let mut lists: Vec<GenericList> = Vec::new();
        let mut lists_span: Option<proc_macro2::Span> = None;
        parse_args(
            attr,
            "api",
            "`crate = \"path\"`, `store` on an impl block of a `#[undra::store]` struct, `generic` on a struct, an enum or an impl block with type parameters, and `generic(T = [Todo, Note])` on a function with a type parameter",
            |meta| {
                if meta.path.is_ident("crate") {
                    root = Some(root_arg(meta)?);
                    Ok(true)
                } else if meta.path.is_ident("store") {
                    flag(meta, code::E0008, "store")?;
                    store = Some(syn::spanned::Spanned::span(&meta.path));
                    Ok(true)
                } else if meta.path.is_ident("generic") && meta.input.peek(syn::token::Paren) {
                    lists.extend(generic_lists(meta)?);
                    lists_span = Some(syn::spanned::Spanned::span(&meta.path));
                    Ok(true)
                } else if meta.path.is_ident("generic") {
                    flag(meta, code::E0008, "generic")?;
                    generic = Some(syn::spanned::Spanned::span(&meta.path));
                    Ok(true)
                } else {
                    Ok(false)
                }
            },
        )?;
        if let Some(span) = generic.filter(|_| {
            !matches!(
                item,
                syn::Item::Struct(_) | syn::Item::Enum(_) | syn::Item::Impl(_)
            )
        }) {
            return Err(Diag::new(
                code::E0008,
                "`generic` is only valid on a struct, an enum or an impl block",
                "`#[undra::api(generic)]` marks a type with type parameters, or the impl block of an object with them, as the template of named instantiations; a function with a type parameter lists the types it crosses for instead",
                "remove `generic`, apply the attribute to a struct, an enum or the impl block of an object with a type parameter, or list the types of a function: `generic(T = [Todo, Note])`",
            )
            .at(span));
        }
        if let Some(span) = lists_span.filter(|_| !matches!(item, syn::Item::Fn(_))) {
            return Err(match &item {
                syn::Item::Struct(_) | syn::Item::Enum(_) | syn::Item::Impl(_) => {
                    let name = match &item {
                        syn::Item::Struct(i) => i.ident.to_string(),
                        syn::Item::Enum(i) => i.ident.to_string(),
                        syn::Item::Impl(i) => ty_name(&i.self_ty),
                        _ => String::new(),
                    };
                    Diag::new(
                        code::E0008,
                        format!("`generic(..)` with a list on `{name}`"),
                        "a list instantiates the type parameter of a function; a type is instantiated under a name, by an alias",
                        format!(
                            "write `generic` alone and declare `#[undra::api] pub type Todo{name} = {name}<Todo>;`"
                        ),
                    )
                    .at(span)
                }
                other => wrong_item(
                    "api",
                    "a struct, an enum, an `impl` block, a free `fn` or a type alias that names an instantiation of a generic",
                    other,
                ),
            });
        }
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
            syn::Item::Struct(item) if generic.is_some() => {
                record::expand_struct_as(root, item, record::Expand::Template)
            }
            syn::Item::Enum(item) if generic.is_some() => {
                record::expand_enum_as(root, item, Mode::Api, record::Expand::Template)
            }
            syn::Item::Struct(item) => record::expand_struct(root, item),
            syn::Item::Enum(item) => record::expand_enum(root, item, Mode::Api),
            syn::Item::Impl(item) if generic.is_some() => {
                object::expand_impl_template(root, store.is_some(), item)
            }
            syn::Item::Impl(item) => object::expand_impl(root, store.is_some(), item),
            syn::Item::Fn(item) if !lists.is_empty() => {
                object::expand_generic_fn(root, item, &lists)
            }
            syn::Item::Fn(item) => object::expand_fn(root, item),
            syn::Item::Type(item) => generic::expand_alias(root, item),
            other => Err(wrong_item(
                "api",
                "a struct, an enum, an `impl` block, a free `fn` or a type alias that names an instantiation of a generic",
                &other,
            )),
        }
    })
}

/// The name of the type of an impl block, for a message.
fn ty_name(ty: &syn::Type) -> String {
    match ty {
        syn::Type::Path(path) => path
            .path
            .segments
            .last()
            .map_or_else(String::new, |seg| seg.ident.to_string()),
        other => types::ty_string(other),
    }
}

/// `undra::__instantiate!`: what the hidden macro of a generic data type calls (ADR-042).
pub(crate) fn expand_instantiate(input: TokenStream) -> TokenStream {
    generic::expand_instantiate(input)
}

/// `undra::__compose_store!`: what the local macro of a generic store's struct calls with the
/// signatures its impl block handed over (ADR-058).
pub(crate) fn expand_compose_store(input: TokenStream) -> TokenStream {
    generic_object::expand_compose_store(input)
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
            port::expand_trait(root, requested, dispatcher_by_use, false, item)
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

/// `#[undra::callback]` (ADR-041).
pub(crate) fn expand_callback(attr: TokenStream, item: TokenStream) -> TokenStream {
    run(item, &[], |item| match item {
        syn::Item::Trait(item) => {
            let (root, background) = port::parse_callback_args(attr)?;
            port::expand_trait(root, port::Requested::Callback, false, background, item)
        }
        other => Err(wrong_item("callback", "a trait definition", &other)),
    })
}

/// `#[undra::store]`.
pub(crate) fn expand_store(attr: TokenStream, item: TokenStream) -> TokenStream {
    let recovery_attr = attr.clone();
    let generic_store = mentions_argument(&attr, "generic");
    let recover = move |item: &mut syn::Item| {
        let stubs = if generic_store {
            recover_generic_object(item)
        } else {
            TokenStream::new()
        };
        let rest = store::recover(recovery_root(recovery_attr, "store"), item);
        quote! { #stubs #rest }
    };
    run_recovering(item, &[], recover, |item| {
        let mut root: Option<Root> = None;
        let mut hook: Option<syn::Path> = None;
        let mut generic = false;
        parse_args(
            attr,
            "store",
            "`crate = \"path\"`, `restore = \"Self::function\"` and `generic` on a struct with type parameters",
            |meta| {
                if meta.path.is_ident("crate") {
                    root = Some(root_arg(meta)?);
                    Ok(true)
                } else if meta.path.is_ident("generic") {
                    flag(meta, code::E0008, "generic")?;
                    generic = true;
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
            syn::Item::Struct(item) if generic => {
                store::expand_store_as(root, hook, item, store::StoreMode::Template)
            }
            syn::Item::Struct(item) => store::expand_store(root, hook, item),
            other => Err(wrong_item("store", "a struct", &other)),
        }
    })
}
