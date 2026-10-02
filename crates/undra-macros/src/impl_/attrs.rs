//! Attribute handling: `#[undra(..)]` helper attributes, macro arguments and doc comments.
//!
//! Helper attributes are *consumed*: they are removed from the emitted item so that `rustc`
//! never sees an unknown `undra` attribute. Anything unrecognised or misplaced is E0008;
//! nothing is silently ignored.

use proc_macro2::TokenStream;
use syn::meta::ParseNestedMeta;
use syn::visit_mut::{self, VisitMut};
use syn::{Attribute, Expr, ExprLit, Lit, LitStr, Meta};

use super::diag::{Diag, Errors, code};
use super::paths::Root;

/// What the `#[undra(..)]` attributes on one node said.
#[derive(Default)]
pub(crate) struct UndraAttr {
    /// `#[undra(crate = "path")]`.
    pub(crate) root: Option<Root>,
    /// `#[undra(default)]`.
    pub(crate) default: bool,
    /// `#[undra(key = "field")]`, with the literal for error spans.
    pub(crate) key: Option<LitStr>,
    /// `#[undra(no_coalesce)]`.
    pub(crate) no_coalesce: bool,
    /// `#[undra(coalesce)]` on a fire-and-forget method of a callback interface (ADR-041).
    pub(crate) coalesce: bool,
}

/// Which `#[undra(..)]` options are legal on a node.
#[derive(Clone, Copy)]
pub(crate) struct Site {
    /// Used in messages: "a record field", "an item".
    pub(crate) name: &'static str,
    pub(crate) root: bool,
    pub(crate) default: bool,
    pub(crate) key: bool,
    pub(crate) no_coalesce: bool,
    pub(crate) coalesce: bool,
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
        coalesce: false,
        schema: false,
    };
    /// A field of a record or of an enum variant.
    pub(crate) const FIELD: Site = Site {
        name: "a record or variant field",
        root: false,
        default: true,
        key: false,
        no_coalesce: false,
        coalesce: false,
        schema: true,
    };
    /// A signal field of a store.
    pub(crate) const SIGNAL: Site = Site {
        name: "a store signal field",
        root: false,
        default: true,
        key: true,
        no_coalesce: true,
        coalesce: false,
        schema: true,
    };
    /// A non-signal field of a store.
    pub(crate) const STATE_FIELD: Site = Site {
        name: "a non-signal store field",
        root: false,
        default: false,
        key: false,
        no_coalesce: false,
        coalesce: false,
        schema: true,
    };
    /// A node that takes no `#[undra(..)]` options at all (variants, methods, parameters).
    pub(crate) const NOTHING: Site = Site {
        name: "this position",
        root: false,
        default: false,
        key: false,
        no_coalesce: false,
        coalesce: false,
        schema: true,
    };
    /// A method of a callback interface (`#[undra::callback]`): `#[undra(coalesce)]` is legal.
    pub(crate) const CALLBACK_METHOD: Site = Site {
        name: "a callback interface method",
        root: false,
        default: false,
        key: false,
        no_coalesce: false,
        coalesce: true,
        schema: true,
    };
    /// A private method of an API impl block: not part of the schema, so anything goes except
    /// `#[undra(..)]` options.
    pub(crate) const PRIVATE: Site = Site {
        name: "a private method",
        root: false,
        default: false,
        key: false,
        no_coalesce: false,
        coalesce: false,
        schema: false,
    };
}

/// Whether `attr` is a `#[undra(..)]` helper attribute.
pub(crate) fn is_undra_attr(attr: &Attribute) -> bool {
    attr.path().is_ident("undra")
}

