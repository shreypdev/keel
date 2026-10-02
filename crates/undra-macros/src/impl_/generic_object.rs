//! Generic objects and stores (ADR-058 decision 3).
//!
//! An object or a store written once for several types is a **template**, instantiated by an
//! alias, exactly as ADR-042's data types are (see [`super::generic`]):
//!
//! ```ignore
//! #[undra::store(generic, restore = "Self::assemble")]
//! pub struct Selection<T> { rows: Signal<Vec<T>>, count: Computed<u32> }
//!
//! #[undra::api(store, generic)]
//! impl<T: Row> Selection<T> {
//!     pub fn new(ctx: Ctx) -> Self { .. }
//!     pub fn toggle(&self, row: T) { .. }
//! }
//!
//! #[undra::api] pub type TodoSelection = Selection<Todo>;
//! ```
//!
//! # What the template is
//!
//! The template registers nothing: no type id, no dispatcher, no place in the schema. The impl
//! block is emitted as written, together with a hidden `macro_rules!` that carries the block's
//! **public signatures** (bodies emptied), each type parameter replaced by a metavariable, and the
//! type itself applied to its own parameters (`Selection<T>`) replaced by the alias. A store's
//! template also carries the struct's fields. The placeholders are **positional** (the block's
//! parameter at position *i* of the self type's arguments), so `impl<U> Selection<U>` composes
//! with `struct Selection<T>`.
//!
//! # The instantiation
//!
//! The alias calls the template, and `undra::__instantiate!` runs the ordinary object expansion
//! (and, for a store, the ordinary store expansion) on the concrete tokens with the alias as the
//! self type, without emitting the items themselves. The schema gets an `ObjectDef` named after
//! the alias that is byte for byte what a hand-written object of that name produces.
//!
//! # How a store's two halves meet
//!
//! A store's struct has a macro of its own, its impl block another, and one exported template has
//! to hold both. The struct's macro defines a **local** `macro_rules!`; the impl block's macro,
//! which must stand below the struct in the same module, calls it with its signatures; and that
//! macro calls the hidden [`expand_compose_store`], which emits the one template. The local macro
//! is named after the rule, so a block that is not below its struct is reported by `rustc` as
//! "cannot find macro `_undra_error_E0011_Selection_is_not_a_generic_store_declared_above_this_impl_block_in_the_same_module`".

use std::collections::HashMap;

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::visit_mut::{self, VisitMut};
use syn::{ImplItem, ItemImpl, ItemStruct, Type, Visibility};

use super::common::{GenericOn, check_generics_on};
use super::diag::{Diag, Errors, code};
use super::generic::{ALIAS, KeepReadable, arity_message, replace_idents, template_macro_name};
use super::naming::unraw;
use super::types::ty_string;

/// The placeholder of the type parameter at position `index` of a template's self type.
fn param_placeholder(index: usize) -> String {
    format!("__UNDRA_PARAM_{index}")
}

/// The name of the metavariable the template's macro binds the type argument at `index` to.
fn metavariable(index: usize) -> syn::Ident {
    format_ident!("__undra_p{}", index)
}

/// What the header of an impl block says about its template: the name of the type, and the names
/// of the block's type parameters in the order of the self type's arguments.
#[derive(Clone, Debug)]
pub(crate) struct Header {
    /// The type's name, `Selection`.
    pub(crate) name: String,
    /// The block's parameters in the order the self type applies them: `["T"]`, `["K", "V"]`.
    pub(crate) params: Vec<String>,
}

