//! Helpers shared by every expansion.

use proc_macro2::TokenStream;
use quote::quote;

use super::attrs::{Site, take};
use super::diag::{Diag, Errors, code};
use super::paths::Root;
use super::types::KType;

/// E0002 / E0003 for every generic parameter of an item.
pub(crate) fn check_generics(generics: &syn::Generics, item: &str, errors: &mut Errors) {
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
            syn::GenericParam::Type(ty) => errors.push(
                Diag::new(
                    code::E0002,
                    format!("generic parameter `{}` on `{item}`", ty.ident),
                    "the schema describes concrete types; every target language would need one instantiation per use",
                    "remove the parameter, and declare one concrete `#[undra::api]` type per instantiation",
                )
                .on(param),
            ),
            syn::GenericParam::Const(constant) => errors.push(
                Diag::new(
                    code::E0002,
                    format!("const generic `{}` on `{item}`", constant.ident),
                    "the schema describes concrete types; every target language would need one instantiation per use",
                    "remove the parameter, and declare one concrete `#[undra::api]` type per instantiation",
                )
                .on(param),
            ),
        }
    }
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
