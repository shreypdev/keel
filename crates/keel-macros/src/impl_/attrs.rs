//! Attribute handling: `#[keel(..)]` helper attributes, macro arguments and doc comments.
//!
//! Helper attributes are *consumed*: they are removed from the emitted item so that `rustc`
//! never sees an unknown `keel` attribute. Anything unrecognised or misplaced is E0008;
//! nothing is silently ignored.

use proc_macro2::TokenStream;
use syn::meta::ParseNestedMeta;
use syn::visit_mut::{self, VisitMut};
use syn::{Attribute, Expr, ExprLit, Lit, LitStr, Meta};

use super::diag::{Diag, Errors, code};
use super::paths::Root;

/// What the `#[keel(..)]` attributes on one node said.
#[derive(Default)]
pub(crate) struct KeelAttr {
    /// `#[keel(crate = "path")]`.
    pub(crate) root: Option<Root>,
    /// `#[keel(default)]`.
    pub(crate) default: bool,
    /// `#[keel(key = "field")]`, with the literal for error spans.
    pub(crate) key: Option<LitStr>,
    /// `#[keel(no_coalesce)]`.
    pub(crate) no_coalesce: bool,
}

/// Which `#[keel(..)]` options are legal on a node.
#[derive(Clone, Copy)]
pub(crate) struct Site {
    /// Used in messages: "a record field", "an item".
    pub(crate) name: &'static str,
    pub(crate) root: bool,
    pub(crate) default: bool,
    pub(crate) key: bool,
    pub(crate) no_coalesce: bool,
    /// Whether the node is part of the schema: `#[cfg]` on it would make the schema differ
    /// between builds, so it is rejected (R1, R7).
    pub(crate) schema: bool,
}

impl Site {
    /// The type, impl block, trait or function a macro is applied to.
    pub(crate) const ITEM: Site = Site {
        name: "an item",
        root: true,
        default: false,
        key: false,
        no_coalesce: false,
        schema: false,
    };
    /// A field of a record or of an enum variant.
    pub(crate) const FIELD: Site = Site {
        name: "a record or variant field",
        root: false,
        default: true,
        key: false,
        no_coalesce: false,
        schema: true,
    };
    /// A signal field of a store.
    pub(crate) const SIGNAL: Site = Site {
        name: "a store signal field",
        root: false,
        default: false,
        key: true,
        no_coalesce: true,
        schema: true,
    };
    /// A non-signal field of a store.
    pub(crate) const STATE_FIELD: Site = Site {
        name: "a non-signal store field",
        root: false,
        default: false,
        key: false,
        no_coalesce: false,
        schema: true,
    };
    /// A node that takes no `#[keel(..)]` options at all (variants, methods, parameters).
    pub(crate) const NOTHING: Site = Site {
        name: "this position",
        root: false,
        default: false,
        key: false,
        no_coalesce: false,
        schema: true,
    };
    /// A private method of an API impl block: not part of the schema, so anything goes except
    /// `#[keel(..)]` options.
    pub(crate) const PRIVATE: Site = Site {
        name: "a private method",
        root: false,
        default: false,
        key: false,
        no_coalesce: false,
        schema: false,
    };
}

/// Whether `attr` is a `#[keel(..)]` helper attribute.
pub(crate) fn is_keel_attr(attr: &Attribute) -> bool {
    attr.path().is_ident("keel")
}

/// Removes every `#[keel(..)]` attribute from `attrs` and parses them for `site`.
pub(crate) fn take(attrs: &mut Vec<Attribute>, site: Site, errors: &mut Errors) -> KeelAttr {
    let mut out = KeelAttr::default();
    let mut kept = Vec::with_capacity(attrs.len());
    if site.schema {
        reject_cfg(attrs, site, errors);
    }
    for attr in attrs.drain(..) {
        if !is_keel_attr(&attr) {
            kept.push(attr);
            continue;
        }
        if let Err(error) = parse_one(&attr, site, &mut out) {
            errors.push(error);
        }
    }
    *attrs = kept;
    out
}