/// Checks the header of the impl block of a generic object (E0074, E0008, E0003, E0002): the block
/// is for the type with its own type parameters, each exactly once.
pub(crate) fn check_header(item: &ItemImpl, errors: &mut Errors) -> Option<Header> {
    let Type::Path(path) = &*item.self_ty else {
        // An impl for something that is not a named type: the ordinary expansion says so.
        return None;
    };
    let seg = path.path.segments.last()?;
    if path.qself.is_some() {
        return None;
    }
    let name = unraw(&seg.ident);

    // Lifetimes and const parameters are what they are everywhere else; bounds and a `where`
    // clause are the author's.
    let mut others = item.generics.clone();
    others.params = others
        .params
        .into_iter()
        .filter(|param| !matches!(param, syn::GenericParam::Type(_)))
        .collect();
    others.where_clause = None;
    check_generics_on(&others, &name, GenericOn::Template, errors);

    let params: Vec<&syn::TypeParam> = item.generics.type_params().collect();
    if params.is_empty() {
        errors.push(
            Diag::new(
                code::E0008,
                format!("`generic` on `{name}`, which has no type parameters"),
                "`generic` marks the template of named instantiations, and a template needs a type parameter to be instantiated with",
                format!("remove `generic`, or add the type parameter: `impl<T> {name}<T> {{ .. }}`"),
            )
            .on(&item.self_ty),
        );
        return None;
    }

    // The self type applies each parameter of the block once, as a plain path.
    let applied: Option<Vec<String>> = match &seg.arguments {
        syn::PathArguments::AngleBracketed(args) => args
            .args
            .iter()
            .map(|arg| match arg {
                syn::GenericArgument::Type(Type::Path(p))
                    if p.qself.is_none()
                        && p.path.leading_colon.is_none()
                        && p.path.segments.len() == 1
                        && p.path.segments[0].arguments.is_none() =>
                {
                    Some(p.path.segments[0].ident.to_string())
                }
                _ => None,
            })
            .collect(),
        _ => None,
    };
    let declared: Vec<String> = params.iter().map(|p| p.ident.to_string()).collect();
    let right = applied.as_ref().is_some_and(|applied| {
        applied.len() == declared.len()
            && declared
                .iter()
                .all(|d| applied.iter().filter(|a| *a == d).count() == 1)
    });
    if !right {
        let own = declared.join(", ");
        errors.push(
            Diag::new(
                code::E0074,
                format!(
                    "the impl block of the generic object `{name}` is for `{}`",
                    ty_string(&item.self_ty)
                ),
                format!(
                    "a generic object is instantiated by naming its type arguments (`pub type Todo{name} = {name}<Todo>;`), so its block is for the type with its own parameters, each exactly once"
                ),
                format!(
                    "write `impl<{own}> {name}<{own}>` and put the type you meant in the methods' types"
                ),
            )
            .on(&item.self_ty),
        );
        return None;
    }
    Some(Header {
        name,
        params: applied.unwrap_or_default(),
    })
}

/// The generics of a `#[undra::store(generic)]` struct: type parameters only (a lifetime is E0003,
/// a const parameter or a default E0002), at least one (E0008); bounds and a `where` clause are the
/// author's. The names of the parameters, in order.
pub(crate) fn check_struct_generics(
    generics: &syn::Generics,
    name: &str,
    errors: &mut Errors,
) -> Option<Vec<String>> {
    let mut others = generics.clone();
    others.where_clause = None;
    check_generics_on(&others, name, GenericOn::Template, errors);
    let params: Vec<String> = generics.type_params().map(|p| unraw(&p.ident)).collect();
    if params.is_empty() {
        errors.push(
            Diag::new(
                code::E0008,
                format!("`generic` on `{name}`, which has no type parameters"),
                "`generic` marks the template of named instantiations, and a template needs a type parameter to be instantiated with",
                format!("remove `generic`, or add the type parameter: `struct {name}<T> {{ .. }}`"),
            )
            .at(proc_macro2::Span::call_site()),
        );
        return None;
    }
    Some(params)
}

/// E0074 for a method of a generic object that has type parameters of its own, with or without a
/// list (`listed` is the parameter of the first list): an instantiation of a generic object is one
/// class on each platform, and a second level of generics would multiply every one of them.
pub(crate) fn method_generics_refused(
    sig: &syn::Signature,
    object: &str,
    errors: &mut Errors,
    listed: Option<&syn::Ident>,
) {
    let method = unraw(&sig.ident);
    let diag = |what: String| {
        Diag::new(
            code::E0074,
            what,
            "an instantiation of a generic object is one class on each platform; a type parameter on a method would multiply every instantiation by it",
            "make the parameter a type parameter of the object and name it in the alias, or write the method for a concrete type",
        )
    };
    if let Some(list) = listed {
        errors.push(
            diag(format!(
                "`generic(..)` on `{method}`, a method of the generic object `{object}`"
            ))
            .on(list),
        );
    } else if let Some(param) = sig.generics.type_params().next() {
        errors.push(
            diag(format!(
                "generic parameter `{}` on `{method}`, a method of the generic object `{object}`",
                param.ident
            ))
            .on(param),
        );
    }
}

/// Replaces the type applied to its own parameters (`Selection<T>`) by the alias placeholder.
struct AppliedSelf<'a> {
    name: &'a str,
    params: &'a [String],
}

