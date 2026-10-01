//! `#[undra::error]`: what an error enum gets on top of `#[undra::api]`.
//!
//! Every variant carries `#[error("..")]` (thiserror style). The message may interpolate
//! fields: `{0}` for tuple fields, `{name}` for named fields, with the usual format specs
//! (`{0:?}`, `{name:>8}`). An implicit `{}` is rejected: the schema keeps the message text, and the
//! platforms resolve a placeholder from it by position or name, never by counting `{}`s.
//! `#[error(transparent)]` forwards `Display` and `source()` to the variant's single field.
//!
//! Generated: `Display`, `std::error::Error` (with `source()` for `#[source]`/`#[from]`
//! fields and transparent variants) and `From<Inner>` for every `#[from]` field. A
//! `#[derive(Debug)]` is added if the enum does not derive it, because `std::error::Error`
//! requires it.

use std::collections::BTreeSet;

use proc_macro2::{Span, TokenStream};
use quote::quote;

use super::diag::{Diag, Errors, code};
use super::record::{FieldModel, Shape, VariantModel, bindings};

/// The `#[error(..)]` attribute of one variant.
pub(crate) enum ErrorAttr {
    /// `#[error("template")]`, with the template rewritten to use `__fN` bindings.
    Message {
        /// The template as the user wrote it (goes into the schema).
        text: String,
        /// The template with every argument replaced by its `__f<position>` binding.
        rewritten: String,
        /// The field positions the template uses.
        used: BTreeSet<usize>,
    },
    /// `#[error(transparent)]`.
    Transparent,
}

fn invalid(
    node: &impl quote::ToTokens,
    what: impl std::fmt::Display,
    why: &str,
    help: &str,
) -> syn::Error {
    Diag::new(code::E0010, what, why, help).on(node)
}

/// Removes `#[error(..)]` from a variant and parses it against the variant's fields.
pub(crate) fn take_message(
    attrs: &mut Vec<syn::Attribute>,
    variant: &syn::Ident,
    shape: Shape,
    fields: &[FieldModel],
    errors: &mut Errors,
) -> Option<ErrorAttr> {
    let mut found: Vec<syn::Attribute> = Vec::new();
    attrs.retain(|attr| {
        if attr.path().is_ident("error") {
            found.push(attr.clone());
            false
        } else {
            true
        }
    });
    let Some(attr) = found.first() else {
        errors.push(invalid(
            variant,
            format!("variant `{variant}` has no `#[error(..)]` message"),
            "every variant of a `#[undra::error]` enum needs a message, because the platforms show it to users and logs",
            "add `#[error(\"what went wrong\")]`, or `#[error(transparent)]` to forward to the inner error",
        ));
        return None;
    };
    if found.len() > 1 {
        errors.push(invalid(
            &found[1],
            format!("variant `{variant}` has more than one `#[error(..)]`"),
            "a variant has exactly one message",
            "keep a single `#[error(..)]` per variant",
        ));
    }
    let syn::Meta::List(list) = &attr.meta else {
        errors.push(invalid(
            attr,
            format!("`#[error]` on variant `{variant}` needs a message"),
            "the attribute carries the text the platforms show for this error",
            "write `#[error(\"message\")]` or `#[error(transparent)]`",
        ));
        return None;
    };
    // `transparent`
    if syn::parse2::<syn::Ident>(list.tokens.clone()).is_ok_and(|ident| ident == "transparent") {
        if fields.len() != 1 {
            errors.push(invalid(
                attr,
                format!(
                    "`#[error(transparent)]` on variant `{variant}` needs exactly one field, found {}",
                    fields.len()
                ),
                "`transparent` forwards `Display` and `source()` to the variant's only field",
                "give the variant a single field holding the inner error",
            ));
        }
        return Some(ErrorAttr::Transparent);
    }
    let lit: syn::LitStr = match syn::parse2(list.tokens.clone()) {
        Ok(lit) => lit,
        Err(_) => {
            errors.push(invalid(
                &list.tokens,
                format!("`#[error(..)]` on variant `{variant}` must be a single string literal or `transparent`"),
                "the message is a template over the variant's fields; extra `format!` arguments are not supported",
                "write `#[error(\"message with {0} or {name}\")]`",
            ));
            return None;
        }
    };
    match rewrite_template(&lit.value(), shape, fields) {
        Ok((rewritten, used)) => Some(ErrorAttr::Message {
            text: lit.value(),
            rewritten,
            used,
        }),
        Err(reason) => {
            errors.push(invalid(
                &lit,
                format!("invalid message for variant `{variant}`: {reason}"),
                "the message is a template over the variant's fields",
                "use `{0}`, `{1}`, .. for tuple fields and `{name}` for named fields",
            ));
            None
        }
    }
}

