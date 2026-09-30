//! `#[keel::query]` and `#[keel::mutation]` (SPEC 4.5 and 9).
//!
//! ```ignore
//! #[keel::query(key = "todos:{page}", stale = "30s", persist, retry = 3)]
//! pub async fn todos(ctx: &Ctx, page: u32) -> Result<Vec<Todo>, HttpError> { .. }
//!
//! #[keel::mutation(idempotent)]
//! pub async fn add_todo(ctx: &Ctx, title: String) -> Result<Todo, HttpError> { .. }
//! ```
//!
//! The function is kept. Next to it the macro emits `pub struct TodosQuery` (`AddTodoMutation`
//! for mutations): inherent constants (`QUERY_ID`/`MUTATION_ID`, `KEY`, `STALE_MS`, `PERSIST`,
//! `RETRY`, `IDEMPOTENT`), an implementation of `::keel::query::QueryDef` (`MutationDef`) whose
//! `Params`/`Input` is the tuple of the parameters after `ctx`, and a `QueryMeta` registration.
//!
//! Defaults: `retry` is 3 for queries (SPEC 9) and 0 for mutations (a mutation that is not
//! safe to replay must not retry silently); `stale` is absent (always stale); `persist` and
//! `idempotent` are off; a mutation's `key` defaults to the empty string.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{ItemFn, LitInt, LitStr};

use super::attrs::{parse_args, root_arg};
use super::common::{item_root, param_meta, submit};
use super::diag::{Diag, Errors, code};
use super::naming::{pascal_case, unraw};
use super::object::analyze;
use super::paths::Root;
use super::types::{KType, map_return};

/// Whether the attribute is `#[keel::query]` or `#[keel::mutation]`.
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
}

fn args_error(what: impl std::fmt::Display, why: &str, help: &str, span: Span) -> syn::Error {
    Diag::new(code::E0040, what, why, help).at(span)
}

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