impl VisitMut for AppliedSelf<'_> {
    fn visit_type_mut(&mut self, ty: &mut Type) {
        if let Type::Path(path) = ty {
            let last = path.path.segments.last();
            let applied = path.qself.is_none()
                && last.is_some_and(|seg| {
                    seg.ident == self.name
                        && match &seg.arguments {
                            syn::PathArguments::AngleBracketed(args) => {
                                args.args.len() == self.params.len()
                                    && args.args.iter().zip(self.params).all(|(arg, param)| {
                                        matches!(arg, syn::GenericArgument::Type(Type::Path(p))
                                            if p.qself.is_none()
                                                && p.path.segments.len() == 1
                                                && p.path.segments[0].ident == param.as_str()
                                                && p.path.segments[0].arguments.is_none())
                                    })
                            }
                            _ => false,
                        }
                });
            if applied {
                *ty = syn::parse_str(ALIAS).expect("the alias placeholder is a type");
                return;
            }
        }
        visit_mut::visit_type_mut(self, ty);
    }
}

/// Replaces each type parameter in a type position by the placeholder of its position.
struct Positional<'a> {
    params: &'a [String],
}

impl VisitMut for Positional<'_> {
    fn visit_type_mut(&mut self, ty: &mut Type) {
        if let Type::Path(path) = ty {
            let simple = path.qself.is_none()
                && path.path.leading_colon.is_none()
                && path.path.segments.len() == 1
                && path.path.segments[0].arguments.is_none();
            if simple {
                let ident = path.path.segments[0].ident.to_string();
                if let Some(index) = self.params.iter().position(|p| *p == ident) {
                    path.path.segments[0].ident = format_ident!("{}", param_placeholder(index));
                    return;
                }
            }
        }
        visit_mut::visit_type_mut(self, ty);
    }
}

