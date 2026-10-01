//! Compile-time identity checks for every type position the schema records (R1).
//!
//! The type mapper (`types.rs`) reads *spellings*: the last segment of a path decides what the
//! schema says. Nothing in the mapper can see what the spelling resolves to, so a user type
//! called `Bytes`, `use model::Item as Todo`, or `type Todo = Item` would make the schema
//! describe one type while the generated `Encode`/`Decode` code moves another. The checks in
//! this module close that gap at compile time, in the user's crate, on the user's tokens:
//!
//! * **Built-in mappings** (scalars, `String`, `Bytes`, `Uuid`, `Timestamp`, `Duration` and the
//!   `Vec`/`Option`/`HashMap`/`BTreeMap`/`Box`/`Result` constructors): a same-type assertion
//!   between the spelling and the canonical type, through a trait defined in the expansion so
//!   its `#[diagnostic::on_unimplemented]` can carry the branded message (E0060).
//! * **Named mappings** (records, enums, errors): every type declared with `#[undra::api]` or
//!   `#[undra::error]` has an inherent `UNDRA_TYPE_ID`, the hash of its declared name. A const
//!   assertion compares it with the hash of the name the schema recorded, so an alias or a
//!   renamed import fails (E0061). Objects carry `__UNDRA_IS_OBJECT` and get their own message
//!   (E0064), and the error position of a `Result` also requires `UNDRA_IS_ERROR`.
//!
//! A type that is not an Undra type at all has no `UNDRA_TYPE_ID`: it falls back to `0`, and the
//! assertion says so instead of calling it an alias. That is the case of a type that merely
//! needs `#[undra::api]` (where the `Encode`/`Decode` bounds report it as E0001 as well, which is
//! why the two messages agree on the fix) and of an alias of a type with no Undra declaration
//! (`type Id = u64`), which no other check would catch before `undra build`.
//!
//! The fallback trait is how "an inherent constant if the type has one, else a default" is
//! expressed: inherent associated items win over trait items in `<T>::NAME` resolution, and the
//! trait is blanket-implemented for every type.

use std::collections::BTreeSet;

use proc_macro2::TokenStream;
use quote::{quote, quote_spanned};
use syn::spanned::Spanned;
use syn::visit_mut::VisitMut;
use syn::{GenericArgument, PathArguments, Type};

use super::diag::{Diag, code};
use super::paths::Root;
use super::types::{KType, ty_string};

/// What a built-in constructor is compared with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Wrapper {
    Box,
    Vec,
    Option,
    HashMap,
    BTreeMap,
    Result,
}

impl Wrapper {
    fn of(name: &str) -> Option<Wrapper> {
        Some(match name {
            "Box" => Wrapper::Box,
            "Vec" => Wrapper::Vec,
            "Option" => Wrapper::Option,
            "HashMap" => Wrapper::HashMap,
            "BTreeMap" => Wrapper::BTreeMap,
            "Result" => Wrapper::Result,
            _ => return None,
        })
    }