/// E0008 for `#[cfg(..)]` and `#[cfg_attr(..)]` on a node that is part of the schema.
///
/// Attribute macros see the tokens before nested `cfg`s are resolved, and the generated
/// encoders would not know which fields exist. More fundamentally, a schema that depends on
/// the build configuration breaks compatibility checking between the core and the platforms.
fn reject_cfg(attrs: &[Attribute], site: Site, errors: &mut Errors) {
    for attr in attrs {
        let path = attr.path();
        if path.is_ident("cfg") || path.is_ident("cfg_attr") {
            errors.push(
                Diag::new(
                    code::E0008,
                    format!("`#[{}]` on {} is not supported", path.get_ident().map_or_else(String::new, ToString::to_string), site.name),
                    "the schema must be identical in every build: it is hashed, and the platforms check the hash when they load the core",
                    "remove the attribute; gate the whole type or function with `#[cfg]` instead, or split it into two types",
                )
                .on(attr),
            );
        }
    }
}

fn parse_one(attr: &Attribute, site: Site, out: &mut KeelAttr) -> syn::Result<()> {
    if !matches!(attr.meta, Meta::List(_)) {
        return Err(Diag::new(
            code::E0008,
            "`#[keel]` needs arguments",
            "the `keel` helper attribute configures a field or an item: `#[keel(default)]`, `#[keel(key = \"id\")]`, `#[keel(no_coalesce)]`, `#[keel(crate = \"path\")]`",
            "write one of the supported options",
        )
        .on(attr));
    }
    attr.parse_nested_meta(|meta| {
        let name = meta
            .path
            .get_ident()
            .map(ToString::to_string)
            .unwrap_or_default();
        let misplaced = |option: &str| {
            Diag::new(
                code::E0008,
                format!("`#[keel({option})]` is not valid on {}", site.name),
                option_hint(option),
                "remove the option, or move it to where it applies",
            )
            .on(&meta.path)
        };
        match name.as_str() {
            "crate" => {
                if !site.root {
                    return Err(misplaced("crate"));
                }
                let lit: LitStr = meta.value()?.parse()?;
                out.root = Some(Root::from_lit(&lit)?);
                Ok(())
            }
            "default" => {
                if !site.default {
                    return Err(misplaced("default"));
                }
                out.default = true;
                Ok(())
            }
            "key" => {
                if !site.key {
                    return Err(misplaced("key"));
                }
                let lit: LitStr = meta.value()?.parse()?;
                out.key = Some(lit);
                Ok(())
            }
            "no_coalesce" => {
                if !site.no_coalesce {
                    return Err(misplaced("no_coalesce"));
                }
                out.no_coalesce = true;
                Ok(())
            }
            other => Err(Diag::new(
                code::E0008,
                format!("unknown option `{other}` in `#[keel(..)]`"),
                "the options are `crate = \"path\"` (items), `default` (record fields), `key = \"field\"` and `no_coalesce` (store signal fields)",
                "remove or correct the option",
            )
            .on(&meta.path)),
        }
    })
}

fn option_hint(option: &str) -> &'static str {
    match option {
        "crate" => {
            "`crate = \"path\"` is an item-level option: it names the crate that generated code refers to"
        }
        "default" => {
            "`default` marks a field of a `#[keel::api]` record or enum variant as having a default in generated constructors"
        }
        "key" => {
            "`key = \"field\"` turns a store's `Signal<Vec<T>>` into a keyed list that ships patches"
        }
        "no_coalesce" => {
            "`no_coalesce` makes a store signal deliver every commit instead of coalescing them"
        }
        _ => "this option is not valid here",
    }
}

