//! Helpers shared by every expansion.

use proc_macro2::TokenStream;
use quote::quote;

use super::attrs::{Site, take};
use super::diag::{Diag, Errors, code};
use super::paths::Root;
use super::types::{KType, ty_string};

/// What carries the generic parameters, for the help of E0002 (ADR-042 decision 2.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GenericOn {
    /// A struct or enum under `#[undra::api]` without `generic`: it may become a template.
    Data,
    /// A `#[undra::error]` enum: never generic.
    Error,
    /// An object, store, function, method, port, callback, query or mutation: never generic.
    Other,
    /// A struct or enum under `#[undra::api(generic)]`: type parameters are what it is for.
    Template,
}

impl GenericOn {
    fn why(self) -> &'static str {
        match self {
            GenericOn::Data => {
                "the schema describes concrete types; every target language would need one instantiation per use"
            }
            GenericOn::Error => {
                "the schema describes concrete types, and the platforms throw an error by name"
            }
            GenericOn::Other => {
                "the schema describes concrete types, and objects, stores, functions, methods, ports, callbacks, queries and mutations are dispatched by id: one dispatcher per instantiation, and a schema that can name a type parameter, would be needed"
            }
            GenericOn::Template => "only type parameters are supported by a generic data type",
        }
    }

    fn help(self) -> &'static str {
        match self {
            GenericOn::Data => {
                "mark the struct or enum `#[undra::api(generic)]` and declare each instantiation under a name: `#[undra::api] pub type TodoPage = Page<Todo>;`; or remove the parameter and declare one concrete `#[undra::api]` type per instantiation"
            }
            GenericOn::Error => {
                "remove the parameter and declare one concrete `#[undra::error]` enum per use"
            }
            GenericOn::Other => {
                "remove the parameter and write the concrete types; only data types can be generic: mark a struct or enum `#[undra::api(generic)]` and declare each instantiation under a name (`#[undra::api] pub type TodoPage = Page<Todo>;`)"
            }
            GenericOn::Template => "remove the parameter",
        }
    }
}

/// E0002 / E0003 for every generic parameter of an item that is not a data type (an object, a
/// store, a function, a method, a port, a callback, a query, a mutation).
pub(crate) fn check_generics(generics: &syn::Generics, item: &str, errors: &mut Errors) {
    check_generics_on(generics, item, GenericOn::Other, errors);
}

/// E0002 / E0003 for every generic parameter of an item, with the help of what it is.
///
/// Only type parameters are supported by a template (ADR-042): a lifetime is E0003, a const
/// parameter and a `where` clause E0002.
pub(crate) fn check_generics_on(
    generics: &syn::Generics,
    item: &str,
    on: GenericOn,
    errors: &mut Errors,
) {
    for param in &generics.params {
        match param {
            syn::GenericParam::Lifetime(lifetime) => errors.push(
                Diag::new(
                    code::E0003,
                    format!("lifetime `{}` on `{item}`", lifetime.lifetime),
                    "everything crosses the boundary by value; a borrow cannot outlive the call that made it",
                    "remove the lifetime parameter and use owned types",
                )
                .on(param),
            ),
            // A template exists to have them; only a default is refused.
            syn::GenericParam::Type(ty) if on == GenericOn::Template => {
                if let Some(default) = &ty.default {
                    errors.push(
                        Diag::new(
                            code::E0002,
                            format!("default type `{}` for the parameter `{}` of `{item}`", ty_string(default), ty.ident),
                            "only type parameters are supported: an instantiation names every argument, so a default would only hide which type a signature means",
                            "remove the default and write the argument in each alias, `pub type TodoPage = Page<Todo>;`",
                        )
                        .on(param),
                    );
                }
            }
            syn::GenericParam::Type(ty) => errors.push(
                Diag::new(
                    code::E0002,
                    format!("generic parameter `{}` on `{item}`", ty.ident),
                    on.why(),
                    on.help(),
                )
                .on(param),
            ),
            syn::GenericParam::Const(constant) => errors.push(
                Diag::new(
                    code::E0002,
                    format!("const generic `{}` on `{item}`", constant.ident),
                    "only type parameters are supported: a const parameter changes the layout of the type with its value, and the schema cannot name it",
                    "remove the const parameter and write the number in the type, or declare one concrete `#[undra::api]` type per value",
                )
                .on(param),
            ),
        }
    }
    if let Some(clause) = &generics.where_clause {
        errors.push(
            Diag::new(
                code::E0002,
                format!("`where` clause on `{item}`"),
                "only type parameters are supported: the schema records no bounds, and each instantiation is a plain named type",
                "remove the `where` clause; write a bound on the parameter itself (`T: Clone`) if the type needs one",
            )
            .on(clause),
        );
    }
}

/// Whether `tokens` mention `Self`. A free function cannot, so a signature that does sits in an
/// `impl` block: the one placement a macro on a function cannot see directly.
pub(crate) fn mentions_self(tokens: TokenStream) -> bool {
    use proc_macro2::TokenTree;
    tokens.into_iter().any(|tree| match tree {
        TokenTree::Ident(ident) => ident == "Self",
        TokenTree::Group(group) => mentions_self(group.stream()),
        _ => false,
    })
}