    /// The canonical type applied to `args` (the spelled arguments, so each level of nesting is
    /// checked on its own).
    fn canonical(self, args: &[&Type]) -> TokenStream {
        match self {
            Wrapper::Box => quote!(::std::boxed::Box<#(#args),*>),
            Wrapper::Vec => quote!(::std::vec::Vec<#(#args),*>),
            Wrapper::Option => quote!(::core::option::Option<#(#args),*>),
            Wrapper::HashMap => quote!(::std::collections::HashMap<#(#args),*>),
            Wrapper::BTreeMap => quote!(::std::collections::BTreeMap<#(#args),*>),
            Wrapper::Result => quote!(::core::result::Result<#(#args),*>),
        }
    }
}

/// One same-type assertion.
enum Same {
    /// A scalar or wire leaf against its canonical type.
    Leaf { spelled: Type, kty: KType },
    /// A constructor applied to spelled arguments against the canonical constructor.
    Wrapper {
        spelled: Type,
        wrapper: Wrapper,
        args: Vec<Type>,
    },
}

/// One named-type assertion.
struct Named {
    ty: Type,
    name: String,
    /// The type is the error side of a `Result`.
    error: bool,
}

/// The identity checks of one expansion. Build it while the models are analysed, then append
/// [`Checks::emit`] to the expansion.
#[derive(Default)]
pub(crate) struct Checks {
    seen: BTreeSet<String>,
    same: Vec<Same>,
    named: Vec<Named>,
    /// What `Self` stands for in the spelled types (records and enums may write `Vec<Self>`);
    /// the checks live outside the type, where `Self` does not exist.
    self_name: Option<syn::Ident>,
}

impl Checks {
    /// An empty set of checks.
    pub(crate) fn new() -> Checks {
        Checks::default()
    }

    /// An empty set of checks for the fields of the type `name`: `Self` in a spelled type is
    /// replaced by `name` in the assertions.
    pub(crate) fn for_type(name: &syn::Ident) -> Checks {
        Checks {
            self_name: Some(name.clone()),
            ..Checks::default()
        }
    }

    /// `ty` with `Self` replaced by the type's name.
    fn resolved(&self, ty: &Type) -> Type {
        let mut ty = ty.clone();
        if let Some(name) = &self.self_name {
            VisitMut::visit_type_mut(&mut ReplaceSelf { name }, &mut ty);
        }
        ty
    }

    /// Records the checks for a value type `ty` that the mapper turned into `kty`.
    pub(crate) fn ty(&mut self, ty: &Type, kty: &KType) {
        self.walk(ty, kty, false);
    }

    /// Like [`Checks::ty`] for the error side of a `Result`, which must also be a
    /// `#[undra::error]` enum.
    pub(crate) fn error_ty(&mut self, ty: &Type, kty: &KType) {
        self.walk(ty, kty, true);
    }

    /// Records the checks for a return type the mapper turned into `kty`. `Unit` (no return
    /// type, or `()`) needs none.
    pub(crate) fn ret(&mut self, output: &syn::ReturnType, kty: &KType) {
        if let syn::ReturnType::Type(_, ty) = output {
            self.walk(ty, kty, false);
        }
    }

    fn fresh(&mut self, key: String) -> bool {
        self.seen.insert(key)
    }

    fn walk(&mut self, ty: &Type, kty: &KType, error: bool) {
        match ty {
            Type::Paren(inner) => return self.walk(&inner.elem, kty, error),
            Type::Group(inner) => return self.walk(&inner.elem, kty, error),
            Type::ImplTrait(impl_trait) => {
                match (kty, stream_item(impl_trait)) {
                    (KType::Stream(item_kty), Some(item)) => self.walk(item, item_kty, false),
                    // `impl Stream<Item = Result<T, E>>` recorded as `Result<Stream<T>, E>`
                    // (ADR-036): `T` is the item, `E` the error side.
                    (KType::Result(ok, err), Some(item)) => {
                        if let (KType::Stream(item_kty), Some((t, e))) =
                            (&**ok, super::types::result_parts(item))
                        {
                            self.walk(t, item_kty, false);
                            self.walk(e, err, true);
                        }
                    }
                    _ => {}
                }
                return;
            }
            Type::Path(_) => {}
            _ => return,
        }
        let Type::Path(path) = ty else { return };
        let Some(last) = path.path.segments.last() else {
            return;
        };
        let name = last.ident.to_string();
        let args = type_args(&last.arguments);

        // `Box<T>` is transparent in the schema: check the `Box`, then `T` against the same
        // schema type.
        if name == "Box" && args.len() == 1 {
            self.wrapper(ty, Wrapper::Box, &args);
            return self.walk(args[0], kty, error);
        }
        match kty {
            KType::Vec(inner) if args.len() == 1 => {
                self.wrapper(ty, Wrapper::Vec, &args);
                self.walk(args[0], inner, false);
            }
            KType::Option(inner) if args.len() == 1 => {
                self.wrapper(ty, Wrapper::Option, &args);
                self.walk(args[0], inner, false);
            }
            KType::Map(key, value) if args.len() == 2 => {
                if let Some(wrapper @ (Wrapper::HashMap | Wrapper::BTreeMap)) = Wrapper::of(&name) {
                    self.wrapper(ty, wrapper, &args);
                }
                self.walk(args[0], key, false);
                self.walk(args[1], value, false);
            }
            KType::Result(ok, err) if args.len() == 2 => {
                // `Result<impl Stream<..>, E>` cannot be spelled as a type argument: only the
                // parts are checked.
                if !matches!(**ok, KType::Stream(_)) {
                    self.wrapper(ty, Wrapper::Result, &args);
                }
                // `Result<impl Stream<Item = Result<T, E>>, E>` (ADR-036): the item's `E` is
                // checked against the same error type as the opening's.
                if let (KType::Stream(item_kty), Type::ImplTrait(impl_trait)) = (&**ok, args[0]) {
                    if let Some((t, e)) =
                        stream_item(impl_trait).and_then(super::types::result_parts)
                    {
                        self.walk(t, item_kty, false);
                        self.walk(e, err, true);
                        self.walk(args[1], err, true);
                        return;
                    }
                }
                self.walk(args[0], ok, false);
                self.walk(args[1], err, true);
            }
            KType::Named(record) => {
                // `Self` in a record or enum field is the type itself: trivially right.
                if name == "Self" && path.path.segments.len() == 1 {
                    return;
                }
                self.named(ty, record, error);
            }
            KType::Unit | KType::Stream(_) => {}
            KType::Vec(_) | KType::Option(_) | KType::Map(..) | KType::Result(..) => {}
            leaf => self.leaf(ty, leaf),
        }
    }

    fn leaf(&mut self, ty: &Type, kty: &KType) {
        if self.fresh(format!("leaf:{}", ty_string(ty))) {
            self.same.push(Same::Leaf {
                spelled: self.resolved(ty),
                kty: kty.clone(),
            });
        }
    }

    fn wrapper(&mut self, ty: &Type, wrapper: Wrapper, args: &[&Type]) {
        if self.fresh(format!("wrapper:{}", ty_string(ty))) {
            self.same.push(Same::Wrapper {
                spelled: self.resolved(ty),
                wrapper,
                args: args.iter().map(|t| self.resolved(t)).collect(),
            });
        }
    }

    fn named(&mut self, ty: &Type, name: &str, error: bool) {
        if self.fresh(format!("named:{}:{name}:{error}", ty_string(ty))) {
            self.named.push(Named {
                ty: self.resolved(ty),
                name: name.to_owned(),
                error,
            });
        }
    }

    /// Whether nothing needs checking.
    pub(crate) fn is_empty(&self) -> bool {
        self.same.is_empty() && self.named.is_empty()
    }

    /// The tokens of every check: one anonymous constant whose items are private to it.
    pub(crate) fn emit(&self, root: &Root) -> TokenStream {
        self.emit_as(root, None)
    }

    /// Like [`Checks::emit`], with the constant named. An anonymous constant is a module-level
    /// item only; a named one is also valid as an associated constant, which is what lets a
    /// function or query placed in an `impl` block by mistake get one error from `rustc` instead
    /// of one for each of its generated items (see the guard in `query.rs`).
    pub(crate) fn emit_named(&self, root: &Root, name: &syn::Ident) -> TokenStream {
        self.emit_as(root, Some(name))
    }

    fn emit_as(&self, root: &Root, name: Option<&syn::Ident>) -> TokenStream {
        if self.is_empty() {
            return TokenStream::new();
        }
        let wire = root.wire();
        let meta = root.meta();

        let same_items = if self.same.is_empty() {
            TokenStream::new()
        } else {
            let attr = same_as_attribute();
            let asserts = self.same.iter().map(|same| match same {
                Same::Leaf { spelled, kty } => {
                    let canonical = canonical_leaf(kty, &wire);
                    quote!(__undra_same::<#spelled, #canonical>();)
                }
                Same::Wrapper {
                    spelled,
                    wrapper,
                    args,
                } => {
                    let args: Vec<&Type> = args.iter().collect();
                    let canonical = wrapper.canonical(&args);
                    quote!(__undra_same::<#spelled, #canonical>();)
                }
            });
            quote! {
                #attr
                trait __UndraSameAs<T: ?::core::marker::Sized> {}
                impl<T: ?::core::marker::Sized> __UndraSameAs<T> for T {}
                fn __undra_same<A, B>()
                where
                    A: ?::core::marker::Sized + __UndraSameAs<B>,
                    B: ?::core::marker::Sized,
                {
                }
                fn __undra_identity() {
                    #(#asserts)*
                }
            }
        };

        let named_items = if self.named.is_empty() {
            TokenStream::new()
        } else {
            let asserts = self.named.iter().map(|named| named.assertion(&meta));
            quote! {
                trait __UndraFallback {
                    const UNDRA_TYPE_ID: u32 = 0;
                    const UNDRA_IS_ERROR: bool = false;
                    const __UNDRA_IS_OBJECT: bool = false;
                }
                impl<T: ?::core::marker::Sized> __UndraFallback for T {}
                #(#asserts)*
            }
        };

        match name {
            None => quote! {
                #[doc(hidden)]
                #[allow(
                    non_camel_case_types,
                    dead_code,
                    unused,
                    unused_braces,
                    clippy::all
                )]
                const _: () = {
                    #same_items
                    #named_items
                };
            },
            Some(name) => quote! {
                #[doc(hidden)]
                #[allow(
                    non_camel_case_types,
                    non_upper_case_globals,
                    dead_code,
                    unused,
                    unused_braces,
                    clippy::all
                )]
                const #name: () = {
                    #same_items
                    #named_items
                };
            },
        }
    }
}

/// Rewrites `Self` to the type's name.
struct ReplaceSelf<'a> {
    name: &'a syn::Ident,
}

impl VisitMut for ReplaceSelf<'_> {
    fn visit_type_path_mut(&mut self, node: &mut syn::TypePath) {
        syn::visit_mut::visit_type_path_mut(self, node);
        if node.qself.is_none() && node.path.segments.len() == 1 {
            let seg = &mut node.path.segments[0];
            if seg.ident == "Self" && seg.arguments.is_none() {
                let mut name = self.name.clone();
                name.set_span(seg.ident.span());
                seg.ident = name;
            }
        }
    }
}

impl Named {
    fn assertion(&self, meta: &TokenStream) -> TokenStream {
        let span = self.ty.span();
        let ty = &self.ty;
        let name = &self.name;
        let shown = ty_string(ty);

        let object = panic_text(&Diag::new(
            code::E0064,
            format!("`{shown}` is an object and cannot be used as a value"),
            "an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a parameter or a return value",
            "return a record with the data the platform needs, or construct the object from the platform with one of its constructors",
        ));
        let mismatch = panic_text(&Diag::new(
            code::E0061,
            format!(
                "`{name}` here is an alias or a renamed import of an Undra type that is declared under another name"
            ),
            format!(
                "Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type that name resolves to; with `type {name} = Other` or `use path::Other as {name}` the platforms would be told `{name}` and receive the layout of `Other`"
            ),
            format!(
                "write the type under the name it is declared with (`Other` in the examples above), or declare a separate `#[undra::api] struct {name}` if you mean a distinct type"
            ),
        ));
        let undeclared = panic_text(&Diag::new(
            code::E0061,
            format!("`{name}` is not a type declared with `#[undra::api]`"),
            "Undra describes a type to the platforms by the name it is written with, so the name must be a record or enum declared with `#[undra::api]` or an error declared with `#[undra::error]`; anything else, such as a plain struct or an alias (`type Id = u64`), has no definition the platforms could generate",
            format!(
                "add `#[undra::api]` to `{name}`, or, if it is an alias, write the type it stands for where it is used"
            ),
        ));
        let not_error = panic_text(&Diag::new(
            code::E0001,
            format!(
                "`{shown}` is used as the error type of a `Result`, but it is not a `#[undra::error]` enum"
            ),
            "the platforms throw the error type by name, and only `#[undra::error]` enums carry the messages they show",
            format!(
                "declare it with `#[undra::error]`, for example `#[undra::error] enum {name} {{ #[error(\"failed\")] Failed }}`"
            ),
        ));
        let error_check = if self.error {
            quote_spanned! {span=>
                if !<#ty>::UNDRA_IS_ERROR {
                    ::core::panic!(#not_error);
                }
            }
        } else {
            TokenStream::new()
        };
        quote_spanned! {span=>
            const _: () = {
                if <#ty>::__UNDRA_IS_OBJECT {
                    ::core::panic!(#object);
                }
                // `0` is the fallback of a type that has no Undra declaration at all.
                let __undra_id = <#ty>::UNDRA_TYPE_ID;
                if __undra_id == 0 {
                    ::core::panic!(#undeclared);
                }
                if __undra_id != #meta::ids::type_id(#name) {
                    ::core::panic!(#mismatch);
                }
                #error_check
            };
        }
    }
}

/// The message of a const `panic!` (a format string: braces escaped).
pub(crate) fn panic_text(diag: &Diag) -> String {
    diag.message().replace('{', "{{").replace('}', "}}")
}

/// The `Item` type of `impl Stream<Item = T>`.
fn stream_item(impl_trait: &syn::TypeImplTrait) -> Option<&Type> {
    impl_trait.bounds.iter().find_map(|bound| {
        let syn::TypeParamBound::Trait(bound) = bound else {
            return None;
        };
        let seg = bound.path.segments.last()?;
        if seg.ident != "Stream" {
            return None;
        }
        let PathArguments::AngleBracketed(args) = &seg.arguments else {
            return None;
        };
        args.args.iter().find_map(|arg| match arg {
            GenericArgument::AssocType(assoc) if assoc.ident == "Item" => Some(&assoc.ty),
            _ => None,
        })
    })
}

/// The type arguments of a path segment (lifetimes and others were rejected by the mapper).
fn type_args(arguments: &PathArguments) -> Vec<&Type> {
    match arguments {
        PathArguments::AngleBracketed(args) => args
            .args
            .iter()
            .filter_map(|arg| match arg {
                GenericArgument::Type(ty) => Some(ty),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The type the schema means by a leaf.
fn canonical_leaf(kty: &KType, wire: &TokenStream) -> TokenStream {
    match kty {
        KType::Bool => quote!(::core::primitive::bool),
        KType::I8 => quote!(::core::primitive::i8),
        KType::I16 => quote!(::core::primitive::i16),
        KType::I32 => quote!(::core::primitive::i32),
        KType::I64 => quote!(::core::primitive::i64),
        KType::U8 => quote!(::core::primitive::u8),
        KType::U16 => quote!(::core::primitive::u16),
        KType::U32 => quote!(::core::primitive::u32),
        KType::U64 => quote!(::core::primitive::u64),
        KType::F32 => quote!(::core::primitive::f32),
        KType::F64 => quote!(::core::primitive::f64),
        KType::String => quote!(::std::string::String),
        KType::Bytes => quote!(#wire::Bytes),
        KType::Uuid => quote!(#wire::Uuid),
        KType::Timestamp => quote!(#wire::Timestamp),
        KType::Duration => quote!(::core::time::Duration),
        // Not leaves: the walker never asks.
        _ => quote!(()),
    }
}

/// `#[diagnostic::on_unimplemented(..)]` for the same-type trait (E0060).
fn same_as_attribute() -> TokenStream {
    let diag = Diag::new(
        code::E0060,
        "`{Self}` is spelled like the built-in Undra type `{T}`, but it is a different type",
        "the schema records this position as the built-in type, so the platforms would read the bytes of `{T}` where the generated code writes `{Self}`",
        "rename your type, or import the built-in one (`{T}`) where it is used",
    );
    on_unimplemented(&diag, "this is not `{T}`")
}

/// `#[diagnostic::on_unimplemented(message = .., label = ..)]` carrying a branded diagnostic.
///
/// The message is the whole multi-line diagnostic, as the const-assertion diagnostics are, so
/// every Undra error looks the same. Only `{Self}` and the trait's parameters may appear in
/// braces.
pub(crate) fn on_unimplemented(diag: &Diag, label: &str) -> TokenStream {
    let message = diag.message();
    quote! {
        #[diagnostic::on_unimplemented(message = #message, label = #label)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::impl_::types::{Allow, Pos, map_type};

    fn emitted(src: &str) -> String {
        let ty: Type = syn::parse_str(src).unwrap();
        let kty = map_type(&ty, Pos::Field, Allow::NONE).unwrap();
        let mut checks = Checks::new();
        checks.ty(&ty, &kty);
        checks.emit(&Root::default()).to_string()
    }

    fn has(out: &str, needle: &str) -> bool {
        crate::tests::has(out, needle)
    }

    #[test]
    fn leaves_compare_against_the_canonical_type() {
        let out = emitted("Bytes");
        assert!(
            has(&out, "__undra_same::<Bytes, ::undra::wire::Bytes>();"),
            "{out}"
        );
        let out = emitted("u32");
        assert!(
            has(&out, "__undra_same::<u32, ::core::primitive::u32>();"),
            "{out}"
        );
        let out = emitted("std::time::Duration");
        assert!(
            has(
                &out,
                "__undra_same::<std::time::Duration, ::core::time::Duration>();"
            ),
            "{out}"
        );
    }

    #[test]
    fn constructors_and_their_arguments_are_checked_level_by_level() {
        let out = emitted("Option<Vec<Uuid>>");
        assert!(
            has(
                &out,
                "__undra_same::<Option<Vec<Uuid>>, ::core::option::Option<Vec<Uuid>>>();"
            ),
            "{out}"
        );
        assert!(
            has(&out, "__undra_same::<Vec<Uuid>, ::std::vec::Vec<Uuid>>();"),
            "{out}"
        );
        assert!(
            has(&out, "__undra_same::<Uuid, ::undra::wire::Uuid>();"),
            "{out}"
        );
        let out = emitted("HashMap<String, Box<Todo>>");
        assert!(
            has(&out, "::std::collections::HashMap<String, Box<Todo>>"),
            "{out}"
        );
        assert!(
            has(
                &out,
                "__undra_same::<Box<Todo>, ::std::boxed::Box<Todo>>();"
            ),
            "{out}"
        );
    }

    #[test]
    fn named_types_compare_type_ids() {
        let out = emitted("Vec<crate::model::Todo>");
        assert!(
            has(
                &out,
                "let __undra_id = <crate::model::Todo>::UNDRA_TYPE_ID;"
            ),
            "{out}"
        );
        // `0` is the fallback of a type that is not declared with Undra at all.
        assert!(has(&out, "if __undra_id == 0 {"), "{out}");
        assert!(
            has(
                &out,
                "if __undra_id != ::undra::meta::ids::type_id(\"Todo\") {"
            ),
            "{out}"
        );
        assert!(has(&out, "__UNDRA_IS_OBJECT"), "{out}");
        assert!(!has(&out, "UNDRA_IS_ERROR {"), "{out}");
    }

    #[test]
    fn self_is_not_checked_and_repeats_are_emitted_once() {
        let ty: Type = syn::parse_str("Vec<Self>").unwrap();
        let kty = KType::Vec(Box::new(KType::Named("Node".into())));
        let mut checks = Checks::new();
        checks.ty(&ty, &kty);
        let out = checks.emit(&Root::default()).to_string();
        assert!(!has(&out, "let __undra_id"), "{out}");
        let ty: Type = syn::parse_str("String").unwrap();
        let mut checks = Checks::new();
        checks.ty(&ty, &KType::String);
        checks.ty(&ty, &KType::String);
        let out = checks.emit(&Root::default()).to_string();
        assert_eq!(out.matches("__undra_same :: < String").count(), 1, "{out}");
    }

    #[test]
    fn nothing_to_check_emits_nothing() {
        assert!(Checks::new().emit(&Root::default()).is_empty());
        let mut checks = Checks::new();
        checks.ret(&syn::ReturnType::Default, &KType::Unit);
        assert!(checks.is_empty());
    }

    #[test]
    fn error_positions_also_require_an_error_enum() {
        let ty: Type = syn::parse_str("TodoError").unwrap();
        let mut checks = Checks::new();
        checks.error_ty(&ty, &KType::Named("TodoError".into()));
        let out = checks.emit(&Root::default()).to_string();
        assert!(has(&out, "!<TodoError>::UNDRA_IS_ERROR"), "{out}");
        assert!(has(&out, "error[undra::E0001]"), "{out}");
    }
}
