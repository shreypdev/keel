//! Generic data types and their named instantiations (ADR-042 decision 2).
//!
//! The schema describes concrete types, so a generic type has no wire identity of its own. What it
//! has is a **template**, and every use of it is named:
//!
//! ```ignore
//! #[undra::api(generic)]
//! pub struct Page<T> { pub items: Vec<T>, pub next: Option<String> }
//!
//! #[undra::api]
//! pub type TodoPage = Page<Todo>;      // the schema's `TodoPage`, a record like any other
//! ```
//!
//! # The template
//!
//! `#[undra::api(generic)]` on a struct or an enum with type parameters expands to the item, the
//! generic codecs (`impl<T: Encode> Encode for Page<T>`, the wire of every instantiation is the
//! same field walk) and a hidden `#[macro_export] macro_rules! __undra_template_Page` that carries
//! the definition with each type parameter replaced by a metavariable. The macro is re-exported
//! under the type's own name (`pub use __undra_template_Page as Page;`): macros live in their own
//! namespace, so `Page!` and `Page<T>` do not collide, and a `use model::Page;` imports both, which
//! is how an alias in another module finds the template.
//!
//! # The instantiation
//!
//! `#[undra::api] pub type TodoPage = Page<Todo>;` emits the alias and `Page! { TodoPage, "docs",
//! Todo }`. The template macro passes its definition, with `Todo` where the parameter was, to the
//! hidden `undra::__instantiate!`, which runs the ordinary record or enum expansion
//! ([`Expand::Instance`]) under the alias's name: the registration of `TodoPage`, the inherent
//! `impl TodoPage { const UNDRA_TYPE_ID .. }` that lets a signature spell the alias (E0061), and
//! the identity checks of the type arguments. The template's own types were checked once, where
//! they resolve (a `macro_rules!` resolves names where it is *invoked*, so re-checking them at
//! the alias could name types the alias's module has not imported).
//!
//! The inherent impl is also what E0070 rides on: it carries a constant named after the rule, so a
//! second alias of one instantiation, whose impl defines it again, is reported by `rustc` as that
//! constant defined twice. An alias in another crate than the template's is reported by
//! [`expand_instantiate`] itself.

use std::collections::{BTreeSet, HashMap};

use proc_macro2::{TokenStream, TokenTree};
use quote::{ToTokens, format_ident, quote};
use syn::visit_mut::{self, VisitMut};

use super::attrs::{Site, docs, take};
use super::common::{GenericOn, check_generics_on};
use super::diag::{Diag, Errors, code};
use super::paths::Root;
use super::record::{Expand, FieldModel, Instance, Mode, expand_enum_as, expand_struct_as};
use super::types::{is_std_type_name, ty_string};

/// The placeholder that stands for the alias's name in a template's definition: the macro replaces
/// it by `$__alias`.
const ALIAS: &str = "__UNDRA_ALIAS";

/// The placeholder of the type parameter `T`: the macro replaces it by `$T`.
fn param_placeholder(param: &str) -> String {
    format!("__UNDRA_PARAM_{param}")
}

/// The name of the hidden macro of the template `name`.
pub(crate) fn template_macro_name(name: &str) -> syn::Ident {
    format_ident!("__undra_template_{}", name)
}

// ---------------------------------------------------------------------------------------------
// The template
// ---------------------------------------------------------------------------------------------

/// The generics of `#[undra::api(generic)]`: type parameters only, and at least one.
pub(crate) fn check_template_generics(generics: &syn::Generics, item: &str, errors: &mut Errors) {
    check_generics_on(generics, item, GenericOn::Template, errors);
    if generics.type_params().next().is_none() && errors.is_empty() {
        errors.push(
            Diag::new(
                code::E0008,
                format!("`generic` on `{item}`, which has no type parameters"),
                "`#[undra::api(generic)]` marks a struct or an enum as the template of named instantiations, and a template needs a type parameter to be instantiated with",
                "remove `generic`, or add the type parameter: `struct Page<T> { .. }`",
            )
            .at(proc_macro2::Span::call_site()),
        );
    }
}