/// The signature-only impl block an instantiation reads: `impl __UNDRA_ALIAS { pub fn
/// toggle(&self, row: __UNDRA_PARAM_0) {} .. }`, with the type applied to its own parameters
/// replaced by the alias and the parameters by their placeholders.
pub(crate) fn impl_definition(item: &ItemImpl, header: &Header) -> TokenStream {
    let fns = item.items.iter().filter_map(|it| match it {
        ImplItem::Fn(func) if matches!(func.vis, Visibility::Public(_)) => {
            let attrs = func
                .attrs
                .iter()
                .filter(|a| a.path().is_ident("doc") || a.path().is_ident("undra"));
            let sig = &func.sig;
            Some(quote!(#(#attrs)* pub #sig {}))
        }
        _ => None,
    });
    let alias = format_ident!("{}", ALIAS);
    let mut definition: ItemImpl = syn::parse_quote!(impl #alias { #(#fns)* });
    AppliedSelf {
        name: &header.name,
        params: &header.params,
    }
    .visit_item_impl_mut(&mut definition);
    Positional {
        params: &header.params,
    }
    .visit_item_impl_mut(&mut definition);
    definition.to_token_stream()
}

/// The struct an instantiation of a generic store reads: named as the alias, without generics,
/// each type parameter (by its position in the struct's own list) replaced by its placeholder.
pub(crate) fn struct_definition(item: &ItemStruct, params: &[String]) -> TokenStream {
    let mut definition = item.clone();
    definition.attrs.clear();
    definition.ident = format_ident!("{}", ALIAS);
    definition.generics = syn::Generics::default();
    let mut definition = syn::Item::Struct(definition);
    KeepReadable.visit_item_mut(&mut definition);
    Positional { params }.visit_item_mut(&mut definition);
    definition.to_token_stream()
}

/// The name of the local macro through which the impl block of a generic store hands its
/// signatures to the store's struct, which then exports the one template. `rustc` names it when
/// the block is not below its struct in the same module.
pub(crate) fn compose_macro_name(name: &str) -> syn::Ident {
    format_ident!(
        "_undra_error_{}_{}_is_not_a_generic_store_declared_above_this_impl_block_in_the_same_module",
        code::E0011,
        name
    )
}

/// What a template tells `__instantiate!` beyond the definitions.
pub(crate) struct TemplateInfo<'a> {
    /// `"object"` or `"store"`.
    pub(crate) kind: &'static str,
    /// The type's name, `Selection`.
    pub(crate) name: &'a str,
    /// The names of the template's type parameters, for the arity message.
    pub(crate) params: &'a [String],
    /// The documentation an alias without its own inherits: a plain object's block docs, a
    /// store's struct docs.
    pub(crate) docs: &'a str,
    /// A store's block docs.
    pub(crate) impl_docs: &'a str,
    /// A store's `restore = ".."` hook, or empty.
    pub(crate) restore: &'a str,
    /// The path of the `undra` crate as the template was written with it.
    pub(crate) root: &'a str,
}

/// The exported template of a generic object or store: the hidden `macro_rules!` that an alias
/// calls, and its re-export under the type's name.
pub(crate) fn object_template(info: &TemplateInfo<'_>, definitions: TokenStream) -> TokenStream {
    let mut replacements: HashMap<String, TokenStream> = HashMap::new();
    replacements.insert(ALIAS.to_owned(), quote!($__alias));
    let mut matchers = Vec::new();
    for index in 0..info.params.len() {
        let var = metavariable(index);
        replacements.insert(param_placeholder(index), quote!($#var));
        matchers.push(quote!($#var:ty));
    }
    let definitions = replace_idents(definitions, &replacements);
    // The type arguments, as an item after the definitions: the instantiation names the rule of
    // E0070 after them, so a second alias of the same instantiation in the same module defines the
    // same constant twice, which `rustc` reports before anything else.
    let vars: Vec<syn::Ident> = (0..info.params.len()).map(metavariable).collect();
    let macro_name = template_macro_name(info.name);
    let type_name = format_ident!("{}", info.name);
    let root: syn::Path = syn::parse_str(info.root).expect("a root path");
    let crate_name = std::env::var("CARGO_CRATE_NAME").unwrap_or_default();
    let TemplateInfo {
        kind,
        name,
        docs,
        impl_docs,
        restore,
        root: root_path,
        ..
    } = info;
    let arity = arity_message(name, info.params);
    quote! {
        #[doc(hidden)]
        #[macro_export]
        macro_rules! #macro_name {
            ( $__alias:ident, $__docs:literal, #(#matchers),* ) => {
                #root::__instantiate! {
                    #[undra_instance(
                        kind = #kind,
                        template = #name,
                        crate_name = #crate_name,
                        root = #root_path,
                        docs = #docs,
                        impl_docs = #impl_docs,
                        restore = #restore,
                        alias_docs = $__docs
                    )]
                    #definitions
                    type __UndraInstanceArgs = ( #($#vars,)* );
                }
            };
            ( $($__rest:tt)* ) => {
                ::core::compile_error!(#arity);
            };
        }

        #[doc(hidden)]
        #[allow(unused_imports, unreachable_pub)]
        pub use #macro_name as #type_name;
    }
}

/// A template that failed to expand still has to exist, so the aliases written for it are not also
/// "cannot find macro": a stub that swallows them. The failure has been reported already.
pub(crate) fn stub_template(name: &str) -> TokenStream {
    let macro_name = template_macro_name(name);
    let type_name = format_ident!("{}", name);
    quote! {
        #[doc(hidden)]
        #[macro_export]
        macro_rules! #macro_name {
            ( $($__rest:tt)* ) => {};
        }

        #[doc(hidden)]
        #[allow(unused_imports, unreachable_pub)]
        pub use #macro_name as #type_name;
    }
}

/// The local macro of a store that failed to expand: it swallows what the impl block hands it, so
/// the block adds no "cannot find macro" to the one error that was reported.
pub(crate) fn stub_compose(name: &str) -> TokenStream {
    let compose = compose_macro_name(name);
    quote! {
        #[doc(hidden)]
        #[allow(unused_macros)]
        macro_rules! #compose {
            ( $($__rest:tt)* ) => {};
        }
    }
}

/// `undra::__compose_store!`: called by the local macro of a generic store's struct with the
/// signatures its impl block handed over; emits the one exported template.
pub(crate) fn expand_compose_store(input: TokenStream) -> TokenStream {
    struct Compose {
        name: syn::LitStr,
        root: syn::LitStr,
        docs: syn::LitStr,
        restore: syn::LitStr,
        params: Vec<String>,
        definition: TokenStream,
        impl_docs: syn::LitStr,
        block: TokenStream,
    }
    impl syn::parse::Parse for Compose {
        fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
            let name = input.parse()?;
            let root = input.parse()?;
            let docs = input.parse()?;
            let restore = input.parse()?;
            let content;
            syn::bracketed!(content in input);
            let params = content
                .parse_terminated(<syn::Ident as syn::parse::Parse>::parse, syn::Token![,])?
                .into_iter()
                .map(|ident| ident.to_string())
                .collect();
            let definition;
            syn::braced!(definition in input);
            let impl_docs = input.parse()?;
            let block;
            syn::braced!(block in input);
            Ok(Compose {
                name,
                root,
                docs,
                restore,
                params,
                definition: definition.parse()?,
                impl_docs,
                block: block.parse()?,
            })
        }
    }
    let compose: Compose = match syn::parse2(input) {
        Ok(compose) => compose,
        Err(error) => return error.to_compile_error(),
    };
    let definition = compose.definition;
    let block = compose.block;
    let info = TemplateInfo {
        kind: "store",
        name: &compose.name.value(),
        params: &compose.params,
        docs: &compose.docs.value(),
        impl_docs: &compose.impl_docs.value(),
        restore: &compose.restore.value(),
        root: &compose.root.value(),
    };
    object_template(&info, quote!(#definition #block))
}