/// Removes every `#[undra(..)]` attribute from `attrs` and parses them for `site`.
pub(crate) fn take(attrs: &mut Vec<Attribute>, site: Site, errors: &mut Errors) -> UndraAttr {
    let mut out = UndraAttr::default();
    let mut kept = Vec::with_capacity(attrs.len());
    if site.schema {
        reject_cfg(attrs, site, errors);
    }
    for attr in attrs.drain(..) {
        if !is_undra_attr(&attr) {
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
        if path.is_ident("cfg_attr") && is_schema_neutral_cfg_attr(attr) {
            continue;
        }
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

/// `#[cfg_attr(docsrs, doc(cfg(feature = "x")))]` and friends: a conditional attribute whose
/// every expansion leaves the schema alone (documentation flags, lint levels, inlining hints).
///
/// `doc = "text"` is not neutral: the text is part of the schema, so it may not depend on the
/// build. The list form (`doc(hidden)`, `doc(cfg(..))`, `doc(alias = "..")`) is.
fn is_schema_neutral_cfg_attr(attr: &Attribute) -> bool {
    let Ok(args) =
        attr.parse_args_with(syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated)
    else {
        return false;
    };
    // The first element is the condition; the rest are the attributes it switches on.
    let mut attributes = args.iter().skip(1).peekable();
    if attributes.peek().is_none() {
        return false;
    }
    attributes.all(|meta| match meta {
        Meta::List(list) if list.path.is_ident("doc") => true,
        Meta::List(list) => ["allow", "warn", "deny", "forbid", "expect"]
            .iter()
            .any(|name| list.path.is_ident(name)),
        Meta::Path(path) => ["must_use", "inline", "cold", "track_caller"]
            .iter()
            .any(|name| path.is_ident(name)),
        Meta::NameValue(_) => false,
    })
}

/// The candidate closest to `name` (a typo of at most two edits, or one that only differs in
/// case or underscores), for a "did you mean" help.
pub(crate) fn closest<'a>(name: &str, candidates: &[&'a str]) -> Option<&'a str> {
    let squash = |s: &str| {
        s.chars()
            .filter(|c| *c != '_')
            .collect::<String>()
            .to_lowercase()
    };
    let wanted = squash(name);
    candidates
        .iter()
        .copied()
        .filter(|candidate| *candidate != name)
        .map(|candidate| {
            let distance = if squash(candidate) == wanted {
                0
            } else {
                edit_distance(&wanted, &squash(candidate))
            };
            (distance, candidate)
        })
        .filter(|(distance, candidate)| *distance <= if candidate.len() <= 4 { 1 } else { 2 })
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, candidate)| candidate)
}

/// Levenshtein distance, for short option names.
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = if ca == *cb {
                diagonal
            } else {
                1 + diagonal.min(above).min(row[j])
            };
            diagonal = above;
        }
    }
    row[b.len()]
}

/// The option names a description like "`crate = \"path\"`, and `store` on an impl block"
/// mentions: the leading identifier of every backticked part.
fn option_names(expected: &str) -> Vec<&str> {
    expected
        .split('`')
        .skip(1)
        .step_by(2)
        .filter_map(|part| {
            let end = part
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(part.len());
            (end > 0).then(|| &part[..end])
        })
        .collect()
}