/// Refuses what a template cannot say about its type parameters: an associated type of one
/// (`T::Item`) has no name the schema could record, because each instantiation has its own.
pub(crate) fn check_parameter_use<'a>(
    fields: impl IntoIterator<Item = &'a FieldModel>,
    params: &[String],
    errors: &mut Errors,
) {
    struct Assoc<'p> {
        params: &'p [String],
        found: Vec<syn::Type>,
    }
    impl VisitMut for Assoc<'_> {
        fn visit_type_path_mut(&mut self, node: &mut syn::TypePath) {
            if node.qself.is_none()
                && node.path.segments.len() > 1
                && self.params.iter().any(|p| node.path.segments[0].ident == p)
            {
                self.found.push(syn::Type::Path(node.clone()));
            }
            visit_mut::visit_type_path_mut(self, node);
        }
    }
    for field in fields {
        let mut assoc = Assoc {
            params,
            found: Vec::new(),
        };
        assoc.visit_type_mut(&mut field.ty.clone());
        for ty in assoc.found {
            errors.push(
                Diag::new(
                    code::E0001,
                    format!(
                        "`{}` cannot cross the boundary: it is an associated type of a type parameter",
                        ty_string(&ty)
                    ),
                    "each instantiation has its own associated type, and the schema records one type per field",
                    "take the type as a parameter of its own: `struct Page<T, C>` and `pub type TodoPage = Page<Todo, String>;`",
                )
                .on(&ty),
            );
        }
    }
}

/// Replaces each type parameter in a type position by the placeholder of its metavariable. Only
/// type positions are touched: a field, a variant or a path of another namespace that happens to
/// be called `T` is not a use of the parameter.
struct Substitute<'a> {
    params: &'a BTreeSet<String>,
}

impl VisitMut for Substitute<'_> {
    fn visit_type_mut(&mut self, ty: &mut syn::Type) {
        if let syn::Type::Path(path) = ty {
            let simple = path.qself.is_none()
                && path.path.leading_colon.is_none()
                && path.path.segments.len() == 1
                && path.path.segments[0].arguments.is_none();
            if simple {
                let ident = path.path.segments[0].ident.to_string();
                if self.params.contains(&ident) {
                    path.path.segments[0].ident = format_ident!("{}", param_placeholder(&ident));
                    return;
                }
            }
        }
        visit_mut::visit_type_mut(self, ty);
    }
}

/// Keeps the attributes of a field or variant that an instantiation reads: documentation and the
/// `#[undra(..)]` helpers (`default`).
struct KeepReadable;

impl VisitMut for KeepReadable {
    fn visit_field_mut(&mut self, field: &mut syn::Field) {
        field
            .attrs
            .retain(|a| a.path().is_ident("doc") || a.path().is_ident("undra"));
        visit_mut::visit_field_mut(self, field);
    }

    fn visit_variant_mut(&mut self, variant: &mut syn::Variant) {
        variant
            .attrs
            .retain(|a| a.path().is_ident("doc") || a.path().is_ident("undra"));
        visit_mut::visit_variant_mut(self, variant);
    }
}

/// `tokens` with every identifier of `replacements` replaced by its tokens.
fn replace_idents(tokens: TokenStream, replacements: &HashMap<String, TokenStream>) -> TokenStream {
    tokens
        .into_iter()
        .flat_map(|tree| match tree {
            TokenTree::Ident(ident) => match replacements.get(&ident.to_string()) {
                Some(replacement) => replacement.clone().into_iter().collect::<Vec<_>>(),
                None => vec![TokenTree::Ident(ident)],
            },
            TokenTree::Group(group) => {
                let mut rebuilt = proc_macro2::Group::new(
                    group.delimiter(),
                    replace_idents(group.stream(), replacements),
                );
                rebuilt.set_span(group.span());
                vec![TokenTree::Group(rebuilt)]
            }
            other => vec![other],
        })
        .collect()
}