/// Parses macro arguments (`#[keel::api(crate = "..", store)]`).
///
/// `handle` is called for every argument; it returns `Ok(true)` if it consumed it. An
/// argument nobody consumed is E0008 listing `expected`.
pub(crate) fn parse_args(
    attr: TokenStream,
    macro_name: &str,
    expected: &str,
    mut handle: impl FnMut(&ParseNestedMeta<'_>) -> syn::Result<bool>,
) -> syn::Result<()> {
    if attr.is_empty() {
        return Ok(());
    }
    let parser = syn::meta::parser(|meta| {
        if handle(&meta)? {
            return Ok(());
        }
        let shown = meta
            .path
            .get_ident()
            .map_or_else(|| "?".to_owned(), ToString::to_string);
        Err(Diag::new(
            code::E0008,
            format!("unknown argument `{shown}` for `#[keel::{macro_name}]`"),
            format!("`#[keel::{macro_name}]` accepts: {expected}"),
            "remove or correct the argument",
        )
        .on(&meta.path))
    });
    syn::parse::Parser::parse2(parser, attr)
}

/// Reads a `crate = "path"` argument.
pub(crate) fn root_arg(meta: &ParseNestedMeta<'_>) -> syn::Result<Root> {
    let lit: LitStr = meta.value()?.parse()?;
    Root::from_lit(&lit)
}

/// Joins the doc comment lines of `attrs`, one leading space stripped per line.
pub(crate) fn docs(attrs: &[Attribute]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let Meta::NameValue(name_value) = &attr.meta {
            if let Expr::Lit(ExprLit {
                lit: Lit::Str(text),
                ..
            }) = &name_value.value
            {
                let value = text.value();
                for line in value.split('\n') {
                    let line = line.strip_prefix(' ').unwrap_or(line);
                    lines.push(line.trim_end().to_owned());
                }
            }
        }
    }
    lines.join("\n").trim().to_owned()
}

/// Removes helper attributes (`#[keel(..)]` and any names in `also`) from a whole item.
///
/// Used on the fallback path, so that after a diagnostic the original item is still emitted
/// without producing follow-up "cannot find attribute" errors.
pub(crate) struct StripHelpers<'a> {
    also: &'a [&'a str],
}

impl<'a> StripHelpers<'a> {
    /// Strips `keel` plus the attribute names in `also` (`error`, `from`, `source`).
    pub(crate) fn new(also: &'a [&'a str]) -> StripHelpers<'a> {
        StripHelpers { also }
    }

    fn clean(&self, attrs: &mut Vec<Attribute>) {
        attrs.retain(|attr| {
            let path = attr.path();
            !(path.is_ident("keel") || self.also.iter().any(|name| path.is_ident(name)))
        });
    }
}

impl VisitMut for StripHelpers<'_> {
    fn visit_item_struct_mut(&mut self, node: &mut syn::ItemStruct) {
        self.clean(&mut node.attrs);
        visit_mut::visit_item_struct_mut(self, node);
    }
    fn visit_item_enum_mut(&mut self, node: &mut syn::ItemEnum) {
        self.clean(&mut node.attrs);
        visit_mut::visit_item_enum_mut(self, node);
    }
    fn visit_item_impl_mut(&mut self, node: &mut syn::ItemImpl) {
        self.clean(&mut node.attrs);
        visit_mut::visit_item_impl_mut(self, node);
    }
    fn visit_item_fn_mut(&mut self, node: &mut syn::ItemFn) {
        self.clean(&mut node.attrs);
        visit_mut::visit_item_fn_mut(self, node);
    }
    fn visit_item_trait_mut(&mut self, node: &mut syn::ItemTrait) {
        self.clean(&mut node.attrs);
        visit_mut::visit_item_trait_mut(self, node);
    }
    fn visit_field_mut(&mut self, node: &mut syn::Field) {
        self.clean(&mut node.attrs);
        visit_mut::visit_field_mut(self, node);
    }
    fn visit_variant_mut(&mut self, node: &mut syn::Variant) {
        self.clean(&mut node.attrs);
        visit_mut::visit_variant_mut(self, node);
    }
    fn visit_impl_item_fn_mut(&mut self, node: &mut syn::ImplItemFn) {
        self.clean(&mut node.attrs);
        visit_mut::visit_impl_item_fn_mut(self, node);
    }
    fn visit_trait_item_fn_mut(&mut self, node: &mut syn::TraitItemFn) {
        self.clean(&mut node.attrs);
        visit_mut::visit_trait_item_fn_mut(self, node);
    }
    fn visit_pat_type_mut(&mut self, node: &mut syn::PatType) {
        self.clean(&mut node.attrs);
        visit_mut::visit_pat_type_mut(self, node);
    }
    fn visit_receiver_mut(&mut self, node: &mut syn::Receiver) {
        self.clean(&mut node.attrs);
        visit_mut::visit_receiver_mut(self, node);
    }
}

