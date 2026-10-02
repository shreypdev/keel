//! Maps Rust type syntax to the schema's [`KType`] (SPEC 2.1) and rejects everything that
//! has no wire representation (diagnostics E0001 to E0006 and E0012).
//!
//! The mapper works on syntax only: a path it does not know is assumed to name a type
//! declared with `#[undra::api]` and becomes [`KType::Named`] with the last path segment.
//! Whether that name really resolves is checked by `undra-bindgen` (`Schema::validate`);
//! whether the type really implements `Encode`/`Decode` is checked by `rustc` on the
//! generated code, whose errors point at the offending field. That a spelling really *is*
//! the type the schema names (a user type called `Bytes`, `use a::Item as Todo`) cannot be
//! seen from syntax: `check.rs` emits compile-time assertions for it (E0060, E0061).
//!
//! | Rust | `KType` |
//! |---|---|
//! | `bool`, `i8`..`i64`, `u8`..`u64`, `f32`, `f64` | the primitive |
//! | `String` | `String` |
//! | `Bytes` | `Bytes` |
//! | `Vec<T>` | `Vec(T)` |
//! | `Option<T>` | `Option(T)` |
//! | `HashMap<K, V>`, `BTreeMap<K, V>` | `Map(K, V)` |
//! | `Duration`, `Timestamp`, `Uuid` | the same-named variants |
//! | `Box<T>` | `T` (transparent, so recursive types can be written) |
//! | `Arc<T>`, `&T`, `Option<&T>`, `Option<Arc<T>>`, `Vec<Arc<T>>` as a method, constructor or function parameter; `Arc<T>`, `Option<Arc<T>>`, `Vec<Arc<T>>` as a method or function return (ADR-040) | `Object("T")` (and `Option`/`Vec` of it) |
//! | `Arc<dyn Trait>`, `Option<Arc<dyn Trait>>` as a method, constructor or function parameter (ADR-041) | `Callback("Trait")` (and `Option` of it) |
//! | `Option<Option<T>>` | rejected (E0063): Kotlin and TypeScript cannot tell `Some(None)` from `None` |
//! | `Handle` | rejected (E0001): handles are how objects cross, not a value type |
//! | `()` | `Unit` (return types only) |
//! | `Result<T, E>` | `Result(T, E)` (return types only) |
//! | `impl Stream<Item = T>` | `Stream(T)` (return types only) |
//! | any other path `a::b::Name` | `Named("Name")` |

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, quote};
use syn::{GenericArgument, PathArguments, ReturnType, Type, TypeParamBound};

use super::diag::{Diag, code};

/// The schema type of a value, in the macro's own representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum KType {
    Bool,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    F32,
    F64,
    String,
    Bytes,
    Unit,
    Duration,
    Timestamp,
    Uuid,
    Decimal,
    Option(Box<KType>),
    Vec(Box<KType>),
    Map(Box<KType>, Box<KType>),
    Named(std::string::String),
    Result(Box<KType>, Box<KType>),
    Stream(Box<KType>),
    /// An object crossing as a parameter or a return (ADR-040).
    Object(std::string::String),
    /// A host callback interface passed in as a parameter (ADR-041).
    Callback(std::string::String),
}