/// The hidden `macro_rules!` of a template, and its re-export under the type's name.
///
/// `item` is the definition as written (helpers kept); `docs` the template's documentation, which
/// an alias without its own inherits.
pub(crate) fn template_macro(
    root: &Root,
    item: &syn::Item,
    params: &[String],
    template_docs: &str,
) -> TokenStream {
    let (name, mut definition) = match item {
        syn::Item::Struct(item) => (item.ident.clone(), syn::Item::Struct(item.clone())),
        syn::Item::Enum(item) => (item.ident.clone(), syn::Item::Enum(item.clone())),
        other => unreachable!(
            "a template is a struct or an enum, not {}",
            other.to_token_stream()
        ),
    };
    let name_str = name.to_string();
    let wanted: BTreeSet<String> = params.iter().cloned().collect();

    // The definition an instantiation reads: named as the alias, without generics, with the type
    // parameters in type positions replaced.
    match &mut definition {
        syn::Item::Struct(item) => {
            item.attrs.clear();
            item.ident = format_ident!("{}", ALIAS);
            item.generics = syn::Generics::default();
        }
        syn::Item::Enum(item) => {
            item.attrs.clear();
            item.ident = format_ident!("{}", ALIAS);
            item.generics = syn::Generics::default();
        }
        _ => {}
    }
    KeepReadable.visit_item_mut(&mut definition);
    Substitute { params: &wanted }.visit_item_mut(&mut definition);

    let mut replacements: HashMap<String, TokenStream> = HashMap::new();
    replacements.insert(ALIAS.to_owned(), quote!($__alias));
    for param in params {
        let metavariable = format_ident!("{}", param);
        replacements.insert(param_placeholder(param), quote!($#metavariable));
    }
    let definition = replace_idents(definition.to_token_stream(), &replacements);

    let matchers: Vec<TokenStream> = params
        .iter()
        .map(|param| {
            let metavariable = format_ident!("{}", param);
            quote!($#metavariable:ty)
        })
        .collect();
    let macro_name = template_macro_name(&name_str);
    let instantiate = root.instantiate();
    let root_path = root.path_string();
    let crate_name = std::env::var("CARGO_CRATE_NAME").unwrap_or_default();
    let arity = params.len();
    let arity_message = Diag::new(
        code::E0002,
        format!(
            "`{name_str}` takes {arity} type argument{} ({}), as declared with `#[undra::api(generic)]`",
            if arity == 1 { "" } else { "s" },
            params
                .iter()
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        "an instantiation names every type parameter of the template: the alias is what the schema and the platforms see",
        format!(
            "write all the arguments: `#[undra::api] pub type TodoPage = {name_str}<{}>;`",
            params
                .iter()
                .map(|_| "Todo")
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
    .message();

    quote! {
        #[doc(hidden)]
        #[macro_export]
        macro_rules! #macro_name {
            ( $__alias:ident, $__docs:literal, #(#matchers),* ) => {
                #instantiate! {
                    #[undra_instance(
                        template = #name_str,
                        crate_name = #crate_name,
                        root = #root_path,
                        docs = #template_docs,
                        alias_docs = $__docs
                    )]
                    #definition
                }
            };
            ( $($__rest:tt)* ) => {
                ::core::compile_error!(#arity_message);
            };
        }

        #[doc(hidden)]
        #[allow(unused_imports, unreachable_pub)]
        pub use #macro_name as #name;
    }
}

/// The constant of an instantiation's inherent impl that names E0070: the impl of a second alias
/// of the same instantiation defines it again, and `rustc` reports that by its name.
pub(crate) fn duplicate_alias_constant(template: &str) -> TokenStream {
    let name = format_ident!(
        "_undra_error_{}_this_instantiation_of_{}_is_declared_twice_keep_one_alias_per_instantiation",
        code::E0070,
        template
    );
    quote! {
        #[doc(hidden)]
        #[allow(non_upper_case_globals)]
        pub const #name: () = ();
    }
}

// ---------------------------------------------------------------------------------------------
// The alias
// ---------------------------------------------------------------------------------------------

/// `#[undra::api] pub type TodoPage = Page<Todo>;`: the alias, and the invocation of the template.
pub(crate) fn expand_alias(
    root: Option<Root>,
    mut item: syn::ItemType,
) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    take(&mut item.attrs, Site::NOTHING, &mut errors);
    if root.is_some() {
        errors.push(
            Diag::new(
                code::E0008,
                "`crate = \"..\"` is not accepted on an alias",
                "an instantiation uses the crate path its template was declared with",
                "remove it from the alias; write `#[undra::api(generic, crate = \"..\")]` on the template if the path needs to change",
            )
            .on(&item.ident),
        );
    }
    let name = item.ident.to_string();
    if !item.generics.params.is_empty() {
        errors.push(
            Diag::new(
                code::E0002,
                format!("generic parameter on the alias `{name}`"),
                "an alias that names an instantiation is concrete: it is the name the schema and the platforms use",
                format!("write `#[undra::api] pub type {name} = Page<Todo>;` with the arguments spelled out"),
            )
            .on(&item.generics),
        );
    }
    let (path, args) = match instantiation_of(&item.ty) {
        Ok(found) => found,
        Err(error) => {
            errors.push(not_an_instantiation(&item, &error));
            errors.finish()?;
            unreachable!("an error was pushed")
        }
    };
    errors.finish()?;

    let alias = &item.ident;
    let alias_docs = docs(&item.attrs);
    let mut macro_path = path;
    if let Some(last) = macro_path.segments.last_mut() {
        last.arguments = syn::PathArguments::None;
    }
    Ok(quote! {
        #item

        #macro_path! { #alias, #alias_docs, #(#args),* }
    })
}

/// Why a type alias is not the instantiation of a template.
enum NotAnInstantiation {
    /// The right-hand side is not a path with type arguments.
    Shape,
    /// It names a standard library generic (`Vec<u8>`), which is never a template.
    Std(String),
    /// An argument is a lifetime, a constant or a binding.
    Argument(syn::Error),
}

/// The path of the template and the type arguments of `ty` (`Page<Todo>`).
fn instantiation_of(ty: &syn::Type) -> Result<(syn::Path, Vec<syn::Type>), NotAnInstantiation> {
    let mut ty = ty;
    while let syn::Type::Paren(inner) = ty {
        ty = &inner.elem;
    }
    let syn::Type::Path(path) = ty else {
        return Err(NotAnInstantiation::Shape);
    };
    let Some(last) = path.path.segments.last() else {
        return Err(NotAnInstantiation::Shape);
    };
    let syn::PathArguments::AngleBracketed(arguments) = &last.arguments else {
        return Err(NotAnInstantiation::Shape);
    };
    if path.qself.is_some() || arguments.args.is_empty() {
        return Err(NotAnInstantiation::Shape);
    }
    let name = last.ident.to_string();
    if is_std_type_name(&name) {
        return Err(NotAnInstantiation::Std(name));
    }
    let mut types = Vec::new();
    for argument in &arguments.args {
        match argument {
            syn::GenericArgument::Type(ty) => types.push(ty.clone()),
            syn::GenericArgument::Lifetime(lifetime) => {
                return Err(NotAnInstantiation::Argument(
                    Diag::new(
                        code::E0003,
                        format!("lifetime `{lifetime}` in `{}`", ty_string(ty)),
                        "everything crosses the boundary by value; a borrow cannot outlive the call that made it",
                        "use an owned type",
                    )
                    .on(lifetime),
                ))
            }
            other => {
                return Err(NotAnInstantiation::Argument(
                    Diag::new(
                        code::E0002,
                        format!("`{}` is not a type argument", other.to_token_stream()),
                        "only type parameters are supported: a template is instantiated with types",
                        "write a type for each parameter, `pub type TodoPage = Page<Todo>;`",
                    )
                    .on(other),
                ))
            }
        }
    }
    Ok((path.path.clone(), types))
}

fn not_an_instantiation(item: &syn::ItemType, why: &NotAnInstantiation) -> syn::Error {
    let name = &item.ident;
    let shown = ty_string(&item.ty);
    match why {
        NotAnInstantiation::Argument(error) => error.clone(),
        NotAnInstantiation::Std(std) => Diag::new(
            code::E0007,
            format!("`{shown}` is not a generic data type of yours: `#[undra::api]` on the alias `{name}` cannot instantiate `{std}`"),
            "`#[undra::api]` on a type alias instantiates a generic struct or enum declared with `#[undra::api(generic)]`; the standard types (`Vec`, `Option`, maps, ..) already cross the boundary, spelled as they are",
            format!("remove `#[undra::api]` from `{name}` and write `{shown}` where it is used, or declare your own template: `#[undra::api(generic)] struct Page<T> {{ .. }}`"),
        )
        .on(&item.ty),
        NotAnInstantiation::Shape => Diag::new(
            code::E0007,
            format!("`#[undra::api]` on the type alias `{name}`, which does not name an instantiation"),
            "a type alias is not a schema type of its own; the only alias that is names an instantiation of a generic data type declared with `#[undra::api(generic)]`, so it can be given a name the platforms generate a type for",
            format!("write `#[undra::api] pub type {name} = Page<Todo>;` for a template `Page`, or remove `#[undra::api]` and write the aliased type where it is used"),
        )
        .on(&item.ident),
    }
}

// ---------------------------------------------------------------------------------------------
// The instantiation
// ---------------------------------------------------------------------------------------------

/// What the template's macro tells `__instantiate!` beyond the definition.
#[derive(Default)]
struct Config {
    template: String,
    crate_name: String,
    root: Option<Root>,
    docs: String,
    alias_docs: String,
}

fn take_config(attrs: &mut Vec<syn::Attribute>) -> syn::Result<Config> {
    let Some(position) = attrs
        .iter()
        .position(|a| a.path().is_ident("undra_instance"))
    else {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "`undra::__instantiate!` is called by the macro of a `#[undra::api(generic)]` type, not written by hand",
        ));
    };
    let attr = attrs.remove(position);
    let mut config = Config::default();
    attr.parse_nested_meta(|meta| {
        let lit: syn::LitStr = meta.value()?.parse()?;
        let value = lit.value();
        match meta.path.get_ident().map(ToString::to_string).as_deref() {
            Some("template") => config.template = value,
            Some("crate_name") => config.crate_name = value,
            Some("root") => config.root = Some(Root::from_lit(&lit)?),
            Some("docs") => config.docs = value,
            Some("alias_docs") => config.alias_docs = value,
            _ => return Err(meta.error("unknown key")),
        }
        Ok(())
    })?;
    Ok(config)
}

