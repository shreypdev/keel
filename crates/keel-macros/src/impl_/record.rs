//! Records (`#[keel::api] struct`) and enums (`#[keel::api] enum`, `#[keel::error]`).
//!
//! For a record the macro emits, next to the item as written:
//!
//! * `impl Todo { pub const KEEL_TYPE_ID: u32 }`
//! * `impl Encode` (fields in declaration order) and `impl Decode`
//!   (with an exact `MIN_ENCODED_LEN`, so `Vec<Todo>` decoders can reject impossible counts),
//! * `static __KEEL_META_Todo: RecordMeta` and its `inventory` registration.
//!
//! Enums are the same with a `u16` variant index in front of the variant's fields.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Fields, ItemEnum, ItemStruct};

use super::attrs::{Site, take};
use super::common::{check_generics, derived, derives, field_meta, item_root, submit};
use super::diag::{Diag, Errors, code};
use super::error::{ErrorAttr, take_field_attrs, take_message};
use super::naming::unraw;
use super::paths::Root;
use super::types::{KType, map_field};

/// The shape of a struct or variant body.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Shape {
    Unit,
    Tuple,
    Named,
}

/// One field of a record or variant.
pub(crate) struct FieldModel {
    pub(crate) ident: Option<syn::Ident>,
    /// The field name in the schema: the identifier, or the tuple index.
    pub(crate) name: String,
    pub(crate) ty: syn::Type,
    pub(crate) kty: KType,
    pub(crate) default: bool,
    pub(crate) docs: String,
    /// `#[from]` (error enums).
    pub(crate) from: bool,
    /// `#[source]` or `#[from]` (error enums).
    pub(crate) source: bool,
}

/// One enum variant.
pub(crate) struct VariantModel {
    pub(crate) ident: syn::Ident,
    pub(crate) index: u16,
    pub(crate) shape: Shape,
    pub(crate) fields: Vec<FieldModel>,
    pub(crate) docs: String,
    /// The `#[error(..)]` attribute (error enums only).
    pub(crate) error: Option<ErrorAttr>,
    pub(crate) span: Span,
}

/// Whether an enum is a plain `#[keel::api]` enum or a `#[keel::error]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Api,
    Error,
}

fn parse_fields<'a>(
    fields: impl Iterator<Item = &'a mut syn::Field>,
    mode: Mode,
    errors: &mut Errors,
) -> Vec<FieldModel> {
    let mut out = Vec::new();
    for (index, field) in fields.enumerate() {
        let name = field
            .ident
            .as_ref()
            .map_or_else(|| index.to_string(), unraw);
        let attr = take(&mut field.attrs, Site::FIELD, errors);
        let (from, source) = if mode == Mode::Error {
            take_field_attrs(&mut field.attrs)
        } else {
            (false, false)
        };
        let kty = match map_field(&field.ty, &name) {
            Ok(kty) => kty,
            Err(err) => {
                errors.push(err.into_error());
                KType::Unit
            }
        };
        out.push(FieldModel {
            ident: field.ident.clone(),
            name,
            ty: field.ty.clone(),
            kty,
            default: attr.default,
            docs: super::attrs::docs(&field.attrs),
            from,
            source: source || from,
        });
    }
    out
}

fn item_shape(what: &str, node: &impl quote::ToTokens, why: &str, help: &str) -> syn::Error {
    Diag::new(code::E0007, what, why, help).on(node)
}

// ---------------------------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------------------------