/// The help of an unknown option or argument: the nearest name if there is one, else the list.
fn unknown_help(name: &str, candidates: &[&str]) -> String {
    match closest(name, candidates) {
        Some(near) => format!("did you mean `{near}`? Otherwise remove `{name}`"),
        None if candidates.is_empty() => format!("remove `{name}`"),
        None => format!(
            "remove `{name}`, or use one of: {}",
            candidates
                .iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn parse_one(attr: &Attribute, site: Site, out: &mut UndraAttr) -> syn::Result<()> {
    if !matches!(attr.meta, Meta::List(_)) {
        return Err(Diag::new(
            code::E0008,
            "`#[undra]` needs arguments",
            "the `undra` helper attribute configures a field or an item: `#[undra(default)]`, `#[undra(key = \"id\")]`, `#[undra(no_coalesce)]`, `#[undra(crate = \"path\")]`",
            "write the option you meant inside parentheses, for example `#[undra(default)]` on a record field or `#[undra(key = \"id\")]` on a `Signal<Vec<T>>`",
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
                format!("`#[undra({option})]` is not valid on {}", site.name),
                option_hint(option),
                option_home(option),
            )
            .on(&meta.path)
        };
        match name.as_str() {
            "crate" => {
                if !site.root {
                    return Err(misplaced("crate"));
                }
                out.root = Some(root_arg(&meta)?);
                Ok(())
            }
            "default" => {
                if !site.default {
                    return Err(misplaced("default"));
                }
                flag(&meta, code::E0008, "default")?;
                out.default = true;
                Ok(())
            }
            "key" => {
                if !site.key {
                    return Err(misplaced("key"));
                }
                let lit: LitStr = option_value(
                    &meta,
                    code::E0008,
                    "key",
                    "a string literal naming a field",
                    "key = \"id\"",
                )?;
                out.key = Some(lit);
                Ok(())
            }
            "no_coalesce" => {
                if !site.no_coalesce {
                    return Err(misplaced("no_coalesce"));
                }
                flag(&meta, code::E0008, "no_coalesce")?;
                out.no_coalesce = true;
                Ok(())
            }
            "coalesce" => {
                if !site.coalesce {
                    return Err(misplaced("coalesce"));
                }
                flag(&meta, code::E0008, "coalesce")?;
                out.coalesce = true;
                Ok(())
            }
            other => Err(Diag::new(
                code::E0008,
                format!("unknown option `{other}` in `#[undra(..)]`"),
                "the options are `crate = \"path\"` (items), `default` (record fields), `key = \"field\"` and `no_coalesce` (store signal fields), and `coalesce` (fire-and-forget methods of a callback interface)",
                unknown_help(other, &["crate", "default", "key", "no_coalesce", "coalesce"]),
            )
            .on(&meta.path)),
        }
    })
}

/// Where an option belongs: the fix for one that sits somewhere else.
fn option_home(option: &str) -> &'static str {
    match option {
        "crate" => {
            "move it to the item: `#[undra(crate = \"path\")]` on a struct, enum, trait or impl block, or `crate = \"path\"` in the macro's arguments"
        }
        "default" => {
            "move it to a field of a `#[undra::api]` record or enum variant, or to a `Signal<T>` field of a `#[undra::store]` struct, or remove it"
        }
        "key" => "move it to a `Signal<Vec<T>>` field of a `#[undra::store]` struct, or remove it",
        "no_coalesce" => "move it to a signal field of a `#[undra::store]` struct, or remove it",
        "coalesce" => "move it to a fire-and-forget method of a `#[undra::callback]` trait, or remove it",
        _ => "remove the option, or move it to where it applies",
    }
}

fn option_hint(option: &str) -> &'static str {
    match option {
        "crate" => {
            "`crate = \"path\"` is an item-level option: it names the crate that generated code refers to"
        }
        "default" => {
            "`default` marks a field of a `#[undra::api]` record or enum variant as having a default in generated constructors and in migrations, and a store's `Signal<T>` as restored with `T::default()` when a snapshot lacks it"
        }
        "key" => {
            "`key = \"field\"` turns a store's `Signal<Vec<T>>` into a keyed list that ships patches"
        }
        "no_coalesce" => {
            "`no_coalesce` makes a store signal deliver every commit instead of coalescing them"
        }
        "coalesce" => {
            "`coalesce` makes the host deliver only the newest pending call of a fire-and-forget callback method to each instance (progress reporting)"
        }
        _ => "this option is not valid here",
    }
}

/// Parses macro arguments (`#[undra::api(crate = "..", store)]`).
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
            format!("unknown argument `{shown}` for `#[undra::{macro_name}]`"),
            format!("`#[undra::{macro_name}]` accepts: {expected}"),
            unknown_help(&shown, &option_names(expected)),
        )
        .on(&meta.path))
    });
    syn::parse::Parser::parse2(parser, attr)
}

/// Reads the value of `option = value`: a diagnostic with `code` (not `syn`'s "expected `=`" or
/// "expected string literal") when the value is missing or has the wrong kind.
///
/// `expects` says what the value is ("a string literal"), `example` shows a correct use.
pub(crate) fn option_value<T: syn::parse::Parse>(
    meta: &ParseNestedMeta<'_>,
    code: &'static str,
    option: &str,
    expects: &str,
    example: &str,
) -> syn::Result<T> {
    if !meta.input.peek(syn::Token![=]) {
        return Err(Diag::new(
            code,
            format!("`{option}` needs a value"),
            format!("`{option}` takes {expects}"),
            format!("write `{example}`"),
        )
        .on(&meta.path));
    }
    let value = meta.value()?;
    value.parse::<T>().map_err(|error| {
        Diag::new(
            code,
            format!("`{option}` must be {expects}"),
            format!("`{option}` takes {expects}; anything else cannot be read at compile time"),
            format!("write `{example}`"),
        )
        .at(error.span())
    })
}

/// A flag option (`default`, `persist`): a diagnostic with `code` if a value follows.
pub(crate) fn flag(
    meta: &ParseNestedMeta<'_>,
    code: &'static str,
    option: &str,
) -> syn::Result<()> {
    if meta.input.peek(syn::Token![=]) {
        return Err(Diag::new(
            code,
            format!("`{option}` takes no value"),
            format!(
                "`{option}` switches a behaviour on by being present; there is nothing to set it to"
            ),
            format!("write `{option}` alone"),
        )
        .on(&meta.path));
    }
    Ok(())
}