/// `undra::__instantiate!`: the registration of one instantiation (see the module documentation).
pub(crate) fn expand_instantiate(input: TokenStream) -> TokenStream {
    let item: syn::Item = match syn::parse2(input) {
        Ok(item) => item,
        Err(error) => return error.to_compile_error(),
    };
    match instantiate(item) {
        Ok(tokens) => tokens,
        Err(error) => error.to_compile_error(),
    }
}

fn instantiate(mut item: syn::Item) -> syn::Result<TokenStream> {
    let attrs = match &mut item {
        syn::Item::Struct(item) => &mut item.attrs,
        syn::Item::Enum(item) => &mut item.attrs,
        other => {
            return Err(syn::Error::new_spanned(
                other,
                "`undra::__instantiate!` takes the definition of a struct or an enum",
            ));
        }
    };
    let config = take_config(attrs)?;
    let alias = match &item {
        syn::Item::Struct(item) => item.ident.clone(),
        syn::Item::Enum(item) => item.ident.clone(),
        _ => unreachable!("checked above"),
    };

    // The impl of an instantiation is only legal in the crate of its template.
    let here = std::env::var("CARGO_CRATE_NAME").unwrap_or_default();
    if !config.crate_name.is_empty() && !here.is_empty() && config.crate_name != here {
        return Err(foreign_alias(
            &alias,
            &config.template,
            &config.crate_name,
            &here,
        ));
    }

    let instance = Instance {
        docs: if config.alias_docs.is_empty() {
            config.docs
        } else {
            config.alias_docs
        },
        template: config.template,
    };
    match item {
        syn::Item::Struct(item) => expand_struct_as(config.root, item, Expand::Instance(instance)),
        syn::Item::Enum(item) => {
            expand_enum_as(config.root, item, Mode::Api, Expand::Instance(instance))
        }
        _ => unreachable!("checked above"),
    }
}

