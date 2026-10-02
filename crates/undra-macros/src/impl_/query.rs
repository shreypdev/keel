//! `#[undra::query]` and `#[undra::mutation]` (SPEC 4.5 and 9).
//!
//! ```ignore
//! #[undra::query(key = "todos:{page}", stale = "30s", persist, retry = 3)]
//! pub async fn todos(ctx: &Ctx, page: u32) -> Result<Vec<Todo>, HttpError> { .. }
//!
//! #[undra::mutation(idempotent)]
//! pub async fn add_todo(ctx: &Ctx, title: String) -> Result<Todo, HttpError> { .. }
//! ```
//!
//! The function is kept. Next to it the macro emits `pub struct TodosQuery` (`AddTodoMutation`
//! for mutations): inherent constants (`QUERY_ID`/`MUTATION_ID`, `KEY`, `STALE_MS`, `PERSIST`,
//! `RETRY`, `IDEMPOTENT`), an implementation of `::undra::query::QueryDef` (`MutationDef`) whose
//! `Params`/`Input` is the tuple of the parameters after `ctx`, a `QueryMeta` registration (the
//! schema) and a `QueryRegistration` / `MutationRegistration` (how `undra-query` finds the
//! definition by id, for platform calls and the offline queue).
//!
//! Defaults: `retry` is 3 for queries (SPEC 9) and 0 for mutations (a mutation that is not
//! safe to replay must not retry silently); `stale` is absent (always stale); `persist` and
//! `idempotent` are off; a mutation's `key` defaults to the empty string.
//!
//! # Polling and paging (ADR-043)
//!
//! `interval = "30s"` (at least `1s`, else E0040: use a stream for real-time data) and
//! `poll_in_background` make a query poll. `infinite` makes it a paged query:
//!
//! ```ignore
//! #[undra::query(key = "feed/{filter}", infinite, item_key = "id", stale = "1m", refetch_pages = 3, persist, persist_pages = 2)]
//! async fn feed(ctx: &Ctx, filter: Filter, #[undra(cursor)] cursor: Option<String>)
//!     -> Result<undra::query::Page<Post, String>, ApiError> { .. }
//! ```
//!
//! `Page<T, C = String>` is recognised by its spelling in the success type (it is a plain Rust
//! struct, never a schema type); exactly one parameter is `#[undra(cursor)]` of type `Option<C>`
//! (it is not part of the key and not a parameter of the schema); `item_key` names a field of the
//! record `T` and is required. Each violation is E0073. The macro emits `QueryDef` with
//! `Output = Vec<T>` (and `fetch` the first page's rows), `InfiniteQueryDef` with the page fetch
//! and the key of a row, and a `QueryMeta` whose `params` leave the cursor out, whose `returns`
//! is `Result<Vec<T>, E>` and whose `infinite` names the cursor type and the item key.

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{FnArg, ItemFn, LitInt, LitStr, Pat};

use super::attrs::{flag, option_value, parse_args, root_arg};
use super::check::Checks;
use super::common::{GenericOn, item_root, mentions_self, param_meta, send_assertion, submit};
use super::diag::{Diag, Errors, code};
use super::naming::{pascal_case, unraw};
use super::object::{Kindred, ParamModel, analyze, arg_local};
use super::paths::Root;
use super::types::{Allow, KType, Pos, map_return_at, map_type, ty_string};

/// Whether the attribute is `#[undra::query]` or `#[undra::mutation]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flavor {
    Query,
    Mutation,
}

impl Flavor {
    fn attribute(self) -> &'static str {
        match self {
            Flavor::Query => "query",
            Flavor::Mutation => "mutation",
        }
    }
}

/// The parsed macro arguments.
#[derive(Default)]
pub(crate) struct Args {
    root: Option<Root>,
    key: Option<LitStr>,
    stale: Option<(u64, LitStr)>,
    persist: Option<Span>,
    retry: Option<u32>,
    idempotent: bool,
    interval: Option<(u64, LitStr)>,
    poll_in_background: Option<Span>,
    infinite: Option<Span>,
    item_key: Option<LitStr>,
    refetch_pages: Option<(u32, Span)>,
    persist_pages: Option<(u32, Span)>,
}

fn args_error(
    what: impl std::fmt::Display,
    why: &str,
    help: impl std::fmt::Display,
    span: Span,
) -> syn::Error {
    Diag::new(code::E0040, what, why, help).at(span)
}

/// The shortest polling interval the attribute accepts, in milliseconds (a stream is the tool
/// below it); `undra_query::MIN_POLL_INTERVAL_MS` is the same number at run time.
const MIN_INTERVAL_MS: u64 = 1_000;

/// Parses `"500ms"`, `"30s"`, `"5m"`, `"2h"`, `"1d"` into milliseconds.
pub(crate) fn parse_duration_ms(text: &str) -> Result<u64, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| format!("`{text}` has no unit"))?;
    let (digits, unit) = text.split_at(split);
    if digits.is_empty() {
        return Err(format!("`{text}` does not start with a whole number"));
    }
    let value: u64 = digits
        .parse()
        .map_err(|_| format!("`{digits}` is too large"))?;
    let factor: u64 = match unit.trim() {
        "ms" => 1,
        "s" => 1_000,
        "m" => 60_000,
        "h" => 3_600_000,
        "d" => 86_400_000,
        other => return Err(format!("`{other}` is not a unit (use ms, s, m, h or d)")),
    };
    value
        .checked_mul(factor)
        .ok_or_else(|| format!("`{text}` overflows"))
}

/// Parses the arguments of `#[undra::query(..)]` / `#[undra::mutation(..)]`.
pub(crate) fn parse_query_args(attr: TokenStream, flavor: Flavor) -> syn::Result<Args> {
    let mut args = Args::default();
    parse_args(
        attr,
        flavor.attribute(),
        "`key = \"..\"`, `stale = \"30s\"`, `persist`, `retry = N`, `idempotent`, `interval = \"30s\"`, `poll_in_background`, `infinite`, `item_key = \"id\"`, `refetch_pages = N`, `persist_pages = N` and `crate = \"path\"`",
        |meta| {
            let name = meta
                .path
                .get_ident()
                .map(ToString::to_string)
                .unwrap_or_default();
            match name.as_str() {
                "crate" => {
                    args.root = Some(root_arg(meta)?);
                    Ok(true)
                }
                "key" => {
                    args.key = Some(option_value(
                        meta,
                        code::E0040,
                        "key",
                        "a string literal",
                        "key = \"todos:{page}\"",
                    )?);
                    Ok(true)
                }
                "stale" => {
                    let lit: LitStr = option_value(
                        meta,
                        code::E0040,
                        "stale",
                        "a string literal with a unit",
                        "stale = \"30s\"",
                    )?;
                    match parse_duration_ms(&lit.value()) {
                        Ok(ms) => args.stale = Some((ms, lit)),
                        Err(reason) => {
                            return Err(args_error(
                                format!("invalid `stale` duration: {reason}"),
                                "`stale` is how long a cached result counts as fresh",
                                "write a whole number and a unit, for example `stale = \"30s\"`",
                                lit.span(),
                            ));
                        }
                    }
                    Ok(true)
                }
                "persist" => {
                    flag(meta, code::E0040, "persist")?;
                    args.persist = Some(meta.path.span());
                    Ok(true)
                }
                "retry" => {
                    let lit: LitInt =
                        option_value(meta, code::E0040, "retry", "a whole number", "retry = 3")?;
                    args.retry = Some(lit.base10_parse::<u32>().map_err(|_| {
                        args_error(
                            "`retry` must be a whole number of attempts",
                            "`retry = 3` means three attempts after the first failure",
                            "write `retry = 3`",
                            lit.span(),
                        )
                    })?);
                    Ok(true)
                }
                "idempotent" => {
                    flag(meta, code::E0040, "idempotent")?;
                    args.idempotent = true;
                    Ok(true)
                }
                "interval" => {
                    let lit: LitStr = option_value(
                        meta,
                        code::E0040,
                        "interval",
                        "a string literal with a unit",
                        "interval = \"30s\"",
                    )?;
                    match parse_duration_ms(&lit.value()) {
                        Ok(ms) if ms >= MIN_INTERVAL_MS => args.interval = Some((ms, lit)),
                        Ok(_) => {
                            return Err(args_error(
                                format!("`interval = \"{}\"` is below one second", lit.value()),
                                "a poll fetches the whole result again each time, so a query polled more than once a second is really a stream: use a stream for real-time data",
                                "write `interval = \"1s\"` or more, or deliver the changes with a `#[undra::port]` event or a stream",
                                lit.span(),
                            ));
                        }
                        Err(reason) => {
                            return Err(args_error(
                                format!("invalid `interval` duration: {reason}"),
                                "`interval` is how long after a fetch ends the next one starts",
                                "write a whole number and a unit, for example `interval = \"30s\"`",
                                lit.span(),
                            ));
                        }
                    }
                    Ok(true)
                }
                "poll_in_background" => {
                    flag(meta, code::E0040, "poll_in_background")?;
                    args.poll_in_background = Some(meta.path.span());
                    Ok(true)
                }
                "infinite" => {
                    flag(meta, code::E0040, "infinite")?;
                    args.infinite = Some(meta.path.span());
                    Ok(true)
                }
                "item_key" => {
                    args.item_key = Some(option_value(
                        meta,
                        code::E0040,
                        "item_key",
                        "a string literal naming a field",
                        "item_key = \"id\"",
                    )?);
                    Ok(true)
                }
                "refetch_pages" | "persist_pages" => {
                    let lit: LitInt = option_value(
                        meta,
                        code::E0040,
                        &name,
                        "a whole number of pages",
                        &format!("{name} = 3"),
                    )?;
                    let pages = lit
                        .base10_parse::<u32>()
                        .ok()
                        .filter(|n| *n >= 1)
                        .ok_or_else(|| {
                            args_error(
                                format!("`{name}` must be a whole number of at least 1"),
                                "it counts pages, and a list has its first page at least",
                                format!("write `{name} = 3`"),
                                lit.span(),
                            )
                        })?;
                    let slot = if name == "refetch_pages" {
                        &mut args.refetch_pages
                    } else {
                        &mut args.persist_pages
                    };
                    *slot = Some((pages, meta.path.span()));
                    Ok(true)
                }
                _ => Ok(false),
            }
        },
    )?;
    Ok(args)
}