/// Parses the arguments of `#[keel::query(..)]` / `#[keel::mutation(..)]`.
pub(crate) fn parse_query_args(attr: TokenStream, flavor: Flavor) -> syn::Result<Args> {
    let mut args = Args::default();
    parse_args(
        attr,
        flavor.attribute(),
        "`key = \"..\"`, `stale = \"30s\"`, `persist`, `retry = N`, `idempotent` and `crate = \"path\"`",
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
                    args.key = Some(meta.value()?.parse()?);
                    Ok(true)
                }
                "stale" => {
                    let lit: LitStr = meta.value()?.parse()?;
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
                    args.persist = Some(meta.path.span());
                    Ok(true)
                }
                "retry" => {
                    let lit: LitInt = meta.value()?.parse()?;
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
                    args.idempotent = true;
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

/// Expands `#[keel::query]` or `#[keel::mutation]` on an `async fn`.
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
        }
    }

    // The function itself.
    let analysis = analyze(&mut item.sig, &mut errors);
    if item.sig.asyncness.is_none() {
        errors.push(signature_error(
            format!("`{fn_name}` must be `async`"),
            &item.sig.ident,
            &format!("`#[keel::{attribute}]` functions fetch or write through ports, which are asynchronous"),
            "write `async fn`",
        ));
    }
    if analysis.has_receiver {
        errors.push(signature_error(
            format!("`{fn_name}` takes `self`"),
            &item.sig.ident,
            &format!("`#[keel::{attribute}]` applies to free functions"),
            "make it a free function",
        ));
    }
    if analysis.ctx.is_none() {
        errors.push(signature_error(
            format!("`{fn_name}` must take `ctx: &Ctx` as its first parameter"),
            &item.sig.ident,
            "the runtime hands every query and mutation a context to reach ports and other queries",
            "add `ctx: &Ctx` (or `ctx: Ctx`) before the other parameters",
        ));
    }
    let ret = match map_return(&item.sig.output) {
        Ok(ret) => ret,
        Err(err) => {
            errors.push(err.into_error());
            KType::Unit
        }
    };
    let (ok_ty, err_ty) = match (&ret, &item.sig.output) {
        (KType::Result(ok, _), syn::ReturnType::Type(_, ty))
            if !matches!(**ok, KType::Stream(_)) =>
        {
            result_arguments(ty)
        }
        _ => {
            if errors.is_empty() {
                errors.push(signature_error(
                    format!("`{fn_name}` must return `Result<T, E>`"),
                    &item.sig.output,
                    "the cache stores the success value and the error separately, and the platforms show both",
                    "return `Result<T, E>` where `E` is a `#[keel::error]` enum",
                ));
            }
            (None, None)
        }
    };
    if let Some(key) = &args.key {
        let names: Vec<String> = analysis.params.iter().map(|p| p.name.clone()).collect();
        if let Err(error) = check_key(key, &names) {
            errors.push(error);
        }
    }
    errors.finish()?;
    let (Some(ok_ty), Some(err_ty)) = (ok_ty, err_ty) else {
        return Err(syn::Error::new(
            Span::call_site(),
            "keel: internal error: a validated query has no Result type",
        ));
    };

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
    let meta_static = format_ident!("__KEEL_META_{}", struct_name);
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
    let struct_doc = format!(
        "The `{fn_name}` {attribute}: its identifiers and settings, and the function `keel-query` runs."
    );

    let param_tys: Vec<&syn::Type> = analysis.params.iter().map(|p| &p.ty).collect();
    let param_names: Vec<&syn::Ident> = analysis.params.iter().map(|p| &p.ident).collect();
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
    let run_body = quote_spanned! {span=>
        // E0022: reported by `rustc` at the function, see `object.rs`.
        fn __keel_assert_send<T: ::core::marker::Send>(_: &T) {}
        let ( #(#param_names,)* ) = __params;
        let __fut = async move { #fn_ident( #ctx_arg #(, #param_names)* ).await };
        __keel_assert_send(&__fut);
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
        }
    };

    let trait_impl = match flavor {
        Flavor::Query => quote! {
            #[automatically_derived]
            impl #query::QueryDef for #struct_name {
                const ID: u32 = Self::#id_name;
                const KEY: &'static str = Self::KEY;
                const STALE_MS: ::core::option::Option<u64> = Self::STALE_MS;
                const PERSIST: bool = Self::PERSIST;
                const RETRY: u32 = Self::RETRY;
                type Params = ( #(#param_tys,)* );
                type Output = #ok_ty;
                type Error = #err_ty;
                #[allow(unused_variables)]
                fn fetch(__ctx: #runtime::Ctx, __params: Self::Params) -> #boxed {
                    #run_body
                }
            }
        },
        Flavor::Mutation => quote! {
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

    let metas = analysis
        .params
        .iter()
        .map(|p| param_meta(&meta, &p.name, &p.kty));
    let returns = ret.meta(&meta);
    let registration = submit(&root, "Query", &meta_static);

    Ok(quote! {
        #item

        #[doc = #struct_doc]
        #vis struct #struct_name;

        #inherent

        #trait_impl

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
        };
        #registration
    })
}

/// `Result<T, E>` -> `(T, E)`.
fn result_arguments(ty: &syn::Type) -> (Option<syn::Type>, Option<syn::Type>) {
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
            "pub const QUERY_ID: u32 = ::keel::meta::ids::query_id(\"todos\")",
            "pub const KEY: &'static str = \"todos:{page}\"",
            "pub const STALE_MS: ::core::option::Option<u64> = ::core::option::Option::Some(30000u64)",
            "pub const PERSIST: bool = true",
            "pub const RETRY: u32 = 5u32",
            "pub const IDEMPOTENT: bool = false",
            "impl ::keel::query::QueryDef for TodosQuery",
            "type Params = (u32, String,)",
            "type Output = Vec<Todo>",
            "type Error = HttpError",
            "todos(&__ctx, page, q)",
            "kind: ::keel::meta::QueryKind::Query",
            "::keel::meta::Registration::Query",
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
            "pub const MUTATION_ID: u32 = ::keel::meta::ids::mutation_id(\"add_todo\")",
            "impl ::keel::query::MutationDef for AddTodoMutation",
            "type Input = (String,)",
            "pub const RETRY: u32 = 0u32",
            "pub const IDEMPOTENT: bool = true",
            "pub const KEY: &'static str = \"\"",
            "add_todo(__ctx, title)",
            "kind: ::keel::meta::QueryKind::Mutation",
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
            message.starts_with("error[keel::E0040]: query `todos` has no `key`"),
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
            message.starts_with("error[keel::E0040]: mutation `m` has `stale`"),
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
            message.contains("unknown argument `bogus` for `#[keel::query]`"),
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
                .contains("error[keel::E0041]: `q` must be `async`")
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
                .contains("error[keel::E0001]")
        );
        assert!(
            e("async fn q<T>(ctx: &Ctx, s: T) -> Result<u8, E> { Ok(1) }")
                .contains("error[keel::E0002]")
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
        assert!(has(&out, "__keel_assert_send(&__fut)"), "{out}");
    }
}
