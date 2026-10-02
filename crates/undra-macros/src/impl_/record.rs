//! Records (`#[undra::api] struct`) and enums (`#[undra::api] enum`, `#[undra::error]`).
//!
//! For a record the macro emits, next to the item as written:
//!
//! * `impl Todo { pub const UNDRA_TYPE_ID: u32 }` (enums also `UNDRA_IS_ERROR`)
//! * `impl Encode` (fields in declaration order) and `impl Decode`
//!   (with an exact `MIN_ENCODED_LEN`, so `Vec<Todo>` decoders can reject impossible counts),
//! * `static __UNDRA_META_Todo: RecordMeta` and its `inventory` registration;
//! * the identity checks of `check.rs` for every field type (a user type called `Bytes` or an
//!   aliased import must not pass for the type the schema names).
//!
//! Enums are the same with a `u16` variant index in front of the variant's fields. A record
//! must have at least one field (E0007): zero-width items defeat length validation (SPEC 3.1).

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::visit_mut::VisitMut;
use syn::{Fields, ItemEnum, ItemStruct};

use super::attrs::{Site, take};
use super::check::Checks;
use super::common::{
    GenericOn, check_generics_on, derived, derives, field_meta, item_root, submit,
};
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

/// Whether an enum is a plain `#[undra::api]` enum or a `#[undra::error]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Api,
    Error,
}

/// What to do with `#[error]`, `#[from]` and `#[source]` (the helper attributes of
/// `#[undra::error]`) on an item that is not an error enum.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Helpers {
    /// They are not ours to judge: another derive (`thiserror::Error`) may own them.
    Ignore,
    /// They are reported (E0010) and dropped: nothing would accept them, and `rustc` would only
    /// say "cannot find attribute".
    Report,
}

impl Helpers {
    /// A plain enum that derives no `Error` reports them; structs and other derives leave them.
    fn of_enum(item: &ItemEnum, mode: Mode) -> Helpers {
        if mode == Mode::Api && !derives(&item.attrs, "Error") {
            Helpers::Report
        } else {
            Helpers::Ignore
        }
    }
}

/// What a record, enum or error that failed to expand still provides, so the error is the only
/// one. Everything that uses the type (`fn f(t: Todo)`, `Vec<Todo>`, a `Result<_, TodoError>`)
/// needs its `Encode`, `Decode`, `UNDRA_TYPE_ID` and, for an error, `Display` and `Error`; without
/// them every use adds "cannot cross the boundary" and identity-check errors on top of the real
/// one. The stubs carry no behaviour: the build stops on the real error first.
pub(crate) fn recover(args_root: Option<Root>, mode: Mode, item: &mut syn::Item) -> TokenStream {
    let (name, generics, is_enum) = match item {
        syn::Item::Struct(item) => (item.ident.clone(), item.generics.clone(), false),
        syn::Item::Enum(item) => (item.ident.clone(), item.generics.clone(), true),
        _ => return TokenStream::new(),
    };
    // A field written `&str` is rejected as E0001 ("use an owned `String`"); left in the item, it
    // would also be `rustc`'s "missing lifetime specifier", whose advice (introduce a lifetime)
    // contradicts it. The item only has to type-check here, so the borrow gets one.
    StaticRefs.visit_item_mut(item);
    let attrs = match item {
        syn::Item::Struct(item) => &mut item.attrs,
        syn::Item::Enum(item) => &mut item.attrs,
        _ => return TokenStream::new(),
    };
    let root = item_root(&mut attrs.clone(), args_root, &mut Errors::new());
    // A generic error enum is E0002 and cannot be spelled anywhere: no `Error` impl to follow it.
    let is_error = mode == Mode::Error && is_enum && generics.params.is_empty();
    if is_error && !derives(attrs, "Debug") {
        // `std::error::Error` needs `Debug`; the expansion adds the derive, so the fallback does.
        attrs.push(syn::parse_quote!(#[derive(::core::fmt::Debug)]));
    }
    let wire = root.wire();
    let meta = root.meta();
    let name_str = unraw(&name);
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    let is_error_const = if is_enum {
        quote! {
            #[doc(hidden)]
            pub const UNDRA_IS_ERROR: bool = #is_error;
        }
    } else {
        TokenStream::new()
    };
    // The field names of a struct, so a keyed list of it does not add "no such field".
    let field_names: Vec<String> = match item {
        syn::Item::Struct(item) => match &item.fields {
            Fields::Named(named) => named
                .named
                .iter()
                .filter_map(|f| f.ident.as_ref().map(unraw))
                .collect(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    };
    let fields_stub = if is_enum {
        TokenStream::new()
    } else {
        quote! {
            #[doc(hidden)]
            pub const __UNDRA_FIELDS: &'static [&'static str] = &[ #(#field_names),* ];
        }
    };
    let error_impls = if is_error {
        quote! {
            impl #impl_generics ::core::fmt::Display for #name #type_generics #where_clause {
                fn fmt(&self, __fmt: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                    ::core::result::Result::Ok(())
                }
            }
            impl #impl_generics ::std::error::Error for #name #type_generics #where_clause {}
        }
    } else {
        TokenStream::new()
    };
    // A failed type is not a map key worth a second error either.
    quote! {
        #[allow(dead_code)]
        impl #impl_generics #name #type_generics #where_clause {
            #[doc(hidden)]
            pub const UNDRA_TYPE_ID: u32 = #meta::ids::type_id(#name_str);
            #is_error_const
            #fields_stub
        }
        impl #impl_generics #wire::leaf::MapKey for #name #type_generics #where_clause {}
        impl #impl_generics #wire::Encode for #name #type_generics #where_clause {
            fn encode(&self, __w: &mut #wire::Writer) {}
        }
        impl #impl_generics #wire::Decode for #name #type_generics #where_clause {
            const MIN_ENCODED_LEN: usize = 0;
            fn decode(__r: &mut #wire::Reader<'_>) -> ::core::result::Result<Self, #wire::WireError> {
                ::core::result::Result::Err(#wire::WireError::InvalidTag {
                    tag: 0,
                    at: __r.position(),
                    ty: #name_str,
                })
            }
        }
        #error_impls
    }
}