/// Checks that every `{name}` in the key names a parameter, and that braces balance.
fn check_key(key: &LitStr, params: &[String]) -> syn::Result<()> {
    let text = key.value();
    let mut rest = text.as_str();
    while let Some(open) = rest.find(['{', '}']) {
        if rest[open..].starts_with('}') {
            return Err(args_error(
                "unmatched `}` in `key`",
                "`{param}` marks where a parameter is spliced into the cache key",
                "balance the braces",
                key.span(),
            ));
        }
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            return Err(args_error(
                "unterminated `{` in `key`",
                "`{param}` marks where a parameter is spliced into the cache key",
                "close the brace",
                key.span(),
            ));
        };
        let name = &after[..close];
        if !params.iter().any(|p| p == name) {
            let known = if params.is_empty() {
                "the function has no parameters after `ctx`".to_owned()
            } else {
                format!("the parameters are: {}", params.join(", "))
            };
            return Err(args_error(
                format!("`{{{name}}}` in `key` is not a parameter of the function"),
                "each placeholder is replaced by the encoded parameter of that name",
                &known,
                key.span(),
            ));
        }
        rest = &after[close + 1..];
    }
    Ok(())
}

fn signature_error(what: String, node: &impl quote::ToTokens, why: &str, help: &str) -> syn::Error {
    Diag::new(code::E0041, what, why, help).on(node)
}