/// Removes `#[from]` and `#[source]` from a field; returns `(from, source)`.
pub(crate) fn take_field_attrs(attrs: &mut Vec<syn::Attribute>) -> (bool, bool) {
    let mut from = false;
    let mut source = false;
    attrs.retain(|attr| {
        if attr.path().is_ident("from") {
            from = true;
            false
        } else if attr.path().is_ident("source") {
            source = true;
            false
        } else {
            true
        }
    });
    (from, source)
}

/// Checks the `#[from]` / `#[source]` rules that need the whole variant.
pub(crate) fn validate(variants: &[VariantModel], errors: &mut Errors) {
    for variant in variants {
        let from_fields = variant.fields.iter().filter(|f| f.from).count();
        if from_fields > 0 && variant.fields.len() != 1 {
            errors.push(
                Diag::new(
                    code::E0010,
                    format!(
                        "`#[from]` on variant `{}` needs the variant to have exactly one field",
                        variant.ident
                    ),
                    "`From<Inner>` converts a single value into the variant, so there must be nothing else to fill in",
                    "remove the other fields, or drop `#[from]` and use `#[source]`",
                )
                .at(variant.span),
            );
        }
        let sources = variant.fields.iter().filter(|f| f.source).count();
        if sources > 1 {
            errors.push(
                Diag::new(
                    code::E0010,
                    format!("variant `{}` has more than one source field", variant.ident),
                    "an error has a single `source()`",
                    "keep `#[source]` on one field",
                )
                .at(variant.span),
            );
        }
    }
}

/// Rewrites a message template so every placeholder names a `__f<position>` binding.
fn rewrite_template(
    text: &str,
    shape: Shape,
    fields: &[FieldModel],
) -> Result<(String, BTreeSet<usize>), String> {
    let mut out = String::with_capacity(text.len() + 8);
    let mut used = BTreeSet::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => {
                if chars.peek() == Some(&'{') {
                    chars.next();
                    out.push_str("{{");
                    continue;
                }
                let mut inner = String::new();
                loop {
                    match chars.next() {
                        Some('}') => break,
                        Some(ch) => inner.push(ch),
                        None => return Err("unterminated `{`".to_owned()),
                    }
                }
                let (arg, spec) = match inner.split_once(':') {
                    Some((arg, spec)) => (arg.trim(), Some(spec)),
                    None => (inner.trim(), None),
                };
                if spec.is_some_and(|s| s.contains('$') || s.contains('*')) {
                    return Err(format!(
                        "`{{{inner}}}`: width and precision arguments are not supported"
                    ));
                }
                if arg.is_empty() {
                    return Err(format!(
                        "`{{{inner}}}` does not say which field it shows; write its position (`{{0}}`) or its name (`{{name}}`)"
                    ));
                }
                let position = if arg.chars().all(|ch| ch.is_ascii_digit()) {
                    if shape != Shape::Tuple {
                        return Err(format!(
                            "`{{{inner}}}` refers to a position, but the variant has {}",
                            if shape == Shape::Unit {
                                "no fields"
                            } else {
                                "named fields; use `{name}`"
                            }
                        ));
                    }
                    let position = arg
                        .parse::<usize>()
                        .map_err(|_| format!("`{{{inner}}}` is not a valid position"))?;
                    if position >= fields.len() {
                        return Err(format!(
                            "`{{{inner}}}` refers to field {position}, but the variant has {}",
                            fields.len()
                        ));
                    }
                    position
                } else {
                    if shape != Shape::Named {
                        return Err(format!(
                            "`{{{inner}}}` names a field, but the variant has {}",
                            if shape == Shape::Unit {
                                "no fields"
                            } else {
                                "tuple fields; use `{0}`"
                            }
                        ));
                    }
                    fields
                        .iter()
                        .position(|f| f.name == arg)
                        .ok_or_else(|| format!("`{{{inner}}}` names no field of the variant"))?
                };
                used.insert(position);
                out.push_str("{__f");
                out.push_str(&position.to_string());
                if let Some(spec) = spec {
                    out.push(':');
                    out.push_str(spec);
                }
                out.push('}');
            }
            '}' => {
                if chars.peek() == Some(&'}') {
                    chars.next();
                    out.push_str("}}");
                } else {
                    return Err("unmatched `}`".to_owned());
                }
            }
            other => out.push(other),
        }
    }
    Ok((out, used))
}