#[cfg(test)]
mod tests {
    use quote::{ToTokens, quote};
    use syn::parse_quote;

    use super::*;

    fn field_attrs(tokens: TokenStream) -> Vec<Attribute> {
        let field: syn::Field =
            syn::parse::Parser::parse2(syn::Field::parse_named, tokens).unwrap();
        field.attrs
    }

    #[test]
    fn keel_attributes_are_consumed() {
        let mut attrs = field_attrs(quote! {
            #[doc = " A note."]
            #[keel(default)]
            #[allow(dead_code)]
            pub note: String
        });
        let mut errors = Errors::new();
        let parsed = take(&mut attrs, Site::FIELD, &mut errors);
        assert!(errors.is_empty());
        assert!(parsed.default);
        assert_eq!(attrs.len(), 2);
        assert!(attrs.iter().all(|a| !is_keel_attr(a)));
    }

    #[test]
    fn several_options_in_one_attribute() {
        let mut attrs = field_attrs(quote! {
            #[keel(key = "id", no_coalesce)]
            rows: Signal<Vec<Row>>
        });
        let mut errors = Errors::new();
        let parsed = take(&mut attrs, Site::SIGNAL, &mut errors);
        assert!(errors.is_empty());
        assert_eq!(parsed.key.unwrap().value(), "id");
        assert!(parsed.no_coalesce);
        assert!(attrs.is_empty());
    }

