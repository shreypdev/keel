//! `#[undra::migrate(..)]`: a migration hook for persisted data an older build wrote (ADR-037
//! decision 6).
//!
//! ```ignore
//! #[undra::migrate(ty = "Todo")]                       // any persisted Todo whose structure changed
//! fn todo_v1(old: &DynValue) -> Result<Todo, MigrateError> { .. }
//!
//! #[undra::migrate(store = "Profile", signal = "age")] // one signal of one store in a snapshot
//! fn age(old: Option<&DynValue>) -> Result<f32, MigrateError> { .. }
//!
//! #[undra::migrate(mutation = "add_todo", from = "0x1f..")] // a queued mutation's input
//! fn add_todo(old: &DynRecord) -> Result<DynRecord, MigrateError> { .. }
//! ```
//!
//! The function stays as written. The expansion adds a wrapper that encodes a typed result with
//! its `Encode` and submits a `persist::Migration` through `inventory`; the runtime runs it under
//! the panic guard when structural migration cannot convert a value (`undra-runtime::persist`).
//!
//! What the macro checks (E0066): the arguments name exactly one kind of target; the function is a
//! plain, non-generic `fn` of one parameter of the target's shape returning
//! `Result<_, MigrateError>`; `from` is a 64-bit fingerprint; and, for `ty = "X"`, at compile time
//! in the user's crate, that the returned type is the type `X` declared with `#[undra::api]` (a
//! const assertion on `UNDRA_TYPE_ID`, like E0061). A store, signal or mutation is not named by the
//! function's signature, so its existence (and a signal hook's return type) is checked when a
//! runtime starts, which logs an E0066 ERROR for a target the core does not have.

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{FnArg, GenericArgument, PathArguments, ReturnType, Type};

use super::attrs::{option_value, parse_args, root_arg};
use super::check::panic_text;
use super::diag::{Diag, Errors, code};
use super::paths::Root;
use super::types::{Allow, Pos, map_type, ty_string};

/// The parsed arguments.
#[derive(Default)]
struct Args {
    root: Option<Root>,
    ty: Option<syn::LitStr>,
    store: Option<syn::LitStr>,
    signal: Option<syn::LitStr>,
    mutation: Option<syn::LitStr>,
    from: Option<(syn::LitStr, u64)>,
}

/// What the hook converts, once the arguments are checked.
enum Target {
    Type(String),
    Signal { store: String, signal: String },
    Mutation(String),
}

const EXPECTED: &str = "`ty = \"Record\"`, `store = \"Store\", signal = \"field\"`, `mutation = \"name\"`, `from = \"0x..\"` and `crate = \"path\"`";

fn e0066(what: impl core::fmt::Display, fix: impl core::fmt::Display) -> Diag {
    Diag::new(
        code::E0066,
        what,
        "a migration hook converts persisted data of one record or enum, one signal of one store, or one mutation's queued input to this build's types; the runtime finds it by that target and calls it with the old value in a fixed shape",
        fix,
    )
}

fn parse(attr: TokenStream) -> syn::Result<Args> {
    let mut args = Args::default();
    parse_args(attr, "migrate", EXPECTED, |meta| {
        let string = |name: &str, example: &str| -> syn::Result<syn::LitStr> {
            option_value(meta, code::E0008, name, "a string literal", example)
        };
        if meta.path.is_ident("crate") {
            args.root = Some(root_arg(meta)?);
        } else if meta.path.is_ident("ty") {
            args.ty = Some(string("ty", "ty = \"Todo\"")?);
        } else if meta.path.is_ident("store") {
            args.store = Some(string("store", "store = \"Profile\"")?);
        } else if meta.path.is_ident("signal") {
            args.signal = Some(string("signal", "signal = \"age\"")?);
        } else if meta.path.is_ident("mutation") {
            args.mutation = Some(string("mutation", "mutation = \"add_todo\"")?);
        } else if meta.path.is_ident("from") {
            let lit = string("from", "from = \"0x0123456789abcdef\"")?;
            let text = lit.value();
            let digits = text.strip_prefix("0x").unwrap_or(&text).replace('_', "");
            let Ok(value) = u64::from_str_radix(&digits, 16) else {
                return Err(e0066(
                    format!("`from = \"{text}\"` is not a fingerprint"),
                    "write the 64-bit fingerprint of the old type in hex, as the runtime's log or `undra.types.<fingerprint>` key shows it: `from = \"0x0123456789abcdef\"`, or leave `from` out to take every old version",
                )
                .on(&lit));
            };
            args.from = Some((lit, value));
        } else {
            return Ok(false);
        }
        Ok(true)
    })?;
    Ok(args)
}