/// Reads a `crate = "path"` argument.
pub(crate) fn root_arg(meta: &ParseNestedMeta<'_>) -> syn::Result<Root> {
    let lit: LitStr = option_value(
        meta,
        code::E0008,
        "crate",
        "a string literal naming a path",
        "crate = \"::undra\"",
    )?;
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

/// Whether `path` names one of the Undra attribute macros (`undra::query`, `undra_macros::api`).
///
/// An attribute macro on a nested item (a method of an `#[undra::api] impl`) is expanded after
/// the outer macro; once the outer macro has reported it, the fallback drops it so it is not
/// reported twice.
pub(crate) fn is_undra_macro_path(path: &syn::Path) -> bool {
    path.segments.len() >= 2
        && path
            .segments
            .first()
            .is_some_and(|seg| seg.ident == "undra" || seg.ident == "undra_macros")
}

/// Removes helper attributes (`#[undra(..)]` and any names in `also`) from a whole item.
///
/// Used on the fallback path, so that after a diagnostic the original item is still emitted
/// without producing follow-up "cannot find attribute" errors.
pub(crate) struct StripHelpers<'a> {
    also: &'a [&'a str],
}

impl<'a> StripHelpers<'a> {
    /// Strips `undra` plus the attribute names in `also` (`error`, `from`, `source`).
    pub(crate) fn new(also: &'a [&'a str]) -> StripHelpers<'a> {
        StripHelpers { also }
    }

    fn clean(&self, attrs: &mut Vec<Attribute>) {
        attrs.retain(|attr| {
            let path = attr.path();
            !(path.is_ident("undra")
                || is_undra_macro_path(path)
                || self.also.iter().any(|name| path.is_ident(name)))
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
    fn undra_attributes_are_consumed() {
        let mut attrs = field_attrs(quote! {
            #[doc = " A note."]
            #[undra(default)]
            #[allow(dead_code)]
            pub note: String
        });
        let mut errors = Errors::new();
        let parsed = take(&mut attrs, Site::FIELD, &mut errors);
        assert!(errors.is_empty());
        assert!(parsed.default);
        assert_eq!(attrs.len(), 2);
        assert!(attrs.iter().all(|a| !is_undra_attr(a)));
    }

    #[test]
    fn several_options_in_one_attribute() {
        let mut attrs = field_attrs(quote! {
            #[undra(key = "id", no_coalesce)]
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
        let mut attrs: Vec<Attribute> = vec![parse_quote!(#[undra(crate = "::undra_runtime")])];
        let mut errors = Errors::new();
        let parsed = take(&mut attrs, Site::ITEM, &mut errors);
        assert!(errors.is_empty());
        let root = parsed.root.unwrap();
        assert_eq!(
            root.runtime().to_string().replace(' ', ""),
            "::undra_runtime::runtime"
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
        let message = first_error(vec![parse_quote!(#[undra(bogus)])], Site::FIELD);
        assert!(message.starts_with("error[undra::E0008]: unknown option `bogus`"));
        assert!(
            message.contains("= docs: https://shreypdev.github.io/undra/docs/errors.html#E0008")
        );
    }

    #[test]
    fn misplaced_option_is_e0008() {
        // ADR-037: `default` is valid on a store signal; on a non-signal field it is not.
        let message = first_error(vec![parse_quote!(#[undra(default)])], Site::STATE_FIELD);
        assert!(message.contains("`#[undra(default)]` is not valid on a non-signal store field"));
        let mut attrs = vec![parse_quote!(#[undra(default)])];
        let mut errors = Errors::new();
        assert!(take(&mut attrs, Site::SIGNAL, &mut errors).default);
        assert!(errors.is_empty());
        let message = first_error(vec![parse_quote!(#[undra(key = "id")])], Site::FIELD);
        assert!(message.contains("`#[undra(key)]` is not valid on a record or variant field"));
        let message = first_error(vec![parse_quote!(#[undra(crate = "::k")])], Site::FIELD);
        assert!(message.contains("`#[undra(crate)]` is not valid"));
        let message = first_error(vec![parse_quote!(#[undra(no_coalesce)])], Site::NOTHING);
        assert!(message.contains("`#[undra(no_coalesce)]` is not valid on this position"));
    }

    #[test]
    fn cfg_on_schema_members_is_e0008() {
        let message = first_error(vec![parse_quote!(#[cfg(test)])], Site::FIELD);
        assert!(
            message.starts_with(
                "error[undra::E0008]: `#[cfg]` on a record or variant field is not supported"
            ),
            "{message}"
        );
        let message = first_error(
            vec![parse_quote!(#[cfg_attr(test, derive(Clone))])],
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
    fn documentation_and_lint_cfg_attrs_are_schema_neutral() {
        let mut attrs: Vec<Attribute> = vec![
            parse_quote!(#[cfg_attr(docsrs, doc(cfg(feature = "x")))]),
            parse_quote!(#[cfg_attr(test, allow(dead_code), doc(hidden))]),
            parse_quote!(#[cfg_attr(feature = "hot", inline)]),
        ];
        let mut errors = Errors::new();
        take(&mut attrs, Site::FIELD, &mut errors);
        assert!(errors.is_empty(), "neutral cfg_attrs are accepted");
        assert_eq!(attrs.len(), 3, "and kept for rustc");
        // A `doc = \"..\"` text, a derive or an unknown attribute may change the schema.
        for attr in [
            parse_quote!(#[cfg_attr(test, doc = "only in tests")]),
            parse_quote!(#[cfg_attr(test, derive(Debug))]),
            parse_quote!(#[cfg_attr(test, serde(skip))]),
            parse_quote!(#[cfg_attr(test)]),
        ] {
            let message = first_error(vec![attr], Site::FIELD);
            assert!(message.contains("`#[cfg_attr]`"), "{message}");
        }
    }

    #[test]
    fn bare_undra_attribute_is_e0008() {
        let message = first_error(vec![parse_quote!(#[undra])], Site::FIELD);
        assert!(message.contains("`#[undra]` needs arguments"));
    }

    #[test]
    fn bad_crate_path_is_e0008() {
        let message = first_error(vec![parse_quote!(#[undra(crate = "1 2")])], Site::ITEM);
        assert!(message.contains("error[undra::E0008]"));
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
        assert!(message.contains("unknown argument `nope` for `#[undra::api]`"));
        assert!(message.contains("accepts: store, crate"));
    }

    #[test]
    fn empty_args_are_fine() {
        assert!(parse_args(TokenStream::new(), "api", "", |_| Ok(false)).is_ok());
    }

    #[test]
    fn strip_helpers_removes_nested_attributes() {
        let mut item: syn::Item = parse_quote! {
            #[undra(crate = "::k")]
            #[derive(Clone)]
            enum E {
                #[error("x")]
                A(#[from] #[undra(default)] u8),
                B { #[source] inner: String },
            }
        };
        StripHelpers::new(&["error", "from", "source"]).visit_item_mut(&mut item);
        let rendered = item.to_token_stream().to_string();
        assert!(!rendered.contains("undra"));
        assert!(!rendered.contains("error"));
        assert!(!rendered.contains("from"));
        assert!(!rendered.contains("source"));
        assert!(rendered.contains("derive"));
    }

    #[test]
    fn the_nearest_name_is_a_typo_or_a_spelling_variant() {
        let options = ["crate", "default", "key", "no_coalesce"];
        assert_eq!(closest("defualt", &options), Some("default"));
        assert_eq!(closest("Default", &options), Some("default"));
        assert_eq!(closest("nocoalesce", &options), Some("no_coalesce"));
        assert_eq!(closest("kee", &options), Some("key"));
        assert_eq!(closest("crates", &options), Some("crate"));
        // Too far, or the name itself: no suggestion.
        assert_eq!(closest("frobnicate", &options), None);
        assert_eq!(closest("key", &options), None);
        assert_eq!(closest("ke", &["key", "crate"]), Some("key"));
        assert_eq!(
            closest("kx", &["key"]),
            None,
            "two edits is too far for a short name"
        );
        assert_eq!(closest("xyz", &["key"]), None, "short names allow one edit");
    }

    #[test]
    fn option_names_are_read_from_the_description() {
        assert_eq!(
            option_names("`crate = \"path\"`, and `store` on an impl block"),
            ["crate", "store"]
        );
        assert_eq!(
            option_names("`key = \"..\"`, `stale = \"30s\"`, `persist`, `retry = N`"),
            ["key", "stale", "persist", "retry"]
        );
        assert!(option_names("nothing on an `impl Trait for Type` block").len() == 1);
    }
}