/// Expands `#[undra::query]` or `#[undra::mutation]` on an `async fn`.
pub(crate) fn expand(flavor: Flavor, args: Args, mut item: ItemFn) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    let root = item_root(&mut item.attrs, args.root.clone(), &mut errors);
    let attribute = flavor.attribute();
    let fn_name = unraw(&item.sig.ident);

    // Arguments per flavor.
    match flavor {
        Flavor::Query => {
            if args.key.is_none() {
                errors.push(
                    Diag::new(
                        code::E0040,
                        format!("query `{fn_name}` has no `key`"),
                        "the cache is addressed by key: two calls share a cached result only if their keys are equal",
                        format!(
                            "add `key = \"{fn_name}\"`, with `{{param}}` placeholders for the parameters that distinguish results"
                        ),
                    )
                    .on(&item.sig.ident),
                );
            }
        }
        Flavor::Mutation => {
            if let Some((_, lit)) = &args.stale {
                errors.push(
                    Diag::new(
                        code::E0040,
                        format!("mutation `{fn_name}` has `stale`"),
                        "`stale` is how long a cached *read* stays fresh; a mutation writes and caches nothing",
                        "remove `stale`; put it on the query the mutation invalidates",
                    )
                    .at(lit.span()),
                );
            }
            if let Some(span) = args.persist {
                errors.push(
                    Diag::new(
                        code::E0040,
                        format!("mutation `{fn_name}` has `persist`"),
                        "`persist` stores cached results on disk; a mutation has no result to cache (use `idempotent` to let the offline queue replay it)",
                        "remove `persist`",
                    )
                    .at(span),
                );
            }
            for (option, span) in args.query_only() {
                errors.push(
                    Diag::new(
                        code::E0040,
                        format!("mutation `{fn_name}` has `{option}`"),
                        "polling and paging apply to cached reads; a mutation writes and caches nothing",
                        format!("remove `{option}`; put it on the query the mutation invalidates"),
                    )
                    .at(span),
                );
            }
        }
    }
    let infinite = flavor == Flavor::Query && args.infinite.is_some();
    if flavor == Flavor::Query {
        check_infinite_arguments(&args, &fn_name, &mut errors);
    }
    // `#[undra(cursor)]` is the query macro's own parameter attribute: taken off before the
    // parameters are analysed, which refuse every other `#[undra(..)]` there.
    let cursors = take_cursor_attributes(&mut item.sig);
    if !infinite {
        for (name, span) in &cursors {
            errors.push(
                Diag::new(
                    code::E0073,
                    format!("`#[undra(cursor)]` on `{name}`, which is not a parameter of an `infinite` query"),
                    "the cursor is how an infinite query asks for its next page; an ordinary query has no pages",
                    "remove the attribute, or add `infinite` and `item_key = \"..\"` to the query",
                )
                .at(*span),
            );
        }
    } else if cursors.len() != 1 {
        errors.push(
            Diag::new(
                code::E0073,
                if cursors.is_empty() {
                    format!("the infinite query `{fn_name}` has no `#[undra(cursor)]` parameter")
                } else {
                    format!(
                        "the infinite query `{fn_name}` has {} `#[undra(cursor)]` parameters",
                        cursors.len()
                    )
                },
                "the core asks for each page with the cursor the previous one returned: exactly one parameter carries it, `None` for the first page",
                "mark one parameter `#[undra(cursor)] cursor: Option<String>` (or the cursor type your server uses)",
            )
            .on(&item.sig.ident),
        );
    }

    // The function itself.
    let generic_on = match flavor {
        Flavor::Query => GenericOn::Query,
        Flavor::Mutation => GenericOn::Mutation,
    };
    let analysis = analyze(&mut item.sig, &mut errors, Kindred::Query, Some(generic_on));
    if item.sig.asyncness.is_none() {
        errors.push(signature_error(
            format!("`{fn_name}` must be `async`"),
            &item.sig.ident,
            &format!("`#[undra::{attribute}]` functions fetch or write through ports, which are asynchronous"),
            "write `async fn`",
        ));
    }
    if analysis.has_receiver {
        errors.push(signature_error(
            format!("`{fn_name}` takes `self`"),
            &item.sig.ident,
            &format!("`#[undra::{attribute}]` applies to free functions"),
            "make it a free function",
        ));
    } else if mentions_self(item.sig.to_token_stream()) {
        // Written inside an `impl` block that is not `#[undra::api]`: a free function cannot
        // name `Self`. The one finding: the rest of the signature would only add noise.
        return Err(
            Diag::new(
                code::E0007,
                format!("`#[undra::{attribute}]` on `{fn_name}`, which is inside an `impl` block: its signature uses `Self`"),
                format!("a {attribute} is a free function, and the macro generates a struct next to it, which an impl block cannot hold"),
                format!("move `{fn_name}` out of the impl block, to module level, and spell out the type instead of `Self`"),
            )
            .on(&item.sig.ident),
        );
    }
    if analysis.ctx.is_none() {
        errors.push(signature_error(
            format!("`{fn_name}` must take `ctx: &Ctx` as its first parameter"),
            &item.sig.ident,
            "the runtime hands every query and mutation a context to reach ports and other queries",
            "add `ctx: &Ctx` (or `ctx: Ctx`) before the other parameters",
        ));
    }
    // An infinite query's success type is `Page<T, C>`; the schema and the cache know it as the
    // list `Vec<T>` its handle shows.
    let shape = if infinite {
        infinite_shape(
            &item.sig.output,
            &analysis.params,
            &cursors,
            &args,
            &fn_name,
            &mut errors,
        )
    } else {
        None
    };
    let output: syn::ReturnType = shape
        .as_ref()
        .map_or_else(|| item.sig.output.clone(), |shape| shape.output.clone());
    let ret = if infinite && shape.is_none() {
        // Reported above (E0073); mapping the original spelling would add noise.
        KType::Unit
    } else {
        match map_return_at(&output, Pos::QueryReturn) {
            Ok(ret) => ret,
            Err(err) => {
                errors.push(err.into_error());
                KType::Unit
            }
        }
    };
    let (ok_ty, err_ty) = match (&ret, &output, &shape) {
        (KType::Result(..), _, Some(shape)) => {
            (Some(shape.list_ty.clone()), Some(shape.err_ty.clone()))
        }
        (KType::Result(ok, _), syn::ReturnType::Type(_, ty), None)
            if !matches!(**ok, KType::Stream(_)) =>
        {
            if flavor == Flavor::Query {
                check_cacheable(ok, &output, &fn_name, &mut errors);
            }
            result_arguments(ty)
        }
        _ => {
            if errors.is_empty() {
                errors.push(signature_error(
                    format!("`{fn_name}` must return `Result<T, E>`"),
                    &item.sig.output,
                    "the cache stores the success value and the error separately, and the platforms show both",
                    "return `Result<T, E>` where `E` is a `#[undra::error]` enum",
                ));
            }
            (None, None)
        }
    };
    if let Some(key) = &args.key {
        // The cursor differs per page and every page shares one entry: it is no part of the key.
        let cursor_in_key = shape.as_ref().is_some_and(|shape| {
            let cursor = &analysis.params[shape.cursor].name;
            key_names(key).iter().any(|name| name == cursor)
        });
        if let (true, Some(shape)) = (cursor_in_key, &shape) {
            errors.push(
                Diag::new(
                    code::E0073,
                    format!(
                        "the cursor `{}` is in the `key` of the infinite query `{fn_name}`",
                        analysis.params[shape.cursor].name
                    ),
                    "every page of one list shares one cache entry, so the cursor, which differs per page, cannot be part of its key",
                    "remove the placeholder from `key`; the key holds the parameters that distinguish lists",
                )
                .at(key.span()),
            );
        } else {
            let names: Vec<String> = analysis
                .params
                .iter()
                .enumerate()
                .filter(|(at, _)| shape.as_ref().is_none_or(|shape| shape.cursor != *at))
                .map(|(_, p)| p.name.clone())
                .collect();
            if let Err(error) = check_key(key, &names) {
                errors.push(error);
            }
        }
    }
    let mut checks = Checks::new();
    for p in &analysis.params {
        checks.ty(&p.ty, &p.kty);
    }
    checks.ret(&output, &ret);
    errors.finish()?;
    let (Some(ok_ty), Some(err_ty)) = (ok_ty, err_ty) else {
        // `result_arguments` reads what `map_return` accepted; if the two ever disagree, say
        // so at the signature rather than panicking or hiding it.
        return Err(signature_error(
            format!("`{fn_name}` must return `Result<T, E>`"),
            &item.sig.output,
            "the cache stores the success value and the error separately, and the platforms show both",
            "spell the return type as `Result<T, E>` with `E` a `#[undra::error]` enum",
        ));
    };
    // Named, not `const _`: valid in an `impl` block too, see the guard below.
    let checks_name = format_ident!("__UNDRA_CHECKS_{}", pascal_case(&fn_name));
    let checks = checks.emit_named(&root, &checks_name);

    // Generation.
    let meta = root.meta();
    let query = root.query();
    let runtime = root.runtime();
    let fn_ident = item.sig.ident.clone();
    let suffix = match flavor {
        Flavor::Query => "Query",
        Flavor::Mutation => "Mutation",
    };
    let struct_name = format_ident!("{}{}", pascal_case(&fn_name), suffix);
    let meta_static = format_ident!("__UNDRA_META_{}", struct_name);
    let vis = item.vis.clone();
    let key = args.key.as_ref().map_or_else(String::new, LitStr::value);
    let stale = match &args.stale {
        Some((ms, _)) => quote!(::core::option::Option::Some(#ms)),
        None => quote!(::core::option::Option::None),
    };
    let persist = args.persist.is_some();
    let retry = args.retry.unwrap_or(match flavor {
        Flavor::Query => 3,
        Flavor::Mutation => 0,
    });
    let idempotent = args.idempotent;
    let interval = match &args.interval {
        Some((ms, _)) => quote!(::core::option::Option::Some(#ms)),
        None => quote!(::core::option::Option::None),
    };
    let poll_in_background = args.poll_in_background.is_some();
    let refetch_pages = match args.refetch_pages {
        Some((n, _)) => quote!(::core::option::Option::Some(#n)),
        None => quote!(::core::option::Option::None),
    };
    let persist_pages = args.persist_pages.map_or(1, |(n, _)| n);
    let struct_doc = format!(
        "The `{fn_name}` {attribute}: its identifiers and settings, and the function `undra-query` runs."
    );

    // An infinite query's cursor is not one of its parameters: the cache asks for the page.
    let cursor_at = shape.as_ref().map(|shape| shape.cursor);
    let visible: Vec<(usize, &super::object::ParamModel)> = analysis
        .params
        .iter()
        .enumerate()
        .filter(|(at, _)| Some(*at) != cursor_at)
        .collect();
    let param_tys: Vec<&syn::Type> = visible.iter().map(|(_, p)| &p.ty).collect();
    // Positional locals, so no parameter name can collide with `__params`, `__ctx` or `__fut`.
    let param_names: Vec<syn::Ident> = visible.iter().map(|(at, _)| arg_local(*at)).collect();
    let call_args: Vec<TokenStream> = (0..analysis.params.len())
        .map(|at| {
            if Some(at) == cursor_at {
                quote!(__cursor)
            } else {
                let local = arg_local(at);
                quote!(#local)
            }
        })
        .collect();
    let ctx_arg = if analysis.ctx.is_some_and(|c| c.by_ref) {
        quote!(&__ctx)
    } else {
        quote!(__ctx)
    };
    let boxed = quote! {
        ::core::pin::Pin<::std::boxed::Box<
            dyn ::core::future::Future<
                Output = ::core::result::Result<#ok_ty, #err_ty>
            > + ::core::marker::Send
        >>
    };
    let span = fn_ident.span();
    let send_fn = send_assertion(span);
    let run_body = quote_spanned! {span=>
        // E0022: reported by `rustc` at the function, see `object.rs`.
        #[allow(non_snake_case)]
        fn #send_fn<T: ::core::marker::Send>(_: &T) {}
        let ( #(#param_names,)* ) = __params;
        let __fut = async move { #fn_ident( #ctx_arg #(, #call_args)* ).await };
        #send_fn(&__fut);
        ::std::boxed::Box::pin(__fut)
    };

    let id_const = match flavor {
        Flavor::Query => quote!(#meta::ids::query_id(#fn_name)),
        Flavor::Mutation => quote!(#meta::ids::mutation_id(#fn_name)),
    };
    let (id_name, kind_ident) = match flavor {
        Flavor::Query => ("QUERY_ID", "Query"),
        Flavor::Mutation => ("MUTATION_ID", "Mutation"),
    };
    let id_name = syn::Ident::new(id_name, Span::call_site());
    let kind_ident = syn::Ident::new(kind_ident, Span::call_site());

    let inherent_polling = (flavor == Flavor::Query).then(|| {
        quote! {
            /// The polling interval in milliseconds, if the query polls by default.
            pub const INTERVAL_MS: ::core::option::Option<u64> = #interval;
            /// Whether the query keeps polling while the app is in the background.
            pub const POLL_IN_BACKGROUND: bool = #poll_in_background;
        }
    });
    let inherent_paging = shape.as_ref().map(|shape| {
        let item_key = &shape.item_key;
        quote! {
            /// The field of a row that identifies it (`item_key`): the handle's list is keyed on it.
            pub const ITEM_KEY: &'static str = #item_key;
            /// How many pages a refetch loads at most (`refetch_pages`); none: every loaded page.
            pub const REFETCH_PAGES: ::core::option::Option<u32> = #refetch_pages;
            /// How many of the first pages a persisted entry stores (`persist_pages`).
            pub const PERSIST_PAGES: u32 = #persist_pages;
        }
    });
    let inherent = quote! {
        impl #struct_name {
            /// The stable id: `fnv1a32` of `query.<fn>` or `mutation.<fn>`.
            pub const #id_name: u32 = #id_const;
            /// The cache key template.
            pub const KEY: &'static str = #key;
            /// The staleness window in milliseconds, if any.
            pub const STALE_MS: ::core::option::Option<u64> = #stale;
            /// Whether results are persisted.
            pub const PERSIST: bool = #persist;
            /// Retry attempts after a failure.
            pub const RETRY: u32 = #retry;
            /// Whether the call is safe to replay.
            pub const IDEMPOTENT: bool = #idempotent;
            #inherent_polling
            #inherent_paging
        }
    };

    let trait_impl = match (flavor, &shape) {
        (Flavor::Query, None) => quote! {
            #[automatically_derived]
            impl #query::QueryDef for #struct_name {
                const ID: u32 = Self::#id_name;
                const KEY: &'static str = Self::KEY;
                const STALE_MS: ::core::option::Option<u64> = Self::STALE_MS;
                const PERSIST: bool = Self::PERSIST;
                const RETRY: u32 = Self::RETRY;
                const INTERVAL_MS: ::core::option::Option<u64> = Self::INTERVAL_MS;
                const POLL_IN_BACKGROUND: bool = Self::POLL_IN_BACKGROUND;
                type Params = ( #(#param_tys,)* );
                type Output = #ok_ty;
                type Error = #err_ty;
                #[allow(unused_variables)]
                fn fetch(__ctx: #runtime::Ctx, __params: Self::Params) -> #boxed {
                    #run_body
                }
            }
        },
        (Flavor::Query, Some(shape)) => {
            let item_ty = &shape.item_ty;
            let cursor_ty = &shape.cursor_ty;
            let page_boxed = quote! {
                ::core::pin::Pin<::std::boxed::Box<
                    dyn ::core::future::Future<
                        Output = ::core::result::Result<#query::Page<#item_ty, #cursor_ty>, #err_ty>
                    > + ::core::marker::Send
                >>
            };
            let item_key = item_key_function(&root, shape, &fn_name);
            quote! {
                #[automatically_derived]
                impl #query::QueryDef for #struct_name {
                    const ID: u32 = Self::#id_name;
                    const KEY: &'static str = Self::KEY;
                    const STALE_MS: ::core::option::Option<u64> = Self::STALE_MS;
                    const PERSIST: bool = Self::PERSIST;
                    const RETRY: u32 = Self::RETRY;
                    const INTERVAL_MS: ::core::option::Option<u64> = Self::INTERVAL_MS;
                    const POLL_IN_BACKGROUND: bool = Self::POLL_IN_BACKGROUND;
                    const PAGED: ::core::option::Option<&'static #query::PagedVTable> =
                        ::core::option::Option::Some(#query::paged_vtable::<Self>());
                    type Params = ( #(#param_tys,)* );
                    type Output = #ok_ty;
                    type Error = #err_ty;
                    // The first page's rows.
                    fn fetch(__ctx: #runtime::Ctx, __params: Self::Params) -> #boxed {
                        #query::fetch_first_page::<Self>(__ctx, __params)
                    }
                }

                #[automatically_derived]
                impl #query::InfiniteQueryDef for #struct_name {
                    type Item = #item_ty;
                    type Cursor = #cursor_ty;
                    const REFETCH_PAGES: ::core::option::Option<u32> = Self::REFETCH_PAGES;
                    const PERSIST_PAGES: u32 = Self::PERSIST_PAGES;
                    #[allow(unused_variables)]
                    fn fetch_page(
                        __ctx: #runtime::Ctx,
                        __params: Self::Params,
                        __cursor: ::core::option::Option<#cursor_ty>,
                    ) -> #page_boxed {
                        #run_body
                    }
                    #item_key
                }
            }
        }
        (Flavor::Mutation, _) => quote! {
            #[automatically_derived]
            impl #query::MutationDef for #struct_name {
                const ID: u32 = Self::#id_name;
                const KEY: &'static str = Self::KEY;
                const RETRY: u32 = Self::RETRY;
                const IDEMPOTENT: bool = Self::IDEMPOTENT;
                type Input = ( #(#param_tys,)* );
                type Output = #ok_ty;
                type Error = #err_ty;
                #[allow(unused_variables)]
                fn execute(__ctx: #runtime::Ctx, __params: Self::Input) -> #boxed {
                    #run_body
                }
            }
        },
    };

    let metas = visible
        .iter()
        .map(|(_, p)| param_meta(&meta, &p.name, &p.kty));
    let returns = ret.meta(&meta);
    let infinite_meta = match &shape {
        Some(shape) => {
            let cursor = shape.cursor_kty.meta(&meta);
            let item_key = &shape.item_key;
            quote!(::core::option::Option::Some(#meta::InfiniteMeta { cursor: #cursor, item_key: #item_key }))
        }
        None => quote!(::core::option::Option::None),
    };
    let registration = submit(&root, "Query", &meta_static);
    // The type-erased half, so the client finds the definition by id: a platform constructs a
    // query handle (or calls a mutation) knowing only the id, and the offline queue replays
    // a mutation by id after a restart. With it, the query runtime's init hook (hydration) and
    // dispatch layer: submitted here rather than by `undra-query` itself, so a core that declares
    // no query and no mutation does not link the query runtime (ADR-052). The runtime keeps one
    // hook and one layer per name, however many definitions submit them.
    let inventory = quote!(#meta::inventory);
    let erased = match flavor {
        Flavor::Query => quote!(#query::QueryRegistration::of::<#struct_name>()),
        Flavor::Mutation => quote!(#query::MutationRegistration::of::<#struct_name>()),
    };
    let erased_registration = quote! {
        #inventory::submit! { #erased }
        #inventory::submit! { #query::__private::HYDRATE }
        #inventory::submit! { #query::__private::LAYER }
    };

    let items = quote! {
        #[doc = #struct_doc]
        #vis struct #struct_name;

        #inherent

        #[allow(non_upper_case_globals)]
        static #meta_static: #meta::QueryMeta = #meta::QueryMeta {
            name: #fn_name,
            query_id: #id_const,
            kind: #meta::QueryKind::#kind_ident,
            key: #key,
            params: &[ #(#metas),* ],
            returns: #returns,
            stale_ms: #stale,
            persist: #persist,
            idempotent: #idempotent,
            interval_ms: #interval,
            poll_in_background: #poll_in_background,
            infinite: #infinite_meta,
        };
        #registration
        #erased_registration
    };

    // A macro cannot see the block it sits in, and the items above are module-level: a query
    // inside a plain `impl` block (one that is not `#[undra::api]`, which `object.rs` reports)
    // would make `rustc` complain about each of them (a struct, impls, statics, ..: ten errors
    // that never mention the query). Declared and invoked through a `macro_rules!` whose name is
    // the rule, there is one parse error ("macro definition is not supported in `trait`s or
    // `impl`s ... move it out to a nearby module scope") and one "cannot find macro
    // `_undra_error_E0007_a_query_is_a_free_function_move_it_out_of_the_impl_block`".
    //
    // Only what holds no token of the user's goes in: an error that `rustc` reports on a type
    // the user wrote (a parameter that cannot cross the boundary) would otherwise say that it
    // "originates in the macro `_undra_error_E0007_..`". So the `QueryDef` impl, which names the
    // parameter and result types, and the checks stay outside. In an `impl` block they are not
    // items `rustc` accepts either (one more parse error for the impl; the checks are a named
    // constant, which is valid there).
    let guard = format_ident!(
        "_undra_error_{}_a_{}_is_a_free_function_move_it_out_of_the_impl_block",
        code::E0007,
        attribute
    );
    Ok(quote! {
        #item

        #[doc(hidden)]
        macro_rules! #guard {
            () => { #items };
        }
        #guard!();

        #trait_impl

        #checks
    })
}

impl Args {
    /// The options only a query has (polling and paging), with where they were written: what a
    /// mutation may not carry.
    fn query_only(&self) -> Vec<(&'static str, Span)> {
        let mut found = Vec::new();
        if let Some((_, lit)) = &self.interval {
            found.push(("interval", lit.span()));
        }
        if let Some(span) = self.poll_in_background {
            found.push(("poll_in_background", span));
        }
        if let Some(span) = self.infinite {
            found.push(("infinite", span));
        }
        if let Some(lit) = &self.item_key {
            found.push(("item_key", lit.span()));
        }
        if let Some((_, span)) = self.refetch_pages {
            found.push(("refetch_pages", span));
        }
        if let Some((_, span)) = self.persist_pages {
            found.push(("persist_pages", span));
        }
        found
    }
}

fn infinite_error(what: impl std::fmt::Display, why: &str, help: &str, span: Span) -> syn::Error {
    Diag::new(code::E0073, what, why, help).at(span)
}

/// E0073 for the paging options that only make sense together: `item_key`, `refetch_pages` and
/// `persist_pages` need `infinite`, `infinite` needs `item_key`, `persist_pages` needs `persist`.
fn check_infinite_arguments(args: &Args, fn_name: &str, errors: &mut Errors) {
    if args.infinite.is_none() {
        let orphans = [
            ("item_key", args.item_key.as_ref().map(Spanned::span)),
            ("refetch_pages", args.refetch_pages.map(|(_, span)| span)),
            ("persist_pages", args.persist_pages.map(|(_, span)| span)),
        ];
        for (option, span) in orphans {
            if let Some(span) = span {
                errors.push(infinite_error(
                    format!("`{option}` on `{fn_name}`, which is not `infinite`"),
                    "paging options describe the pages of an infinite query; an ordinary query has one result",
                    &format!("add `infinite` to the query, or remove `{option}`"),
                    span,
                ));
            }
        }
        return;
    }
    if args.item_key.is_none() {
        errors.push(infinite_error(
            format!("the infinite query `{fn_name}` has no `item_key`"),
            "the handle shows the pages as one list keyed by a field of its rows: keyed patches (a next page arrives as the appended rows) and row identity need it",
            "add `item_key = \"id\"`, naming the field of the row type that identifies a row",
            args.infinite.unwrap_or_else(Span::call_site),
        ));
    }
    if let (Some((_, span)), None) = (args.persist_pages, args.persist) {
        errors.push(infinite_error(
            format!("`persist_pages` on `{fn_name}`, which does not `persist`"),
            "`persist_pages` says how many of the first pages a persisted entry stores; without `persist` nothing is stored",
            "add `persist` to the query, or remove `persist_pages`",
            span,
        ));
    }
}

/// Removes every `#[undra(cursor)]` from the parameters of `sig`, returning the names of the
/// parameters that carried one and where it was written.
fn take_cursor_attributes(sig: &mut syn::Signature) -> Vec<(String, Span)> {
    let mut found = Vec::new();
    for arg in &mut sig.inputs {
        let FnArg::Typed(typed) = arg else {
            continue;
        };
        let name = match &*typed.pat {
            Pat::Ident(pat) => unraw(&pat.ident),
            _ => String::new(),
        };
        typed.attrs.retain(|attr| {
            let is_cursor = attr.path().is_ident("undra")
                && attr
                    .parse_args::<syn::Ident>()
                    .is_ok_and(|ident| ident == "cursor");
            if is_cursor {
                found.push((name.clone(), attr.span()));
            }
            !is_cursor
        });
    }
    found
}

/// What an `infinite` query's signature says (ADR-043 decision 2.1).
struct InfiniteShape {
    /// `-> Result<Vec<T>, E>`: what the schema and the cache know the query by.
    output: syn::ReturnType,
    /// `Vec<T>`.
    list_ty: syn::Type,
    /// The row type `T`.
    item_ty: syn::Type,
    /// The error type `E`.
    err_ty: syn::Type,
    /// The cursor type `C` of the `Option<C>` parameter.
    cursor_ty: syn::Type,
    cursor_kty: KType,
    /// The index of the cursor among the analysed parameters.
    cursor: usize,
    item_key: LitStr,
}

/// `Option<C>` -> `C`.
fn option_inner(ty: &syn::Type) -> Option<&syn::Type> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let seg = path.path.segments.last()?;
    if seg.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    match (args.args.len(), args.args.first()) {
        (1, Some(syn::GenericArgument::Type(inner))) => Some(inner),
        _ => None,
    }
}

/// `Page<T>` -> `(T, None)`, `Page<T, C>` -> `(T, Some(C))`, by the spelling of its last segment.
fn page_of(ty: &syn::Type) -> Option<(syn::Type, Option<syn::Type>)> {
    let mut ty = ty;
    loop {
        match ty {
            syn::Type::Paren(inner) => ty = &inner.elem,
            syn::Type::Group(inner) => ty = &inner.elem,
            _ => break,
        }
    }
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let seg = path.path.segments.last()?;
    if seg.ident != "Page" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    let mut types = args.args.iter().map(|a| match a {
        syn::GenericArgument::Type(t) => Some(t.clone()),
        _ => None,
    });
    let item = types.next()??;
    let cursor = match types.next() {
        Some(Some(cursor)) => Some(cursor),
        Some(None) => return None,
        None => None,
    };
    if types.next().is_some() {
        return None;
    }
    Some((item, cursor))
}

/// Reads the signature of an `infinite` query: the cursor parameter, `Page<T, C>` in the success
/// type, `T` a record. Every violation is E0073 (reported on `errors`); `None` then.
fn infinite_shape(
    output: &syn::ReturnType,
    params: &[ParamModel],
    cursors: &[(String, Span)],
    args: &Args,
    fn_name: &str,
    errors: &mut Errors,
) -> Option<InfiniteShape> {
    let item_key = args.item_key.clone()?;
    let [(cursor_name, _)] = cursors else {
        return None;
    };
    let cursor = params.iter().position(|p| &p.name == cursor_name)?;
    let cursor_param = &params[cursor];
    let Some(cursor_ty) = option_inner(&cursor_param.ty).cloned() else {
        errors.push(infinite_error(
            format!(
                "the cursor `{cursor_name}` of `{fn_name}` is `{}`, not an `Option<C>`",
                ty_string(&cursor_param.ty)
            ),
            "the cursor is `None` for the first page and the previous page's `next` after it",
            &format!("write `#[undra(cursor)] {cursor_name}: Option<String>`, or another value type for the cursor your server uses"),
            cursor_param.ty.span(),
        ));
        return None;
    };
    let syn::ReturnType::Type(_, ty) = output else {
        errors.push(infinite_error(
            format!("the infinite query `{fn_name}` returns nothing"),
            "an infinite query returns one page of rows and the cursor of the next",
            "return `Result<undra::query::Page<T, C>, E>`",
            Span::call_site(),
        ));
        return None;
    };
    let (Some(ok_ty), Some(err_ty)) = result_arguments(ty) else {
        errors.push(infinite_error(
            format!("the infinite query `{fn_name}` does not return `Result<Page<T, C>, E>`"),
            "an infinite query returns one page of rows and the cursor of the next, or its error",
            "return `Result<undra::query::Page<T, C>, E>` where `E` is a `#[undra::error]` enum",
            ty.span(),
        ));
        return None;
    };
    let Some((item_ty, page_cursor)) = page_of(&ok_ty) else {
        errors.push(infinite_error(
            format!(
                "the success type of the infinite query `{fn_name}` is `{}`, not `Page<T, C>`",
                ty_string(&ok_ty)
            ),
            "the macro finds the rows and the next cursor of a page by the `Page<T, C>` struct of `undra::query`; any other type has no `next`",
            "return `Result<undra::query::Page<T, C>, E>` (`C` is `String` when left out)",
            ok_ty.span(),
        ));
        return None;
    };
    // The cursor of the page is the cursor parameter's type: spelled alike (the macro compares
    // spellings; `Page<T>` means `Page<T, String>`).
    let spelled = |ty: &syn::Type| ty_string(ty).replace(' ', "");
    let page_cursor_text = page_cursor
        .as_ref()
        .map_or_else(|| "String".to_owned(), spelled);
    if page_cursor_text != spelled(&cursor_ty) {
        errors.push(infinite_error(
            format!(
                "the cursor of `{fn_name}` is `Option<{}>` but its pages carry `{page_cursor_text}`",
                spelled(&cursor_ty)
            ),
            "the `next` of each page is the value the next call receives as its cursor: they are one type",
            "spell the cursor parameter and the `Page<T, C>` with the same `C`",
            cursor_param.ty.span(),
        ));
        return None;
    }
    if let Ok(kty) = map_type(&item_ty, Pos::QueryReturn, Allow::NONE) {
        if !matches!(kty, KType::Named(_)) {
            errors.push(infinite_error(
                format!(
                    "the rows of the infinite query `{fn_name}` are `{}`, not a record",
                    ty_string(&item_ty)
                ),
                "the handle's list is keyed by a field of its rows (`item_key`), so a row is a record declared with `#[undra::api]`",
                "make the rows a record, for example `struct Post { id: u64, .. }`",
                item_ty.span(),
            ));
            return None;
        }
    }
    let cursor_kty = match &cursor_param.kty {
        KType::Option(inner) => (**inner).clone(),
        other => other.clone(),
    };
    let list_ty: syn::Type = syn::parse_quote!(Vec<#item_ty>);
    let output: syn::ReturnType = syn::parse_quote!(-> Result<#list_ty, #err_ty>);
    Some(InfiniteShape {
        output,
        list_ty,
        item_ty,
        err_ty,
        cursor_ty,
        cursor_kty,
        cursor,
        item_key,
    })
}

/// The names between braces in a key template.
fn key_names(key: &LitStr) -> Vec<String> {
    let text = key.value();
    let mut names = Vec::new();
    let mut rest = text.as_str();
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            break;
        };
        names.push(after[..close].to_owned());
        rest = &after[close + 1..];
    }
    names
}

/// The key as the field it reads: `id`, `r#type` for a keyword, and for a name that is not an
/// identifier at all one no field has (the constant check has already failed for it).
fn key_field(name: &str, span: Span) -> syn::Ident {
    let mut ident = syn::parse_str::<syn::Ident>(name)
        .or_else(|_| syn::parse_str::<syn::Ident>(&format!("r#{name}")))
        .unwrap_or_else(|_| format_ident!("__undra_not_a_field_name"));
    ident.set_span(span);
    ident
}

/// `fn item_key(&T) -> u64` of an infinite query: the FNV-1a hash of the encoded `item_key` field,
/// the same function `#[undra::store]` emits for `#[undra(key = "..")]` (so a row has one key
/// wherever it is listed). The macro cannot see the fields of `T`: `T::__UNDRA_FIELDS` (every
/// `#[undra::api]` record has it) is searched in constants, and a key that names no field is E0073
/// listing the fields there are, reported on the string where `item_key` was written.
fn item_key_function(root: &Root, shape: &InfiniteShape, fn_name: &str) -> TokenStream {
    let meta = root.meta();
    let wire = root.wire();
    let item_ty = &shape.item_ty;
    // The message, with the list of fields left for the constant to fill in.
    const HOLE: &str = "\u{0}";
    let diag = Diag::new(
        code::E0073,
        format!(
            "`item_key = \"{}\"` of the infinite query `{fn_name}` names no field of `{}`",
            shape.item_key.value(),
            ty_string(item_ty)
        ),
        format!(
            "`item_key` names the field of the rows that identifies them, and `{}` has {HOLE}",
            ty_string(item_ty)
        ),
        "write the name of one of those fields as `item_key`",
    );
    let message = diag.message();
    let (before, after) = message.split_once(HOLE).unwrap_or((&message, ""));
    let span = shape.item_key.span();
    let key_name = shape.item_key.value();
    let field = key_field(&key_name, span);
    let check = quote_spanned! {span=>
        const __UNDRA_FIELDS: &[&str] = <#item_ty>::__UNDRA_FIELDS;
        const __UNDRA_INDEX: usize = #meta::keys::index_of(__UNDRA_FIELDS, #key_name);
        const __UNDRA_MESSAGE_LEN: usize = #meta::keys::message_len(#before, __UNDRA_FIELDS, #after);
        const __UNDRA_MESSAGE: [u8; __UNDRA_MESSAGE_LEN] =
            #meta::keys::message::<__UNDRA_MESSAGE_LEN>(#before, __UNDRA_FIELDS, #after);
        const __UNDRA_MESSAGE_TEXT: &str = #meta::keys::as_str(&__UNDRA_MESSAGE);
        const __UNDRA_KEY_IS_A_FIELD: bool = if __UNDRA_INDEX == usize::MAX {
            ::core::panic!("{}", __UNDRA_MESSAGE_TEXT)
        } else {
            true
        };
    };
    // The statements that use the constant carry the string's span too, so rustc's "erroneous
    // constant" note lands where the error does.
    let encode = quote_spanned! {span=>
        let __row: &<__UndraGate<{ __UNDRA_KEY_IS_A_FIELD }> as __UndraPass<#item_ty>>::Out = __item;
        #wire::Encode::encode(&__row.#field, &mut __buf);
    };
    quote_spanned! {item_ty.span()=>
        #[allow(non_camel_case_types, dead_code)]
        fn item_key(__item: &#item_ty) -> u64 {
            trait __UndraKeyed {
                const __UNDRA_FIELDS: &'static [&'static str] = &[];
            }
            impl<__T: ?::core::marker::Sized> __UndraKeyed for __T {}
            // `Out` is `T` only for `__UndraGate<true>`: when the check failed the row has no
            // type, so `rustc` adds no "no field" error after the diagnostic.
            struct __UndraGate<const __OK: bool>;
            trait __UndraPass<__T: ?::core::marker::Sized> {
                type Out: ?::core::marker::Sized;
            }
            impl<__T: ?::core::marker::Sized> __UndraPass<__T> for __UndraGate<true> {
                type Out = __T;
            }
            #check
            ::std::thread_local! {
                static __UNDRA_KEY_BUF: ::core::cell::RefCell<#wire::Writer> =
                    ::core::cell::RefCell::new(#wire::Writer::new());
            }
            __UNDRA_KEY_BUF.with(|__buf| {
                let mut __buf = __buf.borrow_mut();
                __buf.clear();
                #encode
                #meta::ids::fnv1a64(__buf.as_slice())
            })
        }
    }
}

/// A query caches a value, so its success type cannot be `()` (use a mutation for effects) or
/// an `Option` (the handle's `data` is already optional before the first result arrives).
fn check_cacheable(ok: &KType, output: &syn::ReturnType, fn_name: &str, errors: &mut Errors) {
    let (what, why, help) = match ok {
        KType::Unit => (
            format!("query `{fn_name}` returns `()`"),
            "a query caches the value it returns; a query with nothing to return has nothing to cache",
            "use `#[undra::mutation]` for a call that only has effects, or return the data the platform needs",
        ),
        KType::Option(_) => (
            format!("query `{fn_name}` returns an `Option`"),
            "the handle's `data` signal is already optional (no result yet), so an optional result would be ambiguous: `None` could mean \"not loaded\" or \"loaded, absent\"",
            "return a record or a list (an empty `Vec` says \"nothing\"), or an enum naming the cases",
        ),
        _ => return,
    };
    errors.push(Diag::new(code::E0042, what, why, help).on(output));
}

/// `Result<T, E>` -> `(T, E)`.
fn result_arguments(ty: &syn::Type) -> (Option<syn::Type>, Option<syn::Type>) {
    let mut ty = ty;
    loop {
        match ty {
            syn::Type::Paren(inner) => ty = &inner.elem,
            syn::Type::Group(inner) => ty = &inner.elem,
            _ => break,
        }
    }
    if let syn::Type::Path(path) = ty {
        if let Some(seg) = path.path.segments.last() {
            if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                let mut types = args.args.iter().filter_map(|a| match a {
                    syn::GenericArgument::Type(t) => Some(t.clone()),
                    _ => None,
                });
                return (types.next(), types.next());
            }
        }
    }
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::has;

    fn run(flavor: Flavor, attr: &str, src: &str) -> Result<String, String> {
        let args = parse_query_args(attr.parse().unwrap(), flavor).map_err(|e| e.to_string())?;
        let item: ItemFn = syn::parse_str(src).unwrap();
        expand(flavor, args, item)
            .map(|t| t.to_string())
            .map_err(|e| {
                e.into_iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join("\n---\n")
            })
    }

    const TODOS: &str = "pub async fn todos(ctx: &Ctx, page: u32, q: String) -> Result<Vec<Todo>, HttpError> { Ok(vec![]) }";

    #[test]
    fn durations() {
        assert_eq!(parse_duration_ms("500ms"), Ok(500));
        assert_eq!(parse_duration_ms("30s"), Ok(30_000));
        assert_eq!(parse_duration_ms("5m"), Ok(300_000));
        assert_eq!(parse_duration_ms("2h"), Ok(7_200_000));
        assert_eq!(parse_duration_ms("1d"), Ok(86_400_000));
        assert_eq!(parse_duration_ms(" 7 s"), Ok(7_000));
        assert!(parse_duration_ms("30").unwrap_err().contains("no unit"));
        assert!(parse_duration_ms("s").unwrap_err().contains("whole number"));
        assert!(parse_duration_ms("5y").unwrap_err().contains("not a unit"));
        assert!(
            parse_duration_ms("99999999999999999999s")
                .unwrap_err()
                .contains("too large")
        );
        assert!(
            parse_duration_ms("18446744073709551615d")
                .unwrap_err()
                .contains("overflows")
        );
    }

    #[test]
    fn a_query_gets_constants_a_def_impl_and_meta() {
        let out = run(
            Flavor::Query,
            "key = \"todos:{page}\", stale = \"30s\", persist, retry = 5",
            TODOS,
        )
        .unwrap();
        for needle in [
            "pub struct TodosQuery;",
            "pub const QUERY_ID: u32 = ::undra::meta::ids::query_id(\"todos\")",
            "pub const KEY: &'static str = \"todos:{page}\"",
            "pub const STALE_MS: ::core::option::Option<u64> = ::core::option::Option::Some(30000u64)",
            "pub const PERSIST: bool = true",
            "pub const RETRY: u32 = 5u32",
            "pub const IDEMPOTENT: bool = false",
            "impl ::undra::query::QueryDef for TodosQuery",
            "type Params = (u32, String,)",
            "type Output = Vec<Todo>",
            "type Error = HttpError",
            "todos(&__ctx, __undra_a0, __undra_a1)",
            "kind: ::undra::meta::QueryKind::Query",
            "::undra::meta::Registration::Query",
            "::undra::query::QueryRegistration::of::<TodosQuery>()",
            // The query runtime is linked by the definitions that need it (ADR-052).
            "::undra::meta::inventory::submit! { ::undra::query::__private::HYDRATE }",
            "::undra::meta::inventory::submit! { ::undra::query::__private::LAYER }",
        ] {
            assert!(has(&out, needle), "missing `{needle}` in {out}");
        }
    }

    #[test]
    fn a_mutation_has_its_own_ids_and_defaults() {
        let out = run(
            Flavor::Mutation,
            "idempotent",
            "pub async fn add_todo(ctx: Ctx, title: String) -> Result<Todo, HttpError> { todo }",
        )
        .unwrap();
        for needle in [
            "pub struct AddTodoMutation;",
            "pub const MUTATION_ID: u32 = ::undra::meta::ids::mutation_id(\"add_todo\")",
            "impl ::undra::query::MutationDef for AddTodoMutation",
            "type Input = (String,)",
            "pub const RETRY: u32 = 0u32",
            "pub const IDEMPOTENT: bool = true",
            "pub const KEY: &'static str = \"\"",
            "add_todo(__ctx, __undra_a0)",
            "kind: ::undra::meta::QueryKind::Mutation",
            "::undra::query::MutationRegistration::of::<AddTodoMutation>()",
            "::undra::meta::inventory::submit! { ::undra::query::__private::HYDRATE }",
            "::undra::meta::inventory::submit! { ::undra::query::__private::LAYER }",
        ] {
            assert!(has(&out, needle), "missing `{needle}` in {out}");
        }
    }

    #[test]
    fn a_parameterless_query_has_a_unit_tuple() {
        let out = run(
            Flavor::Query,
            "key = \"all\"",
            "async fn all(ctx: &Ctx) -> Result<u8, E> { Ok(1) }",
        )
        .unwrap();
        assert!(has(&out, "type Params = ()"), "{out}");
        assert!(has(&out, "let () = __params"), "{out}");
        assert!(has(&out, "RETRY: u32 = 3u32"), "{out}");
    }

    #[test]
    fn e0040_query_without_key_and_mutation_with_stale() {
        let message = run(Flavor::Query, "stale = \"1s\"", TODOS).unwrap_err();
        assert!(
            message.starts_with("error[undra::E0040]: query `todos` has no `key`"),
            "{message}"
        );
        assert!(message.contains("help: add `key = \"todos\"`"), "{message}");
        let message = run(
            Flavor::Mutation,
            "stale = \"1s\"",
            "async fn m(ctx: &Ctx) -> Result<u8, E> { Ok(1) }",
        )
        .unwrap_err();
        assert!(
            message.starts_with("error[undra::E0040]: mutation `m` has `stale`"),
            "{message}"
        );
        let message = run(
            Flavor::Mutation,
            "persist",
            "async fn m(ctx: &Ctx) -> Result<u8, E> { Ok(1) }",
        )
        .unwrap_err();
        assert!(message.contains("mutation `m` has `persist`"), "{message}");
    }

    #[test]
    fn e0040_bad_arguments() {
        let message = run(Flavor::Query, "key = \"k\", stale = \"soon\"", TODOS).unwrap_err();
        assert!(message.contains("invalid `stale` duration"), "{message}");
        let message = run(Flavor::Query, "key = \"k\", retry = 99999999999", TODOS).unwrap_err();
        assert!(
            message.contains("`retry` must be a whole number"),
            "{message}"
        );
        let message = run(Flavor::Query, "key = \"todos:{nope}\"", TODOS).unwrap_err();
        assert!(
            message.contains("`{nope}` in `key` is not a parameter"),
            "{message}"
        );
        assert!(message.contains("the parameters are: page, q"), "{message}");
        let message = run(Flavor::Query, "key = \"todos:{page\"", TODOS).unwrap_err();
        assert!(message.contains("unterminated `{`"), "{message}");
        let message = run(Flavor::Query, "key = \"todos}\"", TODOS).unwrap_err();
        assert!(message.contains("unmatched `}`"), "{message}");
        let message = run(Flavor::Query, "key = \"k\", bogus", TODOS).unwrap_err();
        assert!(
            message.contains("unknown argument `bogus` for `#[undra::query]`"),
            "{message}"
        );
        let message = run(
            Flavor::Query,
            "key = \"{x}\"",
            "async fn q(ctx: &Ctx) -> Result<u8, E> { Ok(1) }",
        )
        .unwrap_err();
        assert!(
            message.contains("the function has no parameters after `ctx`"),
            "{message}"
        );
    }

    #[test]
    fn e0041_signature_rules() {
        let e = |src: &str| run(Flavor::Query, "key = \"k\"", src).unwrap_err();
        assert!(
            e("fn q(ctx: &Ctx) -> Result<u8, E> { Ok(1) }")
                .contains("error[undra::E0041]: `q` must be `async`")
        );
        assert!(
            e("async fn q(page: u32) -> Result<u8, E> { Ok(1) }").contains("must take `ctx: &Ctx`")
        );
        assert!(e("async fn q(ctx: &Ctx) -> u8 { 1 }").contains("must return `Result<T, E>`"));
        assert!(e("async fn q(ctx: &Ctx) {}").contains("must return `Result<T, E>`"));
        assert!(
            e("async fn q(ctx: &Ctx) -> Result<impl Stream<Item = u8>, E> { todo }")
                .contains("E0041")
        );
        assert!(
            e("async fn q(&self, ctx: &Ctx) -> Result<u8, E> { Ok(1) }").contains("takes `self`")
        );
    }

    #[test]
    fn parameter_and_generic_errors_keep_their_codes() {
        let e = |src: &str| run(Flavor::Query, "key = \"k\"", src).unwrap_err();
        assert!(
            e("async fn q(ctx: &Ctx, s: &str) -> Result<u8, E> { Ok(1) }")
                .contains("error[undra::E0001]")
        );
        assert!(
            e("async fn q<T>(ctx: &Ctx, s: T) -> Result<u8, E> { Ok(1) }")
                .contains("error[undra::E0002]")
        );
    }

    #[test]
    fn parenthesised_result_types_are_read_through() {
        // L4: this used to fail with "undra: internal error: a validated query has no Result type".
        let out = run(
            Flavor::Query,
            "key = \"k\"",
            "async fn q(ctx: &Ctx) -> (Result<u8, E>) { Ok(1) }",
        )
        .unwrap();
        assert!(has(&out, "type Output = u8"), "{out}");
        assert!(has(&out, "type Error = E"), "{out}");
    }

    #[test]
    fn a_query_cannot_cache_unit_or_an_option() {
        let message = run(
            Flavor::Query,
            "key = \"k\"",
            "async fn q(ctx: &Ctx) -> Result<(), E> { Ok(()) }",
        )
        .unwrap_err();
        assert!(
            message.starts_with("error[undra::E0042]: query `q` returns `()`"),
            "{message}"
        );
        let message = run(
            Flavor::Query,
            "key = \"k\"",
            "async fn q(ctx: &Ctx) -> Result<Option<u8>, E> { Ok(None) }",
        )
        .unwrap_err();
        assert!(message.contains("returns an `Option`"), "{message}");
        assert!(
            run(
                Flavor::Mutation,
                "",
                "async fn m(ctx: &Ctx) -> Result<Option<u8>, E> { Ok(None) }"
            )
            .is_ok(),
            "mutations may return either"
        );
    }

    // ----- polling (ADR-043 decision 1) --------------------------------------------------------

    #[test]
    fn interval_and_poll_in_background_reach_the_constants_the_trait_and_the_meta() {
        let out = run(
            Flavor::Query,
            "key = \"todos:{page}\", interval = \"30s\", poll_in_background",
            TODOS,
        )
        .unwrap();
        for needle in [
            "pub const INTERVAL_MS: ::core::option::Option<u64> = ::core::option::Option::Some(30000u64)",
            "pub const POLL_IN_BACKGROUND: bool = true",
            "const INTERVAL_MS: ::core::option::Option<u64> = Self::INTERVAL_MS",
            "const POLL_IN_BACKGROUND: bool = Self::POLL_IN_BACKGROUND",
            "interval_ms: ::core::option::Option::Some(30000u64)",
            "poll_in_background: true",
            "infinite: ::core::option::Option::None",
        ] {
            assert!(has(&out, needle), "missing `{needle}` in {out}");
        }
        let plain = run(Flavor::Query, "key = \"todos:{page}\"", TODOS).unwrap();
        assert!(
            has(&plain, "pub const POLL_IN_BACKGROUND: bool = false"),
            "{plain}"
        );
        assert!(
            has(&plain, "interval_ms: ::core::option::Option::None"),
            "{plain}"
        );
    }

    #[test]
    fn an_interval_is_at_least_one_second_and_a_poll_is_not_a_stream() {
        assert!(run(Flavor::Query, "key = \"k\", interval = \"1s\"", TODOS).is_ok());
        assert!(run(Flavor::Query, "key = \"k\", interval = \"1000ms\"", TODOS).is_ok());
        for text in ["999ms", "500ms", "0s"] {
            let message = run(
                Flavor::Query,
                &format!("key = \"k\", interval = \"{text}\""),
                TODOS,
            )
            .unwrap_err();
            assert!(
                message.starts_with(&format!(
                    "error[undra::E0040]: `interval = \"{text}\"` is below one second"
                )),
                "{message}"
            );
            assert!(
                message.contains("use a stream for real-time data"),
                "{message}"
            );
        }
        let message = run(Flavor::Query, "key = \"k\", interval = \"soon\"", TODOS).unwrap_err();
        assert!(message.contains("invalid `interval` duration"), "{message}");
        let message = run(Flavor::Query, "key = \"k\", interval = 30", TODOS).unwrap_err();
        assert!(
            message.contains("`interval` must be a string literal"),
            "{message}"
        );
        let message = run(
            Flavor::Query,
            "key = \"k\", poll_in_background = true",
            TODOS,
        )
        .unwrap_err();
        assert!(
            message.contains("`poll_in_background` takes no value"),
            "{message}"
        );
    }

    #[test]
    fn a_mutation_has_no_polling_or_paging() {
        for option in [
            "interval = \"5s\"",
            "poll_in_background",
            "infinite",
            "item_key = \"id\"",
            "refetch_pages = 2",
            "persist_pages = 2",
        ] {
            let message = run(
                Flavor::Mutation,
                option,
                "async fn m(ctx: &Ctx) -> Result<u8, E> { Ok(1) }",
            )
            .unwrap_err();
            let name = option.split([' ', '=']).next().unwrap();
            assert!(
                message.starts_with(&format!("error[undra::E0040]: mutation `m` has `{name}`")),
                "{message}"
            );
        }
    }

    // ----- infinite queries (ADR-043 decision 2) -----------------------------------------------

    const FEED: &str = "pub async fn feed(ctx: &Ctx, filter: Filter, #[undra(cursor)] cursor: Option<String>) -> Result<undra::query::Page<Post, String>, ApiError> { todo }";

    const FEED_ARGS: &str = "key = \"feed/{filter}\", infinite, item_key = \"id\"";

    #[test]
    fn an_infinite_query_gets_the_paging_impls_and_a_schema_without_the_cursor() {
        let out = run(
            Flavor::Query,
            &format!("{FEED_ARGS}, stale = \"1m\", refetch_pages = 3, persist, persist_pages = 2"),
            FEED,
        )
        .unwrap();
        for needle in [
            "pub struct FeedQuery;",
            // The cursor is not a parameter: the key and `Params` are the filter alone.
            "type Params = (Filter,)",
            "type Output = Vec<Post>",
            "type Error = ApiError",
            "const PAGED: ::core::option::Option<&'static ::undra::query::PagedVTable> = ::core::option::Option::Some(::undra::query::paged_vtable::<Self>())",
            "::undra::query::fetch_first_page::<Self>(__ctx, __params)",
            "impl ::undra::query::InfiniteQueryDef for FeedQuery",
            "type Item = Post",
            "type Cursor = String",
            "const REFETCH_PAGES: ::core::option::Option<u32> = Self::REFETCH_PAGES",
            "const PERSIST_PAGES: u32 = Self::PERSIST_PAGES",
            "pub const REFETCH_PAGES: ::core::option::Option<u32> = ::core::option::Option::Some(3u32)",
            "pub const PERSIST_PAGES: u32 = 2u32",
            "pub const ITEM_KEY: &'static str = \"id\"",
            "__cursor: ::core::option::Option<String>",
            "Output = ::core::result::Result<::undra::query::Page<Post, String>, ApiError>",
            // The function gets the cursor where it was declared.
            "feed(&__ctx, __undra_a0, __cursor)",
            "fn item_key(__item: &Post) -> u64",
            "<Post>::__UNDRA_FIELDS",
            "::undra::meta::keys::index_of(__UNDRA_FIELDS, \"id\")",
            "::undra::meta::ids::fnv1a64(__buf.as_slice())",
            "params: &[::undra::meta::ParamMeta { name: \"filter\"",
            "returns: ::undra::meta::TypeRefMeta::Result(&::undra::meta::TypeRefMeta::Vec(&::undra::meta::TypeRefMeta::Named(\"Post\")), &::undra::meta::TypeRefMeta::Named(\"ApiError\"))",
            "infinite: ::core::option::Option::Some(::undra::meta::InfiniteMeta { cursor: ::undra::meta::TypeRefMeta::String, item_key: \"id\" })",
        ] {
            assert!(has(&out, needle), "missing `{needle}` in {out}");
        }
        assert!(
            !has(&out, "name: \"cursor\""),
            "the cursor is not in the schema: {out}"
        );
        assert!(
            !has(&out, "undra(cursor)"),
            "the helper attribute is consumed: {out}"
        );
    }

    #[test]
    fn the_cursor_can_come_first_and_be_any_value_type_and_page_defaults_to_a_string_cursor() {
        let out = run(
            Flavor::Query,
            "key = \"f\", infinite, item_key = \"id\"",
            "async fn f(ctx: &Ctx, #[undra(cursor)] after: Option<u64>, q: String) -> Result<Page<Post, u64>, E> { todo }",
        )
        .unwrap();
        assert!(has(&out, "f(&__ctx, __cursor, __undra_a1)"), "{out}");
        assert!(has(&out, "type Params = (String,)"), "{out}");
        assert!(has(&out, "type Cursor = u64"), "{out}");
        assert!(
            has(&out, "cursor: ::undra::meta::TypeRefMeta::U64"),
            "{out}"
        );
        // `Page<T>` is `Page<T, String>`.
        let out = run(
            Flavor::Query,
            "key = \"f\", infinite, item_key = \"id\"",
            "async fn f(ctx: &Ctx, #[undra(cursor)] c: Option<String>) -> Result<Page<Post>, E> { todo }",
        )
        .unwrap();
        assert!(has(&out, "type Params = ()"), "{out}");
        assert!(has(&out, "type Cursor = String"), "{out}");
    }

    fn infinite(attr: &str, src: &str) -> String {
        run(Flavor::Query, attr, src).unwrap_err()
    }

    #[test]
    fn e0073_the_paging_options_come_together() {
        let message = infinite("key = \"feed/{filter}\", infinite", FEED);
        assert!(
            message.starts_with("error[undra::E0073]: the infinite query `feed` has no `item_key`"),
            "{message}"
        );
        assert!(
            message.contains("help: add `item_key = \"id\"`"),
            "{message}"
        );
        for (option, expect) in [
            (
                "item_key = \"id\"",
                "`item_key` on `todos`, which is not `infinite`",
            ),
            (
                "refetch_pages = 2",
                "`refetch_pages` on `todos`, which is not `infinite`",
            ),
            (
                "persist_pages = 2",
                "`persist_pages` on `todos`, which is not `infinite`",
            ),
        ] {
            let message = infinite(&format!("key = \"k\", {option}"), TODOS);
            assert!(
                message.starts_with(&format!("error[undra::E0073]: {expect}")),
                "{message}"
            );
        }
        let message = infinite(&format!("{FEED_ARGS}, persist_pages = 2"), FEED);
        assert!(
            message.starts_with(
                "error[undra::E0073]: `persist_pages` on `feed`, which does not `persist`"
            ),
            "{message}"
        );
        let message = infinite(&format!("{FEED_ARGS}, refetch_pages = 0"), FEED);
        assert!(
            message.starts_with(
                "error[undra::E0040]: `refetch_pages` must be a whole number of at least 1"
            ),
            "{message}"
        );
        let message = infinite(&format!("{FEED_ARGS}, persist, persist_pages = x"), FEED);
        assert!(
            message.contains("`persist_pages` must be a whole number of pages"),
            "{message}"
        );
    }

    #[test]
    fn e0073_exactly_one_cursor_of_type_option() {
        let none = "async fn f(ctx: &Ctx, q: String) -> Result<Page<Post>, E> { todo }";
        let message = infinite("key = \"f\", infinite, item_key = \"id\"", none);
        assert!(
            message.starts_with(
                "error[undra::E0073]: the infinite query `f` has no `#[undra(cursor)]` parameter"
            ),
            "{message}"
        );
        let two = "async fn f(ctx: &Ctx, #[undra(cursor)] a: Option<String>, #[undra(cursor)] b: Option<String>) -> Result<Page<Post>, E> { todo }";
        let message = infinite("key = \"f\", infinite, item_key = \"id\"", two);
        assert!(
            message.contains("has 2 `#[undra(cursor)]` parameters"),
            "{message}"
        );
        let not_option =
            "async fn f(ctx: &Ctx, #[undra(cursor)] c: String) -> Result<Page<Post>, E> { todo }";
        let message = infinite("key = \"f\", infinite, item_key = \"id\"", not_option);
        assert!(
            message.contains("the cursor `c` of `f` is `String`, not an `Option<C>`"),
            "{message}"
        );
        let on_plain = infinite(
            "key = \"k\"",
            "async fn q(ctx: &Ctx, #[undra(cursor)] c: Option<String>) -> Result<u8, E> { todo }",
        );
        assert!(
            on_plain.starts_with("error[undra::E0073]: `#[undra(cursor)]` on `c`, which is not a parameter of an `infinite` query"),
            "{on_plain}"
        );
    }

    #[test]
    fn e0073_the_cursor_is_not_part_of_the_key() {
        let message = infinite(
            "key = \"feed/{filter}/{cursor}\", infinite, item_key = \"id\"",
            FEED,
        );
        assert!(
            message.starts_with("error[undra::E0073]: the cursor `cursor` is in the `key` of the infinite query `feed`"),
            "{message}"
        );
        assert_eq!(
            message.matches("error[").count(),
            1,
            "one finding, not two: {message}"
        );
    }

    #[test]
    fn e0073_the_success_type_is_a_page_of_records_with_the_cursors_type() {
        let not_page = "async fn f(ctx: &Ctx, #[undra(cursor)] c: Option<String>) -> Result<Vec<Post>, E> { todo }";
        let message = infinite("key = \"f\", infinite, item_key = \"id\"", not_page);
        assert!(
            message.starts_with("error[undra::E0073]: the success type of the infinite query `f` is `Vec<Post>`, not `Page<T, C>`"),
            "{message}"
        );
        let mismatch = "async fn f(ctx: &Ctx, #[undra(cursor)] c: Option<String>) -> Result<Page<Post, u64>, E> { todo }";
        let message = infinite("key = \"f\", infinite, item_key = \"id\"", mismatch);
        assert!(
            message.contains("the cursor of `f` is `Option<String>` but its pages carry `u64`"),
            "{message}"
        );
        let defaulted = "async fn f(ctx: &Ctx, #[undra(cursor)] c: Option<u64>) -> Result<Page<Post>, E> { todo }";
        let message = infinite("key = \"f\", infinite, item_key = \"id\"", defaulted);
        assert!(message.contains("its pages carry `String`"), "{message}");
        let scalar = "async fn f(ctx: &Ctx, #[undra(cursor)] c: Option<String>) -> Result<Page<u32>, E> { todo }";
        let message = infinite("key = \"f\", infinite, item_key = \"id\"", scalar);
        assert!(
            message.contains("the rows of the infinite query `f` are `u32`, not a record"),
            "{message}"
        );
        let plain_result =
            "async fn f(ctx: &Ctx, #[undra(cursor)] c: Option<String>) -> Result<u8, E> { todo }";
        let message = infinite("key = \"f\", infinite, item_key = \"id\"", plain_result);
        assert!(
            message.starts_with(
                "error[undra::E0073]: the success type of the infinite query `f` is `u8`"
            ),
            "{message}"
        );
        let no_result = "async fn f(ctx: &Ctx, #[undra(cursor)] c: Option<String>) -> u8 { 1 }";
        let message = infinite("key = \"f\", infinite, item_key = \"id\"", no_result);
        assert!(
            message.contains("does not return `Result<Page<T, C>, E>`"),
            "{message}"
        );
    }

    #[test]
    fn an_infinite_query_keeps_every_other_rule_of_a_query() {
        let message = infinite("infinite, item_key = \"id\"", FEED);
        assert!(message.contains("query `feed` has no `key`"), "{message}");
        let message = infinite("key = \"feed/{nope}\", infinite, item_key = \"id\"", FEED);
        assert!(
            message.contains("`{nope}` in `key` is not a parameter"),
            "{message}"
        );
        assert!(message.contains("the parameters are: filter"), "{message}");
        let sync = "fn feed(ctx: &Ctx, #[undra(cursor)] c: Option<String>) -> Result<Page<Post>, E> { todo }";
        let message = infinite("key = \"f\", infinite, item_key = \"id\"", sync);
        assert!(message.contains("E0041"), "{message}");
    }

    // ----- snapshots of the paging and polling expansions ---------------------------------------

    /// Compares the pretty-printed expansion with `tests/snapshots/<name>.rs` (regenerate with
    /// `UPDATE_SNAPSHOTS=1 cargo test -p undra-macros`, like the other snapshots).
    fn snapshot(name: &str, attr: &str, src: &str) {
        let args = parse_query_args(attr.parse().unwrap(), Flavor::Query).unwrap();
        let item: ItemFn = syn::parse_str(src).unwrap();
        let tokens = expand(Flavor::Query, args, item).unwrap_or_else(|e| {
            panic!(
                "fixture `{name}` produced a diagnostic: {:?}",
                e.to_string()
            )
        });
        let file: syn::File = syn::parse2(tokens).expect("the expansion parses");
        let actual = prettyplease::unparse(&file);
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("snapshots")
            .join(format!("{name}.rs"));
        if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
            std::fs::write(&path, &actual).expect("write the snapshot");
            return;
        }
        let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "missing snapshot {}; create it with UPDATE_SNAPSHOTS=1 cargo test -p undra-macros",
                path.display()
            )
        });
        assert!(
            expected == actual,
            "snapshot `{name}` changed; review and run UPDATE_SNAPSHOTS=1 cargo test -p undra-macros"
        );
    }

    #[test]
    fn snapshot_of_a_polling_query() {
        snapshot(
            "query_polling",
            "key = \"ticker/{symbol}\", stale = \"10s\", interval = \"30s\", poll_in_background",
            "/// The latest quote.\npub async fn ticker(ctx: &Ctx, symbol: String) -> Result<Quote, HttpError> { todo!() }",
        );
    }

    #[test]
    fn snapshot_of_an_infinite_query() {
        snapshot(
            "query_infinite",
            "key = \"feed/{filter}\", infinite, item_key = \"id\", stale = \"1m\", interval = \"30s\", refetch_pages = 3, persist, persist_pages = 2",
            "/// The posts of a filter, newest first.\npub async fn feed(ctx: &Ctx, filter: Filter, #[undra(cursor)] cursor: Option<String>) -> Result<undra::query::Page<Post, String>, ApiError> { todo!() }",
        );
    }

    #[test]
    fn crate_override_and_send_assertion() {
        let out = run(
            Flavor::Query,
            "key = \"k\", crate = \"::k\"",
            "async fn q(ctx: &Ctx) -> Result<u8, E> { Ok(1) }",
        )
        .unwrap();
        assert!(has(&out, "impl ::k::query::QueryDef for QQuery"), "{out}");
        assert!(
            has(
                &out,
                "_undra_error_E0022_the_future_of_an_async_method_must_be_Send(&__fut)"
            ),
            "{out}"
        );
    }
}