    #[test]
    fn crate_override_parses_on_items() {
        let mut attrs: Vec<Attribute> = vec![parse_quote!(#[keel(crate = "::keel_runtime")])];
        let mut errors = Errors::new();
        let parsed = take(&mut attrs, Site::ITEM, &mut errors);
        assert!(errors.is_empty());
        let root = parsed.root.unwrap();
        assert_eq!(
            root.runtime().to_string().replace(' ', ""),
            "::keel_runtime::runtime"
        );
    }

    fn first_error(attrs: Vec<Attribute>, site: Site) -> String {
        let mut attrs = attrs;
        let mut errors = Errors::new();
        take(&mut attrs, site, &mut errors);
        errors
            .into_error()
            .expect("an error")
            .into_iter()
            .next()
            .unwrap()
            .to_string()
    }

    #[test]
    fn unknown_option_is_e0008() {
        let message = first_error(vec![parse_quote!(#[keel(bogus)])], Site::FIELD);
        assert!(message.starts_with("error[keel::E0008]: unknown option `bogus`"));
        assert!(message.contains("= docs: https://keel.dev/errors/E0008"));
    }

    #[test]
    fn misplaced_option_is_e0008() {
        let message = first_error(vec![parse_quote!(#[keel(default)])], Site::SIGNAL);
        assert!(message.contains("`#[keel(default)]` is not valid on a store signal field"));
        let message = first_error(vec![parse_quote!(#[keel(key = "id")])], Site::FIELD);
        assert!(message.contains("`#[keel(key)]` is not valid on a record or variant field"));
        let message = first_error(vec![parse_quote!(#[keel(crate = "::k")])], Site::FIELD);
        assert!(message.contains("`#[keel(crate)]` is not valid"));
        let message = first_error(vec![parse_quote!(#[keel(no_coalesce)])], Site::NOTHING);
        assert!(message.contains("`#[keel(no_coalesce)]` is not valid on this position"));
    }

    #[test]
    fn cfg_on_schema_members_is_e0008() {
        let message = first_error(vec![parse_quote!(#[cfg(test)])], Site::FIELD);
        assert!(
            message.starts_with(
                "error[keel::E0008]: `#[cfg]` on a record or variant field is not supported"
            ),
            "{message}"
        );
        let message = first_error(
            vec![parse_quote!(#[cfg_attr(test, allow(dead_code))])],
            Site::NOTHING,
        );
        assert!(message.contains("`#[cfg_attr]`"), "{message}");
        // Private methods and items are not part of the schema.
        let mut attrs: Vec<Attribute> = vec![parse_quote!(#[cfg(test)])];
        let mut errors = Errors::new();
        take(&mut attrs, Site::PRIVATE, &mut errors);
        take(&mut attrs, Site::ITEM, &mut errors);
        assert!(errors.is_empty());
        assert_eq!(attrs.len(), 1, "cfg is kept where it is allowed");
    }

    #[test]
    fn bare_keel_attribute_is_e0008() {
        let message = first_error(vec![parse_quote!(#[keel])], Site::FIELD);
        assert!(message.contains("`#[keel]` needs arguments"));
    }

    #[test]
    fn bad_crate_path_is_e0008() {
        let message = first_error(vec![parse_quote!(#[keel(crate = "1 2")])], Site::ITEM);
        assert!(message.contains("error[keel::E0008]"));
    }

    #[test]
    fn docs_are_joined_and_trimmed() {
        let attrs: Vec<Attribute> = vec![
            parse_quote!(#[doc = " First line."]),
            parse_quote!(#[doc = " Second line.  "]),
            parse_quote!(#[doc = ""]),
            parse_quote!(#[doc = "  indented"]),
            parse_quote!(#[allow(dead_code)]),
        ];
        assert_eq!(docs(&attrs), "First line.\nSecond line.\n\n indented");
        assert_eq!(docs(&[]), "");
    }

    #[test]
    fn parse_args_reports_unknown_arguments() {
        let mut seen = Vec::new();
        let result = parse_args(
            quote!(store, crate = "::k", nope),
            "api",
            "store, crate",
            |meta| {
                if meta.path.is_ident("store") {
                    seen.push("store");
                    return Ok(true);
                }
                if meta.path.is_ident("crate") {
                    root_arg(meta)?;
                    seen.push("crate");
                    return Ok(true);
                }
                Ok(false)
            },
        );
        assert_eq!(seen, ["store", "crate"]);
        let message = result.unwrap_err().to_string();
        assert!(message.contains("unknown argument `nope` for `#[keel::api]`"));
        assert!(message.contains("accepts: store, crate"));
    }

    #[test]
    fn empty_args_are_fine() {
        assert!(parse_args(TokenStream::new(), "api", "", |_| Ok(false)).is_ok());
    }

    #[test]
    fn strip_helpers_removes_nested_attributes() {
        let mut item: syn::Item = parse_quote! {
            #[keel(crate = "::k")]
            #[derive(Clone)]
            enum E {
                #[error("x")]
                A(#[from] #[keel(default)] u8),
                B { #[source] inner: String },
            }
        };
        StripHelpers::new(&["error", "from", "source"]).visit_item_mut(&mut item);
        let rendered = item.to_token_stream().to_string();
        assert!(!rendered.contains("keel"));
        assert!(!rendered.contains("error"));
        assert!(!rendered.contains("from"));
        assert!(!rendered.contains("source"));
        assert!(rendered.contains("derive"));
    }
}