/// The name of the function that asserts a future or stream is `Send` (E0022).
///
/// `rustc` reports a future that is not `Send` itself, with the value held across an `.await` and
/// the `.await` in question, and it cannot carry an Undra code. What it does print is the name of
/// the bound it was asked to check ("required by a bound in `..`"), so the name carries the code
/// and the rule: that note is how an engineer who sees the error finds the explanation.
///
/// `span` is where a call to it is reported: the method, so that the error points at it and not at
/// generated code (the definition uses [`proc_macro2::Span::call_site`]; both resolve to the
/// same name).
pub(crate) fn send_assertion(span: proc_macro2::Span) -> syn::Ident {
    let mut name = quote::format_ident!(
        "_undra_error_{}_the_future_of_an_async_method_must_be_Send",
        code::E0022
    );
    name.set_span(span);
    name
}

/// The `inventory::submit!` for one registration.
pub(crate) fn submit(root: &Root, variant: &str, meta_static: &syn::Ident) -> TokenStream {
    let meta = root.meta();
    let variant = syn::Ident::new(variant, proc_macro2::Span::call_site());
    quote! {
        #meta::inventory::submit! { #meta::Registration::#variant(&#meta_static) }
    }
}

/// `#[automatically_derived]` is applied to generated trait impls so lints written for
/// hand-written code stay quiet.
pub(crate) fn derived() -> TokenStream {
    quote!(#[automatically_derived])
}

/// A `FieldMeta { .. }` expression.
pub(crate) fn field_meta(
    meta: &TokenStream,
    name: &str,
    kty: &KType,
    default: bool,
    docs: &str,
) -> TokenStream {
    let ty = kty.meta(meta);
    quote! {
        #meta::FieldMeta { name: #name, ty: #ty, default: #default, docs: #docs }
    }
}

/// A `ParamMeta { .. }` expression.
pub(crate) fn param_meta(meta: &TokenStream, name: &str, kty: &KType) -> TokenStream {
    let ty = kty.meta(meta);
    quote! {
        #meta::ParamMeta { name: #name, ty: #ty }
    }
}

/// Parses the item-level `#[undra(..)]` attributes and combines them with a `crate = ".."`
/// macro argument. The item attribute wins.
pub(crate) fn item_root(
    attrs: &mut Vec<syn::Attribute>,
    args_root: Option<Root>,
    errors: &mut Errors,
) -> Root {
    let parsed = take(attrs, Site::ITEM, errors);
    parsed.root.or(args_root).unwrap_or_default()
}

/// Whether the attribute list already derives `name` (`#[derive(Debug)]`).
pub(crate) fn derives(attrs: &[syn::Attribute], name: &str) -> bool {
    let mut found = false;
    for attr in attrs {
        if !attr.path().is_ident("derive") {
            continue;
        }
        let _ = attr.parse_nested_meta(|meta| {
            if meta
                .path
                .segments
                .last()
                .is_some_and(|seg| seg.ident == name)
            {
                found = true;
            }
            Ok(())
        });
    }
    found
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    #[test]
    fn generics_are_diagnosed_by_kind() {
        let generics: syn::Generics = parse_quote!(<'a, T, const N: usize>);
        let mut errors = Errors::new();
        check_generics(&generics, "Todo", &mut errors);
        let messages: Vec<String> = errors
            .into_error()
            .unwrap()
            .into_iter()
            .map(|e| e.to_string())
            .collect();
        assert_eq!(messages.len(), 3);
        assert!(messages[0].starts_with("error[undra::E0003]"));
        assert!(messages[1].starts_with("error[undra::E0002]: generic parameter `T`"));
        assert!(messages[2].starts_with("error[undra::E0002]: const generic `N`"));
        assert!(messages[2].contains("only type parameters are supported"));
    }

    #[test]
    fn the_help_depends_on_what_carries_the_parameter() {
        let generics: syn::Generics = parse_quote!(<T>);
        let help = |on| {
            let mut errors = Errors::new();
            check_generics_on(&generics, "Page", on, &mut errors);
            errors.into_error().unwrap().to_string()
        };
        assert!(help(GenericOn::Data).contains("#[undra::api(generic)]"));
        assert!(help(GenericOn::Data).contains("pub type TodoPage = Page<Todo>;"));
        assert!(help(GenericOn::Other).contains("only data types can be generic"));
        assert!(help(GenericOn::Error).contains("one concrete `#[undra::error]`"));
    }

    #[test]
    fn a_where_clause_is_e0002() {
        let item: syn::ItemStruct = parse_quote!(
            struct S<T>
            where
                T: Clone,
            {
                a: T,
            }
        );
        let mut errors = Errors::new();
        check_generics_on(&item.generics, "S", GenericOn::Data, &mut errors);
        let messages: Vec<String> = errors
            .into_error()
            .unwrap()
            .into_iter()
            .map(|e| e.to_string())
            .collect();
        assert_eq!(messages.len(), 2);
        assert!(messages[1].starts_with("error[undra::E0002]: `where` clause on `S`"));
        assert!(messages[1].contains("only type parameters are supported"));
    }

    #[test]
    fn no_generics_no_errors() {
        let generics = syn::Generics::default();
        let mut errors = Errors::new();
        check_generics(&generics, "Todo", &mut errors);
        assert!(errors.is_empty());
    }

    #[test]
    fn derives_finds_paths_and_lists() {
        let attrs: Vec<syn::Attribute> = vec![
            parse_quote!(#[derive(Clone, core::fmt::Debug)]),
            parse_quote!(#[derive(PartialEq)]),
        ];
        assert!(derives(&attrs, "Debug"));
        assert!(derives(&attrs, "Clone"));
        assert!(derives(&attrs, "PartialEq"));
        assert!(!derives(&attrs, "Eq"));
        assert!(!derives(&[], "Debug"));
    }
}