impl KType {
    /// The `TypeRefMeta` constant expression for this type. `meta` is the `#root::meta` path.
    pub(crate) fn meta(&self, meta: &TokenStream) -> TokenStream {
        let leaf = |name: &str| {
            let ident = syn::Ident::new(name, Span::call_site());
            quote!(#meta::TypeRefMeta::#ident)
        };
        match self {
            KType::Bool => leaf("Bool"),
            KType::I8 => leaf("I8"),
            KType::I16 => leaf("I16"),
            KType::I32 => leaf("I32"),
            KType::I64 => leaf("I64"),
            KType::U8 => leaf("U8"),
            KType::U16 => leaf("U16"),
            KType::U32 => leaf("U32"),
            KType::U64 => leaf("U64"),
            KType::F32 => leaf("F32"),
            KType::F64 => leaf("F64"),
            KType::String => leaf("String"),
            KType::Bytes => leaf("Bytes"),
            KType::Unit => leaf("Unit"),
            KType::Duration => leaf("Duration"),
            KType::Timestamp => leaf("Timestamp"),
            KType::Uuid => leaf("Uuid"),
            KType::Decimal => leaf("Decimal"),
            KType::Option(inner) => {
                let inner = inner.meta(meta);
                quote!(#meta::TypeRefMeta::Option(&#inner))
            }
            KType::Vec(inner) => {
                let inner = inner.meta(meta);
                quote!(#meta::TypeRefMeta::Vec(&#inner))
            }
            KType::Stream(inner) => {
                let inner = inner.meta(meta);
                quote!(#meta::TypeRefMeta::Stream(&#inner))
            }
            KType::Map(key, value) => {
                let key = key.meta(meta);
                let value = value.meta(meta);
                quote!(#meta::TypeRefMeta::Map(&#key, &#value))
            }
            KType::Result(ok, err) => {
                let ok = ok.meta(meta);
                let err = err.meta(meta);
                quote!(#meta::TypeRefMeta::Result(&#ok, &#err))
            }
            KType::Named(name) => quote!(#meta::TypeRefMeta::Named(#name)),
            KType::Object(name) => quote!(#meta::TypeRefMeta::Object(#name)),
            KType::Callback(name) => quote!(#meta::TypeRefMeta::Callback(#name)),
        }
    }

    /// Whether this type may be a map key (SPEC 2.1: `String`, integers, `Bool`, `Uuid`).
    pub(crate) fn is_valid_map_key(&self) -> bool {
        matches!(
            self,
            KType::String
                | KType::Bool
                | KType::I8
                | KType::I16
                | KType::I32
                | KType::I64
                | KType::U8
                | KType::U16
                | KType::U32
                | KType::U64
                | KType::Uuid
        )
    }

    /// Whether this is `Unit`.
    pub(crate) fn is_unit(&self) -> bool {
        matches!(self, KType::Unit)
    }
}

/// Where a type appears; only used to word diagnostics and to pick E0012.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pos {
    /// A record or variant field.
    Field,
    /// A parameter of a method, constructor or free function (objects and callbacks may stand
    /// here, ADR-040, ADR-041).
    Param,
    /// The return type of a method or free function (objects may stand here, ADR-040).
    Return,
    /// The value type of a store signal.
    Signal,
    /// A parameter of a port (or callback) method.
    PortParam,
    /// The return type of a port (or callback) method.
    PortReturn,
    /// A parameter of a query or mutation.
    QueryParam,
    /// The result of a query or mutation.
    QueryReturn,
}

impl Pos {
    fn describe(self) -> &'static str {
        match self {
            Pos::Field => "a field",
            Pos::Param | Pos::PortParam | Pos::QueryParam => "a parameter",
            Pos::Return | Pos::PortReturn | Pos::QueryReturn => "a return type",
            Pos::Signal => "a signal value",
        }
    }

    /// Why an object cannot stand here, and what to do instead (ADR-040 decision 9).
    fn object_refusal(self) -> (&'static str, &'static str) {
        match self {
            Pos::Field => (
                "a record or enum field holds a value: it is copied, compared, hashed and `Codable`, and none of that can carry a reference to an object",
                "keep the data the platform needs in the field (an id, a record), and put the child object behind a method of the parent",
            ),
            Pos::Signal => (
                "a store signal holds a value: it is copied, compared, coalesced and snapshotted, and none of that can carry a reference to an object",
                "keep an id or a record in the signal, and return the child object from a method of the store",
            ),
            Pos::PortParam | Pos::PortReturn => (
                "a port implementation on the platform would receive a reference it has no wrapper type for",
                "pass a record or an id through the port",
            ),
            Pos::QueryParam | Pos::QueryReturn => (
                "query and mutation values are cached, compared and persisted, so they must be values",
                "use a record or an id, and return the object from a method that calls the query",
            ),
            Pos::Param | Pos::Return => (
                "an object can only stand alone, or inside `Option` or `Vec`, as a method or function parameter or return",
                "use `Arc<T>`, `Option<Arc<T>>` or `Vec<Arc<T>>` (a parameter may also be `&T` or `Option<&T>`)",
            ),
        }
    }
}

/// Where a type appears, plus the name `Self` stands for (in records and enums, where
/// `Option<Box<Self>>` is the idiomatic way to write a recursive type).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cx<'a> {
    pub(crate) pos: Pos,
    pub(crate) self_name: Option<&'a str>,
}

impl From<Pos> for Cx<'_> {
    fn from(pos: Pos) -> Self {
        Cx {
            pos,
            self_name: None,
        }
    }
}

/// What the outermost node of a type may be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Allow {
    pub(crate) result: bool,
    pub(crate) stream: bool,
    pub(crate) unit: bool,
    /// Where an object (`Arc<T>`, `&T`) may stand (ADR-040).
    pub(crate) object: Objects,
    /// Whether a callback (`Arc<dyn Trait>`) may stand here, alone or in `Option` (ADR-041).
    pub(crate) callback: Callbacks,
}

/// Where an object spelling is accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Objects {
    /// Nowhere.
    No,
    /// A parameter: `&T`, `Arc<T>`, `Option<&T>`, `Option<Arc<T>>`, `Vec<Arc<T>>`.
    Param,
    /// A return: `Arc<T>`, `Option<Arc<T>>`, `Vec<Arc<T>>`.
    Return,
    /// Inside an `Option` of a parameter: `&T` or `Arc<T>`.
    InnerRef,
    /// Inside a `Vec`, or an `Option` of a return: `Arc<T>`.
    InnerOwned,
}

/// Where a callback spelling is accepted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Callbacks {
    /// Nowhere.
    No,
    /// A parameter: `Arc<dyn Trait>` or `Option<Arc<dyn Trait>>`.
    Param,
    /// Inside the `Option` of a parameter.
    Inner,
}

impl Allow {
    /// A plain wire type: nothing special.
    pub(crate) const NONE: Allow = Allow {
        result: false,
        stream: false,
        unit: false,
        object: Objects::No,
        callback: Callbacks::No,
    };
    /// A parameter of a method, constructor or free function: a value, an object or a callback.
    pub(crate) const PARAM: Allow = Allow {
        object: Objects::Param,
        callback: Callbacks::Param,
        ..Allow::NONE
    };
    /// The outermost type of a return: `T`, `()`, `Result<T, E>`, `impl Stream<Item = T>`.
    pub(crate) const RETURN: Allow = Allow {
        result: true,
        stream: true,
        unit: true,
        object: Objects::No,
        callback: Callbacks::No,
    };
    /// A method's or free function's return: as [`Allow::RETURN`], and objects.
    pub(crate) const RETURN_OBJECTS: Allow = Allow {
        object: Objects::Return,
        ..Allow::RETURN
    };
}

/// A rejected type: the diagnostic plus the syntax it is reported on.
#[derive(Debug)]
pub(crate) struct TyErr {
    pub(crate) diag: Diag,
    tokens: TokenStream,
}

impl TyErr {
    fn new(node: &impl ToTokens, diag: Diag) -> TyErr {
        TyErr {
            diag,
            tokens: node.to_token_stream(),
        }
    }

    /// Re-labels the error, keeping the span: used to turn E0001-E0006 into E0030.
    pub(crate) fn with_diag(self, diag: Diag) -> TyErr {
        TyErr { diag, ..self }
    }

    /// The `syn` error to emit.
    pub(crate) fn into_error(self) -> syn::Error {
        syn::Error::new_spanned(self.tokens, self.diag.message())
    }
}

impl From<TyErr> for syn::Error {
    fn from(err: TyErr) -> syn::Error {
        err.into_error()
    }
}

/// Renders a type for use in a message: `Vec < Box < dyn Any > >` becomes `Vec<Box<dyn Any>>`.
pub(crate) fn ty_string(ty: &impl ToTokens) -> String {
    let raw = ty.to_token_stream().to_string();
    let mut s = raw;
    for (from, to) in [
        (" < ", "<"),
        (" <", "<"),
        ("< ", "<"),
        (" > ", ">"),
        (" >", ">"),
        (" , ", ", "),
        (" ,", ","),
        (":: ", "::"),
        (" ::", "::"),
        ("& ", "&"),
        ("( ", "("),
        (" )", ")"),
        (" (", "("),
        ("[ ", "["),
        (" ]", "]"),
        (">+", "> +"),
    ] {
        s = s.replace(from, to);
    }
    s
}

const ALLOWED_SET: &str = "bool, i8..i64, u8..u64, f32, f64, String, Bytes, Vec<T>, Option<T>, HashMap<K, V>, BTreeMap<K, V>, Duration, Timestamp, Uuid, and types declared with #[undra::api]";

/// Maps the type of a record or variant field. A trait object anywhere inside is reported as
/// E0012 on the whole field type, as in the blueprint's example.
pub(crate) fn map_field(ty: &Type, field: &str, self_name: &str) -> Result<KType, TyErr> {
    let cx = Cx {
        pos: Pos::Field,
        self_name: Some(self_name),
    };
    match map_type(ty, cx, Allow::NONE) {
        // A callback in a field keeps its own diagnostic (E0004, with the help of its position).
        Err(err) if err.diag.code == code::E0004 && !err.diag.what.starts_with("callback") => {
            let shown = ty_string(ty);
            Err(TyErr::new(
                ty,
                Diag::new(
                    code::E0012,
                    format!("`{shown}` cannot cross the boundary"),
                    "trait objects have no wire representation",
                    format!(
                        "make `{field}` a concrete `#[undra::api]` type, or an enum listing the cases you need"
                    ),
                ),
            ))
        }
        other => other,
    }
}

/// Maps the return type of a method or free function: `T`, `()`, `Result<T, E>`,
/// `impl Stream<Item = T>`, `Result<impl Stream<Item = T>, E>`, (ADR-036) `impl Stream<Item =
/// Result<T, E>>` or `Result<impl Stream<Item = Result<T, E>>, E>` (the same `E`), which the
/// schema records exactly as `Result<Stream<T>, E>`: a stream that can end with its typed error
/// part-way. An object may also be returned (`Arc<T>`, `Option<Arc<T>>`, `Vec<Arc<T>>`, alone or
/// as the `Ok` of a `Result`, ADR-040).
pub(crate) fn map_method_return(ret: &ReturnType) -> Result<KType, TyErr> {
    match ret {
        ReturnType::Default => Ok(KType::Unit),
        ReturnType::Type(_, ty) => map_type(ty, Pos::Return, Allow::RETURN_OBJECTS),
    }
}

/// The same as [`map_method_return`] without objects, reported at `pos` (a port's or a query's
/// return).
pub(crate) fn map_return_at(ret: &ReturnType, pos: Pos) -> Result<KType, TyErr> {
    match ret {
        ReturnType::Default => Ok(KType::Unit),
        ReturnType::Type(_, ty) => map_type(ty, pos, Allow::RETURN),
    }
}

/// Maps `ty` found at `pos`, where the outermost node may be what `allow` says.
pub(crate) fn map_type<'a>(ty: &Type, cx: impl Into<Cx<'a>>, allow: Allow) -> Result<KType, TyErr> {
    let cx = cx.into();
    let pos = cx.pos;
    match ty {
        Type::Paren(inner) => map_type(&inner.elem, cx, allow),
        Type::Group(inner) => map_type(&inner.elem, cx, allow),
        Type::Tuple(tuple) if tuple.elems.is_empty() => {
            if allow.unit {
                Ok(KType::Unit)
            } else {
                Err(TyErr::new(
                    ty,
                    Diag::new(
                        code::E0001,
                        format!("`()` cannot be used as {}", pos.describe()),
                        "`()` occupies zero bytes on the wire, so it is only legal as a return type or as a variant without fields; `Vec<()>`, `Option<()>` and maps of `()` defeat length validation",
                        "remove the value, or use `bool` if you need a marker",
                    ),
                ))
            }
        }
        Type::Tuple(tuple) => Err(TyErr::new(
            ty,
            Diag::new(
                code::E0001,
                format!("tuple `{}` cannot cross the boundary", ty_string(ty)),
                "tuples have no schema representation, and the other languages would have to invent names for the elements",
                format!(
                    "declare a record with named fields: `#[undra::api] struct Pair {{ {} }}`",
                    tuple_fields(tuple)
                ),
            ),
        )),
        Type::Reference(reference) => match object_reference(reference, allow) {
            Some(name) => Ok(KType::Object(name)),
            None => Err(reference_in_list(reference, ty, allow)
                .unwrap_or_else(|| reference_error(ty, reference))),
        },
        Type::Path(path) => map_path(path, ty, cx, allow),
        Type::TraitObject(object) if has_stream_bound(&object.bounds) => Err(dyn_stream_error(ty)),
        Type::TraitObject(_) => Err(TyErr::new(
            ty,
            Diag::new(
                code::E0004,
                format!("trait object `{}` cannot cross the boundary", ty_string(ty)),
                "trait objects have no wire representation and no equivalent in Swift, Kotlin or TypeScript",
                "use a concrete `#[undra::api]` type or an enum listing the cases you need; to call into the platform, declare a `#[undra::callback]` trait and take `Arc<dyn Trait>` as a parameter",
            ),
        )),
        Type::ImplTrait(impl_trait) => map_impl_trait(impl_trait, ty, cx, allow),
        Type::BareFn(_) => Err(TyErr::new(
            ty,
            Diag::new(
                code::E0004,
                format!(
                    "function pointer `{}` cannot cross the boundary",
                    ty_string(ty)
                ),
                "callbacks have no wire representation",
                "declare a `#[undra::callback]` trait and take `Arc<dyn Trait>`, or return a stream (`impl Stream<Item = T>`) for a sequence of results",
            ),
        )),
        Type::Ptr(_) => Err(TyErr::new(
            ty,
            Diag::new(
                code::E0001,
                format!("raw pointer `{}` cannot cross the boundary", ty_string(ty)),
                "pointers are only meaningful inside one process and one allocator",
                "use an owned value, or an object handle by returning a `#[undra::api]` object",
            ),
        )),
        Type::Array(syn::TypeArray { elem, .. }) | Type::Slice(syn::TypeSlice { elem, .. }) => {
            let elem = ty_string(elem);
            let help = if elem == "u8" {
                "use `Bytes` for raw bytes, or `Vec<u8>`".to_owned()
            } else {
                format!("use `Vec<{elem}>`")
            };
            Err(TyErr::new(
                ty,
                Diag::new(
                    code::E0001,
                    format!("`{}` cannot cross the boundary", ty_string(ty)),
                    "arrays and slices have no schema representation: the other languages have no fixed-length array to decode into",
                    help,
                ),
            ))
        }
        Type::Never(_) => Err(TyErr::new(
            ty,
            Diag::new(
                code::E0001,
                "the never type `!` cannot cross the boundary",
                "it has no values to encode",
                "return `()` or a `Result<T, E>` instead",
            ),
        )),
        Type::Infer(_) => Err(TyErr::new(
            ty,
            Diag::new(
                code::E0001,
                "`_` cannot be used in a public signature",
                "the schema needs every type spelled out",
                "write the type",
            ),
        )),
        _ => Err(TyErr::new(
            ty,
            Diag::new(
                code::E0001,
                format!("`{}` cannot cross the boundary", ty_string(ty)),
                format!("only these types are supported: {ALLOWED_SET}"),
                "write one of the supported types here, for example `String`, `Vec<T>` or a record declared with `#[undra::api]`",
            ),
        )),
    }
}

/// The fields of a record that replaces a tuple: `first: A, second: B`.
fn tuple_fields(tuple: &syn::TypeTuple) -> String {
    const NAMES: [&str; 4] = ["first", "second", "third", "fourth"];
    tuple
        .elems
        .iter()
        .enumerate()
        .map(|(index, elem)| match NAMES.get(index) {
            Some(name) => format!("{name}: {}", ty_string(elem)),
            None => format!("field{index}: {}", ty_string(elem)),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn reference_error(ty: &Type, reference: &syn::TypeReference) -> TyErr {
    if let Some(lifetime) = &reference.lifetime {
        return TyErr::new(
            ty,
            Diag::new(
                code::E0003,
                format!("lifetime `{lifetime}` in `{}`", ty_string(ty)),
                "everything crosses the boundary by value; a borrow cannot outlive the call that made it",
                "use an owned type such as `String` or `Vec<T>`",
            ),
        );
    }
    let shown = ty_string(ty);
    let help = match &*reference.elem {
        Type::Path(p) if p.path.is_ident("str") => "use an owned `String`".to_owned(),
        Type::Slice(slice) => format!("use `Vec<{}>`", ty_string(&slice.elem)),
        other => format!("use the owned type `{}`", ty_string(other)),
    };
    TyErr::new(
        ty,
        Diag::new(
            code::E0001,
            format!("`{shown}` cannot cross the boundary"),
            "references have no wire representation; every value crosses by copy",
            help,
        ),
    )
}

/// Names a type that can only be a record, an enum or a built-in: never an object.
const NOT_OBJECTS: &[&str] = &[
    "bool",
    "i8",
    "i16",
    "i32",
    "i64",
    "u8",
    "u16",
    "u32",
    "u64",
    "f32",
    "f64",
    "usize",
    "isize",
    "i128",
    "u128",
    "char",
    "str",
    "String",
    "Bytes",
    "Duration",
    "Timestamp",
    "Uuid",
    "Vec",
    "Option",
    "Box",
    "Arc",
    "Rc",
    "Result",
    "HashMap",
    "BTreeMap",
    "HashSet",
    "BTreeSet",
    "VecDeque",
    "LinkedList",
    "BinaryHeap",
    "Cow",
    "Cell",
    "RefCell",
    "Mutex",
    "RwLock",
    "OnceCell",
    "OnceLock",
    "PathBuf",
    "Path",
    "OsString",
    "OsStr",
    "Instant",
    "SystemTime",
    "Lazy",
    "Signal",
    "Computed",
    "Effect",
    "Ctx",
    "Handle",
    "Pin",
    "Self",
];

/// The name of the object a plain type path spells (`Mailbox`, `crate::mail::Mailbox`): the last
/// segment, when the path has no arguments and does not name a built-in. Whether it really is an
/// object is checked by `check.rs`.
fn object_path_name(path: &syn::TypePath) -> Option<String> {
    if path.qself.is_some() {
        return None;
    }
    let last = path.path.segments.last()?;
    if !last.arguments.is_none() {
        return None;
    }
    let name = last.ident.to_string();
    (!NOT_OBJECTS.contains(&name.as_str())).then(|| strip_raw(&name))
}

/// `&T` where an object may be taken by reference (a parameter, alone or in an `Option`): the
/// object's name. A lifetime, `&mut` and a built-in element are not objects, and keep the
/// reference diagnostics.
fn object_reference(reference: &syn::TypeReference, allow: Allow) -> Option<String> {
    if !matches!(allow.object, Objects::Param | Objects::InnerRef) {
        return None;
    }
    if reference.lifetime.is_some() || reference.mutability.is_some() {
        return None;
    }
    match &*reference.elem {
        Type::Path(path) => object_path_name(path),
        _ => None,
    }
}

/// `&T` of an object inside a `Vec`: the way to write a list of objects is `Vec<Arc<T>>`.
fn reference_in_list(reference: &syn::TypeReference, ty: &Type, allow: Allow) -> Option<TyErr> {
    if allow.object != Objects::InnerOwned
        || reference.lifetime.is_some()
        || reference.mutability.is_some()
    {
        return None;
    }
    let Type::Path(path) = &*reference.elem else {
        return None;
    };
    let name = object_path_name(path)?;
    Some(TyErr::new(
        ty,
        Diag::new(
            code::E0064,
            format!("`{}` cannot be an element of a list", ty_string(ty)),
            "a list owns its elements, and a borrow of an object cannot outlive the call that made it",
            format!("write the list as `Vec<Arc<{name}>>`"),
        ),
    ))
}

/// `Arc<..>`: an object (`Arc<T>`) or a callback (`Arc<dyn Trait>`), where one may stand.
fn map_arc(inner: &Type, ty: &Type, cx: Cx<'_>, allow: Allow) -> Result<KType, TyErr> {
    let mut inner = inner;
    while let Type::Paren(p) = inner {
        inner = &p.elem;
    }
    match inner {
        Type::TraitObject(object) => {
            if has_stream_bound(&object.bounds) {
                return Err(dyn_stream_error(ty));
            }
            let name = callback_trait(object, ty)?;
            match allow.callback {
                Callbacks::Param | Callbacks::Inner => Ok(KType::Callback(name)),
                Callbacks::No => Err(callback_refusal(ty, cx.pos, &name)),
            }
        }
        Type::Path(path) => match object_path_name(path) {
            Some(name) if allow.object != Objects::No => Ok(KType::Object(name)),
            Some(name) => {
                let (why, help) = cx.pos.object_refusal();
                Err(TyErr::new(
                    ty,
                    Diag::new(
                        code::E0064,
                        format!(
                            "object `{}` cannot be {}",
                            ty_string(ty),
                            position_phrase(cx.pos)
                        ),
                        why,
                        format!("{help} (the object here is `{name}`)"),
                    ),
                ))
            }
            None => Err(unsupported(
                ty,
                format!("`{}` cannot cross the boundary", ty_string(ty)),
                "shared ownership does not survive a copy across the boundary",
                "use the owned inner type",
            )),
        },
        _ => Err(unsupported(
            ty,
            format!("`{}` cannot cross the boundary", ty_string(ty)),
            "shared ownership does not survive a copy across the boundary",
            "use the owned inner type",
        )),
    }
}

/// "used as a field", "taken by a query": where an object was refused, for the message.
fn position_phrase(pos: Pos) -> &'static str {
    match pos {
        Pos::Field => "a field of a record or enum",
        Pos::Signal => "the value of a store signal",
        Pos::PortParam => "a parameter of a port",
        Pos::PortReturn => "returned by a port",
        Pos::QueryParam => "a parameter of a query or mutation",
        Pos::QueryReturn => "the result of a query or mutation",
        Pos::Param | Pos::Return => "used here",
    }
}

/// The trait of `dyn Trait`, with its auto-trait bounds (`Send`, `Sync`, `Unpin`, `'static`)
/// dropped: `dyn Trait` and `dyn Trait + Send + Sync` are one type for a trait whose
/// supertraits are `Send + Sync`, which every callback trait has.
fn callback_trait(object: &syn::TypeTraitObject, ty: &Type) -> Result<String, TyErr> {
    let mut found: Option<&syn::TraitBound> = None;
    for bound in &object.bounds {
        match bound {
            TypeParamBound::Trait(bound) => {
                let last = bound.path.segments.last();
                let auto = last.is_some_and(|seg| {
                    matches!(seg.ident.to_string().as_str(), "Send" | "Sync" | "Unpin")
                });
                if auto {
                    continue;
                }
                if last.is_some_and(|seg| matches!(seg.arguments, PathArguments::Parenthesized(_)))
                {
                    return Err(TyErr::new(
                        ty,
                        Diag::new(
                            code::E0004,
                            format!("closure type `{}` cannot cross the boundary", ty_string(ty)),
                            "callbacks have no wire representation",
                            "declare a `#[undra::callback]` trait and take `Arc<dyn Trait>`",
                        ),
                    ));
                }
                if found.is_some() {
                    return Err(TyErr::new(
                        ty,
                        Diag::new(
                            code::E0004,
                            format!("`{}` names more than one trait", ty_string(ty)),
                            "a callback is one `#[undra::callback]` trait: the host implements one protocol per instance",
                            "take `Arc<dyn Trait>` of a single callback trait",
                        ),
                    ));
                }
                found = Some(bound);
            }
            TypeParamBound::Lifetime(lifetime) if lifetime.ident != "static" => {
                return Err(TyErr::new(
                    ty,
                    Diag::new(
                        code::E0003,
                        format!("lifetime `{lifetime}` in `{}`", ty_string(ty)),
                        "the host's instance outlives the call that passed it",
                        "use `Arc<dyn Trait>`",
                    ),
                ));
            }
            _ => {}
        }
    }
    let Some(bound) = found else {
        return Err(TyErr::new(
            ty,
            Diag::new(
                code::E0004,
                format!("trait object `{}` names no trait", ty_string(ty)),
                "a callback is an `Arc<dyn Trait>` of a `#[undra::callback]` trait",
                "name the trait",
            ),
        ));
    };
    let Some(last) = bound.path.segments.last() else {
        return Err(unsupported(
            ty,
            "empty trait path".to_owned(),
            "the schema needs a trait name",
            "write the trait",
        ));
    };
    if !last.arguments.is_none() {
        return Err(TyErr::new(
            ty,
            Diag::new(
                code::E0002,
                format!(
                    "generic trait `{}` cannot cross the boundary",
                    ty_string(ty)
                ),
                "the schema describes concrete callback interfaces; every target language would need one per instantiation",
                "declare a concrete `#[undra::callback]` trait",
            ),
        ));
    }
    Ok(strip_raw(&last.ident.to_string()))
}

/// E0004 for a callback where none may stand, with the help of the position.
fn callback_refusal(ty: &Type, pos: Pos, name: &str) -> TyErr {
    let (why, help) = match pos {
        Pos::Return | Pos::PortReturn | Pos::QueryReturn => (
            "a callback is an object the host implements and passes in for the core to call; the core never hands one out",
            "return a plain value, or an object (`Arc<T>`) the host can call methods on",
        ),
        Pos::Field | Pos::Signal => (
            "a callback is an instance with a lifetime, not a value: it cannot be copied, compared, stored in a record or mirrored from a signal",
            "keep an id or a record here, and take the callback as a parameter of the method that needs it",
        ),
        Pos::PortParam => (
            "a port is implemented once, by the platform, for the whole process; a callback instance in its arguments would need a wrapper type the port implementation does not have",
            "pass a record or an id through the port",
        ),
        Pos::QueryParam => (
            "query and mutation values are cached, compared and persisted, so they must be values",
            "take the callback in the method that runs the query, or pass an id",
        ),
        Pos::Param => (
            "a callback may be a parameter of a method, constructor or function, alone or in an `Option`; it cannot be nested in another type",
            "take one `Arc<dyn Trait>` (or `Option<Arc<dyn Trait>>`) per parameter",
        ),
    };
    TyErr::new(
        ty,
        Diag::new(
            code::E0004,
            format!(
                "callback `{}` cannot be {} (`{name}` is a callback interface)",
                ty_string(ty),
                position_phrase(pos),
            ),
            why,
            help,
        ),
    )
}

fn unsupported(ty: &Type, what: String, why: &str, help: &str) -> TyErr {
    TyErr::new(ty, Diag::new(code::E0001, what, why, help))
}

/// Type arguments of a path segment, or an error for anything that is not a plain type.
fn type_args<'a>(seg: &'a syn::PathSegment, ty: &Type) -> Result<Vec<&'a Type>, TyErr> {
    match &seg.arguments {
        PathArguments::None => Ok(Vec::new()),
        PathArguments::Parenthesized(_) => Err(TyErr::new(
            ty,
            Diag::new(
                code::E0004,
                format!("closure type `{}` cannot cross the boundary", ty_string(ty)),
                "callbacks have no wire representation",
                "declare a `#[undra::callback]` trait and take `Arc<dyn Trait>`",
            ),
        )),
        PathArguments::AngleBracketed(args) => {
            let mut out = Vec::new();
            for arg in &args.args {
                match arg {
                    GenericArgument::Type(t) => out.push(t),
                    GenericArgument::Lifetime(lifetime) => {
                        return Err(TyErr::new(
                            ty,
                            Diag::new(
                                code::E0003,
                                format!("lifetime `{lifetime}` in `{}`", ty_string(ty)),
                                "everything crosses the boundary by value; a borrow cannot outlive the call that made it",
                                "use an owned type",
                            ),
                        ));
                    }
                    other => {
                        return Err(unsupported(
                            ty,
                            format!("`{}` cannot cross the boundary", ty_string(ty)),
                            &format!(
                                "the generic argument `{}` is not a type",
                                other.to_token_stream()
                            ),
                            "use plain type arguments",
                        ));
                    }
                }
            }
            Ok(out)
        }
    }
}

/// A lifetime anywhere in the path (`a::Foo<'a>::Bar`) is E0003.
fn reject_path_lifetimes(path: &syn::TypePath, ty: &Type) -> Result<(), TyErr> {
    for seg in &path.path.segments {
        if let PathArguments::AngleBracketed(args) = &seg.arguments {
            for arg in &args.args {
                if let GenericArgument::Lifetime(lifetime) = arg {
                    return Err(TyErr::new(
                        ty,
                        Diag::new(
                            code::E0003,
                            format!("lifetime `{lifetime}` in `{}`", ty_string(ty)),
                            "everything crosses the boundary by value; a borrow cannot outlive the call that made it",
                            "use an owned type",
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn map_path(path: &syn::TypePath, ty: &Type, cx: Cx<'_>, allow: Allow) -> Result<KType, TyErr> {
    let pos = cx.pos;
    if path.qself.is_some() {
        return Err(unsupported(
            ty,
            format!(
                "qualified path `{}` cannot cross the boundary",
                ty_string(ty)
            ),
            "associated types cannot be described by the schema",
            "name the concrete type",
        ));
    }
    reject_path_lifetimes(path, ty)?;
    if path.path.segments.len() > 1
        && path
            .path
            .segments
            .first()
            .is_some_and(|seg| seg.ident == "Self")
    {
        return Err(unsupported(
            ty,
            format!("`{}` cannot cross the boundary", ty_string(ty)),
            "an associated type has no name the schema could record, and the platforms would have to guess which type it is",
            "name the concrete type",
        ));
    }
    let Some(last) = path.path.segments.last() else {
        return Err(unsupported(
            ty,
            "empty type path".to_owned(),
            "the schema needs a type name",
            "write the type",
        ));
    };
    let name = last.ident.to_string();
    let args = type_args(last, ty)?;
    let bare = last.arguments.is_none();

    // Primitives and the wire leaf types take no arguments.
    if bare {
        let leaf = match name.as_str() {
            "bool" => Some(KType::Bool),
            "i8" => Some(KType::I8),
            "i16" => Some(KType::I16),
            "i32" => Some(KType::I32),
            "i64" => Some(KType::I64),
            "u8" => Some(KType::U8),
            "u16" => Some(KType::U16),
            "u32" => Some(KType::U32),
            "u64" => Some(KType::U64),
            "f32" => Some(KType::F32),
            "f64" => Some(KType::F64),
            "String" => Some(KType::String),
            "Bytes" => Some(KType::Bytes),
            "Duration" => Some(KType::Duration),
            "Timestamp" => Some(KType::Timestamp),
            "Uuid" => Some(KType::Uuid),
            "Decimal" => Some(KType::Decimal),
            _ => None,
        };
        if let Some(leaf) = leaf {
            return Ok(leaf);
        }
    }

    match (name.as_str(), args.len()) {
        ("usize" | "isize", 0) => Err(unsupported(
            ty,
            format!("`{name}` cannot cross the boundary"),
            "its width differs between platforms (32-bit wasm, 64-bit iOS and Android), so the wire layout would too",
            "use a fixed-width integer such as `u32` or `u64`",
        )),
        ("i128" | "u128", 0) => Err(unsupported(
            ty,
            format!("`{name}` cannot cross the boundary"),
            "Swift, Kotlin and TypeScript have no portable 128-bit integer",
            "use two `u64` halves, or `Uuid` for identifiers",
        )),
        ("char", 0) => Err(unsupported(
            ty,
            "`char` cannot cross the boundary".to_owned(),
            "the platforms disagree on what a character is (scalar value, UTF-16 unit, grapheme)",
            "use `String`, or `u32` for a Unicode scalar value",
        )),
        ("str", 0) => Err(unsupported(
            ty,
            "`str` cannot cross the boundary".to_owned(),
            "it is unsized and borrowed",
            "use an owned `String`",
        )),
        ("Box", 1) => map_type(args[0], cx, Allow::NONE),
        ("Vec", 1) => {
            let inner_allow = Allow {
                object: match allow.object {
                    Objects::Param | Objects::Return => Objects::InnerOwned,
                    _ => Objects::No,
                },
                ..Allow::NONE
            };
            Ok(KType::Vec(Box::new(map_type(args[0], cx, inner_allow)?)))
        }
        ("Option", 1) => {
            let inner_allow = Allow {
                object: match allow.object {
                    Objects::Param => Objects::InnerRef,
                    Objects::Return => Objects::InnerOwned,
                    _ => Objects::No,
                },
                callback: match allow.callback {
                    Callbacks::Param => Callbacks::Inner,
                    _ => Callbacks::No,
                },
                ..Allow::NONE
            };
            let inner = map_type(args[0], cx, inner_allow)?;
            if matches!(inner, KType::Option(_)) {
                return Err(TyErr::new(
                    ty,
                    Diag::new(
                        code::E0063,
                        format!(
                            "nested option `{}` cannot cross the boundary",
                            ty_string(ty)
                        ),
                        "Kotlin and TypeScript cannot express nested optionality: `Some(None)` and `None` would both arrive as `null`",
                        "wrap the inner option in a record or an enum that names the two cases",
                    ),
                ));
            }
            Ok(KType::Option(Box::new(inner)))
        }
        ("HashMap" | "BTreeMap", 2) => {
            let key = map_type(args[0], cx, Allow::NONE)?;
            if !key.is_valid_map_key() {
                return Err(TyErr::new(
                    args[0],
                    Diag::new(
                        code::E0006,
                        format!("`{}` cannot be a map key", ty_string(args[0])),
                        "map keys must be `String`, an integer, `bool` or `Uuid`: those compare and hash identically on every platform (floats and composite keys do not)",
                        "use one of those key types, or a `Vec` of records with an explicit key field",
                    ),
                ));
            }
            let value = map_type(args[1], cx, Allow::NONE)?;
            Ok(KType::Map(Box::new(key), Box::new(value)))
        }
        ("HashMap", 3) => Err(unsupported(
            ty,
            format!("`{}` cannot cross the boundary", ty_string(ty)),
            "custom hashers are not part of the schema",
            "use `HashMap<K, V>` with the default hasher",
        )),
        ("Result", 2) => {
            if !allow.result {
                return Err(result_misplaced(ty, pos));
            }
            let ok = map_type(
                args[0],
                cx,
                Allow {
                    result: false,
                    stream: allow.stream,
                    unit: true,
                    object: allow.object,
                    callback: Callbacks::No,
                },
            )?;
            let err = map_error_type(args[1], cx)?;
            // `Result<impl Stream<Item = Result<T, E>>, E>`: the opening and the items fail with
            // the same `E`, so the schema shape is the one `Result<Stream<T>, E>` (ADR-036).
            if let KType::Result(stream, item_err) = ok {
                if *item_err != err {
                    let name = |k: &KType| match k {
                        KType::Named(name) => name.clone(),
                        other => format!("{other:?}"),
                    };
                    return Err(TyErr::new(
                        ty,
                        Diag::new(
                            code::E0005,
                            format!(
                                "the items of the stream in `{}` fail with `{}` but its opening fails with `{}`",
                                ty_string(ty),
                                name(&item_err),
                                name(&err)
                            ),
                            "a stream method has one error type: the platforms throw it whether the stream fails to open or ends with an error part-way",
                            format!(
                                "use the same `#[undra::error]` enum for both (`{}`), or one enum with a variant for each case",
                                ty_string(args[1])
                            ),
                        ),
                    ));
                }
                return Ok(KType::Result(stream, item_err));
            }
            Ok(KType::Result(Box::new(ok), Box::new(err)))
        }
        ("Result", _) => {
            if !allow.result {
                return Err(result_misplaced(ty, pos));
            }
            Err(unsupported(
                ty,
                format!("`{}` hides its error type", ty_string(ty)),
                "the schema needs both type parameters of a `Result` to describe the error the platforms will throw",
                "spell out both: `Result<T, MyError>` where `MyError` is a `#[undra::error]` enum",
            ))
        }
        ("Lazy", 1) => Err(unsupported(
            ty,
            format!(
                "`{}` can only be the type of a store field, not of {}",
                ty_string(ty),
                match pos {
                    Pos::Field => "a record or enum field",
                    Pos::Signal => "a signal value",
                    Pos::Return | Pos::PortReturn | Pos::QueryReturn => "a return type",
                    Pos::Param | Pos::PortParam | Pos::QueryParam => "a parameter",
                }
            ),
            "a `Lazy<T>` is a list the core owns and the platforms page through by handle: it is not a value, so it cannot be a parameter, a return type, a record or enum field, or the value of a signal",
            "write it as a field of a `#[undra::store]` struct (`books: Lazy<Book>`); to give a caller a list, return a `Vec<T>`, or take an offset and a limit and return one page",
        )),
        ("Signal" | "Computed" | "Effect", _) => Err(unsupported(
            ty,
            format!("`{}` cannot cross the boundary", ty_string(ty)),
            "signals are never encoded as values; the platforms mirror them from change-sets",
            "make the signal a field of a `#[undra::store]` struct and pass its value type instead",
        )),
        ("Ctx", 0) => Err(unsupported(
            ty,
            "`Ctx` cannot be passed across the boundary".to_owned(),
            "the runtime injects `Ctx`; it is only accepted as the first parameter of a constructor, a free function, a query or a mutation",
            &format!(
                "remove the parameter; keep a `WeakCtx` (`ctx.downgrade()`) in the object at construction, or call `Ctx::current()`{}",
                reserved_hint("Ctx")
            ),
        )),
        ("Handle", 0) => Err(unsupported(
            ty,
            "`Handle` cannot be used in a public signature".to_owned(),
            "a handle is how the runtime refers to an object instance; the schema has no handle type, because an object crosses the boundary through its constructors, never as a value in a signature",
            &format!(
                "use the object's own methods from the platform (it holds the handle), or return a record with the data the platform needs{}",
                reserved_hint("Handle")
            ),
        )),
        ("Pin", 1) if is_boxed_dyn_stream(args[0]) => Err(dyn_stream_error(ty)),
        ("Self", 0) => match cx.self_name {
            Some(name) => Ok(KType::Named(name.to_owned())),
            None => Err(unsupported(
                ty,
                "`Self` cannot be used as a parameter or return type".to_owned(),
                "the schema needs the type spelled out, and objects cross by handle, not by value",
                "name the record or enum type",
            )),
        },
        ("HashSet" | "BTreeSet" | "VecDeque" | "LinkedList" | "BinaryHeap", _) => Err(unsupported(
            ty,
            format!("`{}` cannot cross the boundary", ty_string(ty)),
            "only sequences (`Vec<T>`) and maps have a wire representation",
            "use `Vec<T>` (deduplicate before sending if you need set semantics)",
        )),
        ("Arc", 1) => map_arc(args[0], ty, cx, allow),
        ("Rc" | "Arc", _) => Err(unsupported(
            ty,
            format!("`{}` cannot cross the boundary", ty_string(ty)),
            "shared ownership does not survive a copy across the boundary",
            "use the owned inner type",
        )),
        ("Cell" | "RefCell" | "Mutex" | "RwLock" | "OnceCell" | "OnceLock", _) => Err(unsupported(
            ty,
            format!("`{}` cannot cross the boundary", ty_string(ty)),
            "interior mutability has no meaning in a copied value",
            "use the plain inner type",
        )),
        ("Cow", _) => Err(unsupported(
            ty,
            format!("`{}` cannot cross the boundary", ty_string(ty)),
            "borrowed data cannot cross the boundary",
            "use the owned type (`String` or `Vec<T>`)",
        )),
        ("PathBuf" | "Path" | "OsString" | "OsStr", 0) => Err(unsupported(
            ty,
            format!("`{name}` cannot cross the boundary"),
            "paths are not portable text",
            &format!("use `String`{}", reserved_hint(&name)),
        )),
        ("Instant", 0) => Err(unsupported(
            ty,
            "`Instant` cannot cross the boundary".to_owned(),
            "monotonic instants are meaningless outside their process",
            &format!(
                "use `Duration` for spans and `Timestamp` for points in time{}",
                reserved_hint("Instant")
            ),
        )),
        ("SystemTime", 0) => Err(unsupported(
            ty,
            "`SystemTime` cannot cross the boundary".to_owned(),
            "it has no portable representation, and the core reads time through the `Clock` port",
            &format!(
                "use `Timestamp` (milliseconds since the Unix epoch){}",
                reserved_hint("SystemTime")
            ),
        )),
        (_, 0) if bare => Ok(KType::Named(strip_raw(&name))),
        _ => Err(unsupported(
            ty,
            format!("generic type `{}` cannot cross the boundary", ty_string(ty)),
            "the schema has no way to name an instantiated generic type",
            "declare a concrete `#[undra::api]` type for this instantiation",
        )),
    }
}

fn strip_raw(name: &str) -> String {
    name.strip_prefix("r#").unwrap_or(name).to_owned()
}

fn result_misplaced(ty: &Type, pos: Pos) -> TyErr {
    let what = match pos {
        // In a return type, `allow.result` is only false below the outermost type.
        Pos::Return => format!("`{}` cannot be nested inside another type", ty_string(ty)),
        _ => format!("`{}` cannot be used as {}", ty_string(ty), pos.describe()),
    };
    TyErr::new(
        ty,
        Diag::new(
            code::E0005,
            what,
            "`Result<T, E>` is how a method reports a typed error; it is only legal as the outermost type of a return",
            "return the `Result` from the method and keep only plain values in fields, parameters and nested types",
        ),
    )
}

/// The error side of a `Result`: it is thrown by name on the platforms, so it must be a named
/// type (`check.rs` then requires it to be a `#[undra::error]` enum).
pub(crate) fn map_error_type<'a>(ty: &Type, cx: impl Into<Cx<'a>>) -> Result<KType, TyErr> {
    let kty = map_type(ty, cx, Allow::NONE)?;
    if matches!(kty, KType::Named(_)) {
        return Ok(kty);
    }
    Err(TyErr::new(
        ty,
        Diag::new(
            code::E0001,
            format!("`{}` cannot be the error type of a `Result`", ty_string(ty)),
            "the platforms throw the error by name, and only a `#[undra::error]` enum carries the messages they show",
            "declare an error enum: `#[undra::error] enum MyError { #[error(\"what went wrong\")] Failed }`, and return `Result<T, MyError>`",
        ),
    ))
}

/// A hint appended to rejections that go by name: a type of the user's own with that name is
/// refused too.
fn reserved_hint(name: &str) -> String {
    format!(
        "; if `{name}` is a type of your own, rename it: Undra recognises this name wherever it is written"
    )
}

fn has_stream_bound(bounds: &syn::punctuated::Punctuated<TypeParamBound, syn::Token![+]>) -> bool {
    bounds.iter().any(|bound| {
        matches!(bound, TypeParamBound::Trait(t)
            if t.path.segments.last().is_some_and(|seg| seg.ident == "Stream"))
    })
}

/// `Box<dyn Stream<..>>` (inside a `Pin`).
fn is_boxed_dyn_stream(ty: &Type) -> bool {
    let Type::Path(path) = ty else {
        return false;
    };
    let Some(seg) = path.path.segments.last() else {
        return false;
    };
    if seg.ident != "Box" {
        return false;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return false;
    };
    args.args.iter().any(|arg| {
        matches!(arg, GenericArgument::Type(Type::TraitObject(object)) if has_stream_bound(&object.bounds))
    })
}

fn dyn_stream_error(ty: &Type) -> TyErr {
    TyErr::new(
        ty,
        Diag::new(
            code::E0004,
            format!("boxed stream `{}` cannot cross the boundary", ty_string(ty)),
            "a stream is described to the platforms as `impl Stream<Item = T>` in a return type; a boxed or `dyn` stream has no schema representation",
            "return `impl Stream<Item = T> + Send + 'static` (box it inside the function if the branches differ)",
        ),
    )
}

fn map_impl_trait(
    impl_trait: &syn::TypeImplTrait,
    ty: &Type,
    cx: Cx<'_>,
    allow: Allow,
) -> Result<KType, TyErr> {
    let pos = cx.pos;
    let mut item: Option<&Type> = None;
    let mut is_stream = false;
    for bound in &impl_trait.bounds {
        match bound {
            TypeParamBound::Trait(bound) => {
                let Some(seg) = bound.path.segments.last() else {
                    continue;
                };
                let name = seg.ident.to_string();
                if name == "Stream" {
                    is_stream = true;
                    if let PathArguments::AngleBracketed(args) = &seg.arguments {
                        for arg in &args.args {
                            if let GenericArgument::AssocType(assoc) = arg {
                                if assoc.ident == "Item" {
                                    item = Some(&assoc.ty);
                                }
                            }
                        }
                    }
                } else if !matches!(name.as_str(), "Send" | "Sync" | "Unpin") {
                    return Err(impl_trait_error(ty));
                }
            }
            TypeParamBound::Lifetime(lifetime) => {
                if lifetime.ident != "static" {
                    return Err(TyErr::new(
                        ty,
                        Diag::new(
                            code::E0003,
                            format!("lifetime `{lifetime}` in `{}`", ty_string(ty)),
                            "a returned stream cannot borrow from the call that created it",
                            "return an owned, `'static` stream",
                        ),
                    ));
                }
            }
            _ => return Err(impl_trait_error(ty)),
        }
    }
    if !is_stream {
        return Err(impl_trait_error(ty));
    }
    if !allow.stream {
        let what = match pos {
            Pos::Return => format!("`{}` cannot be nested inside another type", ty_string(ty)),
            _ => format!("`{}` cannot be used as {}", ty_string(ty), pos.describe()),
        };
        return Err(TyErr::new(
            ty,
            Diag::new(
                code::E0005,
                what,
                "a stream is how a method returns many values over time; it is only legal as the return type, or as the `Ok` side of a returned `Result`",
                "return the stream from the method",
            ),
        ));
    }
    let Some(item) = item else {
        return Err(TyErr::new(
            ty,
            Diag::new(
                code::E0001,
                format!("`{}` does not name its item type", ty_string(ty)),
                "the schema needs the type of the values the stream yields",
                "write `impl Stream<Item = T>`",
            ),
        ));
    };
    // `impl Stream<Item = Result<T, E>>` (ADR-036): an `Err(e)` item ends the stream with its
    // typed error. Recorded as `Result<Stream<T>, E>`, the shape every platform already throws `E`
    // from, so the schema, its hash and the generated code are those of a fallible opening.
    if let Some((ok, err)) = result_parts(item) {
        let ok = map_type(ok, cx, Allow::NONE)?;
        let err = map_error_type(err, cx)?;
        return Ok(KType::Result(
            Box::new(KType::Stream(Box::new(ok))),
            Box::new(err),
        ));
    }
    Ok(KType::Stream(Box::new(map_type(item, cx, Allow::NONE)?)))
}

/// The `T` and `E` of a type spelled `Result<T, E>` (any path ending in `Result` with two type
/// arguments).
pub(crate) fn result_parts(ty: &Type) -> Option<(&Type, &Type)> {
    let Type::Path(path) = ty else {
        return None;
    };
    if path.qself.is_some() {
        return None;
    }
    let seg = path.path.segments.last()?;
    if seg.ident != "Result" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    let mut types = args.args.iter().filter_map(|arg| match arg {
        GenericArgument::Type(ty) => Some(ty),
        _ => None,
    });
    let (Some(ok), Some(err), None) = (types.next(), types.next(), types.next()) else {
        return None;
    };
    Some((ok, err))
}

fn impl_trait_error(ty: &Type) -> TyErr {
    TyErr::new(
        ty,
        Diag::new(
            code::E0004,
            format!("`{}` cannot cross the boundary", ty_string(ty)),
            "`impl Trait` has no wire representation; the only one the schema understands is `impl Stream<Item = T>` in a return type",
            "use a concrete type, or `impl Stream<Item = T>` for a sequence of values over time",
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ty(src: &str) -> Type {
        syn::parse_str(src).unwrap_or_else(|e| panic!("bad test type {src}: {e}"))
    }

    fn field(src: &str) -> Result<KType, TyErr> {
        map_type(&ty(src), Pos::Field, Allow::NONE)
    }

    fn ret(src: &str) -> Result<KType, TyErr> {
        map_type(&ty(src), Pos::Return, Allow::RETURN)
    }

    fn code_of(result: Result<KType, TyErr>) -> &'static str {
        result.expect_err("expected a diagnostic").diag.code
    }

    fn named(n: &str) -> KType {
        KType::Named(n.to_owned())
    }

    fn boxed(k: KType) -> Box<KType> {
        Box::new(k)
    }

    #[test]
    fn primitives_map_to_themselves() {
        let cases = [
            ("bool", KType::Bool),
            ("i8", KType::I8),
            ("i16", KType::I16),
            ("i32", KType::I32),
            ("i64", KType::I64),
            ("u8", KType::U8),
            ("u16", KType::U16),
            ("u32", KType::U32),
            ("u64", KType::U64),
            ("f32", KType::F32),
            ("f64", KType::F64),
            ("String", KType::String),
            ("Bytes", KType::Bytes),
            ("Duration", KType::Duration),
            ("Timestamp", KType::Timestamp),
            ("Uuid", KType::Uuid),
        ];
        for (src, expected) in cases {
            assert_eq!(field(src).unwrap(), expected, "{src}");
        }
    }

    #[test]
    fn paths_use_the_last_segment() {
        assert_eq!(field("std::string::String").unwrap(), KType::String);
        assert_eq!(field("undra::Uuid").unwrap(), KType::Uuid);
        assert_eq!(field("crate::model::Todo").unwrap(), named("Todo"));
        assert_eq!(field("::undra::wire::Bytes").unwrap(), KType::Bytes);
    }

    #[test]
    fn collections_nest() {
        assert_eq!(field("Vec<u8>").unwrap(), KType::Vec(boxed(KType::U8)));
        assert_eq!(
            field("Option<Vec<Todo>>").unwrap(),
            KType::Option(boxed(KType::Vec(boxed(named("Todo")))))
        );
        assert_eq!(
            field("Option<Vec<Option<i32>>>").unwrap(),
            KType::Option(boxed(KType::Vec(boxed(KType::Option(boxed(KType::I32))))))
        );
        assert_eq!(
            field("std::collections::HashMap<String, Vec<i64>>").unwrap(),
            KType::Map(boxed(KType::String), boxed(KType::Vec(boxed(KType::I64))))
        );
        assert_eq!(
            field("BTreeMap<Uuid, Todo>").unwrap(),
            KType::Map(boxed(KType::Uuid), boxed(named("Todo")))
        );
    }

    #[test]
    fn box_is_transparent() {
        assert_eq!(field("Box<Node>").unwrap(), named("Node"));
        assert_eq!(
            field("Option<Box<Node>>").unwrap(),
            KType::Option(boxed(named("Node")))
        );
    }

    #[test]
    fn valid_map_keys() {
        for key in ["String", "bool", "i8", "u64", "Uuid", "i32"] {
            assert!(field(&format!("HashMap<{key}, u8>")).is_ok(), "{key}");
        }
    }

    #[test]
    fn invalid_map_keys_are_e0006() {
        for key in [
            "f64",
            "f32",
            "Bytes",
            "Vec<u8>",
            "Todo",
            "Option<String>",
            "Duration",
        ] {
            assert_eq!(
                code_of(field(&format!("HashMap<{key}, u8>"))),
                code::E0006,
                "{key}"
            );
        }
    }

    #[test]
    fn self_is_the_enclosing_type_in_fields_only() {
        let in_field = |src: &str| map_field(&ty(src), "f", "Node");
        assert_eq!(in_field("Self").unwrap(), named("Node"));
        assert_eq!(
            in_field("Vec<Self>").unwrap(),
            KType::Vec(boxed(named("Node")))
        );
        assert_eq!(
            in_field("Option<Box<Self>>").unwrap(),
            KType::Option(boxed(named("Node")))
        );
        // Elsewhere `Self` is an object, which cannot cross by value.
        assert_eq!(
            code_of(map_type(&ty("Self"), Pos::Param, Allow::NONE)),
            code::E0001
        );
        assert_eq!(code_of(field("Vec<Self>")), code::E0001);
    }

    #[test]
    fn unit_is_only_a_return_type() {
        assert_eq!(ret("()").unwrap(), KType::Unit);
        assert_eq!(code_of(field("()")), code::E0001);
        assert_eq!(code_of(field("Vec<()>")), code::E0001);
        assert_eq!(code_of(field("Option<()>")), code::E0001);
        assert_eq!(code_of(field("HashMap<String, ()>")), code::E0001);
        assert_eq!(
            ret("Result<(), TodoError>").unwrap(),
            KType::Result(boxed(KType::Unit), boxed(named("TodoError")))
        );
        assert_eq!(code_of(ret("Result<i32, ()>")), code::E0001);
        assert_eq!(code_of(ret("Vec<()>")), code::E0001);
    }

    #[test]
    fn results_and_streams_only_in_return_position() {
        assert_eq!(code_of(field("Result<i32, E>")), code::E0005);
        assert_eq!(code_of(field("Option<Result<i32, E>>")), code::E0005);
        assert_eq!(code_of(ret("Option<Result<i32, E>>")), code::E0005);
        assert_eq!(code_of(ret("Result<Result<i32, A>, B>")), code::E0005);
        assert_eq!(code_of(field("impl Stream<Item = i32>")), code::E0005);
        assert_eq!(code_of(ret("Vec<impl Stream<Item = i32>>")), code::E0005);
        assert_eq!(
            code_of(ret("Result<i32, impl Stream<Item = i32>>")),
            code::E0005
        );
    }

    #[test]
    fn return_shapes() {
        assert_eq!(ret("i32").unwrap(), KType::I32);
        assert_eq!(
            ret("Result<Todo, TodoError>").unwrap(),
            KType::Result(boxed(named("Todo")), boxed(named("TodoError")))
        );
        assert_eq!(
            ret("impl Stream<Item = u32>").unwrap(),
            KType::Stream(boxed(KType::U32))
        );
        assert_eq!(
            ret("impl futures_core::Stream<Item = Todo> + Send + 'static").unwrap(),
            KType::Stream(boxed(named("Todo")))
        );
        assert_eq!(
            ret("Result<impl Stream<Item = Todo>, TodoError>").unwrap(),
            KType::Result(
                boxed(KType::Stream(boxed(named("Todo")))),
                boxed(named("TodoError"))
            )
        );
        assert_eq!(code_of(ret("Result<Todo>")), code::E0001);
    }

    #[test]
    fn a_stream_of_results_maps_to_the_fallible_opening_shape() {
        // ADR-036 decision 4: the schema shape is `Result<Stream<T>, E>` in every spelling.
        let expected = KType::Result(
            boxed(KType::Stream(boxed(named("Todo")))),
            boxed(named("TodoError")),
        );
        for src in [
            "impl Stream<Item = Result<Todo, TodoError>>",
            "impl Stream<Item = Result<Todo, TodoError>> + Send + 'static",
            "Result<impl Stream<Item = Result<Todo, TodoError>>, TodoError>",
            "Result<impl Stream<Item = Todo>, TodoError>",
        ] {
            assert_eq!(ret(src).unwrap(), expected, "{src}");
        }
        // Two error types for one stream: E0005, naming both.
        let err = ret("Result<impl Stream<Item = Result<Todo, AError>>, BError>").unwrap_err();
        assert_eq!(err.diag.code, code::E0005);
        assert!(
            err.diag
                .message()
                .contains("fail with `AError` but its opening fails with `BError`"),
            "{}",
            err.diag.message()
        );
        assert!(err.diag.help.contains("BError"), "{}", err.diag.help);
        // A `Result` item is still an error side: a non-error type there is refused like
        // anywhere else, and so is a stream of results in a field.
        assert_eq!(
            code_of(ret("impl Stream<Item = Result<Todo, String>>")),
            code::E0001
        );
        assert_eq!(
            code_of(field("impl Stream<Item = Result<Todo, TodoError>>")),
            code::E0005
        );
        assert_eq!(
            code_of(ret("Vec<impl Stream<Item = Result<Todo, TodoError>>>")),
            code::E0005
        );
    }

    #[test]
    fn map_return_defaults_to_unit() {
        let sig: syn::Signature = syn::parse_quote!(fn f());
        assert_eq!(
            map_return_at(&sig.output, Pos::PortReturn).unwrap(),
            KType::Unit
        );
        let sig: syn::Signature = syn::parse_quote!(fn f() -> u8);
        assert_eq!(
            map_return_at(&sig.output, Pos::PortReturn).unwrap(),
            KType::U8
        );
    }

    #[test]
    fn unsupported_types_are_e0001() {
        for src in [
            "usize",
            "isize",
            "i128",
            "u128",
            "char",
            "&str",
            "&[u8]",
            "&mut String",
            "(i32, i32)",
            "[u8; 4]",
            "*const u8",
            "HashSet<String>",
            "BTreeSet<u8>",
            "VecDeque<u8>",
            "Rc<String>",
            "Arc<String>",
            "std::cell::RefCell<u8>",
            "Mutex<u8>",
            "PathBuf",
            "Instant",
            "SystemTime",
            "Page<Todo>",
            "Signal<i32>",
            "Computed<i32>",
            "Lazy<Todo>",
            "Ctx",
            "Self",
            "<T as Trait>::Out",
            "!",
        ] {
            assert_eq!(code_of(field(src)), code::E0001, "{src}");
        }
    }

    #[test]
    fn trait_objects_and_callbacks_are_e0004() {
        for src in [
            "dyn Any",
            "Box<dyn Any>",
            "Vec<Box<dyn Any>>",
            "Box<dyn Fn(i32) -> i32>",
            "fn(i32) -> i32",
            "impl Fn(i32)",
            "impl Display",
            "impl Iterator<Item = u8>",
        ] {
            assert_eq!(code_of(field(src)), code::E0004, "{src}");
        }
        assert_eq!(code_of(ret("impl Iterator<Item = u8>")), code::E0004);
    }

    #[test]
    fn trait_object_in_a_field_is_e0012_on_the_whole_type() {
        let err = map_field(&ty("Vec<Box<dyn Any>>"), "items", "Cart").unwrap_err();
        assert_eq!(err.diag.code, code::E0012);
        assert_eq!(
            err.diag.what,
            "`Vec<Box<dyn Any>>` cannot cross the boundary"
        );
        assert!(err.diag.help.contains("`items`"));
        // Non-trait-object failures keep their own code.
        assert_eq!(
            map_field(&ty("&str"), "name", "Cart")
                .unwrap_err()
                .diag
                .code,
            code::E0001
        );
    }

    #[test]
    fn lifetimes_are_e0003() {
        for src in [
            "&'a str",
            "Foo<'a>",
            "Vec<&'a str>",
            "std::borrow::Cow<'a, str>",
        ] {
            assert_eq!(code_of(field(src)), code::E0003, "{src}");
        }
        assert_eq!(code_of(ret("impl Stream<Item = u8> + 'a")), code::E0003);
    }

    #[test]
    fn stream_bounds_accept_send_and_static() {
        assert!(ret("impl Stream<Item = u8> + Send + Unpin + 'static").is_ok());
        assert_eq!(code_of(ret("impl Stream<Item = u8> + Clone")), code::E0004);
        assert_eq!(code_of(ret("impl Stream")), code::E0001);
    }

    #[test]
    fn generic_user_types_are_rejected_but_plain_ones_are_named() {
        assert_eq!(field("Todo").unwrap(), named("Todo"));
        assert_eq!(code_of(field("Page<Todo>")), code::E0001);
        assert_eq!(code_of(field("Wrapper<'a, Todo>")), code::E0003);
    }

    #[test]
    fn parens_and_groups_are_transparent() {
        assert_eq!(field("(String)").unwrap(), KType::String);
    }

    #[test]
    fn nested_options_are_e0063_at_any_depth() {
        assert_eq!(code_of(field("Option<Option<i32>>")), code::E0063);
        assert_eq!(code_of(field("Vec<Option<Option<i32>>>")), code::E0063);
        assert_eq!(code_of(field("Option<Box<Option<u8>>>")), code::E0063);
        assert_eq!(code_of(ret("Option<Option<u8>>")), code::E0063);
        assert!(field("Option<Vec<Option<u8>>>").is_ok());
        assert!(field("Vec<Option<u8>>").is_ok());
    }

    #[test]
    fn handle_has_no_schema_type() {
        let err = field("Handle").unwrap_err();
        assert_eq!(err.diag.code, code::E0001);
        assert!(err.diag.what.contains("`Handle`"), "{:?}", err.diag);
        assert!(err.diag.help.contains("rename it"), "{:?}", err.diag);
        assert_eq!(code_of(field("Vec<Handle>")), code::E0001);
    }

    #[test]
    fn the_error_side_of_a_result_must_be_a_name() {
        for src in [
            "Result<u8, String>",
            "Result<u8, u32>",
            "Result<u8, Vec<String>>",
        ] {
            let err = ret(src).unwrap_err();
            assert_eq!(err.diag.code, code::E0001, "{src}");
            assert!(
                err.diag.what.contains("error type"),
                "{src}: {:?}",
                err.diag
            );
        }
        assert!(ret("Result<u8, crate::errors::TodoError>").is_ok());
        assert!(ret("Result<u8, Box<TodoError>>").is_ok());
    }

    #[test]
    fn associated_types_and_boxed_streams_have_their_own_messages() {
        let err = field("Self::Output").unwrap_err();
        assert!(err.diag.what.contains("Self::Output"), "{:?}", err.diag);
        for src in [
            "Pin<Box<dyn Stream<Item = u8> + Send>>",
            "Box<dyn Stream<Item = u8>>",
            "Pin<Box<dyn futures_core::Stream<Item = u8>>>",
        ] {
            let err = ret(src).unwrap_err();
            assert_eq!(err.diag.code, code::E0004, "{src}");
            assert!(
                err.diag.help.contains("return `impl Stream<Item = T>"),
                "{src}: {:?}",
                err.diag
            );
        }
    }

    #[test]
    fn a_result_nested_in_a_return_says_nested_not_return_type() {
        let err = ret("Vec<Result<u8, E>>").unwrap_err();
        assert_eq!(err.diag.code, code::E0005);
        assert!(
            err.diag.what.contains("nested inside another type"),
            "{:?}",
            err.diag
        );
        let err = ret("Vec<impl Stream<Item = u8>>").unwrap_err();
        assert!(
            err.diag.what.contains("nested inside another type"),
            "{:?}",
            err.diag
        );
        // In a field the old wording stands.
        let err = field("Result<u8, E>").unwrap_err();
        assert!(err.diag.what.contains("a field"), "{:?}", err.diag);
    }

    #[test]
    fn names_the_mapper_recognises_come_with_a_rename_hint() {
        for src in ["Instant", "Path", "SystemTime", "Ctx", "PathBuf"] {
            let err = field(src).unwrap_err();
            assert!(err.diag.help.contains("rename it"), "{src}: {:?}", err.diag);
        }
    }

    #[test]
    fn meta_tokens_nest_with_references() {
        let meta = quote!(::undra::meta);
        let kt = KType::Result(
            boxed(KType::Map(
                boxed(KType::Uuid),
                boxed(KType::Vec(boxed(named("Todo")))),
            )),
            boxed(named("TodoError")),
        );
        let rendered = kt.meta(&meta).to_string().replace(' ', "");
        assert_eq!(
            rendered,
            "::undra::meta::TypeRefMeta::Result(&::undra::meta::TypeRefMeta::Map(&::undra::meta::TypeRefMeta::Uuid,&::undra::meta::TypeRefMeta::Vec(&::undra::meta::TypeRefMeta::Named(\"Todo\"))),&::undra::meta::TypeRefMeta::Named(\"TodoError\"))"
        );
    }

    #[test]
    fn ty_string_tidies_token_spacing() {
        assert_eq!(ty_string(&ty("Vec<Box<dyn Any>>")), "Vec<Box<dyn Any>>");
        assert_eq!(ty_string(&ty("&str")), "&str");
        assert_eq!(
            ty_string(&ty("HashMap<String, i32>")),
            "HashMap<String, i32>"
        );
        assert_eq!(ty_string(&ty("std::string::String")), "std::string::String");
        assert_eq!(ty_string(&ty("(i32, u8)")), "(i32, u8)");
    }

    #[test]
    fn diagnostics_mention_what_why_help() {
        let err = field("&str").unwrap_err();
        assert_eq!(err.diag.what, "`&str` cannot cross the boundary");
        assert!(err.diag.why.contains("references"));
        assert_eq!(err.diag.help, "use an owned `String`");
        let message = err.into_error().to_string();
        assert!(message.starts_with("error[undra::E0001]: `&str` cannot cross the boundary"));
    }
}