/// E0070 for an alias declared outside the crate of its template.
fn foreign_alias(alias: &syn::Ident, template: &str, declared_in: &str, here: &str) -> syn::Error {
    foreign_alias_diag(alias, template, declared_in, here).at(alias.span())
}

/// The diagnostic of [`foreign_alias`].
pub(crate) fn foreign_alias_diag(
    alias: &syn::Ident,
    template: &str,
    declared_in: &str,
    here: &str,
) -> Diag {
    Diag::new(
        code::E0070,
        format!(
            "`{alias}` instantiates `{template}`, which is declared in the crate `{declared_in}`, not in `{here}`"
        ),
        "an instantiation adds an inherent impl to the template's type, which only the crate that declares the template may do, and a second crate naming the same instantiation would register it twice",
        format!(
            "declare the alias next to the template, in `{declared_in}`, and use it from here; or declare a concrete `#[undra::api]` struct or enum in this crate"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::has;

    fn expand_template(src: &str) -> String {
        let item: syn::Item = syn::parse_str(src).unwrap();
        let result = match item {
            syn::Item::Struct(item) => {
                super::super::record::expand_struct_as(None, item, Expand::Template)
            }
            syn::Item::Enum(item) => {
                super::super::record::expand_enum_as(None, item, Mode::Api, Expand::Template)
            }
            _ => panic!("not a struct or an enum"),
        };
        match result {
            Ok(tokens) => tokens.to_string(),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn a_template_registers_nothing_and_carries_its_definition() {
        let out =
            expand_template("pub struct Page<T> { pub items: Vec<T>, pub next: Option<String> }");
        assert!(!has(&out, "RecordMeta"), "{out}");
        assert!(!has(&out, "inventory::submit"), "{out}");
        assert!(!has(&out, "UNDRA_TYPE_ID"), "{out}");
        assert!(
            has(
                &out,
                "impl<T: ::undra::wire::Encode> ::undra::wire::Encode for Page<T>"
            ),
            "{out}"
        );
        assert!(
            has(
                &out,
                "impl<T: ::undra::wire::Decode> ::undra::wire::Decode for Page<T>"
            ),
            "{out}"
        );
        assert!(has(&out, "macro_rules! __undra_template_Page"), "{out}");
        assert!(
            has(&out, "($__alias:ident, $__docs:literal, $T:ty)"),
            "{out}"
        );
        assert!(
            has(
                &out,
                "pub struct $__alias { pub items: Vec<$T>, pub next: Option<String> }"
            ),
            "{out}"
        );
        assert!(has(&out, "pub use __undra_template_Page as Page;"), "{out}");
        assert!(has(&out, "::undra::__instantiate!"), "{out}");
    }

    #[test]
    fn nested_generics_are_substituted_and_other_namespaces_are_left_alone() {
        let out = expand_template(
            "pub enum Loadable<T> { T, Loaded(Vec<Option<T>>), Mapped { value: std::collections::HashMap<String, T>, other: crate::T } }",
        );
        // The variant called `T` is not a use of the parameter; neither is a path that ends in `T`.
        assert!(has(&out, "Loaded(Vec<Option<$T>>)"), "{out}");
        assert!(
            has(&out, "value: std::collections::HashMap<String, $T>"),
            "{out}"
        );
        assert!(has(&out, "other: crate::T"), "{out}");
        assert!(has(&out, "{ T, Loaded"), "{out}");
    }

    #[test]
    fn several_parameters_make_several_metavariables() {
        let out = expand_template("pub struct Pair<A, B> { pub a: A, pub b: B }");
        assert!(
            has(&out, "($__alias:ident, $__docs:literal, $A:ty, $B:ty)"),
            "{out}"
        );
        assert!(has(&out, "pub a: $A, pub b: $B"), "{out}");
        // A catch-all rule turns the wrong number of arguments into the branded E0002.
        assert!(has(&out, "($($__rest:tt)*) =>"), "{out}");
        assert!(out.contains("takes 2 type arguments"), "{out}");
    }

    #[test]
    fn template_rules() {
        for (src, code, text) in [
            (
                "struct S<T> where T: Clone { a: T }",
                "E0002",
                "`where` clause",
            ),
            (
                "struct S<T, const N: usize> { a: T }",
                "E0002",
                "const generic `N`",
            ),
            ("struct S<'a, T> { a: T }", "E0003", "lifetime `'a`"),
            ("struct S<T = String> { a: T }", "E0002", "default type"),
            ("struct S { a: u8 }", "E0008", "no type parameters"),
            (
                "struct S<T> { a: T::Item }",
                "E0001",
                "associated type of a type parameter",
            ),
            ("struct S<T> { a: &str, b: T }", "E0001", "`&str`"),
            ("enum E<T> {}", "E0007", "no variants"),
        ] {
            let out = expand_template(src);
            assert!(out.contains(code), "{src}: {out}");
            assert!(out.contains(text), "{src}: {out}");
        }
    }

    #[test]
    fn a_template_keeps_the_bounds_of_its_parameters() {
        let out = expand_template("pub struct Page<T: Clone> { pub items: Vec<T> }");
        assert!(
            has(
                &out,
                "impl<T: Clone + ::undra::wire::Encode> ::undra::wire::Encode for Page<T>"
            ),
            "{out}"
        );
    }

    fn expand_instance(src: &str) -> String {
        let item: syn::Item = syn::parse_str(src).unwrap();
        match instantiate(item) {
            Ok(tokens) => tokens.to_string(),
            Err(error) => error.to_string(),
        }
    }

    const INSTANCE: &str = r#"
        #[undra_instance(template = "Page", crate_name = "", root = "::undra", docs = "A page.", alias_docs = "")]
        pub struct TodoPage { pub items: Vec<Todo>, pub next: Option<String> }
    "#;

    #[test]
    fn an_instance_registers_the_alias_under_its_own_name() {
        let out = expand_instance(INSTANCE);
        assert!(has(&out, "name: \"TodoPage\""), "{out}");
        assert!(
            has(&out, "type_id: ::undra::meta::ids::type_id(\"TodoPage\")"),
            "{out}"
        );
        assert!(has(&out, "docs: \"A page.\""), "{out}");
        assert!(has(&out, "impl TodoPage {"), "{out}");
        assert!(
            has(
                &out,
                "pub const UNDRA_TYPE_ID: u32 = ::undra::meta::ids::type_id(\"TodoPage\")"
            ),
            "{out}"
        );
        assert!(
            has(
                &out,
                "_undra_error_E0070_this_instantiation_of_Page_is_declared_twice"
            ),
            "{out}"
        );
        assert!(has(&out, "::undra::meta::Registration::Record"), "{out}");
        // The codecs are the template's.
        assert!(!has(&out, "wire::Encode for"), "{out}");
        assert!(!has(&out, "wire::Decode for"), "{out}");
    }

    #[test]
    fn an_instance_checks_only_its_type_arguments() {
        // `Option<String>` is the template's (checked there); `Todo` is the argument, which the
        // template's macro passes in a `None`-delimited group.
        let item: syn::ItemStruct = syn::parse_quote! {
            #[undra_instance(template = "Page", crate_name = "", root = "::undra", docs = "", alias_docs = "")]
            pub struct TodoPage { pub items: Vec<Todo>, pub next: Option<String> }
        };
        let grouped = {
            let mut item = item;
            for field in item.fields.iter_mut() {
                if let syn::Type::Path(path) = &mut field.ty {
                    let last = path.path.segments.last_mut().unwrap();
                    if let syn::PathArguments::AngleBracketed(args) = &mut last.arguments {
                        if last.ident == "Vec" {
                            let inner = match args.args.first().unwrap() {
                                syn::GenericArgument::Type(ty) => ty.clone(),
                                _ => unreachable!(),
                            };
                            args.args[0] =
                                syn::GenericArgument::Type(syn::Type::Group(syn::TypeGroup {
                                    group_token: syn::token::Group::default(),
                                    elem: Box::new(inner),
                                }));
                        }
                    }
                }
            }
            item
        };
        let out = match instantiate(syn::Item::Struct(grouped)) {
            Ok(tokens) => tokens.to_string(),
            Err(error) => error.to_string(),
        };
        assert!(has(&out, "<Todo>::UNDRA_TYPE_ID"), "{out}");
        assert!(
            !has(&out, "Option<String>, ::core::option::Option<String>"),
            "{out}"
        );
        assert!(!has(&out, "__undra_same"), "{out}");
    }

    #[test]
    fn an_alias_in_another_crate_is_e0070() {
        let src = INSTANCE.replace("crate_name = \"\"", "crate_name = \"some_other_crate\"");
        let item: syn::Item = syn::parse_str(&src).unwrap();
        // The test's own crate is `undra_macros`, or whatever cargo says: it is not the other one.
        if std::env::var("CARGO_CRATE_NAME").is_ok() {
            let out = instantiate(item).unwrap_err().to_string();
            assert!(
                out.starts_with("error[undra::E0070]: `TodoPage` instantiates `Page`"),
                "{out}"
            );
            assert!(
                out.contains("declared in the crate `some_other_crate`"),
                "{out}"
            );
        }
    }

    #[test]
    fn an_alias_expands_to_the_alias_and_the_template_call() {
        let item: syn::ItemType =
            syn::parse_str("/// A page of todos.\npub type TodoPage = Page<Todo>;").unwrap();
        let out = expand_alias(None, item).unwrap().to_string();
        assert!(has(&out, "pub type TodoPage = Page<Todo>;"), "{out}");
        assert!(
            has(&out, "Page! { TodoPage, \"A page of todos.\", Todo }"),
            "{out}"
        );
        let item: syn::ItemType =
            syn::parse_str("pub type Q = crate::model::Page<Vec<Todo>, String>;").unwrap();
        let out = expand_alias(None, item).unwrap().to_string();
        assert!(
            has(&out, "crate::model::Page! { Q, \"\", Vec<Todo>, String }"),
            "{out}"
        );
    }

    #[test]
    fn an_alias_that_is_not_an_instantiation_is_e0007() {
        for (src, text) in [
            ("pub type Id = u64;", "does not name an instantiation"),
            ("pub type Id = Vec<u8>;", "not a generic data type of yours"),
            (
                "pub type Id = Option<Todo>;",
                "not a generic data type of yours",
            ),
            ("pub type Id = (u8, u8);", "does not name an instantiation"),
        ] {
            let item: syn::ItemType = syn::parse_str(src).unwrap();
            let out = expand_alias(None, item).unwrap_err().to_string();
            assert!(out.starts_with("error[undra::E0007]"), "{src}: {out}");
            assert!(out.contains(text), "{src}: {out}");
        }
        let item: syn::ItemType = syn::parse_str("pub type A<T> = Page<T>;").unwrap();
        assert!(
            expand_alias(None, item)
                .unwrap_err()
                .to_string()
                .contains("E0002")
        );
        let item: syn::ItemType = syn::parse_str("pub type A = Page<'static, Todo>;").unwrap();
        assert!(
            expand_alias(None, item)
                .unwrap_err()
                .to_string()
                .contains("E0003")
        );
        let item: syn::ItemType = syn::parse_str("pub type A = Page<3>;").unwrap();
        assert!(
            expand_alias(None, item)
                .unwrap_err()
                .to_string()
                .contains("E0002")
        );
    }
}