/// Gives every reference without a lifetime `'static` (see [`recover`]).
struct StaticRefs;

impl syn::visit_mut::VisitMut for StaticRefs {
    fn visit_type_reference_mut(&mut self, node: &mut syn::TypeReference) {
        if node.lifetime.is_none() {
            node.lifetime = Some(syn::parse_quote!('static));
        }
        syn::visit_mut::visit_type_reference_mut(self, node);
    }
}

/// Whether the fallback of a failed `#[undra::api]` on `item` drops `#[error]`, `#[from]` and
/// `#[source]`: only on a plain enum that reports them (see [`Helpers`]).
pub(crate) fn drops_error_helpers(item: &syn::Item) -> bool {
    matches!(item, syn::Item::Enum(item) if Helpers::of_enum(item, Mode::Api) == Helpers::Report)
}

fn parse_fields<'a>(
    fields: impl Iterator<Item = &'a mut syn::Field>,
    self_name: &str,
    mode: Mode,
    helpers: Helpers,
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
        } else if helpers == Helpers::Report {
            let (from, source) = take_field_attrs(&mut field.attrs);
            if from || source {
                errors.push(error_attribute_misplaced(
                    &format!("the field `{name}`"),
                    if from { "from" } else { "source" },
                    &field.ty,
                ));
            }
            (false, false)
        } else {
            (false, false)
        };
        let kty = match map_field(&field.ty, &name, self_name) {
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

/// `#[error]`, `#[from]` or `#[source]` on a plain `#[undra::api]` enum: rustc would say "cannot
/// find attribute", which does not say where the attribute lives.
fn error_attribute_misplaced(
    what: &str,
    attribute: &str,
    node: &impl quote::ToTokens,
) -> syn::Error {
    Diag::new(
        code::E0010,
        format!("`#[{attribute}]` on {what} belongs to `#[undra::error]`, not `#[undra::api]`"),
        "`#[error(..)]`, `#[from]` and `#[source]` are the helper attributes of error enums; a plain `#[undra::api]` enum has no messages, `Display` or `From` impls",
        "change the enum's attribute to `#[undra::error]`, or remove the helper attribute",
    )
    .on(node)
}

// ---------------------------------------------------------------------------------------------
// How a struct or an enum is expanded
// ---------------------------------------------------------------------------------------------

/// How a struct or an enum is expanded.
#[derive(Clone, Debug)]
pub(crate) enum Expand {
    /// An ordinary record, enum or error: the item, its codecs, its registration and its checks.
    Plain,
    /// A generic template (`#[undra::api(generic)]`, ADR-042): the item, codecs generic over its
    /// type parameters and the hidden `macro_rules!` that instantiates it. Nothing is registered:
    /// a template has no wire identity of its own.
    Template,
    /// One instantiation (`#[undra::api] pub type TodoPage = Page<Todo>;`): no item and no codecs
    /// (the template's serve), but the registration of `TodoPage`, the inherent constants that let
    /// a signature spell the alias, and the checks of the type arguments.
    Instance(Instance),
}

/// What an [`Expand::Instance`] knows about where it comes from.
#[derive(Clone, Debug)]
pub(crate) struct Instance {
    /// The docs of the record: the alias's own, else the template's.
    pub(crate) docs: String,
    /// The template's name (`Page`), for the E0070 constant.
    pub(crate) template: String,
}

/// What the fields of a struct make it.
enum Body {
    /// `struct Todo { .. }`: a record.
    Record(Vec<FieldModel>),
    /// `struct UserId(pub Uuid);`: a transparent record of one field named `value` (ADR-042).
    Newtype(Box<FieldModel>),
}

impl Body {
    fn fields(&self) -> &[FieldModel] {
        match self {
            Body::Record(fields) => fields,
            Body::Newtype(field) => std::slice::from_ref(&**field),
        }
    }
}

/// The two fixes of a struct that has the wrong shape (E0007).
const SHAPE_HELP: &str = "one field: a newtype, `struct Meters(pub f64);` (it crosses as the `f64`); several: named fields, `struct Pair { first: A, second: B }`; for a marker with no data, an enum with a unit variant";

/// The reason a record needs a field (SPEC 3.1).
const ZERO_WIDTH: &str = "a record without fields occupies zero bytes on the wire, and zero-width items defeat length validation (SPEC section 3.1): a `Vec` of them would accept any count from a four-byte message";

/// The one field of a newtype.
fn parse_newtype_field(field: &mut syn::Field, self_name: &str, errors: &mut Errors) -> FieldModel {
    take(&mut field.attrs, Site::NOTHING, errors);
    let kty = match map_field(&field.ty, "value", self_name) {
        Ok(kty) => kty,
        Err(err) => {
            // The help of a trait object in a field names a field; a newtype has none.
            let err = if err.diag.code == code::E0012 {
                let diag = Diag::new(
                    code::E0012,
                    err.diag.what.clone(),
                    err.diag.why.clone(),
                    "make the type this newtype wraps a concrete `#[undra::api]` type, or an enum listing the cases you need",
                );
                err.with_diag(diag)
            } else {
                err
            };
            errors.push(err.into_error());
            KType::Unit
        }
    };
    FieldModel {
        ident: None,
        name: "value".to_owned(),
        ty: field.ty.clone(),
        kty,
        default: false,
        docs: super::attrs::docs(&field.attrs),
        from: false,
        source: false,
    }
}

/// The fields of a struct, or the E0007 for a shape that is neither a record nor a newtype.
fn parse_struct_body(item: &mut ItemStruct, errors: &mut Errors) -> Body {
    let self_name = unraw(&item.ident);
    match &mut item.fields {
        Fields::Named(named) => {
            if named.named.is_empty() {
                errors.push(item_shape(
                    &format!("record `{}` has no fields", item.ident),
                    &item.ident,
                    ZERO_WIDTH,
                    "add a field, or use an enum with a unit variant if you need a marker",
                ));
            }
            Body::Record(parse_fields(
                named.named.iter_mut(),
                &self_name,
                Mode::Api,
                Helpers::Ignore,
                errors,
            ))
        }
        Fields::Unnamed(unnamed) if unnamed.unnamed.len() == 1 => Body::Newtype(Box::new(
            parse_newtype_field(&mut unnamed.unnamed[0], &self_name, errors),
        )),
        Fields::Unnamed(unnamed) if unnamed.unnamed.is_empty() => {
            errors.push(item_shape(
                &format!("tuple struct `{}` has no fields", item.ident),
                &item.ident,
                ZERO_WIDTH,
                SHAPE_HELP,
            ));
            Body::Record(Vec::new())
        }
        Fields::Unnamed(unnamed) => {
            errors.push(item_shape(
                &format!(
                    "tuple struct `{}` with {} fields cannot be a record",
                    item.ident,
                    unnamed.unnamed.len()
                ),
                unnamed,
                "the schema names every field so the other languages can generate properties, and tuple fields have no names; only a tuple struct with exactly one field is a newtype, which crosses as the type it wraps",
                SHAPE_HELP,
            ));
            Body::Record(Vec::new())
        }
        Fields::Unit => {
            errors.push(item_shape(
                &format!("unit struct `{}` cannot be a record", item.ident),
                &item.ident,
                ZERO_WIDTH,
                SHAPE_HELP,
            ));
            Body::Record(Vec::new())
        }
    }
}

/// `impl<..> Trait for Name<..>` headers of a template: the declared parameters with `bound`
/// added to each type parameter.
fn bounded(generics: &syn::Generics, bound: &TokenStream) -> syn::Generics {
    let mut generics = generics.clone();
    for param in generics.type_params_mut() {
        let bound: syn::TypeParamBound =
            syn::parse2(bound.clone()).expect("a path is a type parameter bound");
        param.bounds.push(bound);
    }
    generics
}

// ---------------------------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------------------------

/// Expands `#[undra::api]` on a struct.
pub(crate) fn expand_struct(args_root: Option<Root>, item: ItemStruct) -> syn::Result<TokenStream> {
    expand_struct_as(args_root, item, Expand::Plain)
}

/// Expands a struct as a record, a generic template or an instantiation (see [`Expand`]).
pub(crate) fn expand_struct_as(
    args_root: Option<Root>,
    mut item: ItemStruct,
    expand: Expand,
) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    let root = item_root(&mut item.attrs, args_root, &mut errors);
    let template = matches!(expand, Expand::Template);
    let params = type_param_names(&item.generics);
    match &expand {
        Expand::Plain => check_generics_on(
            &item.generics,
            &item.ident.to_string(),
            GenericOn::Data,
            &mut errors,
        ),
        Expand::Template => super::generic::check_template_generics(
            &item.generics,
            &item.ident.to_string(),
            &mut errors,
        ),
        Expand::Instance(_) => {}
    }
    // What a template's macro carries: the item as written, helpers and all.
    let definition = template.then(|| item.clone());
    let body = parse_struct_body(&mut item, &mut errors);
    if template {
        super::generic::check_parameter_use(body.fields(), &params, &mut errors);
    }
    errors.finish()?;

    let wire = root.wire();
    let meta = root.meta();
    let name = &item.ident;
    let name_str = unraw(name);
    let docs = match &expand {
        Expand::Instance(instance) => instance.docs.clone(),
        _ => super::attrs::docs(&item.attrs),
    };
    let meta_static = format_ident!("__UNDRA_META_{}", name_str);
    let fields = body.fields();
    let newtype = matches!(body, Body::Newtype(_));

    let mut checks = match &expand {
        Expand::Plain => Checks::for_type(name),
        Expand::Template => Checks::for_template(name, params.clone()),
        Expand::Instance(_) => Checks::for_instance(name),
    };
    for f in fields {
        checks.ty(&f.ty, &f.kty);
    }
    let checks = checks.emit(&root);

    // The registration of a record and of an instantiation (a template has none).
    let field_metas = fields
        .iter()
        .map(|f| field_meta(&meta, &f.name, &f.kty, f.default, &f.docs));
    let registration = submit(&root, "Record", &meta_static);
    let registered = quote! {
        #[allow(non_upper_case_globals)]
        static #meta_static: #meta::RecordMeta = #meta::RecordMeta {
            name: #name_str,
            type_id: #meta::ids::type_id(#name_str),
            fields: &[ #(#field_metas),* ],
            transparent: #newtype,
            docs: #docs,
        };
        #registration
    };

    // What a keyed list (`#[undra(key = "..")]` in a store) needs of its item: the field names.
    // The store looks the key up in this constant, so a key that names no field is a branded
    // error listing these names, and only then reads the field (`undra_meta::keys`). A newtype
    // has no named field to key by.
    let field_names: Vec<&String> = match &body {
        Body::Record(fields) => fields.iter().map(|f| &f.name).collect(),
        Body::Newtype(_) => Vec::new(),
    };
    // On the name's span, so that `rustc` points at the type (or, for an instantiation, at its
    // alias) when it reports one of these defined twice.
    let constants = quote_spanned! {name.span()=>
        /// The stable Undra type id: `fnv1a32` of the type name.
        pub const UNDRA_TYPE_ID: u32 = #meta::ids::type_id(#name_str);
        /// The names of the fields, in declaration order (see `undra_meta::keys`).
        #[doc(hidden)]
        pub const __UNDRA_FIELDS: &'static [&'static str] = &[ #(#field_names),* ];
    };

    match expand {
        Expand::Plain => {
            let codecs = struct_codecs(&wire, &Header::plain(name), fields, newtype);
            let newtype_impl = if newtype {
                newtype_impl(&wire, &Header::plain(name), fields[0].ty.to_token_stream())
            } else {
                TokenStream::new()
            };
            Ok(quote! {
                #item

                impl #name {
                    #constants
                }

                #codecs
                #newtype_impl
                #registered
                #checks
            })
        }
        Expand::Template => {
            let encode_generics = bounded(&item.generics, &quote!(#wire::Encode));
            let decode_generics = bounded(&item.generics, &quote!(#wire::Decode));
            let header = Header::template(name, &item.generics, &encode_generics, &decode_generics);
            let codecs = struct_codecs(&wire, &header, fields, newtype);
            let newtype_impl = if newtype {
                newtype_impl(&wire, &header, fields[0].ty.to_token_stream())
            } else {
                TokenStream::new()
            };
            let definition = definition.expect("a template keeps its definition");
            let instantiator = super::generic::template_macro(
                &root,
                &syn::Item::Struct(definition),
                &params,
                &docs,
            );
            Ok(quote! {
                #item

                #codecs
                #newtype_impl
                #checks
                #instantiator
            })
        }
        Expand::Instance(instance) => {
            let rule = super::generic::duplicate_alias_constant(&instance.template, name.span());
            let type_name = unraw(name);
            Ok(quote_spanned! {name.span()=>
                impl #name {
                    #rule
                    #constants
                    /// The declared name of the instantiation, for a signature that names it
                    /// through a generic application (`Page<T>` in a generic function).
                    #[doc(hidden)]
                    pub const UNDRA_TYPE_NAME: &'static str = #type_name;
                }

                #registered
                #checks
            })
        }
    }
}

/// The names of the type parameters of `generics`.
pub(crate) fn type_param_names(generics: &syn::Generics) -> Vec<String> {
    generics.type_params().map(|p| unraw(&p.ident)).collect()
}

/// Where a pair of codec impls goes: `impl Encode for Todo`, or, for a template,
/// `impl<T: Encode> Encode for Page<T>`.
struct Header {
    /// The type the impls are for: `Todo`, `Page<T>`.
    ty: TokenStream,
    /// The generics of `impl<..> Encode`, with their bounds.
    encode: TokenStream,
    /// The generics of `impl<..> Decode`, with their bounds.
    decode: TokenStream,
    /// The generics of any other `impl<..>` (the declared ones).
    plain: TokenStream,
}

impl Header {
    fn plain(name: &syn::Ident) -> Header {
        Header {
            ty: quote!(#name),
            encode: TokenStream::new(),
            decode: TokenStream::new(),
            plain: TokenStream::new(),
        }
    }

    fn template(
        name: &syn::Ident,
        declared: &syn::Generics,
        encode: &syn::Generics,
        decode: &syn::Generics,
    ) -> Header {
        let (_, ty_generics, _) = declared.split_for_impl();
        let (declared_impl, _, _) = declared.split_for_impl();
        let (encode_impl, _, _) = encode.split_for_impl();
        let (decode_impl, _, _) = decode.split_for_impl();
        Header {
            ty: quote!(#name #ty_generics),
            encode: quote!(#encode_impl),
            decode: quote!(#decode_impl),
            plain: quote!(#declared_impl),
        }
    }
}

/// `impl Newtype for UserId`: what `MapKey` reads to decide whether the newtype is a key.
fn newtype_impl(wire: &TokenStream, header: &Header, inner: TokenStream) -> TokenStream {
    let ty = &header.ty;
    let generics = &header.plain;
    quote! {
        #[automatically_derived]
        impl #generics #wire::leaf::Newtype for #ty {
            type Inner = #inner;
        }
    }
}

/// `Encode` and `Decode` of a record: its fields in order, or, for a newtype, its one field.
fn struct_codecs(
    wire: &TokenStream,
    header: &Header,
    fields: &[FieldModel],
    newtype: bool,
) -> TokenStream {
    let derived = derived();
    let ty_name = &header.ty;
    let encode_generics = &header.encode;
    let decode_generics = &header.decode;
    let (encode_fields, decode_construct, min_len) = if newtype {
        let field = &fields[0];
        let inner = &field.ty;
        (
            vec![quote_spanned! {inner.span()=> #wire::Encode::encode(&self.0, __w); }],
            quote_spanned! {inner.span()=> Self(<#inner as #wire::Decode>::decode(__r)?) },
            quote_spanned! {inner.span()=> <#inner as #wire::Decode>::MIN_ENCODED_LEN },
        )
    } else {
        let encode_fields: Vec<TokenStream> = fields
            .iter()
            .map(|f| {
                let ident = f.ident.as_ref().expect("named field");
                quote_spanned! {f.ty.span()=> #wire::Encode::encode(&self.#ident, __w); }
            })
            .collect();
        let decode_fields = fields.iter().map(|f| {
            let ident = f.ident.as_ref().expect("named field");
            let ty = &f.ty;
            quote_spanned! {f.ty.span()=> #ident: <#ty as #wire::Decode>::decode(__r)? }
        });
        let min_len = fields.iter().map(|f| {
            let ty = &f.ty;
            quote_spanned! {f.ty.span()=> + <#ty as #wire::Decode>::MIN_ENCODED_LEN }
        });
        (
            encode_fields,
            quote! { Self { #(#decode_fields),* } },
            quote! { 0usize #(#min_len)* },
        )
    };
    quote! {
        #derived
        impl #encode_generics #wire::Encode for #ty_name {
            #[allow(unused_variables)]
            fn encode(&self, __w: &mut #wire::Writer) {
                #(#encode_fields)*
            }
        }

        #derived
        impl #decode_generics #wire::Decode for #ty_name {
            const MIN_ENCODED_LEN: usize = #min_len;

            #[allow(unused_variables)]
            fn decode(__r: &mut #wire::Reader<'_>) -> ::core::result::Result<Self, #wire::WireError> {
                ::core::result::Result::Ok(#decode_construct)
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Enums and errors
// ---------------------------------------------------------------------------------------------

fn parse_variants(item: &mut ItemEnum, mode: Mode, errors: &mut Errors) -> Vec<VariantModel> {
    let enum_name = item.ident.to_string();
    let helpers = Helpers::of_enum(item, mode);
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
                    parse_fields(
                        unnamed.unnamed.iter_mut(),
                        &enum_name,
                        mode,
                        helpers,
                        errors,
                    ),
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
                    parse_fields(named.named.iter_mut(), &enum_name, mode, helpers, errors),
                )
            }
        };
        let error = if mode == Mode::Error {
            take_message(&mut variant.attrs, &variant.ident, shape, &fields, errors)
        } else if helpers == Helpers::Report {
            let (misplaced, kept): (Vec<syn::Attribute>, Vec<syn::Attribute>) =
                std::mem::take(&mut variant.attrs)
                    .into_iter()
                    .partition(|attr| attr.path().is_ident("error"));
            variant.attrs = kept;
            for attr in &misplaced {
                errors.push(error_attribute_misplaced(
                    &format!("the variant `{}`", variant.ident),
                    "error",
                    attr,
                ));
            }
            None
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
            let names = variant
                .fields
                .iter()
                .map(|f| f.ident.as_ref().expect("named"));
            quote!(Self::#ident { #(#names: #bindings),* })
        }
    }
}

/// Expands `#[undra::api]` (`Mode::Api`) or `#[undra::error]` (`Mode::Error`) on an enum.
pub(crate) fn expand_enum(
    args_root: Option<Root>,
    item: ItemEnum,
    mode: Mode,
) -> syn::Result<TokenStream> {
    expand_enum_as(args_root, item, mode, Expand::Plain)
}

/// Expands an enum as an enum, a generic template or an instantiation (see [`Expand`]).
pub(crate) fn expand_enum_as(
    args_root: Option<Root>,
    mut item: ItemEnum,
    mode: Mode,
    expand: Expand,
) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    let root = item_root(&mut item.attrs, args_root, &mut errors);
    let template = matches!(expand, Expand::Template);
    let params = type_param_names(&item.generics);
    match &expand {
        Expand::Plain => check_generics_on(
            &item.generics,
            &item.ident.to_string(),
            if mode == Mode::Error {
                GenericOn::Error
            } else {
                GenericOn::Data
            },
            &mut errors,
        ),
        Expand::Template => super::generic::check_template_generics(
            &item.generics,
            &item.ident.to_string(),
            &mut errors,
        ),
        Expand::Instance(_) => {}
    }
    let definition = template.then(|| item.clone());
    let variants = parse_variants(&mut item, mode, &mut errors);
    if mode == Mode::Error {
        super::error::validate(&variants, &mut errors);
    }
    if template {
        let fields: Vec<&FieldModel> = variants.iter().flat_map(|v| &v.fields).collect();
        super::generic::check_parameter_use(fields, &params, &mut errors);
    }
    errors.finish()?;

    let wire = root.wire();
    let meta = root.meta();
    let name = item.ident.clone();
    let name_str = unraw(&name);
    let docs = match &expand {
        Expand::Instance(instance) => instance.docs.clone(),
        _ => super::attrs::docs(&item.attrs),
    };
    let meta_static = format_ident!("__UNDRA_META_{}", name_str);
    let is_error = mode == Mode::Error;

    // `std::error::Error` needs `Debug`; add the derive unless the user wrote one.
    if is_error && !derives(&item.attrs, "Debug") {
        item.attrs
            .push(syn::parse_quote!(#[derive(::core::fmt::Debug)]));
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
    let mut checks = match &expand {
        Expand::Plain => Checks::for_type(&name),
        Expand::Template => Checks::for_template(&name, params.clone()),
        Expand::Instance(_) => Checks::for_instance(&name),
    };
    for f in variants.iter().flat_map(|v| &v.fields) {
        checks.ty(&f.ty, &f.kty);
    }
    let checks = checks.emit(&root);

    let constants = quote_spanned! {name.span()=>
        /// The stable Undra type id: `fnv1a32` of the type name.
        pub const UNDRA_TYPE_ID: u32 = #meta::ids::type_id(#name_str);
        /// Whether this is a `#[undra::error]` enum (what a `Result` may throw).
        #[doc(hidden)]
        pub const UNDRA_IS_ERROR: bool = #is_error;
    };
    let registered = quote! {
        #[allow(non_upper_case_globals)]
        static #meta_static: #meta::EnumMeta = #meta::EnumMeta {
            name: #name_str,
            type_id: #meta::ids::type_id(#name_str),
            is_error: #is_error,
            variants: &[ #(#variant_metas),* ],
            docs: #docs,
        };
        #registration
    };

    let codecs = |header: &Header| {
        let ty_name = &header.ty;
        let encode_generics = &header.encode;
        let decode_generics = &header.decode;
        quote! {
            #derived
            impl #encode_generics #wire::Encode for #ty_name {
                fn encode(&self, __w: &mut #wire::Writer) {
                    match self {
                        #(#encode_arms)*
                    }
                }
            }

            #derived
            impl #decode_generics #wire::Decode for #ty_name {
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
        }
    };

    match expand {
        Expand::Plain => {
            let codecs = codecs(&Header::plain(&name));
            Ok(quote! {
                #item

                impl #name {
                    #constants
                }

                #codecs

                #registered

                #extras

                #checks
            })
        }
        Expand::Template => {
            let encode_generics = bounded(&item.generics, &quote!(#wire::Encode));
            let decode_generics = bounded(&item.generics, &quote!(#wire::Decode));
            let header =
                Header::template(&name, &item.generics, &encode_generics, &decode_generics);
            let codecs = codecs(&header);
            let definition = definition.expect("a template keeps its definition");
            let instantiator =
                super::generic::template_macro(&root, &syn::Item::Enum(definition), &params, &docs);
            Ok(quote! {
                #item

                #codecs

                #checks

                #instantiator
            })
        }
        Expand::Instance(instance) => {
            let rule = super::generic::duplicate_alias_constant(&instance.template, name.span());
            let type_name = unraw(&name);
            Ok(quote_spanned! {name.span()=>
                impl #name {
                    #rule
                    #constants
                    /// The declared name of the instantiation, for a signature that names it
                    /// through a generic application (`Page<T>` in a generic function).
                    #[doc(hidden)]
                    pub const UNDRA_TYPE_NAME: &'static str = #type_name;
                }

                #registered

                #checks
            })
        }
    }
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
            "pub struct Todo { pub id: Uuid, pub title: String, #[undra(default)] pub done: bool }",
        );
        for needle in [
            "::undra::wire::Encode for Todo",
            "::undra::wire::Decode for Todo",
            "UNDRA_TYPE_ID",
            "__UNDRA_META_Todo",
            "::undra::meta::RecordMeta",
            "::undra::meta::inventory::submit!",
            "::undra::meta::Registration::Record",
        ] {
            assert!(has(&out, needle), "missing `{needle}` in {out}");
        }
        assert!(!has(&out, "undra(default)"));
    }

    #[test]
    fn crate_override_is_honoured() {
        let out = expand("#[undra(crate = \"::k\")] struct P { x: i32 }");
        assert!(has(&out, "::k::wire::Encode for P"), "{out}");
        assert!(!has(&out, "::undra::"), "{out}");
    }

    #[test]
    fn a_tuple_struct_of_one_field_is_a_newtype() {
        let out = expand("pub struct UserId(pub Uuid);");
        for needle in [
            "::undra::wire::Encode::encode(&self.0, __w);",
            "::core::result::Result::Ok(Self(<Uuid as ::undra::wire::Decode>::decode(__r)?))",
            "const MIN_ENCODED_LEN: usize = <Uuid as ::undra::wire::Decode>::MIN_ENCODED_LEN;",
            "pub const UNDRA_TYPE_ID: u32 = ::undra::meta::ids::type_id(\"UserId\")",
            "transparent: true",
            "name: \"value\"",
            "ty: ::undra::meta::TypeRefMeta::Uuid",
            "impl ::undra::wire::leaf::Newtype for UserId { type Inner = Uuid; }",
            "::undra::meta::Registration::Record",
        ] {
            assert!(has(&out, needle), "missing `{needle}` in {out}");
        }
        // No `From`, `Deref` or accessor: the author's API decisions.
        for absent in ["impl From", "Deref", "fn value"] {
            assert!(!has(&out, absent), "{absent} in {out}");
        }
    }

    #[test]
    fn a_newtype_of_a_non_leaf_keeps_its_inner_type_checks() {
        let out = expand("struct Wrapped(Vec<Option<UserId>>);");
        assert!(
            has(&out, "<crate::UserId>") || has(&out, "let __undra_id = <UserId>::UNDRA_TYPE_ID;"),
            "{out}"
        );
        assert!(has(&out, "type Inner = Vec<Option<UserId>>;"), "{out}");
    }

    #[test]
    fn newtype_inner_types_follow_the_field_rules() {
        for (src, code) in [
            ("struct N(());", "E0001"),
            ("struct N(&str);", "E0001"),
            ("struct N(Option<Option<u8>>);", "E0063"),
            ("struct N(Box<dyn Any>);", "E0012"),
            ("struct N(Lazy<u8>);", "E0001"),
            ("struct N(Signal<u8>);", "E0001"),
            ("struct N(Arc<dyn Listener>);", "E0004"),
            ("struct N(Arc<Calculator>);", "E0064"),
            ("struct N(Vec<Option<Option<u8>>>);", "E0063"),
            ("struct N(HashMap<f64, u8>);", "E0006"),
        ] {
            let out = expand(src);
            assert!(
                out.contains(&format!("error[undra::{code}]")),
                "{src}: {out}"
            );
        }
        assert!(expand("struct N(Box<dyn Any>);").contains("make the type this newtype wraps"));
    }

    #[test]
    fn a_unit_struct_or_a_wide_tuple_struct_names_both_fixes() {
        for src in [
            "struct P;",
            "struct P();",
            "struct P(i32, i32);",
            "struct P(i32, i32, u8);",
        ] {
            let out = expand(src);
            assert!(out.starts_with("error[undra::E0007]"), "{src}: {out}");
            assert!(
                out.contains("one field: a newtype, `struct Meters(pub f64);`")
                    && out.contains("several: named fields"),
                "{src}: {out}"
            );
        }
        assert!(
            expand("struct P(i32, i32);")
                .contains("tuple struct `P` with 2 fields cannot be a record")
        );
        assert!(expand("struct P;").contains("unit struct `P` cannot be a record"));
    }

    #[test]
    fn a_newtype_field_takes_no_undra_options() {
        let out = expand("struct N(#[undra(default)] u8);");
        assert!(out.contains("E0008"), "{out}");
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
    fn generic_struct_is_e0002_and_points_at_the_template() {
        let out = expand("struct P<T> { x: T }");
        assert!(out.starts_with("error[undra::E0002]"), "{out}");
        assert!(out.contains("#[undra::api(generic)]"), "{out}");
        assert!(out.contains("pub type TodoPage = Page<Todo>;"), "{out}");
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
            out.starts_with("error[undra::E0010]: variant `A` has no `#[error(..)]`"),
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
        assert!(has(&out, "::core::write!(__fmt, \"bad {__f0}\")"), "{out}");
        assert!(has(&out, "::core::fmt::Display::fmt(__f0, __fmt)"), "{out}");
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