/// A pattern that binds only the fields at `wanted` positions (and ignores the rest).
fn partial_pattern(
    variant: &VariantModel,
    bindings: &[syn::Ident],
    wanted: &BTreeSet<usize>,
) -> TokenStream {
    let ident = &variant.ident;
    match variant.shape {
        Shape::Unit => quote!(Self::#ident),
        Shape::Tuple => {
            let slots = bindings.iter().enumerate().map(|(i, b)| {
                if wanted.contains(&i) {
                    quote!(#b)
                } else {
                    quote!(_)
                }
            });
            quote!(Self::#ident( #(#slots),* ))
        }
        Shape::Named => {
            let bound = variant
                .fields
                .iter()
                .enumerate()
                .filter(|(i, _)| wanted.contains(i))
                .map(|(i, f)| {
                    let field = f.ident.as_ref().expect("named field");
                    let binding = &bindings[i];
                    quote!(#field: #binding)
                });
            quote!(Self::#ident { #(#bound,)* .. })
        }
    }
}

/// `Display`, `std::error::Error` and `From` impls for an error enum.
pub(crate) fn expand_extras(
    _root: &super::paths::Root,
    name: &syn::Ident,
    variants: &[VariantModel],
) -> TokenStream {
    let derived = super::common::derived();

    let display_arms = variants.iter().map(|v| {
        let binds = bindings(v.fields.len());
        match &v.error {
            Some(ErrorAttr::Message {
                rewritten, used, ..
            }) => {
                let pattern = partial_pattern(v, &binds, used);
                let template = syn::LitStr::new(rewritten, Span::call_site());
                quote! {
                    #pattern => ::core::write!(__fmt, #template),
                }
            }
            Some(ErrorAttr::Transparent) => {
                let pattern = partial_pattern(v, &binds, &BTreeSet::from([0]));
                let inner = &binds[0];
                quote! {
                    #pattern => ::core::fmt::Display::fmt(#inner, __fmt),
                }
            }
            // Validation reports a variant without a message; nothing is generated then.
            None => TokenStream::new(),
        }
    });

    let mut covered = 0usize;
    let mut source_arms = Vec::new();
    for v in variants {
        let binds = bindings(v.fields.len());
        match &v.error {
            Some(ErrorAttr::Transparent) => {
                let pattern = partial_pattern(v, &binds, &BTreeSet::from([0]));
                let inner = &binds[0];
                source_arms.push(quote! {
                    #pattern => ::std::error::Error::source(#inner),
                });
                covered += 1;
            }
            _ => {
                if let Some(position) = v.fields.iter().position(|f| f.source) {
                    let pattern = partial_pattern(v, &binds, &BTreeSet::from([position]));
                    let inner = &binds[position];
                    source_arms.push(quote! {
                        #pattern => ::core::option::Option::Some(
                            #inner as &(dyn ::std::error::Error + 'static)
                        ),
                    });
                    covered += 1;
                }
            }
        }
    }
    let error_impl = if source_arms.is_empty() {
        quote! {
            #derived
            impl ::std::error::Error for #name {}
        }
    } else {
        let fallback = if covered < variants.len() {
            quote!(_ => ::core::option::Option::None,)
        } else {
            TokenStream::new()
        };
        quote! {
            #derived
            impl ::std::error::Error for #name {
                fn source(&self) -> ::core::option::Option<&(dyn ::std::error::Error + 'static)> {
                    match self {
                        #(#source_arms)*
                        #fallback
                    }
                }
            }
        }
    };

    let from_impls = variants.iter().filter_map(|v| {
        let field = v.fields.iter().find(|f| f.from)?;
        let ty = &field.ty;
        let ident = &v.ident;
        let construct = match &field.ident {
            Some(field_name) => quote!(Self::#ident { #field_name: __source }),
            None => quote!(Self::#ident(__source)),
        };
        Some(quote! {
            #derived
            impl ::core::convert::From<#ty> for #name {
                fn from(__source: #ty) -> Self {
                    #construct
                }
            }
        })
    });

    quote! {
        #derived
        impl ::core::fmt::Display for #name {
            fn fmt(&self, __fmt: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self {
                    #(#display_arms)*
                }
            }
        }

        #error_impl

        #(#from_impls)*
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(name: &str) -> FieldModel {
        FieldModel {
            ident: if name.chars().all(|c| c.is_ascii_digit()) {
                None
            } else {
                Some(syn::Ident::new(name, Span::call_site()))
            },
            name: name.to_owned(),
            ty: syn::parse_quote!(String),
            kty: super::super::types::KType::String,
            default: false,
            docs: String::new(),
            from: false,
            source: false,
        }
    }

    fn tuple_fields(n: usize) -> Vec<FieldModel> {
        (0..n).map(|i| field(&i.to_string())).collect()
    }

    fn rewrite(
        text: &str,
        shape: Shape,
        fields: &[FieldModel],
    ) -> Result<(String, Vec<usize>), String> {
        rewrite_template(text, shape, fields).map(|(s, used)| (s, used.into_iter().collect()))
    }

    #[test]
    fn plain_text_passes_through() {
        assert_eq!(
            rewrite("title cannot be empty", Shape::Unit, &[]).unwrap(),
            ("title cannot be empty".to_owned(), vec![])
        );
    }

    #[test]
    fn positional_placeholders() {
        let fields = tuple_fields(2);
        assert_eq!(
            rewrite("bad {0} and {1:?}", Shape::Tuple, &fields).unwrap(),
            ("bad {__f0} and {__f1:?}".to_owned(), vec![0, 1])
        );
        assert_eq!(
            rewrite("only {1}", Shape::Tuple, &fields).unwrap(),
            ("only {__f1}".to_owned(), vec![1])
        );
    }

    #[test]
    fn named_placeholders_and_specs() {
        let fields = vec![field("code"), field("reason")];
        assert_eq!(
            rewrite("{reason} ({code:>4})", Shape::Named, &fields).unwrap(),
            ("{__f1} ({__f0:>4})".to_owned(), vec![0, 1])
        );
    }

    #[test]
    fn escaped_braces_survive() {
        let fields = tuple_fields(1);
        assert_eq!(
            rewrite("{{literal}} {0}", Shape::Tuple, &fields).unwrap().0,
            "{{literal}} {__f0}"
        );
    }

    #[test]
    fn invalid_templates_are_explained() {
        let tuple = tuple_fields(1);
        let named = vec![field("a")];
        assert!(
            rewrite("{2}", Shape::Tuple, &tuple)
                .unwrap_err()
                .contains("field 2")
        );
        // An implicit position is rejected: the schema keeps the text, and the platforms resolve
        // placeholders by index or name.
        for text in ["{}", "{} then {}", "{:?}", "{:>4}"] {
            let reason = rewrite(text, Shape::Tuple, &tuple_fields(2)).unwrap_err();
            assert!(
                reason.contains("does not say which field"),
                "{text}: {reason}"
            );
            assert!(reason.contains("{0}"), "{text}: {reason}");
        }
        assert!(
            rewrite("{a}", Shape::Tuple, &tuple)
                .unwrap_err()
                .contains("tuple fields")
        );
        assert!(
            rewrite("{0}", Shape::Named, &named)
                .unwrap_err()
                .contains("named fields")
        );
        assert!(
            rewrite("{b}", Shape::Named, &named)
                .unwrap_err()
                .contains("names no field")
        );
        assert!(
            rewrite("{x}", Shape::Unit, &[])
                .unwrap_err()
                .contains("no fields")
        );
        assert!(
            rewrite("{0", Shape::Tuple, &tuple)
                .unwrap_err()
                .contains("unterminated")
        );
        assert!(
            rewrite("a } b", Shape::Unit, &[])
                .unwrap_err()
                .contains("unmatched")
        );
        assert!(
            rewrite("{0:width$}", Shape::Tuple, &tuple)
                .unwrap_err()
                .contains("width")
        );
    }
}