fn target(args: &Args, span: proc_macro2::Span) -> syn::Result<Target> {
    let kinds = usize::from(args.ty.is_some())
        + usize::from(args.store.is_some() || args.signal.is_some())
        + usize::from(args.mutation.is_some());
    let fix = "name exactly one target: `ty = \"Todo\"`, `store = \"Profile\", signal = \"age\"` or `mutation = \"add_todo\"`";
    if kinds != 1 {
        return Err(e0066(
            if kinds == 0 {
                "`#[undra::migrate]` needs a target".to_owned()
            } else {
                "`#[undra::migrate]` names more than one target".to_owned()
            },
            fix,
        )
        .at(span));
    }
    let nonempty = |lit: &syn::LitStr, what: &str| -> syn::Result<String> {
        let value = lit.value();
        if value.is_empty() || !value.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return Err(e0066(
                format!("`{what} = \"{value}\"` is not a name"),
                format!("write the {what}'s name as it is declared in Rust"),
            )
            .on(lit));
        }
        Ok(value)
    };
    if let Some(ty) = &args.ty {
        return Ok(Target::Type(nonempty(ty, "ty")?));
    }
    if let Some(mutation) = &args.mutation {
        return Ok(Target::Mutation(nonempty(mutation, "mutation")?));
    }
    match (&args.store, &args.signal) {
        (Some(store), Some(signal)) => Ok(Target::Signal {
            store: nonempty(store, "store")?,
            signal: nonempty(signal, "signal")?,
        }),
        (Some(store), None) => Err(e0066(
            "`store = \"..\"` needs `signal = \"..\"`",
            "name the signal the hook converts: `store = \"Profile\", signal = \"age\"`; a hook for the store's type as a whole is not a thing, a store migrates signal by signal",
        )
        .on(store)),
        (None, Some(signal)) => Err(e0066(
            "`signal = \"..\"` needs `store = \"..\"`",
            "name the store the signal belongs to: `store = \"Profile\", signal = \"age\"`",
        )
        .on(signal)),
        (None, None) => Err(e0066("`#[undra::migrate]` needs a target", fix).at(span)),
    }
}

/// The last path segment of `ty` and its type arguments.
fn segment(ty: &Type) -> Option<(String, Vec<&Type>)> {
    let ty = match ty {
        Type::Group(group) => &*group.elem,
        Type::Paren(paren) => &*paren.elem,
        other => other,
    };
    let Type::Path(path) = ty else {
        return None;
    };
    if path.qself.is_some() {
        return None;
    }
    let last = path.path.segments.last()?;
    let args = match &last.arguments {
        PathArguments::AngleBracketed(args) => args
            .args
            .iter()
            .filter_map(|a| match a {
                GenericArgument::Type(t) => Some(t),
                _ => None,
            })
            .collect(),
        PathArguments::None => Vec::new(),
        PathArguments::Parenthesized(_) => return None,
    };
    Some((last.ident.to_string(), args))
}

/// Whether `ty` is `&Name` (no lifetime other than elided).
fn is_ref_to(ty: &Type, name: &str) -> bool {
    match ty {
        Type::Reference(r) if r.mutability.is_none() => {
            segment(&r.elem).is_some_and(|(n, args)| n == name && args.is_empty())
        }
        _ => false,
    }
}