/// Expands `#[keel::api]` on a struct.
pub(crate) fn expand_struct(args_root: Option<Root>, mut item: ItemStruct) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    let root = item_root(&mut item.attrs, args_root, &mut errors);
    check_generics(&item.generics, &item.ident.to_string(), &mut errors);
    let fields = match &mut item.fields {
        Fields::Named(named) => parse_fields(named.named.iter_mut(), Mode::Api, &mut errors),
        Fields::Unit => Vec::new(),
        Fields::Unnamed(unnamed) => {
            errors.push(item_shape(
                &format!("tuple struct `{}` cannot be a record", item.ident),
                unnamed,
                "the schema names every field so the other languages can generate properties; tuple fields have no names",
                "use named fields: `struct Meters { value: f64 }`",
            ));
            Vec::new()
        }
    };
    errors.finish()?;

    let wire = root.wire();
    let meta = root.meta();
    let name = &item.ident;
    let name_str = unraw(name);
    let docs = super::attrs::docs(&item.attrs);
    let meta_static = format_ident!("__KEEL_META_{}", name_str);

    let encode_fields = fields.iter().map(|f| {
        let ident = f.ident.as_ref().expect("named field");
        quote_spanned! {f.ty.span()=> #wire::Encode::encode(&self.#ident, __w); }
    });
    let decode_fields = fields.iter().map(|f| {
        let ident = f.ident.as_ref().expect("named field");
        let ty = &f.ty;
        quote_spanned! {f.ty.span()=> #ident: <#ty as #wire::Decode>::decode(__r)? }
    });
    let min_len = fields.iter().map(|f| {
        let ty = &f.ty;
        quote_spanned! {f.ty.span()=> + <#ty as #wire::Decode>::MIN_ENCODED_LEN }
    });
    let field_metas = fields
        .iter()
        .map(|f| field_meta(&meta, &f.name, &f.kty, f.default, &f.docs));
    let derived = derived();
    let registration = submit(&root, "Record", &meta_static);

    Ok(quote! {
        #item

        impl #name {
            /// The stable Keel type id: `fnv1a32` of the type name.
            pub const KEEL_TYPE_ID: u32 = #meta::ids::type_id(#name_str);
        }

        #derived
        impl #wire::Encode for #name {
            #[allow(unused_variables)]
            fn encode(&self, __w: &mut #wire::Writer) {
                #(#encode_fields)*
            }
        }

        #derived
        impl #wire::Decode for #name {
            const MIN_ENCODED_LEN: usize = 0usize #(#min_len)*;

            #[allow(unused_variables)]
            fn decode(__r: &mut #wire::Reader<'_>) -> ::core::result::Result<Self, #wire::WireError> {
                ::core::result::Result::Ok(Self { #(#decode_fields),* })
            }
        }

        #[allow(non_upper_case_globals)]
        static #meta_static: #meta::RecordMeta = #meta::RecordMeta {
            name: #name_str,
            type_id: #meta::ids::type_id(#name_str),
            fields: &[ #(#field_metas),* ],
            docs: #docs,
        };
        #registration
    })
}

// ---------------------------------------------------------------------------------------------
// Enums and errors
// ---------------------------------------------------------------------------------------------

fn parse_variants(item: &mut ItemEnum, mode: Mode, errors: &mut Errors) -> Vec<VariantModel> {
    let enum_name = item.ident.to_string();
    let mut out = Vec::new();
    if item.variants.is_empty() {
        errors.push(item_shape(
            &format!("enum `{enum_name}` has no variants"),
            &item.ident,
            "an enum without variants has no values, so it can never be sent",
            "add at least one variant",
        ));
    }
    if item.variants.len() > usize::from(u16::MAX) {
        errors.push(item_shape(
            &format!("enum `{enum_name}` has too many variants"),
            &item.ident,
            "the wire index of a variant is a `u16`",
            "split the enum",
        ));
    }
    for (index, variant) in item.variants.iter_mut().enumerate() {
        let span = variant.span();
        take(&mut variant.attrs, Site::NOTHING, errors);
        if let Some((_, discriminant)) = &variant.discriminant {
            errors.push(item_shape(
                &format!("variant `{}` has an explicit discriminant", variant.ident),
                discriminant,
                "the wire index of a variant is its position in the declaration, never its discriminant",
                "remove the `= ..`; keep variants in wire order",
            ));
        }
        let (shape, fields) = match &mut variant.fields {
            Fields::Unit => (Shape::Unit, Vec::new()),
            Fields::Unnamed(unnamed) => {
                if unnamed.unnamed.is_empty() {
                    errors.push(item_shape(
                        &format!("variant `{}` has empty parentheses", variant.ident),
                        &variant.ident,
                        "an empty tuple variant and a unit variant are the same on the wire",
                        "write a unit variant without `()`",
                    ));
                }
                (
                    Shape::Tuple,
                    parse_fields(unnamed.unnamed.iter_mut(), mode, errors),
                )
            }
            Fields::Named(named) => {
                if named.named.is_empty() {
                    errors.push(item_shape(
                        &format!("variant `{}` has empty braces", variant.ident),
                        &variant.ident,
                        "an empty struct variant and a unit variant are the same on the wire",
                        "write a unit variant without `{}`",
                    ));
                }
                (
                    Shape::Named,
                    parse_fields(named.named.iter_mut(), mode, errors),
                )
            }
        };
        let error = if mode == Mode::Error {
            take_message(&mut variant.attrs, &variant.ident, shape, &fields, errors)
        } else {
            None
        };
        out.push(VariantModel {
            ident: variant.ident.clone(),
            index: u16::try_from(index).unwrap_or(u16::MAX),
            shape,
            fields,
            docs: super::attrs::docs(&variant.attrs),
            error,
            span,
        });
    }
    out
}

/// The binding names `__f0, __f1, ..` used for the fields of a variant.
pub(crate) fn bindings(count: usize) -> Vec<syn::Ident> {
    (0..count).map(|i| format_ident!("__f{}", i)).collect()
}

/// The pattern that destructures a variant into `bindings`.
pub(crate) fn variant_pattern(variant: &VariantModel, bindings: &[syn::Ident]) -> TokenStream {
    let ident = &variant.ident;
    match variant.shape {
        Shape::Unit => quote!(Self::#ident),
        Shape::Tuple => quote!(Self::#ident( #(#bindings),* )),
        Shape::Named => {
            let names = variant.fields.iter().map(|f| f.ident.as_ref().expect("named"));
            quote!(Self::#ident { #(#names: #bindings),* })
        }
    }
}

/// Expands `#[keel::api]` (`Mode::Api`) or `#[keel::error]` (`Mode::Error`) on an enum.
pub(crate) fn expand_enum(
    args_root: Option<Root>,
    mut item: ItemEnum,
    mode: Mode,
) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    let root = item_root(&mut item.attrs, args_root, &mut errors);
    check_generics(&item.generics, &item.ident.to_string(), &mut errors);
    let variants = parse_variants(&mut item, mode, &mut errors);
    if mode == Mode::Error {
        super::error::validate(&variants, &mut errors);
    }
    errors.finish()?;

    let wire = root.wire();
    let meta = root.meta();
    let name = item.ident.clone();
    let name_str = unraw(&name);
    let docs = super::attrs::docs(&item.attrs);
    let meta_static = format_ident!("__KEEL_META_{}", name_str);
    let is_error = mode == Mode::Error;

    // `std::error::Error` needs `Debug`; add the derive unless the user wrote one.
    if is_error && !derives(&item.attrs, "Debug") {
        item.attrs.push(syn::parse_quote!(#[derive(::core::fmt::Debug)]));
    }

    let encode_arms = variants.iter().map(|v| {
        let binds = bindings(v.fields.len());
        let pattern = variant_pattern(v, &binds);
        let index = v.index;
        let encodes = v.fields.iter().zip(&binds).map(|(f, b)| {
            quote_spanned! {f.ty.span()=> #wire::Encode::encode(#b, __w); }
        });
        quote! {
            #pattern => {
                __w.write_u16(#index);
                #(#encodes)*
            }
        }
    });

    let decode_arms = variants.iter().map(|v| {
        let ident = &v.ident;
        let index = v.index;
        let decodes: Vec<TokenStream> = v
            .fields
            .iter()
            .map(|f| {
                let ty = &f.ty;
                let decode = quote_spanned! {f.ty.span()=> <#ty as #wire::Decode>::decode(__r)? };
                match &f.ident {
                    Some(field) => quote!(#field: #decode),
                    None => decode,
                }
            })
            .collect();
        let construct = match v.shape {
            Shape::Unit => quote!(Self::#ident),
            Shape::Tuple => quote!(Self::#ident( #(#decodes),* )),
            Shape::Named => quote!(Self::#ident { #(#decodes),* }),
        };
        quote!(#index => ::core::result::Result::Ok(#construct),)
    });

    let variant_metas = variants.iter().map(|v| {
        let vname = unraw(&v.ident);
        let index = v.index;
        let tuple = v.shape == Shape::Tuple;
        let docs = &v.docs;
        let fields = v
            .fields
            .iter()
            .map(|f| field_meta(&meta, &f.name, &f.kty, f.default, &f.docs));
        let message = match &v.error {
            Some(ErrorAttr::Message { text, .. }) => {
                quote!(::core::option::Option::Some(#text))
            }
            _ => quote!(::core::option::Option::None),
        };
        quote! {
            #meta::VariantMeta {
                name: #vname,
                index: #index,
                fields: &[ #(#fields),* ],
                tuple: #tuple,
                message: #message,
                docs: #docs,
            }
        }
    });

    let extras = if is_error {
        super::error::expand_extras(&root, &name, &variants)
    } else {
        TokenStream::new()
    };
    let derived = derived();
    let registration = submit(&root, "Enum", &meta_static);

    Ok(quote! {
        #item

        impl #name {
            /// The stable Keel type id: `fnv1a32` of the type name.
            pub const KEEL_TYPE_ID: u32 = #meta::ids::type_id(#name_str);
        }

        #derived
        impl #wire::Encode for #name {
            fn encode(&self, __w: &mut #wire::Writer) {
                match self {
                    #(#encode_arms)*
                }
            }
        }

        #derived
        impl #wire::Decode for #name {
            const MIN_ENCODED_LEN: usize = 2;

            fn decode(__r: &mut #wire::Reader<'_>) -> ::core::result::Result<Self, #wire::WireError> {
                let __at = __r.position();
                match __r.read_u16()? {
                    #(#decode_arms)*
                    __tag => ::core::result::Result::Err(#wire::WireError::InvalidTag {
                        tag: ::core::primitive::u32::from(__tag),
                        at: __at,
                        ty: #name_str,
                    }),
                }
            }
        }

        #[allow(non_upper_case_globals)]
        static #meta_static: #meta::EnumMeta = #meta::EnumMeta {
            name: #name_str,
            type_id: #meta::ids::type_id(#name_str),
            is_error: #is_error,
            variants: &[ #(#variant_metas),* ],
            docs: #docs,
        };
        #registration

        #extras
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::has;

    fn expand(src: &str) -> String {
        let item: syn::ItemStruct = syn::parse_str(src).unwrap();
        match expand_struct(None, item) {
            Ok(tokens) => tokens.to_string(),
            Err(err) => err.to_string(),
        }
    }

    #[test]
    fn record_expansion_mentions_the_contract_names() {
        let out = expand(
            "pub struct Todo { pub id: Uuid, pub title: String, #[keel(default)] pub done: bool }",
        );
        for needle in [
            "::keel::wire::Encode for Todo",
            "::keel::wire::Decode for Todo",
            "KEEL_TYPE_ID",
            "__KEEL_META_Todo",
            "::keel::meta::RecordMeta",
            "::keel::meta::inventory::submit!",
            "::keel::meta::Registration::Record",
        ] {
            assert!(has(&out, needle), "missing `{needle}` in {out}");
        }
        assert!(!has(&out, "keel(default)"));
    }

    #[test]
    fn crate_override_is_honoured() {
        let out = expand("#[keel(crate = \"::k\")] struct P { x: i32 }");
        assert!(has(&out, "::k::wire::Encode for P"), "{out}");
        assert!(!has(&out, "::keel::"), "{out}");
    }

    #[test]
    fn tuple_struct_is_e0007() {
        let out = expand("struct P(i32);");
        assert!(
            out.starts_with("error[keel::E0007]: tuple struct `P`"),
            "{out}"
        );
    }

    #[test]
    fn bad_field_types_are_all_reported() {
        let item: syn::ItemStruct = syn::parse_str(
            "struct P { a: &str, b: f64, c: HashMap<f64, u8>, d: Vec<Box<dyn Any>> }",
        )
        .unwrap();
        let err = expand_struct(None, item).unwrap_err();
        let codes: Vec<String> = err
            .into_iter()
            .map(|e| e.to_string().lines().next().unwrap().to_owned())
            .collect();
        assert_eq!(codes.len(), 3, "{codes:?}");
        assert!(codes[0].contains("E0001"));
        assert!(codes[1].contains("E0006"));
        assert!(codes[2].contains("E0012"));
    }

    #[test]
    fn generic_struct_is_e0002() {
        let out = expand("struct P<T> { x: T }");
        assert!(out.starts_with("error[keel::E0002]"), "{out}");
    }

    fn expand_enum_src(src: &str, mode: Mode) -> String {
        let item: syn::ItemEnum = syn::parse_str(src).unwrap();
        match expand_enum(None, item, mode) {
            Ok(tokens) => tokens.to_string(),
            Err(err) => err.to_string(),
        }
    }

    #[test]
    fn enum_expansion_has_variant_indices() {
        let out = expand_enum_src(
            "enum Shape { Empty, Circle { r: f64 }, Rect(f64, f64) }",
            Mode::Api,
        );
        assert!(has(&out, "write_u16(0u16)"), "{out}");
        assert!(has(&out, "write_u16(1u16)"), "{out}");
        assert!(has(&out, "write_u16(2u16)"), "{out}");
        assert!(has(&out, "InvalidTag"), "{out}");
        assert!(has(&out, "is_error: false"), "{out}");
    }

    #[test]
    fn enum_shape_errors() {
        assert!(expand_enum_src("enum E {}", Mode::Api).contains("E0007"));
        assert!(expand_enum_src("enum E { A = 1 }", Mode::Api).contains("explicit discriminant"));
        assert!(expand_enum_src("enum E { A() }", Mode::Api).contains("empty parentheses"));
        assert!(expand_enum_src("enum E { A {} }", Mode::Api).contains("empty braces"));
        assert!(expand_enum_src("enum E<T> { A(T) }", Mode::Api).contains("E0002"));
    }

    #[test]
    fn error_enum_gets_a_debug_derive_unless_present() {
        let out = expand_enum_src("enum E { #[error(\"x\")] A }", Mode::Error);
        assert!(has(&out, "derive(::core::fmt::Debug)"), "{out}");
        let out = expand_enum_src("#[derive(Debug)] enum E { #[error(\"x\")] A }", Mode::Error);
        assert!(!has(&out, "fmt::Debug)"), "{out}");
    }

    #[test]
    fn error_enum_requires_messages() {
        let out = expand_enum_src("enum E { A }", Mode::Error);
        assert!(
            out.starts_with("error[keel::E0010]: variant `A` has no `#[error(..)]`"),
            "{out}"
        );
    }

    #[test]
    fn error_enum_generates_display_error_and_from() {
        let out = expand_enum_src(
            "enum E { #[error(\"io: {0}\")] Io(#[from] IoError), #[error(\"bad {code}\")] Bad { code: u8 }, #[error(transparent)] Other(#[source] Inner), #[error(\"plain\")] Plain }",
            Mode::Error,
        );
        assert!(has(&out, "impl ::core::fmt::Display for E"), "{out}");
        assert!(has(&out, "impl ::std::error::Error for E"), "{out}");
        assert!(
            has(&out, "impl ::core::convert::From<IoError> for E"),
            "{out}"
        );
        assert!(has(&out, "::core::write!(__fmt, \"io: {__f0}\")"), "{out}");
        assert!(
            has(&out, "::core::write!(__fmt, \"bad {__f0}\")"),
            "{out}"
        );
        assert!(
            has(&out, "::core::fmt::Display::fmt(__f0, __fmt)"),
            "{out}"
        );
        assert!(has(&out, "::std::error::Error::source(__f0)"), "{out}");
        assert!(has(&out, "_ => ::core::option::Option::None"), "{out}");
        assert!(
            has(&out, "message: ::core::option::Option::Some(\"io: {0}\")"),
            "{out}"
        );
    }

    #[test]
    fn error_variant_rules() {
        assert!(
            expand_enum_src("enum E { #[error(\"x\")] A(#[from] u8, u8) }", Mode::Error)
                .contains("exactly one field")
        );
        assert!(
            expand_enum_src("enum E { #[error(transparent)] A(u8, u8) }", Mode::Error)
                .contains("needs exactly one field")
        );
        assert!(
            expand_enum_src("enum E { #[error(\"x {1}\")] A(u8) }", Mode::Error)
                .contains("refers to field 1")
        );
        assert!(
            expand_enum_src("enum E { #[error(\"x\", 1)] A }", Mode::Error)
                .contains("single string literal")
        );
        assert!(expand_enum_src("enum E { #[error] A }", Mode::Error).contains("needs a message"));
    }
}