/// The `T` of `Result<T, MigrateError>`.
fn ok_type(output: &ReturnType) -> Option<&Type> {
    let ReturnType::Type(_, ty) = output else {
        return None;
    };
    let (name, args) = segment(ty)?;
    if name != "Result" || args.len() != 2 {
        return None;
    }
    let (err, _) = segment(args[1])?;
    (err == "MigrateError").then_some(args[0])
}

pub(crate) fn expand(attr: TokenStream, item: syn::ItemFn) -> syn::Result<TokenStream> {
    let args = parse(attr)?;
    let target = target(&args, item.sig.ident.span())?;
    let root = args.root.clone().unwrap_or_default();
    let runtime = root.runtime();
    let wire = root.wire();
    let meta = root.meta();
    let name = &item.sig.ident;
    let name_str = name.to_string();
    let mut errors = Errors::new();

    // The shape of the function.
    let sig = &item.sig;
    let (param_sig, ret_sig, about) = match target {
        Target::Type(_) => (
            "old: &DynValue",
            "Result<T, MigrateError>",
            ", where `T` is the target type",
        ),
        Target::Signal { .. } => (
            "old: Option<&DynValue>",
            "Result<T, MigrateError>",
            ", where `T` is the signal's value type (`old` is `None` when the snapshot lacks the signal)",
        ),
        Target::Mutation(_) => ("old: &DynRecord", "Result<DynRecord, MigrateError>", ""),
    };
    let wrong_shape = |what: String| {
        e0066(
            what,
            format!(
                "write `fn {name_str}({param_sig}) -> {ret_sig}`{about}: a plain, non-generic, non-async function"
            ),
        )
    };
    if sig.asyncness.is_some()
        || !sig.generics.params.is_empty()
        || sig.generics.where_clause.is_some()
        || sig.constness.is_some()
        || sig.unsafety.is_some()
        || sig.variadic.is_some()
        || sig.abi.is_some()
    {
        errors.push(
            wrong_shape(format!(
                "the migration hook `{name_str}` must be a plain, non-generic, non-async `fn`"
            ))
            .on(&sig.ident),
        );
    }
    let param: Option<&Type> = match sig.inputs.iter().collect::<Vec<_>>().as_slice() {
        [FnArg::Typed(pat)] => Some(&pat.ty),
        _ => None,
    };
    let param_ok = param.is_some_and(|ty| match &target {
        Target::Type(_) => is_ref_to(ty, "DynValue"),
        Target::Mutation(_) => is_ref_to(ty, "DynRecord"),
        Target::Signal { .. } => segment(ty).is_some_and(|(n, args)| {
            n == "Option" && args.len() == 1 && is_ref_to(args[0], "DynValue")
        }),
    });
    if !param_ok {
        errors.push(
            wrong_shape(format!(
                "the migration hook `{name_str}` must take one parameter, `{param_sig}`"
            ))
            .on(&sig.inputs),
        );
    }
    let ok = ok_type(&sig.output);
    let ok_valid = match (&target, ok) {
        (Target::Mutation(_), Some(t)) => {
            segment(t).is_some_and(|(n, a)| n == "DynRecord" && a.is_empty())
        }
        (_, Some(_)) => true,
        (_, None) => false,
    };
    if !ok_valid {
        let diag = wrong_shape(format!(
            "the migration hook `{name_str}` must return `{ret_sig}`"
        ));
        errors.push(match &sig.output {
            ReturnType::Type(_, ty) => diag.on(ty),
            ReturnType::Default => diag.on(&sig.ident),
        });
    }
    errors.finish()?;
    let ok = ok.expect("checked above");

    // The type the hook returns, for the start-up check against the schema.
    let returns = match &target {
        Target::Mutation(_) => quote!(::core::option::Option::None),
        _ => match map_type(ok, Pos::Return, Allow::NONE) {
            Ok(kty) => {
                let ty = kty.meta(&meta);
                quote!(::core::option::Option::Some(#ty))
            }
            Err(e) => return Err(e.into_error()),
        },
    };
    let from = match &args.from {
        Some((_, value)) => quote!(::core::option::Option::Some(#value)),
        None => quote!(::core::option::Option::None),
    };
    let wrapper = format_ident!("__undra_migrate_{}", name);
    let (target_tokens, hook_tokens, wrapper_fn) = match &target {
        Target::Type(ty_name) => (
            quote!(#runtime::persist::MigrationTarget::Type(#ty_name)),
            quote!(#runtime::persist::MigrationHook::Value(#wrapper)),
            quote! {
                fn #wrapper(
                    __old: &#runtime::persist::DynValue,
                ) -> ::core::result::Result<::std::vec::Vec<u8>, #runtime::persist::MigrateError> {
                    #name(__old).map(|__v| #wire::Encode::encode_to_vec(&__v))
                }
            },
        ),
        Target::Signal { store, signal } => (
            quote!(#runtime::persist::MigrationTarget::Signal { store: #store, signal: #signal }),
            quote!(#runtime::persist::MigrationHook::Signal(#wrapper)),
            quote! {
                fn #wrapper(
                    __old: ::core::option::Option<&#runtime::persist::DynValue>,
                ) -> ::core::result::Result<::std::vec::Vec<u8>, #runtime::persist::MigrateError> {
                    #name(__old).map(|__v| #wire::Encode::encode_to_vec(&__v))
                }
            },
        ),
        Target::Mutation(mutation) => (
            quote!(#runtime::persist::MigrationTarget::Mutation(#mutation)),
            quote!(#runtime::persist::MigrationHook::Mutation(#name)),
            TokenStream::new(),
        ),
    };

    // `ty = "X"`: the returned type is the type the target names (E0066 in the user's crate).
    let identity = match &target {
        Target::Type(ty_name) => {
            let shown = ty_string(ok);
            let mismatch = panic_text(&e0066(
                format!(
                    "the migration hook `{name_str}` targets `{ty_name}` but returns `{shown}`"
                ),
                format!(
                    "return the current `{ty_name}` (the hook builds today's value from the old one), or target the type it returns: `ty = \"{shown}\"`"
                ),
            ));
            let undeclared = panic_text(&e0066(
                format!(
                    "the migration hook `{name_str}` returns `{shown}`, which is not a record or enum declared with `#[undra::api]`"
                ),
                format!(
                    "a `ty` hook targets and returns a record or enum of the schema: declare `{shown}` with `#[undra::api]` if it is one, or return the `#[undra::api]` type `{ty_name}` names"
                ),
            ));
            let span = ok.span();
            quote_spanned! {span=>
                #[doc(hidden)]
                #[allow(non_camel_case_types, dead_code, unused, clippy::all)]
                const _: () = {
                    trait __UndraFallback {
                        const UNDRA_TYPE_ID: u32 = 0;
                    }
                    impl<T: ?::core::marker::Sized> __UndraFallback for T {}
                    const _: () = {
                        let __undra_id = <#ok>::UNDRA_TYPE_ID;
                        if __undra_id == 0 {
                            ::core::panic!(#undeclared);
                        }
                        if __undra_id != #meta::ids::type_id(#ty_name) {
                            ::core::panic!(#mismatch);
                        }
                    };
                };
            }
        }
        _ => TokenStream::new(),
    };

    Ok(quote! {
        #item

        #[doc(hidden)]
        #[allow(non_snake_case, non_camel_case_types, dead_code, unused, clippy::all)]
        const _: () = {
            #wrapper_fn
            #runtime::inventory::submit! {
                #runtime::persist::Migration {
                    name: ::core::concat!(::core::module_path!(), "::", #name_str),
                    target: #target_tokens,
                    from: #from,
                    returns: #returns,
                    hook: #hook_tokens,
                }
            }
        };

        #identity
    })
}
