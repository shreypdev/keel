//! Objects (`#[undra::api] impl Type { .. }`) and free functions (`#[undra::api] fn`).
//!
//! For an impl block the macro keeps the impl as written and adds:
//!
//! * `impl UndraObject for Type { TYPE_ID, NAME }`;
//! * `fn __undra_dispatch_Type(rt: &dyn Any, call: DispatchCall) -> DispatchOutcome`, which
//!   downcasts `rt` to `&Runtime`, matches `call.method_id` against per-method constants
//!   (`ids::method_id`), decodes the arguments in order, resolves `self` with
//!   `rt.object::<Type>(call.handle)`, runs the method and answers a synchronous result with
//!   `rt.sync_ok(&value)` / `rt.sync_err(&error)` (which write the reply straight into the
//!   caller's buffer under `call_sync`, ADR-028, and build a `DispatchResult::Sync` anywhere
//!   else) and an async or stream result as `DispatchResult::{Async, Stream}`; malformed
//!   requests and stale handles answer `DispatchResult::BadRequest` with a reason, and only an
//!   unknown method id answers `DispatchResult::Unknown`. The decoded arguments are bound to
//!   positional locals (`__undra_a0`, ..), never to the user's parameter names, so no parameter
//!   name can collide with a generated local;
//! * `static __UNDRA_META_Type: ObjectMeta` and its registration;
//! * the identity checks of `check.rs` for every parameter and return type, and the hidden
//!   `__UNDRA_IS_OBJECT` marker that lets them say "an object cannot be a value" (E0064).
//!
//! A type takes one `#[undra::api] impl` block. A macro cannot see the other blocks of the type,
//! so a second one is found by `rustc`: the expansion carries a constant named after the rule
//! (`_undra_error_E0007_<Type>_has_two_undra_api_impl_blocks_merge_them_into_one`), whose
//! duplicate-definition error reads as the diagnostic and comes first. Everything else the block
//! defines that is not an `impl` lives in an anonymous `const _` block, so only the two
//! unavoidable conflicts follow it (`UndraObject` implemented twice, the `__UNDRA_IS_OBJECT`
//! marker defined twice) and not one error for every generated name.
//!
//! Constructors (a `pub fn` without receiver that returns `Self` or `Result<Self, E>`) are
//! listed separately in the meta. Their dispatch arm builds the value, inserts it into the
//! object table (`rt.insert_object(Arc::new(value))`) and replies with the handle. In a
//! store's impl block (`#[undra::api(store)]`) the arm also attaches the signals first (see
//! `store.rs`), and struct literals of the store inside the block get the hidden cell field
//! added.

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::visit_mut::{self, VisitMut};
use syn::{FnArg, ImplItem, ItemFn, ItemImpl, Pat, ReturnType, Signature, Type, Visibility};

use super::attrs::{Site, docs, is_undra_macro_path, take};
use super::check::{Checks, panic_text, primary_trait};
use super::common::{
    GenericOn, check_generics, derived, item_root, mentions_self, param_meta, send_assertion,
    submit,
};
use super::diag::{Diag, Errors, code, in_instance};
use super::generic_fn::{self, Label, Plan};
use super::naming::{fnv1a32, unraw};
use super::paths::Root;
use super::types::{Allow, KType, Pos, map_error_type, map_method_return, map_type, ty_string};

/// A leading `ctx: Ctx` / `ctx: &Ctx` parameter.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CtxParam {
    pub(crate) by_ref: bool,
}

/// A wire parameter.
#[derive(Debug)]
pub(crate) struct ParamModel {
    pub(crate) name: String,
    pub(crate) ty: Type,
    pub(crate) kty: KType,
    /// How the dispatcher decodes it: a value, a handle to an object, or a host callback.
    pub(crate) plan: ParamPlan,
}

/// How an object parameter is spelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ObjShape {
    /// `&T`.
    Ref,
    /// `Arc<T>`.
    Arc,
    /// `Option<&T>`.
    OptionRef,
    /// `Option<Arc<T>>`.
    OptionArc,
    /// `Vec<Arc<T>>`.
    VecArc,
}

/// How the dispatcher turns the bytes of a parameter into the Rust value (ADR-040, ADR-041).
#[derive(Clone, Debug)]
pub(crate) enum ParamPlan {
    /// The wire value itself.
    Plain,
    /// A handle (`Option` or `Vec` of handles) resolved to the object, `elem` being `T`.
    Object { elem: Type, shape: ObjShape },
    /// A host callback instance (`Option` of one) turned into a proxy; `dyn_ty` is `dyn Trait`.
    Callback { dyn_ty: Type, optional: bool },
}

fn peel(mut ty: &Type) -> &Type {
    loop {
        match ty {
            Type::Paren(inner) => ty = &inner.elem,
            Type::Group(inner) => ty = &inner.elem,
            _ => return ty,
        }
    }
}

/// The single type argument of a path type such as `Arc<T>` or `Option<T>`.
fn sole_arg(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = peel(ty) else {
        return None;
    };
    let seg = path.path.segments.last()?;
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    let mut types = args.args.iter().filter_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    });
    match (types.next(), types.next()) {
        (Some(ty), None) => Some(ty),
        _ => None,
    }
}

/// What `T` is in `&T` or `Arc<T>`, and which of the two it was.
fn object_of(ty: &Type) -> Option<(Type, bool)> {
    match peel(ty) {
        Type::Reference(reference) => Some(((*reference.elem).clone(), true)),
        other => sole_arg(other).map(|inner| (inner.clone(), false)),
    }
}

/// Reads how a parameter the mapper accepted is spelled.
fn plan_of(ty: &Type, kty: &KType) -> ParamPlan {
    match kty {
        KType::Object(_) => match object_of(ty) {
            Some((elem, true)) => ParamPlan::Object {
                elem,
                shape: ObjShape::Ref,
            },
            Some((elem, false)) => ParamPlan::Object {
                elem,
                shape: ObjShape::Arc,
            },
            None => ParamPlan::Plain,
        },
        KType::Callback(_) => match sole_arg(ty) {
            Some(inner) => ParamPlan::Callback {
                dyn_ty: primary_trait(inner),
                optional: false,
            },
            None => ParamPlan::Plain,
        },
        KType::Option(inner) => match (&**inner, sole_arg(ty)) {
            (KType::Object(_), Some(arg)) => match object_of(arg) {
                Some((elem, true)) => ParamPlan::Object {
                    elem,
                    shape: ObjShape::OptionRef,
                },
                Some((elem, false)) => ParamPlan::Object {
                    elem,
                    shape: ObjShape::OptionArc,
                },
                None => ParamPlan::Plain,
            },
            (KType::Callback(_), Some(arg)) => match sole_arg(arg) {
                Some(dyn_arg) => ParamPlan::Callback {
                    dyn_ty: primary_trait(dyn_arg),
                    optional: true,
                },
                None => ParamPlan::Plain,
            },
            _ => ParamPlan::Plain,
        },
        KType::Vec(inner) if matches!(**inner, KType::Object(_)) => match sole_arg(ty) {
            Some(arg) => match object_of(arg) {
                Some((elem, false)) => ParamPlan::Object {
                    elem,
                    shape: ObjShape::VecArc,
                },
                _ => ParamPlan::Plain,
            },
            None => ParamPlan::Plain,
        },
        _ => ParamPlan::Plain,
    }
}

/// Which kind of function a signature is analysed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kindred {
    /// A method, constructor or free function: objects and callbacks may be parameters.
    Callable,
    /// A query or mutation: values only.
    Query,
}

/// What a function is for the dispatcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Method,
    Constructor,
    Function,
}

/// A method, constructor or free function, analysed.
#[derive(Debug)]
pub(crate) struct FnModel {
    pub(crate) ident: syn::Ident,
    pub(crate) name: String,
    pub(crate) kind: Kind,
    pub(crate) is_async: bool,
    pub(crate) ctx: Option<CtxParam>,
    pub(crate) params: Vec<ParamModel>,
    /// The schema return type (for constructors: `Named(Type)` or `Result<Named(Type), E>`).
    pub(crate) ret: KType,
    /// A constructor that returns `Arc<Self>` (the singleton pattern, ADR-040): the dispatcher
    /// interns the object instead of inserting a new one.
    pub(crate) ctor_shared: bool,
    /// The `T` of a returned `impl Stream<Item = T>` (also inside `Result<.., E>`). For a stream
    /// that can fail part-way (ADR-036) it is the `Result<T, E>` as written.
    pub(crate) stream_item: Option<Type>,
    /// The Rust return type wraps the stream in a `Result` (`Result<impl Stream<..>, E>`): the
    /// opening can fail. Only meaningful when `stream_item` is set.
    pub(crate) stream_in_result: bool,
    pub(crate) docs: String,
    /// What the items generated for this function are named after: its name, or for one
    /// instantiation of a generic function `newest_of_Todo` (ADR-058).
    pub(crate) suffix: String,
    /// The generic arguments of the call the dispatcher makes (`::<Todo>`), for an instantiation of
    /// a generic function or method.
    pub(crate) turbofish: Option<TokenStream>,
    /// The generic function this is an instantiation of.
    pub(crate) generic: Option<Label>,
}

/// Whether a returned stream sits on the `Ok` side of a `Result` (its opening can fail).
fn stream_in_result(output: &ReturnType) -> bool {
    let ReturnType::Type(_, ty) = output else {
        return false;
    };
    let mut ty: &Type = ty;
    loop {
        match ty {
            Type::Paren(inner) => ty = &inner.elem,
            Type::Group(inner) => ty = &inner.elem,
            _ => break,
        }
    }
    matches!(ty, Type::Path(_)) && super::types::result_parts(ty).is_some()
}

/// The `T` of `impl Stream<Item = T>` at the top of a return type or on the `Ok` side of a
/// returned `Result`.
fn stream_item_type(output: &ReturnType) -> Option<Type> {
    fn find(ty: &Type) -> Option<Type> {
        match ty {
            Type::ImplTrait(impl_trait) => impl_trait.bounds.iter().find_map(|bound| {
                let syn::TypeParamBound::Trait(bound) = bound else {
                    return None;
                };
                let seg = bound.path.segments.last()?;
                if seg.ident != "Stream" {
                    return None;
                }
                let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
                    return None;
                };
                args.args.iter().find_map(|arg| match arg {
                    syn::GenericArgument::AssocType(assoc) if assoc.ident == "Item" => {
                        Some(assoc.ty.clone())
                    }
                    _ => None,
                })
            }),
            Type::Path(path) => {
                let seg = path.path.segments.last()?;
                if seg.ident != "Result" {
                    return None;
                }
                let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
                    return None;
                };
                match args.args.first()? {
                    syn::GenericArgument::Type(ok) => find(ok),
                    _ => None,
                }
            }
            Type::Paren(inner) => find(&inner.elem),
            Type::Group(inner) => find(&inner.elem),
            _ => None,
        }
    }
    match output {
        ReturnType::Type(_, ty) => find(ty),
        ReturnType::Default => None,
    }
}

/// The receiver/parameter part of a signature.
pub(crate) struct Analysis {
    pub(crate) has_receiver: bool,
    pub(crate) ctx: Option<CtxParam>,
    pub(crate) params: Vec<ParamModel>,
    pub(crate) is_async: bool,
}

fn is_ctx_path(ty: &Type) -> bool {
    match ty {
        Type::Path(path) if path.qself.is_none() => path
            .path
            .segments
            .last()
            .is_some_and(|seg| seg.ident == "Ctx" && seg.arguments.is_none()),
        Type::Paren(inner) => is_ctx_path(&inner.elem),
        Type::Group(inner) => is_ctx_path(&inner.elem),
        _ => false,
    }
}

/// `Some((param, shared))` if `ty` is `Ctx` or a reference to it; `shared` is false for
/// `&mut Ctx`, which is diagnosed.
fn ctx_kind(ty: &Type) -> Option<(CtxParam, bool)> {
    match ty {
        Type::Reference(reference) if is_ctx_path(&reference.elem) => {
            Some((CtxParam { by_ref: true }, reference.mutability.is_none()))
        }
        _ if is_ctx_path(ty) => Some((CtxParam { by_ref: false }, true)),
        _ => None,
    }
}

/// Whether `ty` is the `Ctx` parameter of a function (`Ctx` or a reference to it).
pub(crate) fn is_ctx(ty: &Type) -> bool {
    ctx_kind(ty).is_some()
}

fn shape_error(what: String, node: &impl quote::ToTokens, why: &str, help: &str) -> syn::Error {
    Diag::new(code::E0007, what, why, help).on(node)
}

fn mut_self(fn_name: &str, node: &impl quote::ToTokens) -> syn::Error {
    Diag::new(
        code::E0020,
        format!("`&mut self` receiver on method `{fn_name}`"),
        "objects are shared as `Arc<Type>` across calls and threads, so methods only ever get `&self`",
        "take `&self` and mutate through signals (`Signal<T>`), a `Mutex` or another form of interior mutability",
    )
    .on(node)
}

fn self_by_value(fn_name: &str, node: &impl quote::ToTokens) -> syn::Error {
    Diag::new(
        code::E0021,
        format!("`self` by value on method `{fn_name}`"),
        "an object is shared across calls; a method cannot consume it",
        "take `&self` and clone whatever you need to keep",
    )
    .on(node)
}

fn is_plain_path_ref(path: &syn::TypePath) -> bool {
    path.qself.is_none()
        && path.path.segments.len() == 1
        && path.path.segments[0].arguments.is_none()
}

/// `Self` or a plain type name, without arguments.
fn is_plain_path(ty: &Type) -> bool {
    matches!(ty, Type::Path(path) if is_plain_path_ref(path))
}

fn typed_receiver(fn_name: &str, node: &impl quote::ToTokens, ty: &Type) -> syn::Error {
    Diag::new(
        code::E0007,
        format!("the receiver `self: {}` of method `{fn_name}` is not supported", ty_string(ty)),
        "the dispatcher resolves the object from its handle and calls the method with `&Type`; a receiver such as `Arc<Self>`, `Box<Self>` or `Pin<&Self>` needs a different call",
        "write `&self`; clone an `Arc` of the object inside the method if you need one",
    )
    .on(node)
}

/// Checks the generics, receiver and parameters of `sig` and collects the parameters. `generics`
/// says what the signature is, for the text of E0002 for its type parameters; `None` when the
/// caller has dealt with the generics itself (a generic function with a list, ADR-058).
pub(crate) fn analyze(
    sig: &mut Signature,
    errors: &mut Errors,
    site: Kindred,
    generics: Option<GenericOn>,
) -> Analysis {
    let fn_name = sig.ident.to_string();
    if let Some(on) = generics {
        check_generics(&sig.generics, &fn_name, on, errors);
    }
    if sig.unsafety.is_some() || sig.abi.is_some() {
        errors.push(shape_error(
            format!("`{fn_name}` is `unsafe` or `extern`"),
            &sig.ident,
            "the dispatcher calls the function like any safe Rust function",
            "remove `unsafe` / `extern`, or wrap the call in a safe function",
        ));
    }
    if let Some(variadic) = &sig.variadic {
        errors.push(shape_error(
            format!("`{fn_name}` is variadic"),
            variadic,
            "C-style variadics cannot be described by the schema",
            "take a `Vec<T>` instead",
        ));
    }

    let mut has_receiver = false;
    if let Some(receiver) = sig.receiver() {
        has_receiver = true;
        if receiver.colon_token.is_some() {
            match &*receiver.ty {
                Type::Reference(reference) if reference.mutability.is_some() => {
                    errors.push(mut_self(&fn_name, receiver));
                }
                // `self: &Self` (or `&Type`).
                Type::Reference(reference) if is_plain_path(&reference.elem) => {}
                // `self: Self` consumes the object; any other typed receiver is not callable.
                Type::Path(path) if is_plain_path_ref(path) => {
                    errors.push(self_by_value(&fn_name, receiver));
                }
                other => errors.push(typed_receiver(&fn_name, receiver, other)),
            }
        } else {
            match &receiver.reference {
                None => errors.push(self_by_value(&fn_name, receiver)),
                Some(_) if receiver.mutability.is_some() => {
                    errors.push(mut_self(&fn_name, receiver));
                }
                Some((_, Some(lifetime))) => errors.push(
                    Diag::new(
                        code::E0003,
                        format!("lifetime `{lifetime}` on the receiver of `{fn_name}`"),
                        "methods are called through an object handle; a borrow of `self` cannot outlive the call",
                        "write `&self`",
                    )
                    .on(receiver),
                ),
                Some((_, None)) => {}
            }
        }
    }

    let mut ctx = None;
    let mut params = Vec::new();
    let mut position = 0usize;
    for arg in &mut sig.inputs {
        let FnArg::Typed(pat_type) = arg else {
            continue;
        };
        take(&mut pat_type.attrs, Site::NOTHING, errors);
        let index = position;
        position += 1;

        if let Some((param, shared)) = ctx_kind(&pat_type.ty) {
            if has_receiver {
                errors.push(
                    Diag::new(
                        code::E0001,
                        format!("`Ctx` parameter on method `{fn_name}`"),
                        "methods reach the runtime through the object: `Ctx` is only injected into constructors, free functions, queries and mutations",
                        "keep a `WeakCtx` (`ctx.downgrade()`) in the object when it is constructed and upgrade it in the method, or use `Ctx::current()`",
                    )
                    .on(&pat_type.ty),
                );
            } else if index != 0 {
                errors.push(
                    Diag::new(
                        code::E0001,
                        format!("`Ctx` must be the first parameter of `{fn_name}`"),
                        "the dispatcher injects the context before the decoded arguments",
                        "move `ctx` to the front",
                    )
                    .on(&pat_type.ty),
                );
            } else if !shared {
                errors.push(
                    Diag::new(
                        code::E0001,
                        format!("`&mut Ctx` parameter on `{fn_name}`"),
                        "`Ctx` is a cheap shared handle; it is passed by value or by shared reference",
                        "write `ctx: &Ctx` or `ctx: Ctx`",
                    )
                    .on(&pat_type.ty),
                );
            } else {
                ctx = Some(param);
            }
            continue;
        }

        let plain_ident = match &*pat_type.pat {
            Pat::Ident(pat_ident) if pat_ident.by_ref.is_none() && pat_ident.subpat.is_none() => {
                Some(pat_ident.ident.clone())
            }
            _ => None,
        };
        let Some(ident) = plain_ident else {
            errors.push(shape_error(
                format!(
                    "parameter `{}` of `{fn_name}` is a pattern",
                    ty_string(&pat_type.pat)
                ),
                &pat_type.pat,
                "the schema names every parameter so the other languages can label arguments",
                "use a plain name: `title: String`",
            ));
            continue;
        };
        let (pos, allow) = match site {
            Kindred::Callable => (Pos::Param, Allow::PARAM),
            Kindred::Query => (Pos::QueryParam, Allow::NONE),
        };
        let kty = match map_type(&pat_type.ty, pos, allow) {
            Ok(kty) => kty,
            Err(err) => {
                errors.push(err.into_error());
                KType::Unit
            }
        };
        let plan = plan_of(&pat_type.ty, &kty);
        params.push(ParamModel {
            name: unraw(&ident),
            ty: (*pat_type.ty).clone(),
            kty,
            plan,
        });
    }

    Analysis {
        has_receiver,
        ctx,
        params,
        is_async: sig.asyncness.is_some(),
    }
}

/// How a constructor returns.
enum CtorReturn {
    /// `-> Self` or `-> Type`.
    Plain,
    /// `-> Result<Self, E>` or `-> Result<Type, E>`, with the error type.
    Fallible(Box<Type>),
    /// `-> Arc<Self>` or `-> Arc<Type>`: an instance that may already exist (ADR-040).
    Shared,
    /// `-> Result<Arc<Self>, E>`.
    FallibleShared(Box<Type>),
}

impl CtorReturn {
    fn shared(&self) -> bool {
        matches!(self, CtorReturn::Shared | CtorReturn::FallibleShared(_))
    }
}

/// `Arc<Self>` / `Arc<Type>`.
fn is_arc_self(ty: &Type, type_name: &str) -> bool {
    match peel(ty) {
        Type::Path(path) if path.qself.is_none() => {
            let Some(seg) = path.path.segments.last() else {
                return false;
            };
            seg.ident == "Arc" && sole_arg(ty).is_some_and(|inner| is_self_type(inner, type_name))
        }
        _ => false,
    }
}

fn is_self_type(ty: &Type, type_name: &str) -> bool {
    match ty {
        Type::Path(path) if path.qself.is_none() && path.path.segments.len() == 1 => {
            let seg = &path.path.segments[0];
            (seg.ident == "Self" || seg.ident == type_name) && seg.arguments.is_none()
        }
        Type::Paren(inner) => is_self_type(&inner.elem, type_name),
        Type::Group(inner) => is_self_type(&inner.elem, type_name),
        _ => false,
    }
}

fn ctor_return(output: &ReturnType, type_name: &str) -> Option<CtorReturn> {
    let ReturnType::Type(_, ty) = output else {
        return None;
    };
    if is_self_type(ty, type_name) {
        return Some(CtorReturn::Plain);
    }
    if is_arc_self(ty, type_name) {
        return Some(CtorReturn::Shared);
    }
    let Type::Path(path) = &**ty else {
        return None;
    };
    let seg = path.path.segments.last()?;
    if seg.ident != "Result" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    let mut types = args.args.iter().filter_map(|arg| match arg {
        syn::GenericArgument::Type(ty) => Some(ty),
        _ => None,
    });
    let ok = types.next()?;
    let err = types.next()?;
    if is_self_type(ok, type_name) {
        Some(CtorReturn::Fallible(Box::new(err.clone())))
    } else if is_arc_self(ok, type_name) {
        Some(CtorReturn::FallibleShared(Box::new(err.clone())))
    } else {
        None
    }
}

/// Appends `+ 'static` to a returned `impl Stream<Item = T>` that does not have it.
///
/// A returned stream outlives the call that created it, so it can never borrow from `self`.
/// Since Rust 2024 an `impl Trait` in return position captures the lifetime of `&self` by
/// default; the bound tells the compiler (and the dispatcher, which must hand the stream to
/// the runtime) that it does not. If the body really borrows `self`, the error appears in the
/// user's function, where it belongs. The macro adds the bound instead of asking for it
/// because it carries no information the schema needs.
fn ensure_static_streams(ty: &mut Type) {
    match ty {
        Type::ImplTrait(impl_trait) => {
            let is_stream = impl_trait.bounds.iter().any(|bound| {
                matches!(bound, syn::TypeParamBound::Trait(t)
                    if t.path.segments.last().is_some_and(|seg| seg.ident == "Stream"))
            });
            let has_static = impl_trait.bounds.iter().any(
                |bound| matches!(bound, syn::TypeParamBound::Lifetime(l) if l.ident == "static"),
            );
            if is_stream && !has_static {
                impl_trait.bounds.push(syn::parse_quote!('static));
            }
        }
        Type::Path(path) => {
            let Some(seg) = path.path.segments.last_mut() else {
                return;
            };
            if seg.ident != "Result" {
                return;
            }
            if let syn::PathArguments::AngleBracketed(args) = &mut seg.arguments {
                if let Some(syn::GenericArgument::Type(ok)) = args.args.first_mut() {
                    ensure_static_streams(ok);
                }
            }
        }
        Type::Paren(inner) => ensure_static_streams(&mut inner.elem),
        Type::Group(inner) => ensure_static_streams(&mut inner.elem),
        _ => {}
    }
}

fn ensure_static_streams_in(output: &mut ReturnType) {
    if let ReturnType::Type(_, ty) = output {
        ensure_static_streams(ty);
    }
}

// ---------------------------------------------------------------------------------------------
// Code generation shared by impl blocks and free functions
// ---------------------------------------------------------------------------------------------

/// What the arm of a function needs to know about its surroundings.
struct Target<'a> {
    /// The impl type for methods and constructors.
    self_ty: Option<&'a Type>,
    /// Whether the impl block is a store's (constructors attach the signals).
    store: bool,
}

/// Which helper items an expansion needs inside its dispatcher.
#[derive(Default)]
struct Needs {
    send_assert: bool,
    map_stream: bool,
    opening_stream: bool,
    /// `impl Stream<Item = Result<T, E>>` (ADR-036): items and a final typed error.
    try_stream: bool,
    /// An opening future whose stream is already encoded (the fallible-items case).
    opening_raw: bool,
}

/// The local that holds the decoded argument at `index`.
pub(crate) fn arg_local(index: usize) -> syn::Ident {
    format_ident!("__undra_a{}", index)
}

/// The local that holds the raw handle of the object parameter at `index`.
fn handle_local(index: usize) -> syn::Ident {
    format_ident!("__undra_h{}", index)
}

/// The local that holds the host instance of the callback parameter at `index`.
fn callback_local(index: usize) -> syn::Ident {
    format_ident!("__undra_c{}", index)
}

fn enc(wire: &TokenStream, value: &TokenStream) -> TokenStream {
    quote!(#wire::Encode::encode_to_vec(&#value))
}

fn is_stream_ok(ret: &KType) -> bool {
    matches!(ret, KType::Result(ok, _) if matches!(**ok, KType::Stream(_)))
}

/// The expression of the outcome of a method, function or query-like call, of type
/// `DispatchOutcome`.
///
/// A synchronous answer goes through `Runtime::sync_ok` / `sync_err`, which encode straight
/// into the caller's reply buffer under `call_sync` (no heap allocation, ADR-028) and build a
/// `DispatchResult::Sync` anywhere else. An async future and a stream are boxed as before.
fn call_result(root: &Root, m: &FnModel, call: &TokenStream, needs: &mut Needs) -> TokenStream {
    let wire = root.wire();
    let runtime = root.runtime();
    let span = m.ident.span();
    let sync_ok = |value: TokenStream| quote!(__rt.sync_ok(&#value, #wire::Encode::encode));
    let sync_err = |value: TokenStream| quote!(__rt.sync_err(&#value, #wire::Encode::encode));
    let send_fn = send_assertion(span);
    let assert_send = |what: TokenStream| quote_spanned!(span=> #send_fn(&#what););
    let out_ok = |value: TokenStream| {
        let bytes = enc(&wire, &value);
        quote!(::core::result::Result::<_, ::std::vec::Vec<u8>>::Ok(#bytes))
    };
    let out_err = |value: TokenStream| {
        let bytes = enc(&wire, &value);
        quote!(::core::result::Result::<::std::vec::Vec<u8>, _>::Err(#bytes))
    };

    // An object (or several) handed to the host (ADR-040): lowered to handles first.
    if let Some((shape, in_result)) = object_return(&m.ret) {
        return object_result(root, m, call, shape, in_result, needs);
    }

    // A stream whose items are `Result<T, E>` (ADR-036): an `Err(e)` item ends it with flag 2.
    let fallible_items = m
        .stream_item
        .as_ref()
        .is_some_and(|item| super::types::result_parts(item).is_some());
    if fallible_items && is_stream_ok(&m.ret) {
        return fallible_stream_result(root, m, call, needs);
    }

    match (&m.ret, m.is_async) {
        // A stream.
        (KType::Stream(_), false) => {
            needs.map_stream = true;
            needs.send_assert = true;
            let check = assert_send(quote!(__stream));
            quote! {{
                let __stream = __UndraMap(::std::boxed::Box::pin(#call));
                #check
                __undra_out(#runtime::DispatchResult::Stream(::std::boxed::Box::pin(__stream)))
            }}
        }
        (KType::Stream(_), true) => {
            needs.opening_stream = true;
            needs.send_assert = true;
            let item = m
                .stream_item
                .as_ref()
                .expect("a stream return has an item type");
            let check_inner = assert_send(quote!(__s));
            let check = assert_send(quote!(__stream));
            // The stream is boxed inside the future: the future's output must not name the
            // lifetime of the borrow of `self` the call captures (Rust 2024 opaque types do).
            quote! {{
                let __stream = __UndraOpening::new(async move {
                    let __s = #call.await;
                    #check_inner
                    ::core::result::Result::<_, ::std::vec::Vec<u8>>::Ok(
                        ::std::boxed::Box::pin(__s)
                            as ::core::pin::Pin<::std::boxed::Box<
                                dyn #runtime::Stream<Item = #item> + ::core::marker::Send,
                            >>,
                    )
                });
                #check
                __undra_out(#runtime::DispatchResult::Stream(::std::boxed::Box::pin(__stream)))
            }}
        }
        // `Result<impl Stream, E>`.
        (ret, false) if is_stream_ok(ret) => {
            needs.map_stream = true;
            needs.send_assert = true;
            let check = assert_send(quote!(__stream));
            let err = sync_err(quote!(__e));
            quote! {
                match #call {
                    ::core::result::Result::Ok(__s) => {
                        let __stream = __UndraMap(::std::boxed::Box::pin(__s));
                        #check
                        __undra_out(#runtime::DispatchResult::Stream(::std::boxed::Box::pin(__stream)))
                    }
                    ::core::result::Result::Err(__e) => #err,
                }
            }
        }
        (ret, true) if is_stream_ok(ret) => {
            needs.opening_stream = true;
            needs.send_assert = true;
            let item = m
                .stream_item
                .as_ref()
                .expect("a stream return has an item type");
            let check_inner = assert_send(quote!(__s));
            let check = assert_send(quote!(__stream));
            let err = enc(&wire, &quote!(__e));
            quote! {{
                let __stream = __UndraOpening::new(async move {
                    match #call.await {
                        ::core::result::Result::Ok(__s) => {
                            #check_inner
                            ::core::result::Result::Ok(
                                ::std::boxed::Box::pin(__s)
                                    as ::core::pin::Pin<::std::boxed::Box<
                                        dyn #runtime::Stream<Item = #item> + ::core::marker::Send,
                                    >>,
                            )
                        }
                        ::core::result::Result::Err(__e) => ::core::result::Result::Err(#err),
                    }
                });
                #check
                __undra_out(#runtime::DispatchResult::Stream(::std::boxed::Box::pin(__stream)))
            }}
        }
        // A value or `Result<T, E>`.
        (KType::Result(..), false) => {
            let ok = sync_ok(quote!(__v));
            let err = sync_err(quote!(__e));
            quote! {
                match #call {
                    ::core::result::Result::Ok(__v) => #ok,
                    ::core::result::Result::Err(__e) => #err,
                }
            }
        }
        (KType::Result(..), true) => {
            needs.send_assert = true;
            let ok = out_ok(quote!(__v));
            let err = out_err(quote!(__e));
            let check = assert_send(quote!(__fut));
            quote! {{
                let __fut = async move {
                    match #call.await {
                        ::core::result::Result::Ok(__v) => #ok,
                        ::core::result::Result::Err(__e) => #err,
                    }
                };
                #check
                __undra_out(#runtime::DispatchResult::Async(::std::boxed::Box::pin(__fut)))
            }}
        }
        (_, false) => {
            let ok = sync_ok(quote!(__out));
            quote! {{
                let __out = #call;
                #ok
            }}
        }
        (_, true) => {
            needs.send_assert = true;
            let ok = out_ok(quote!(__out));
            let check = assert_send(quote!(__fut));
            quote! {{
                let __fut = async move {
                    let __out = #call.await;
                    #ok
                };
                #check
                __undra_out(#runtime::DispatchResult::Async(::std::boxed::Box::pin(__fut)))
            }}
        }
    }
}

/// How many objects a return hands out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObjReturn {
    /// `Arc<T>`.
    One,
    /// `Option<Arc<T>>`.
    Optional,
    /// `Vec<Arc<T>>`.
    Many,
}

/// The shape of a return that hands out objects, and whether it is the `Ok` of a `Result`.
fn object_return(ret: &KType) -> Option<(ObjReturn, bool)> {
    fn plain(ret: &KType) -> Option<ObjReturn> {
        match ret {
            KType::Object(_) => Some(ObjReturn::One),
            KType::Option(inner) if matches!(**inner, KType::Object(_)) => {
                Some(ObjReturn::Optional)
            }
            KType::Vec(inner) if matches!(**inner, KType::Object(_)) => Some(ObjReturn::Many),
            _ => None,
        }
    }
    match ret {
        KType::Result(ok, _) => plain(ok).map(|shape| (shape, true)),
        other => plain(other).map(|shape| (shape, false)),
    }
}

/// The expression that lowers `value` (the object or objects a method returned) to the handles
/// of the reply, issuing each one through `scope`.
fn lower(shape: ObjReturn, value: TokenStream, scope: &syn::Ident) -> TokenStream {
    let issue = |object: TokenStream| {
        quote! {
            match #scope.issue(#object) {
                ::core::result::Result::Ok(__handle) => __handle,
                // A store that cannot attach its signals is a bug in the app: contained like any
                // other panic (the scope gives back what it had issued while it unwinds).
                ::core::result::Result::Err(__why) => ::core::panic!("{}", __why),
            }
        }
    };
    match shape {
        ObjReturn::One => issue(value),
        ObjReturn::Optional => {
            let inner = issue(quote!(__object));
            quote! {
                match #value {
                    ::core::option::Option::Some(__object) => ::core::option::Option::Some(#inner),
                    ::core::option::Option::None => ::core::option::Option::None,
                }
            }
        }
        ObjReturn::Many => {
            let inner = issue(quote!(__object));
            quote! {{
                let __objects = #value;
                let mut __handles = ::std::vec::Vec::with_capacity(__objects.len());
                for __object in __objects {
                    __handles.push(#inner);
                }
                __handles
            }}
        }
    }
}

/// The outcome of a method that returns objects: each one is issued to the host as a handle, in
/// a scope that gives the references back unless the reply carries them (ADR-040 decision 6).
fn object_result(
    root: &Root,
    m: &FnModel,
    call: &TokenStream,
    shape: ObjReturn,
    in_result: bool,
    needs: &mut Needs,
) -> TokenStream {
    let wire = root.wire();
    let runtime = root.runtime();
    let span = m.ident.span();
    let scope = format_ident!("__scope");
    if !m.is_async {
        let ok = |value: TokenStream| {
            let lowered = lower(shape, value, &scope);
            quote! {{
                let mut #scope = __rt.issue_scope();
                let __lowered = #lowered;
                let __outcome = __rt.sync_ok(&__lowered, #wire::Encode::encode);
                #scope.commit();
                __outcome
            }}
        };
        return if in_result {
            let ok = ok(quote!(__v));
            quote! {
                match #call {
                    ::core::result::Result::Ok(__v) => #ok,
                    ::core::result::Result::Err(__e) => __rt.sync_err(&__e, #wire::Encode::encode),
                }
            }
        } else {
            let ok = ok(quote!(__out));
            quote! {{
                let __out = #call;
                #ok
            }}
        };
    }
    // An asynchronous method lowers in the last poll of its future: after the final `.await`, in
    // the poll that completes the call, so nothing can cancel the call between the issue and the
    // reply (both happen under the core lock). The runtime is reached through a weak context: a
    // runtime that is gone has already answered the call.
    needs.send_assert = true;
    let send_fn = send_assertion(span);
    let lowered_ok = |value: TokenStream| {
        let lowered = lower(shape, value, &scope);
        quote! {
            match __weak.upgrade() {
                ::core::result::Result::Ok(__ctx) => {
                    let mut #scope = __ctx.runtime().issue_scope();
                    let __lowered = #lowered;
                    let __bytes = #wire::Encode::encode_to_vec(&__lowered);
                    #scope.commit();
                    ::core::result::Result::Ok(__bytes)
                }
                ::core::result::Result::Err(_) => ::core::result::Result::Ok(::std::vec::Vec::new()),
            }
        }
    };
    let body = if in_result {
        let ok = lowered_ok(quote!(__v));
        quote! {
            match #call.await {
                ::core::result::Result::Ok(__v) => #ok,
                ::core::result::Result::Err(__e) => {
                    ::core::result::Result::Err(#wire::Encode::encode_to_vec(&__e))
                }
            }
        }
    } else {
        let ok = lowered_ok(quote!(__out));
        quote! {{
            let __out = #call.await;
            #ok
        }}
    };
    quote! {{
        let __weak = __rt.ctx().downgrade();
        let __fut = async move { #body };
        #send_fn(&__fut);
        __undra_out(#runtime::DispatchResult::Async(::std::boxed::Box::pin(__fut)))
    }}
}

/// The outcome (a `DispatchOutcome`) of a constructor: insert the new object and reply with
/// its handle.
fn constructor_result(
    root: &Root,
    m: &FnModel,
    target: &Target<'_>,
    call: &TokenStream,
) -> TokenStream {
    let wire = root.wire();
    let runtime = root.runtime();
    let ok = quote!(__rt.sync_ok(&__handle, #wire::Encode::encode));
    // A constructor that took callbacks has made their proxies by the time it can fail to publish
    // what it built; dropping the unpublished value releases them, so the call must not answer
    // status 5 ("refused: owns nothing"), which makes the host give its references back as well
    // (a double release). It answers a failure of the call (status 2) instead, which keeps them.
    let took_callbacks = m
        .params
        .iter()
        .any(|p| matches!(p.plan, ParamPlan::Callback { .. }));
    let refuse = |reason: TokenStream| {
        if took_callbacks {
            // The host words status 2 as a panic ("the core panicked: ..."), so the message says what
            // it is: the call failed, the core kept nothing, and the references the host lent are its
            // own to forget (not given back by it a second time).
            quote!(__undra_out(#runtime::DispatchResult::Failed(::std::format!(
                "the constructor failed after it took its callbacks (not a panic; the core released \
                 them and the host must not): {}",
                #reason
            ))))
        } else {
            quote!(__undra_bad_request(#reason))
        }
    };
    // What to do with the constructed `__value`: publish it and answer its handle. A store
    // first attaches its signals; if that fails the store is never published and the caller
    // gets a bad request carrying the reason (nothing panics). A constructor that returns an
    // `Arc` interns the instance (ADR-040), a store included: `issue_constructed` attaches the
    // signals and keeps the entry in snapshots, as a constructed object is.
    let finish = if m.ctor_shared {
        let type_name = target.self_ty.map(ty_string).unwrap_or_default();
        let refused = refuse(quote!(::std::format!("`{}`: {}", #type_name, __why)));
        quote! {{
            let mut __scope = __rt.issue_scope();
            match __scope.issue_constructed(__value) {
                ::core::result::Result::Ok(__handle) => {
                    let __outcome = #ok;
                    __scope.commit_constructed();
                    __outcome
                }
                ::core::result::Result::Err(__why) => #refused,
            }
        }}
    } else if target.store {
        let type_name = target.self_ty.map(ty_string).unwrap_or_default();
        let refused = refuse(quote!(::std::format!(
            "store `{}` could not attach its signals: {}",
            #type_name,
            __why
        )));
        quote! {
            match __value.__undra_attach_all() {
                ::core::result::Result::Ok(()) => {
                    let __arc = ::std::sync::Arc::new(__value);
                    let __handle = __rt.insert_object(::std::sync::Arc::clone(&__arc));
                    (*__arc).__undra_set_handle(__handle.0);
                    #ok
                }
                ::core::result::Result::Err(__why) => #refused,
            }
        }
    } else {
        quote! {{
            let __handle = __rt.insert_object(::std::sync::Arc::new(__value));
            #ok
        }}
    };
    if matches!(m.ret, KType::Result(..)) {
        quote! {
            match #call {
                ::core::result::Result::Ok(__value) => #finish,
                ::core::result::Result::Err(__e) => __rt.sync_err(&__e, #wire::Encode::encode),
            }
        }
    } else {
        quote! {{
            let __value = #call;
            #finish
        }}
    }
}

/// The body of the dispatch arm for `m`: decode, call, encode. It evaluates to a
/// `DispatchOutcome` or returns early with a bad request (status 5 and a reason).
fn arm_body(root: &Root, m: &FnModel, target: &Target<'_>, needs: &mut Needs) -> TokenStream {
    let wire = root.wire();
    // How the function is named in the reasons a bad request carries.
    let what = match target.self_ty {
        Some(self_ty) if m.kind != Kind::Function => format!("{}.{}", ty_string(self_ty), m.name),
        _ => m.name.clone(),
    };

    // Decode the arguments in declaration order; any failure is a bad request that says which.
    // The values are bound to positional locals, never to the user's parameter names, so a
    // parameter called `__r` or `__ctx` cannot collide with what the dispatcher generates.
    // An object is read as its handle (`__h<i>`) and a callback as its instance (`__c<i>`); both
    // are resolved only once every argument has decoded.
    let lets = m.params.iter().enumerate().map(|(index, p)| {
        let param = &p.name;
        let fail = quote! {
            return __undra_bad_request(::std::format!(
                "cannot decode argument `{}` of `{}`: {}", #param, #what, __e
            ));
        };
        match &p.plan {
            ParamPlan::Plain => {
                let local = arg_local(index);
                let ty = &p.ty;
                quote_spanned! {p.ty.span()=>
                    let #local: #ty = match <#ty as #wire::Decode>::decode(&mut __r) {
                        ::core::result::Result::Ok(__v) => __v,
                        ::core::result::Result::Err(__e) => { #fail }
                    };
                }
            }
            ParamPlan::Object { shape, .. } => {
                let local = handle_local(index);
                let raw = match shape {
                    ObjShape::Ref | ObjShape::Arc => quote!(u64),
                    ObjShape::OptionRef | ObjShape::OptionArc => quote!(::core::option::Option<u64>),
                    ObjShape::VecArc => quote!(::std::vec::Vec<u64>),
                };
                quote_spanned! {p.ty.span()=>
                    let #local: #raw = match <#raw as #wire::Decode>::decode(&mut __r) {
                        ::core::result::Result::Ok(__v) => __v,
                        ::core::result::Result::Err(__e) => { #fail }
                    };
                }
            }
            ParamPlan::Callback { optional, .. } => {
                let local = callback_local(index);
                let raw = if *optional {
                    quote!(::core::option::Option<u64>)
                } else {
                    quote!(u64)
                };
                let null = if *optional {
                    quote!(#local == ::core::option::Option::Some(0))
                } else {
                    quote!(#local == 0)
                };
                quote_spanned! {p.ty.span()=>
                    let #local: #raw = match <#raw as #wire::Decode>::decode(&mut __r) {
                        ::core::result::Result::Ok(__v) => __v,
                        ::core::result::Result::Err(__e) => { #fail }
                    };
                    if #null {
                        return __undra_bad_request(::std::format!(
                            "argument `{}` of `{}`: callback instance 0 is the null instance", #param, #what
                        ));
                    }
                }
            }
        }
    });
    let decode = quote! {
        let mut __r = #wire::Reader::new(__call.args);
        #(#lets)*
        if let ::core::result::Result::Err(__e) = __r.finish() {
            return __undra_bad_request(::std::format!(
                "cannot decode the arguments of `{}`: {}", #what, __e
            ));
        }
    };

    // Objects: each handle resolved to its `Arc` before the method runs (before its first
    // `.await`), so a host `close()` racing the call never frees what the call uses, and a stale
    // or wrongly typed handle is a bad request naming the parameter (ADR-040 decision 4).
    let resolves = m.params.iter().enumerate().filter_map(|(index, p)| {
        let ParamPlan::Object { elem, shape } = &p.plan else {
            return None;
        };
        let local = arg_local(index);
        let handle = handle_local(index);
        let param = &p.name;
        // A call that outlives its dispatch (an `async` method, a stream) holds its object
        // parameters while it runs: the runtime remembers them for a restore (`Runtime::param`).
        let outlives = m.is_async || m.stream_item.is_some();
        let one = |h: TokenStream| {
            let resolve = if outlives {
                quote!(__rt.param::<#elem>(__call.call_id, #h))
            } else {
                quote!(__rt.object::<#elem>(#h))
            };
            quote! {
                match #resolve {
                    ::core::result::Result::Ok(__o) => __o,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(::std::format!(
                            "argument `{}` of `{}`: {}", #param, #what, __e
                        ));
                    }
                }
            }
        };
        let resolved = match shape {
            ObjShape::Ref | ObjShape::Arc => one(quote!(#handle)),
            ObjShape::OptionRef | ObjShape::OptionArc => {
                let inner = one(quote!(__h));
                quote! {
                    match #handle {
                        ::core::option::Option::Some(__h) => ::core::option::Option::Some(#inner),
                        ::core::option::Option::None => ::core::option::Option::None,
                    }
                }
            }
            ObjShape::VecArc => {
                let inner = one(quote!(__h));
                quote! {{
                    let mut __all = ::std::vec::Vec::with_capacity(#handle.len());
                    for __h in #handle {
                        __all.push(#inner);
                    }
                    __all
                }}
            }
        };
        Some(quote_spanned! {p.ty.span()=> let #local = #resolved; })
    });
    let resolves: Vec<TokenStream> = resolves.collect();

    // Callbacks: pending until now. Everything that could refuse the call has run, so the core
    // takes ownership of the host's references only for a call it will serve (ADR-041 decision
    // 5: "a refused call transfers nothing").
    let callbacks = m.params.iter().enumerate().filter_map(|(index, p)| {
        let ParamPlan::Callback { dyn_ty, optional } = &p.plan else {
            return None;
        };
        let local = arg_local(index);
        let instance = callback_local(index);
        Some(if *optional {
            quote_spanned! {p.ty.span()=>
                let #local = #instance.map(|__i| __rt.callback::<#dyn_ty>(__i));
            }
        } else {
            quote_spanned! {p.ty.span()=>
                let #local = __rt.callback::<#dyn_ty>(#instance);
            }
        })
    });
    let callbacks: Vec<TokenStream> = callbacks.collect();

    // Resolve `self` and the context.
    let receiver = match (m.kind, target.self_ty) {
        (Kind::Method, Some(self_ty)) => quote! {
            let __obj = match __rt.object::<#self_ty>(__call.handle) {
                ::core::result::Result::Ok(__o) => __o,
                ::core::result::Result::Err(__e) => {
                    return __undra_bad_request(::std::format!(
                        "cannot call `{}`: {}", #what, __e
                    ));
                }
            };
        },
        _ => TokenStream::new(),
    };
    let ctx_binding = if m.ctx.is_some() {
        quote!(let __ctx = __rt.ctx();)
    } else {
        TokenStream::new()
    };

    // The call expression.
    let mut call_args: Vec<TokenStream> = Vec::new();
    if m.kind == Kind::Method {
        call_args.push(quote!(&*__obj));
    }
    if let Some(ctx) = &m.ctx {
        call_args.push(if ctx.by_ref {
            quote!(&__ctx)
        } else {
            quote!(__ctx)
        });
    }
    call_args.extend(m.params.iter().enumerate().map(|(index, p)| {
        let local = arg_local(index);
        match &p.plan {
            ParamPlan::Object {
                shape: ObjShape::Ref,
                ..
            } => quote!(&*#local),
            ParamPlan::Object {
                shape: ObjShape::OptionRef,
                ..
            } => quote!(#local.as_deref()),
            _ => quote!(#local),
        }
    }));
    let ident = &m.ident;
    let turbofish = &m.turbofish;
    let call = match target.self_ty {
        Some(self_ty) if m.kind != Kind::Function => {
            quote_spanned!(ident.span()=> #self_ty::#ident #turbofish ( #(#call_args),* ))
        }
        _ => quote_spanned!(ident.span()=> #ident #turbofish ( #(#call_args),* )),
    };

    let result = match m.kind {
        Kind::Constructor => constructor_result(root, m, target, &call),
        _ => call_result(root, m, &call, needs),
    };

    quote! {
        #decode
        #(#resolves)*
        #receiver
        #ctx_binding
        #(#callbacks)*
        #result
    }
}

/// The outcome expression of a method that returns `impl Stream<Item = Result<T, E>>`, possibly
/// inside `Result<.., E>` and possibly `async` (ADR-036). Its items are mapped by `__UndraTry`:
/// `Ok(t)` is an item, `Err(e)` the stream's typed error (flag 2), after which it ends.
fn fallible_stream_result(
    root: &Root,
    m: &FnModel,
    call: &TokenStream,
    needs: &mut Needs,
) -> TokenStream {
    let wire = root.wire();
    let runtime = root.runtime();
    let span = m.ident.span();
    let send_fn = send_assertion(span);
    let assert_send = |what: TokenStream| quote_spanned!(span=> #send_fn(&#what););
    needs.try_stream = true;
    needs.send_assert = true;
    let check = assert_send(quote!(__stream));
    let boxed = quote! {
        ::std::boxed::Box::pin(__UndraTry::new(__s))
            as ::core::pin::Pin<::std::boxed::Box<
                dyn #runtime::Stream<
                        Item = ::core::result::Result<::std::vec::Vec<u8>, ::std::vec::Vec<u8>>,
                    > + ::core::marker::Send,
            >>
    };
    match (m.is_async, m.stream_in_result) {
        (false, false) => quote! {{
            let __stream = __UndraTry::new(#call);
            #check
            __undra_out(#runtime::DispatchResult::Stream(::std::boxed::Box::pin(__stream)))
        }},
        (false, true) => {
            let err = quote!(__rt.sync_err(&__e, #wire::Encode::encode));
            quote! {
                match #call {
                    ::core::result::Result::Ok(__s) => {
                        let __stream = __UndraTry::new(__s);
                        #check
                        __undra_out(#runtime::DispatchResult::Stream(::std::boxed::Box::pin(__stream)))
                    }
                    ::core::result::Result::Err(__e) => #err,
                }
            }
        }
        (true, in_result) => {
            needs.opening_raw = true;
            let check_inner = assert_send(quote!(__s));
            let opened = if in_result {
                let err = enc(&wire, &quote!(__e));
                quote! {
                    match #call.await {
                        ::core::result::Result::Ok(__s) => {
                            #check_inner
                            ::core::result::Result::Ok(#boxed)
                        }
                        ::core::result::Result::Err(__e) => ::core::result::Result::Err(#err),
                    }
                }
            } else {
                quote! {
                    let __s = #call.await;
                    #check_inner
                    ::core::result::Result::<_, ::std::vec::Vec<u8>>::Ok(#boxed)
                }
            };
            quote! {{
                let __stream = __UndraOpeningRaw::new(async move { #opened });
                #check
                __undra_out(#runtime::DispatchResult::Stream(::std::boxed::Box::pin(__stream)))
            }}
        }
    }
}

/// The helper items every dispatcher carries.
fn helpers(root: &Root, needs: &Needs) -> TokenStream {
    let meta = root.meta();
    let runtime = root.runtime();
    let wire = root.wire();

    let send_assert = if needs.send_assert {
        // E0022: the runtime polls futures and streams on its executor thread, so they must be
        // `Send`. `rustc` cannot carry an Undra code, but this assertion, called with the
        // method's span, makes its own "future cannot be sent between threads safely"
        // error (with the offending value and the `.await` it lives across) point at the
        // method instead of at generated code, and its name, which the error's last note
        // quotes, carries the code (see `common::send_assertion`).
        let send_fn = send_assertion(proc_macro2::Span::call_site());
        quote! {
            fn #send_fn<T: ::core::marker::Send>(_: &T) {}
        }
    } else {
        TokenStream::new()
    };

    let map_stream = if needs.map_stream {
        quote! {
            struct __UndraMap<S>(::core::pin::Pin<::std::boxed::Box<S>>);
            impl<S> #runtime::Stream for __UndraMap<S>
            where
                S: #runtime::Stream,
                S::Item: #wire::Encode,
            {
                type Item = ::core::result::Result<::std::vec::Vec<u8>, ::std::vec::Vec<u8>>;
                fn poll_next(
                    self: ::core::pin::Pin<&mut Self>,
                    __cx: &mut ::core::task::Context<'_>,
                ) -> ::core::task::Poll<::core::option::Option<Self::Item>> {
                    let __this = self.get_mut();
                    match #runtime::Stream::poll_next(__this.0.as_mut(), __cx) {
                        ::core::task::Poll::Ready(::core::option::Option::Some(__item)) => {
                            ::core::task::Poll::Ready(::core::option::Option::Some(
                                ::core::result::Result::Ok(#wire::Encode::encode_to_vec(&__item)),
                            ))
                        }
                        ::core::task::Poll::Ready(::core::option::Option::None) => {
                            ::core::task::Poll::Ready(::core::option::Option::None)
                        }
                        ::core::task::Poll::Pending => ::core::task::Poll::Pending,
                    }
                }
            }
        }
    } else {
        TokenStream::new()
    };

    let opening_stream = if needs.opening_stream {
        quote! {
            enum __UndraOpening<F, S> {
                Opening(::core::pin::Pin<::std::boxed::Box<F>>),
                Open(::core::pin::Pin<::std::boxed::Box<S>>),
                Done,
            }
            impl<F, S> __UndraOpening<F, S> {
                fn new(__fut: F) -> Self {
                    Self::Opening(::std::boxed::Box::pin(__fut))
                }
            }
            impl<F, S> #runtime::Stream for __UndraOpening<F, S>
            where
                F: ::core::future::Future<Output = ::core::result::Result<S, ::std::vec::Vec<u8>>>,
                S: #runtime::Stream,
                S::Item: #wire::Encode,
            {
                type Item = ::core::result::Result<::std::vec::Vec<u8>, ::std::vec::Vec<u8>>;
                fn poll_next(
                    self: ::core::pin::Pin<&mut Self>,
                    __cx: &mut ::core::task::Context<'_>,
                ) -> ::core::task::Poll<::core::option::Option<Self::Item>> {
                    let __this = self.get_mut();
                    loop {
                        match __this {
                            Self::Opening(__fut) => {
                                match ::core::future::Future::poll(__fut.as_mut(), __cx) {
                                    ::core::task::Poll::Pending => {
                                        return ::core::task::Poll::Pending;
                                    }
                                    ::core::task::Poll::Ready(::core::result::Result::Ok(__stream)) => {
                                        *__this = Self::Open(::std::boxed::Box::pin(__stream));
                                    }
                                    ::core::task::Poll::Ready(::core::result::Result::Err(__bytes)) => {
                                        *__this = Self::Done;
                                        return ::core::task::Poll::Ready(::core::option::Option::Some(
                                            ::core::result::Result::Err(__bytes),
                                        ));
                                    }
                                }
                            }
                            Self::Open(__stream) => {
                                return match #runtime::Stream::poll_next(__stream.as_mut(), __cx) {
                                    ::core::task::Poll::Ready(::core::option::Option::Some(__item)) => {
                                        ::core::task::Poll::Ready(::core::option::Option::Some(
                                            ::core::result::Result::Ok(#wire::Encode::encode_to_vec(&__item)),
                                        ))
                                    }
                                    ::core::task::Poll::Ready(::core::option::Option::None) => {
                                        *__this = Self::Done;
                                        ::core::task::Poll::Ready(::core::option::Option::None)
                                    }
                                    ::core::task::Poll::Pending => ::core::task::Poll::Pending,
                                };
                            }
                            Self::Done => {
                                return ::core::task::Poll::Ready(::core::option::Option::None);
                            }
                        }
                    }
                }
            }
        }
    } else {
        TokenStream::new()
    };

    let try_stream = if needs.try_stream {
        quote! {
            struct __UndraTry<S> {
                stream: ::core::pin::Pin<::std::boxed::Box<S>>,
                done: bool,
            }
            impl<S> __UndraTry<S> {
                fn new(__stream: S) -> Self {
                    Self {
                        stream: ::std::boxed::Box::pin(__stream),
                        done: false,
                    }
                }
            }
            impl<S, T, E> #runtime::Stream for __UndraTry<S>
            where
                S: #runtime::Stream<Item = ::core::result::Result<T, E>>,
                T: #wire::Encode,
                E: #wire::Encode,
            {
                type Item = ::core::result::Result<::std::vec::Vec<u8>, ::std::vec::Vec<u8>>;
                fn poll_next(
                    self: ::core::pin::Pin<&mut Self>,
                    __cx: &mut ::core::task::Context<'_>,
                ) -> ::core::task::Poll<::core::option::Option<Self::Item>> {
                    let __this = self.get_mut();
                    if __this.done {
                        return ::core::task::Poll::Ready(::core::option::Option::None);
                    }
                    match #runtime::Stream::poll_next(__this.stream.as_mut(), __cx) {
                        ::core::task::Poll::Ready(::core::option::Option::Some(
                            ::core::result::Result::Ok(__item),
                        )) => ::core::task::Poll::Ready(::core::option::Option::Some(
                            ::core::result::Result::Ok(#wire::Encode::encode_to_vec(&__item)),
                        )),
                        // The stream's typed error ends it: the runtime sends it as flag 2.
                        ::core::task::Poll::Ready(::core::option::Option::Some(
                            ::core::result::Result::Err(__error),
                        )) => {
                            __this.done = true;
                            ::core::task::Poll::Ready(::core::option::Option::Some(
                                ::core::result::Result::Err(#wire::Encode::encode_to_vec(&__error)),
                            ))
                        }
                        ::core::task::Poll::Ready(::core::option::Option::None) => {
                            __this.done = true;
                            ::core::task::Poll::Ready(::core::option::Option::None)
                        }
                        ::core::task::Poll::Pending => ::core::task::Poll::Pending,
                    }
                }
            }
        }
    } else {
        TokenStream::new()
    };

    let opening_raw = if needs.opening_raw {
        quote! {
            enum __UndraOpeningRaw<F> {
                Opening(::core::pin::Pin<::std::boxed::Box<F>>),
                Open(
                    ::core::pin::Pin<::std::boxed::Box<
                        dyn #runtime::Stream<
                                Item = ::core::result::Result<::std::vec::Vec<u8>, ::std::vec::Vec<u8>>,
                            > + ::core::marker::Send,
                    >>,
                ),
                Done,
            }
            impl<F> __UndraOpeningRaw<F> {
                fn new(__fut: F) -> Self {
                    Self::Opening(::std::boxed::Box::pin(__fut))
                }
            }
            impl<F> #runtime::Stream for __UndraOpeningRaw<F>
            where
                F: ::core::future::Future<
                    Output = ::core::result::Result<
                        ::core::pin::Pin<::std::boxed::Box<
                            dyn #runtime::Stream<
                                    Item = ::core::result::Result<
                                        ::std::vec::Vec<u8>,
                                        ::std::vec::Vec<u8>,
                                    >,
                                > + ::core::marker::Send,
                        >>,
                        ::std::vec::Vec<u8>,
                    >,
                >,
            {
                type Item = ::core::result::Result<::std::vec::Vec<u8>, ::std::vec::Vec<u8>>;
                fn poll_next(
                    self: ::core::pin::Pin<&mut Self>,
                    __cx: &mut ::core::task::Context<'_>,
                ) -> ::core::task::Poll<::core::option::Option<Self::Item>> {
                    let __this = self.get_mut();
                    loop {
                        match __this {
                            Self::Opening(__fut) => {
                                match ::core::future::Future::poll(__fut.as_mut(), __cx) {
                                    ::core::task::Poll::Pending => {
                                        return ::core::task::Poll::Pending;
                                    }
                                    ::core::task::Poll::Ready(::core::result::Result::Ok(__stream)) => {
                                        *__this = Self::Open(__stream);
                                    }
                                    ::core::task::Poll::Ready(::core::result::Result::Err(__bytes)) => {
                                        *__this = Self::Done;
                                        return ::core::task::Poll::Ready(::core::option::Option::Some(
                                            ::core::result::Result::Err(__bytes),
                                        ));
                                    }
                                }
                            }
                            Self::Open(__stream) => {
                                return match #runtime::Stream::poll_next(__stream.as_mut(), __cx) {
                                    ::core::task::Poll::Ready(::core::option::Option::None) => {
                                        *__this = Self::Done;
                                        ::core::task::Poll::Ready(::core::option::Option::None)
                                    }
                                    __other => __other,
                                };
                            }
                            Self::Done => {
                                return ::core::task::Poll::Ready(::core::option::Option::None);
                            }
                        }
                    }
                }
            }
        }
    } else {
        TokenStream::new()
    };

    quote! {
        fn __undra_out(__result: #runtime::DispatchResult) -> #meta::DispatchOutcome {
            #meta::DispatchOutcome::new(__result)
        }
        fn __undra_unknown() -> #meta::DispatchOutcome {
            __undra_out(#runtime::DispatchResult::Unknown)
        }
        #[allow(dead_code)]
        fn __undra_bad_request(__reason: ::std::string::String) -> #meta::DispatchOutcome {
            __undra_out(#runtime::DispatchResult::BadRequest(__reason))
        }
        #send_assert
        #map_stream
        #opening_stream
        #try_stream
        #opening_raw
    }
}

fn method_meta(root: &Root, m: &FnModel, method_id: &TokenStream) -> TokenStream {
    let meta = root.meta();
    let name = &m.name;
    let docs = &m.docs;
    let params = m.params.iter().map(|p| param_meta(&meta, &p.name, &p.kty));
    let returns = m.ret.meta(&meta);
    let is_async = m.is_async;
    let takes_ctx = m.ctx.is_some();
    let generic = Label::meta(m.generic.as_ref(), &meta);
    quote! {
        #meta::MethodMeta {
            name: #name,
            method_id: #method_id,
            params: &[ #(#params),* ],
            returns: #returns,
            is_async: #is_async,
            takes_ctx: #takes_ctx,
            coalesce: false,
            generic: #generic,
            docs: #docs,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Struct literal patching for stores
// ---------------------------------------------------------------------------------------------

/// Adds the hidden `__undra_cell` field to struct literals of the store type inside its
/// `#[undra::api(store)]` impl block (`Self { .. }` and `Type { .. }`).
///
/// Literals with a `..base` are left alone (the cell comes from the base), as are literals
/// that already name the field, and literals inside nested items.
struct PatchStoreLiterals<'a> {
    name: &'a str,
}

impl VisitMut for PatchStoreLiterals<'_> {
    fn visit_expr_struct_mut(&mut self, node: &mut syn::ExprStruct) {
        visit_mut::visit_expr_struct_mut(self, node);
        let is_store = node.qself.is_none()
            && node.path.segments.len() == 1
            && node.path.segments[0].arguments.is_none()
            && (node.path.segments[0].ident == "Self" || node.path.segments[0].ident == self.name);
        let has_cell = node
            .fields
            .iter()
            .any(|f| matches!(&f.member, syn::Member::Named(id) if id == "__undra_cell"));
        if is_store && node.rest.is_none() && !has_cell {
            node.fields.push(syn::parse_quote!(
                __undra_cell: ::core::default::Default::default()
            ));
        }
    }

    fn visit_item_mut(&mut self, _: &mut syn::Item) {
        // `Self` means something else inside a nested item.
    }
}

// ---------------------------------------------------------------------------------------------
// impl blocks
// ---------------------------------------------------------------------------------------------

/// The name of the type an impl block is for. `report_arguments` says whether a type written with
/// arguments (`impl Cache<Todo>`) is E0002: not when the block's own type parameters have been
/// reported already (`impl<T> Cache<T>` is one error, not two).
fn self_type_name(ty: &Type, report_arguments: bool) -> syn::Result<String> {
    match ty {
        Type::Path(path) if path.qself.is_none() => {
            let seg = path.path.segments.last().expect("a path has a segment");
            if report_arguments && !seg.arguments.is_none() {
                let name = unraw(&seg.ident);
                return Err(Diag::new(
                    code::E0002,
                    format!("generic type `{}` in `#[undra::api] impl`", ty_string(ty)),
                    "the schema describes concrete objects: a generic object crosses once per instantiation, each under a name of its own, which the platforms generate a class for",
                    format!("write the block once for the type with its own parameters, `#[undra::api(generic)] impl<T> {name}<T> {{ .. }}`, and declare the instantiation: `#[undra::api] pub type Todo{name} = {};`", ty_string(ty)),
                )
                .on(ty));
            }
            Ok(unraw(&seg.ident))
        }
        other => Err(shape_error(
            format!("`#[undra::api]` on an impl of `{}`", ty_string(other)),
            other,
            "an object is a named struct or enum; the impl block must name it",
            "write `impl TypeName { .. }`",
        )),
    }
}

/// A public method of an object, analysed: its parameters and return type are checked and its
/// model is made. `analysis` is what [`analyze`] said of `sig`.
fn method_model(
    sig: &mut Signature,
    analysis: Analysis,
    name: String,
    docs: String,
    errors: &mut Errors,
    checks: &mut Checks,
) -> FnModel {
    for p in &analysis.params {
        checks.ty(&p.ty, &p.kty);
    }
    let ret = match map_method_return(&sig.output) {
        Ok(ret) => ret,
        Err(err) => {
            errors.push(err.into_error());
            KType::Unit
        }
    };
    checks.ret(&sig.output, &ret);
    let stream_item = stream_item_type(&sig.output);
    let stream_in_result = stream_in_result(&sig.output);
    ensure_static_streams_in(&mut sig.output);
    FnModel {
        ident: sig.ident.clone(),
        suffix: name.clone(),
        name,
        kind: Kind::Method,
        is_async: analysis.is_async,
        ctx: None,
        params: analysis.params,
        ret,
        ctor_shared: false,
        stream_item,
        stream_in_result,
        docs,
        turbofish: None,
        generic: None,
    }
}

/// What the expansion of one instantiation of a generic object knows about it (ADR-058).
#[derive(Clone, Debug)]
pub(crate) struct ObjectInstance {
    /// The template's name (`Selection`), for the E0070 constant.
    pub(crate) template: String,
    /// Whether this is a plain object's instantiation: its impl block carries the E0070 constant
    /// and the check that the template is not a generic store. A store's instantiation carries
    /// them in the store's own expansion, which comes first.
    pub(crate) plain: bool,
}

/// Expands `#[undra::api]` on an inherent `impl` block.
pub(crate) fn expand_impl(
    args_root: Option<Root>,
    store: bool,
    item: ItemImpl,
) -> syn::Result<TokenStream> {
    expand_impl_as(args_root, store, item, None)
}

/// Expands an impl block: as written, or (`instance`) as one instantiation of a generic object,
/// whose block holds signatures only, is for the alias and is not emitted (ADR-058).
pub(crate) fn expand_impl_as(
    args_root: Option<Root>,
    store: bool,
    mut item: ItemImpl,
    instance: Option<&ObjectInstance>,
) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    let root = item_root(&mut item.attrs, args_root, &mut errors);
    if let Some((_, path, _)) = &item.trait_ {
        errors.push(shape_error(
            format!("`#[undra::api]` on a trait impl (`impl {} for ..`)", ty_string(path)),
            path,
            "the attribute exposes the inherent methods of an object; trait methods have no schema representation",
            "move the methods you want to expose into an inherent `impl Type { .. }` block",
        ));
    }
    let self_ty = (*item.self_ty).clone();
    let own_params = item
        .generics
        .params
        .iter()
        .any(|p| matches!(p, syn::GenericParam::Type(_)));
    let type_name = match self_type_name(&self_ty, !own_params && instance.is_none()) {
        Ok(name) => name,
        Err(error) => {
            errors.push(error);
            String::new()
        }
    };
    if instance.is_none() {
        check_generics(
            &item.generics,
            &type_name,
            GenericOn::ImplBlock,
            &mut errors,
        );
    }
    let type_docs = docs(&item.attrs);

    let mut constructors: Vec<FnModel> = Vec::new();
    let mut methods: Vec<FnModel> = Vec::new();
    let mut checks = match instance {
        Some(_) => Checks::for_instance(&format_ident!("{}", type_name)),
        None => Checks::new(),
    };
    // The checks of the generic methods: one pass over each generic signature, and one set for
    // all the instantiations (ADR-058).
    let mut template_checks: Vec<TokenStream> = Vec::new();
    let mut instance_checks = Checks::for_listed_instance(&format_ident!("{}", type_name));
    for impl_item in &mut item.items {
        let ImplItem::Fn(func) = impl_item else {
            continue;
        };
        // A query or mutation inside the block is reported once; the function is not also
        // "neither a method nor a constructor".
        if reject_undra_macros(&func.attrs, &mut errors) {
            continue;
        }
        let is_public = matches!(func.vis, Visibility::Public(_));
        let attr = take(
            &mut func.attrs,
            if is_public {
                Site::METHOD
            } else {
                Site::PRIVATE
            },
            &mut errors,
        );
        if !is_public {
            continue; // private helpers are not part of the API
        }
        let fn_docs = docs(&func.attrs);
        let receiver = func.sig.receiver().is_some();
        let type_params = func.sig.generics.type_params().next().is_some();

        // A method with a list: one method per listed type (ADR-058).
        if !attr.generic.is_empty() && (receiver || !type_params) {
            let on = GenericOn::Method;
            let Some(plan) = generic_fn::plan(&func.sig, on, &attr.generic, &mut errors) else {
                continue;
            };
            let mut template = Errors::new();
            let analysis = analyze(&mut func.sig, &mut template, Kindred::Callable, None);
            generic_fn::check_parameter_use(&func.sig, &plan.param, &mut template);
            let mut tchecks = Checks::for_template(&func.sig.ident, vec![plan.param.to_string()]);
            for p in &analysis.params {
                tchecks.ty(&p.ty, &p.kty);
            }
            let ret = match map_method_return(&func.sig.output) {
                Ok(ret) => ret,
                Err(err) => {
                    template.push(err.into_error());
                    KType::Unit
                }
            };
            tchecks.ret(&func.sig.output, &ret);
            ensure_static_streams_in(&mut func.sig.output);
            let clean = template.is_empty();
            errors.absorb(template);
            if !clean {
                continue;
            }
            template_checks.push(tchecks.emit(&root));
            let inferred = generic_fn::inferred(&func.sig, &plan.param);
            for listed in &plan.instances {
                let _guard = in_instance(&listed.name);
                let mut sig = generic_fn::concrete_signature(&func.sig, &plan.param, &listed.ty);
                let analysis = analyze(&mut sig, &mut errors, Kindred::Callable, None);
                instance_checks.listed(
                    &listed.ty,
                    &listed.arg_name,
                    &plan.param.to_string(),
                    &plan.fn_name,
                );
                let mut model = method_model(
                    &mut sig,
                    analysis,
                    listed.name.clone(),
                    fn_docs.clone(),
                    &mut errors,
                    &mut instance_checks,
                );
                model.suffix = listed.suffix.clone();
                model.turbofish = Some(listed.turbofish.clone());
                model.generic = Some(Label {
                    of: plan.fn_name.clone(),
                    param: plan.param.to_string(),
                    arg: listed.arg_name.clone(),
                    inferred,
                });
                methods.push(model);
            }
            continue;
        }

        let on = if receiver {
            GenericOn::Method
        } else {
            GenericOn::Constructor
        };
        let analysis = analyze(&mut func.sig, &mut errors, Kindred::Callable, Some(on));
        let name = unraw(&func.sig.ident);

        if analysis.has_receiver {
            let model = method_model(
                &mut func.sig,
                analysis,
                name,
                fn_docs,
                &mut errors,
                &mut checks,
            );
            methods.push(model);
        } else if let Some(returns) = ctor_return(&func.sig.output, &type_name) {
            for p in &analysis.params {
                checks.ty(&p.ty, &p.kty);
            }
            if analysis.is_async {
                errors.push(shape_error(
                    format!("constructor `{name}` is `async`"),
                    &func.sig.asyncness,
                    "a constructor inserts the new object into the object table before it replies, so it must finish synchronously",
                    "construct synchronously and start asynchronous work with `ctx.spawn(..)`",
                ));
            }
            let named = KType::Named(type_name.clone());
            let ctor_shared = returns.shared();
            let ret = match returns {
                CtorReturn::Plain | CtorReturn::Shared => named,
                CtorReturn::Fallible(err_ty) | CtorReturn::FallibleShared(err_ty) => {
                    match map_error_type(&err_ty, Pos::Return) {
                        Ok(err) => {
                            checks.error_ty(&err_ty, &err);
                            KType::Result(Box::new(named), Box::new(err))
                        }
                        Err(err) => {
                            errors.push(err.into_error());
                            named
                        }
                    }
                }
            };
            let name_suffix = name.clone();
            constructors.push(FnModel {
                ident: func.sig.ident.clone(),
                name,
                kind: Kind::Constructor,
                is_async: false,
                ctx: analysis.ctx,
                params: analysis.params,
                ret,
                ctor_shared,
                stream_item: None,
                stream_in_result: false,
                docs: fn_docs,
                suffix: name_suffix,
                turbofish: None,
                generic: None,
            });
        } else {
            errors.push(shape_error(
                format!("`{name}` is neither a method nor a constructor"),
                &func.sig.ident,
                "a `pub fn` in a `#[undra::api] impl` block must take `&self` (a method) or return `Self` / `Result<Self, E>` (a constructor); the return type is read as written, so an alias such as `type R<T> = Result<T, E>` is not followed",
                "add `&self`, make it return `Self` or spell out `Result<Self, E>`, or make it private / move it to a free `#[undra::api] fn`",
            ));
        }
    }

    if store && constructors.is_empty() {
        errors.push(
            Diag::new(
                code::E0011,
                format!("store `{type_name}` has no constructor in this `#[undra::api(store)]` block"),
                "the platforms create a store by calling one of its constructors, and a type takes one `#[undra::api]` impl block, so a constructor in another block is not seen",
                format!(
                    "add `pub fn new(ctx: Ctx) -> Self` to this block; if `{type_name}` has a second `#[undra::api]` block (where the constructor is), move this block's methods into that one"
                ),
            )
            .on(&item.self_ty),
        );
    }

    // Two names whose ids collide would route calls to the wrong method.
    let mut seen: Vec<(u32, &str)> = Vec::new();
    for m in constructors.iter().chain(&methods) {
        let id = fnv1a32(&format!("{type_name}.{}", m.name));
        if let Some((_, other)) = seen.iter().find(|(other_id, _)| *other_id == id) {
            errors.push(shape_error(
                format!("method ids of `{other}` and `{}` collide", m.name),
                &m.ident,
                "ids are 32-bit hashes of `Type.method`, and two of them are equal",
                "rename one of the methods",
            ));
        }
        seen.push((id, &m.name));
    }
    errors.finish()?;

    if store && instance.is_none() {
        let mut patch = PatchStoreLiterals { name: &type_name };
        for impl_item in &mut item.items {
            if let ImplItem::Fn(func) = impl_item {
                patch.visit_impl_item_fn_mut(func);
            }
        }
    }

    let meta = root.meta();
    let runtime = root.runtime();
    let signals = root.signals();
    let wire = root.wire();
    let target = Target {
        self_ty: Some(&self_ty),
        store,
    };
    let mut needs = Needs::default();

    let all: Vec<&FnModel> = constructors.iter().chain(&methods).collect();
    let id_consts: Vec<syn::Ident> = all
        .iter()
        .map(|m| format_ident!("__UNDRA_ID_{}", m.suffix))
        .collect();
    let id_values = all.iter().map(|m| {
        let name = &m.name;
        quote!(#meta::ids::method_id(#type_name, #name))
    });
    let arms = all.iter().zip(&id_consts).map(|(m, id)| {
        let body = arm_body(&root, m, &target, &mut needs);
        quote!(#id => { #body })
    });
    let arms: Vec<TokenStream> = arms.collect();
    let helper_items = helpers(&root, &needs);

    let dispatch_fn = format_ident!("__undra_dispatch_{}", type_name);
    let meta_static = format_ident!("__UNDRA_META_{}", type_name);
    let ctor_metas = constructors.iter().map(|m| {
        let name = &m.name;
        method_meta(&root, m, &quote!(#meta::ids::method_id(#type_name, #name)))
    });
    let method_metas = methods.iter().map(|m| {
        let name = &m.name;
        method_meta(&root, m, &quote!(#meta::ids::method_id(#type_name, #name)))
    });
    let store_meta = if store {
        quote!(::core::option::Option::Some(<#self_ty>::__UNDRA_STORE_META))
    } else {
        quote!(::core::option::Option::None)
    };

    // The impl block and the struct must agree on whether this is a store.
    let probe_message = if store {
        Diag::new(
            code::E0011,
            format!(
                "`{type_name}` is implemented with `#[undra::api(store)]` but the struct has no `#[undra::store]`"
            ),
            "the `store` marker wires the constructors to the struct's signals, which only `#[undra::store]` sets up",
            format!(
                "add `#[undra::store]` to `struct {type_name}`, or remove `store` from the impl attribute"
            ),
        )
    } else {
        Diag::new(
            code::E0011,
            format!(
                "`{type_name}` is a `#[undra::store]` but its `#[undra::api]` impl block is not marked as a store"
            ),
            "the impl block of a store must say so, so its constructors can attach the store's signals and its struct literals get the hidden cell field",
            "write `#[undra::api(store)]` on the impl block",
        )
    };
    // The text of an assertion is a format string: braces escaped.
    let probe_message = panic_text(&probe_message);
    let probe_assert = if store {
        quote!(::core::assert!(<#self_ty>::__UNDRA_IS_STORE, #probe_message);)
    } else {
        quote!(::core::assert!(!<#self_ty>::__UNDRA_IS_STORE, #probe_message);)
    };

    let derived = derived();
    let registration = submit(&root, "Object", &meta_static);
    let checks = checks.emit(&root);
    let instance_checks = instance_checks.emit(&root);
    // The instantiation of a generic object (ADR-058): the block is the template's, so it is not
    // emitted here; the type is the alias, which carries its name next to its id (the name of a
    // generic application that mentions it is read from there), and a plain object's alias carries
    // the E0070 rule and the check that its template is not a generic store.
    let emitted = if instance.is_some() {
        None
    } else {
        Some(&item)
    };
    let alias_span = self_ty.span();
    let alias_rule = match instance {
        Some(instance) if instance.plain => {
            super::generic::duplicate_alias_constant(&instance.template, alias_span)
        }
        _ => TokenStream::new(),
    };
    let object_name = if instance.is_some() {
        quote! {
            /// The declared name of the instantiation, for a signature that names it through a
            /// generic application (`Arc<Selection<T>>`).
            #[doc(hidden)]
            pub const __UNDRA_OBJECT_NAME: &'static str = #type_name;
        }
    } else {
        TokenStream::new()
    };
    let generic_store_probe = match instance {
        Some(instance) if instance.plain => {
            let message = panic_text(&Diag::new(
                code::E0011,
                format!(
                    "`{0}` is a `#[undra::store(generic)]` but its impl block is not marked as a store",
                    instance.template
                ),
                "the impl block of a store must say so, so its constructors can attach the store's signals and its struct literals get the hidden cell field",
                "write `#[undra::api(store, generic)]` on the impl block",
            ));
            quote_spanned! {alias_span=>
                #[doc(hidden)]
                #[allow(non_camel_case_types, dead_code)]
                const _: () = {
                    trait __UndraGenericStoreProbe {
                        const __UNDRA_IS_GENERIC_STORE: bool = false;
                    }
                    impl<__UndraT: ?::core::marker::Sized> __UndraGenericStoreProbe for __UndraT {}
                    ::core::assert!(!<#self_ty>::__UNDRA_IS_GENERIC_STORE, #message);
                };
            }
        }
        _ => TokenStream::new(),
    };
    // What the runtime calls on a store, forwarding to the members `#[undra::store]` defines.
    let store_object = if store {
        quote! {
            #derived
            impl #runtime::StoreObject for #self_ty {
                fn cell(&self) -> &::std::sync::Arc<#signals::StoreCell> {
                    self.__undra_cell_ref()
                }

                fn restore(
                    __ctx: #runtime::Ctx,
                    __r: &mut #wire::Reader<'_>,
                ) -> ::core::result::Result<Self, #wire::WireError> {
                    Self::__undra_restore(__ctx, __r)
                }
            }
        }
    } else {
        TokenStream::new()
    };
    // What the runtime needs of a store to hand one the host did not construct (ADR-040).
    let object_hooks = if store {
        quote! {
            fn __undra_store_cell(&self) -> ::core::option::Option<&::std::sync::Arc<#signals::StoreCell>> {
                ::core::option::Option::Some(self.__undra_cell_ref())
            }
            fn __undra_attach(&self) -> ::core::result::Result<(), #signals::SignalsError> {
                self.__undra_attach_all()
            }
        }
    } else {
        TokenStream::new()
    };
    // A store's docs are its struct's docs, then its impl block's (a plain object's struct has
    // no Undra attribute, so its docs are not visible here: document it on the impl block).
    let object_docs = if store {
        quote! {{
            const __UNDRA_A: &str = <#self_ty>::__UNDRA_DOCS;
            const __UNDRA_B: &str = #type_docs;
            const __UNDRA_SEP: usize = if __UNDRA_A.is_empty() || __UNDRA_B.is_empty() { 0 } else { 2 };
            const __UNDRA_N: usize = __UNDRA_A.len() + __UNDRA_SEP + __UNDRA_B.len();
            const __UNDRA_BYTES: [u8; __UNDRA_N] = {
                let (__a, __b) = (__UNDRA_A.as_bytes(), __UNDRA_B.as_bytes());
                let mut __out = [0u8; __UNDRA_N];
                let mut __i = 0;
                while __i < __a.len() {
                    __out[__i] = __a[__i];
                    __i += 1;
                }
                if __UNDRA_SEP == 2 {
                    __out[__a.len()] = b'\n';
                    __out[__a.len() + 1] = b'\n';
                }
                let mut __j = 0;
                while __j < __b.len() {
                    __out[__a.len() + __UNDRA_SEP + __j] = __b[__j];
                    __j += 1;
                }
                __out
            };
            match ::core::str::from_utf8(&__UNDRA_BYTES) {
                ::core::result::Result::Ok(__s) => __s,
                ::core::result::Result::Err(_) => "",
            }
        }}
    } else {
        quote!(#type_docs)
    };
    // One `#[undra::api] impl` block per type: the object impl and the `__UNDRA_IS_OBJECT`
    // marker are defined on the type itself, so a second block defines them twice. This constant
    // repeats in a second block too, and its duplicate-definition error, which `rustc` reports
    // before the conflicting impls, reads as the rule and the fix.
    let one_block = format_ident!(
        "_undra_error_{}_{}_has_two_undra_api_impl_blocks_merge_them_into_one",
        code::E0007,
        type_name
    );

    // The members `#[undra::store]` defines inherently on the struct, with harmless fallbacks
    // for every type that is not one. The fallbacks live in a trait of their own per type
    // (implemented for that type only, so two objects in one module do not clash). Inherent
    // items win over trait items in method and path resolution, so a real store resolves to
    // its own members, anything else to the fallback, and a
    // mismatch between the struct and the impl block's `store` marker yields the single
    // branded E0011 below instead of a cascade of "no method named .." errors.
    let probe_trait = format_ident!("__UndraStoreProbe_{}", type_name);

    Ok(quote! {
        #emitted

        #[doc(hidden)]
        #[allow(non_upper_case_globals, dead_code)]
        const #one_block: () = ();

        impl #self_ty {
            #alias_rule
            /// Marks the type as an object, so a signature that uses it as a value can say so.
            #[doc(hidden)]
            pub const __UNDRA_IS_OBJECT: bool = true;
            /// The type id of the declared name: what a signature that takes or returns the
            /// object as `Arc<T>` or `&T` is checked against (E0061).
            #[doc(hidden)]
            pub const __UNDRA_OBJECT_ID: u32 = #meta::ids::type_id(#type_name);
            #object_name
        }

        #generic_store_probe

        // Private to this block: a second block for the type does not define any of this twice.
        #[doc(hidden)]
        #[allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code)]
        const _: () = {
            trait #probe_trait {
                const __UNDRA_IS_STORE: bool = false;
                const __UNDRA_DOCS: &'static str = "";
                fn __undra_cell_ref(&self) -> &::std::sync::Arc<#signals::StoreCell> {
                    ::core::unreachable!("not a `#[undra::store]`: E0011 stops the build first")
                }
                fn __undra_restore(
                    _ctx: #runtime::Ctx,
                    _r: &mut #wire::Reader<'_>,
                ) -> ::core::result::Result<Self, #wire::WireError>
                where
                    Self: ::core::marker::Sized,
                {
                    ::core::unreachable!("not a `#[undra::store]`: E0011 stops the build first")
                }
                const __UNDRA_STORE_META: #meta::StoreMeta = #meta::StoreMeta { signals: &[] };
                fn __undra_attach_all(&self) -> ::core::result::Result<(), #signals::SignalsError> {
                    ::core::result::Result::Ok(())
                }
                fn __undra_set_handle(&self, _handle: u64) {}
            }
            impl #probe_trait for #self_ty {}

            // The assertion is the length of an array in a signature, so `rustc` evaluates it
            // while it checks signatures, before any function body: E0011 comes before what the
            // bodies get wrong because of it (a struct literal patched with `__undra_cell` when
            // the struct is not a store).
            fn __undra_store_probe() -> [(); { #probe_assert 0 }] {
                []
            }

            #store_object

            #derived
            impl #runtime::UndraObject for #self_ty {
                const TYPE_ID: u32 = #meta::ids::type_id(#type_name);
                const NAME: &'static str = #type_name;
                #object_hooks
            }

            #[allow(unused_variables, unused_mut, deprecated, clippy::all)]
            fn #dispatch_fn(
                __rt: &dyn ::core::any::Any,
                __call: #meta::DispatchCall<'_>,
            ) -> #meta::DispatchOutcome {
                #helper_items
                let ::core::option::Option::Some(__rt) = __rt.downcast_ref::<#runtime::Runtime>() else {
                    return __undra_unknown();
                };
                #( const #id_consts: u32 = #id_values; )*
                match __call.method_id {
                    #(#arms)*
                    _ => __undra_unknown(),
                }
            }

            static #meta_static: #meta::ObjectMeta = #meta::ObjectMeta {
                name: #type_name,
                type_id: #meta::ids::type_id(#type_name),
                constructors: &[ #(#ctor_metas),* ],
                methods: &[ #(#method_metas),* ],
                store: #store_meta,
                docs: #object_docs,
                dispatch: #dispatch_fn,
            };
            #registration
        };

        #checks
        #(#template_checks)*
        #instance_checks
    })
}

/// E0007 for an Undra attribute macro on a method of an `#[undra::api] impl` block.
///
/// The impl block's macro expands first and sees the method's attributes unexpanded, so this
/// is where "put `#[undra::api]` on the block, not on the method" and "a query is not a method"
/// can be said in Undra's words instead of `rustc`'s.
fn reject_undra_macros(attrs: &[syn::Attribute], errors: &mut Errors) -> bool {
    let mut found = false;
    for attr in attrs {
        let Some(name) = undra_macro_name(attr) else {
            continue;
        };
        let (what, why, help) = match name.as_str() {
            "api" => (
                "`#[undra::api]` on a method of an `#[undra::api] impl` block".to_owned(),
                "the attribute on the impl block already exposes every `pub fn` of it; a method is not exposed one by one",
                "remove the attribute from the method",
            ),
            "query" | "mutation" => (
                format!("`#[undra::{name}]` inside an `impl` block"),
                "a query or mutation is a free function: the macro generates a struct next to it, which an impl block cannot hold",
                "move the function out of the impl block; it takes `ctx: &Ctx` first, so it does not need `self`",
            ),
            other => (
                format!("`#[undra::{other}]` on a method"),
                "this macro applies to a whole item (a type, a trait or a free function), not to a method of an impl block",
                "remove the attribute, or move the item out of the impl block",
            ),
        };
        errors.push(Diag::new(code::E0007, what, why, help).on(attr));
        found = true;
    }
    found
}

/// The macro named by `#[undra::name]` / `#[undra_macros::name]`.
pub(crate) fn undra_macro_name(attr: &syn::Attribute) -> Option<String> {
    let path = attr.path();
    if !is_undra_macro_path(path) {
        return None;
    }
    let name = path.segments.last()?.ident.to_string();
    matches!(
        name.as_str(),
        "api" | "query" | "mutation" | "port" | "store" | "error"
    )
    .then_some(name)
}

// ---------------------------------------------------------------------------------------------
// Free functions
// ---------------------------------------------------------------------------------------------

/// One instantiation of a generic function (ADR-058): the schema name and ids it is registered
/// under, and the call it makes.
pub(crate) struct FnInstance {
    pub(crate) name: String,
    pub(crate) suffix: String,
    pub(crate) turbofish: TokenStream,
    pub(crate) label: Label,
}

/// Expands `#[undra::api]` on a free function.
pub(crate) fn expand_fn(args_root: Option<Root>, item: ItemFn) -> syn::Result<TokenStream> {
    expand_fn_as(args_root, item, None)
}

/// Expands `#[undra::api(generic(T = [Todo, Note]))]` on a free function with a type parameter
/// (ADR-058): the function as written, one analysis of its signature with the type parameter left
/// as it is (so a mistake that does not depend on the type is reported once), and the ordinary
/// function expansion once per listed type, on the signature with the type substituted.
pub(crate) fn expand_generic_fn(
    args_root: Option<Root>,
    mut item: ItemFn,
    lists: &[super::attrs::GenericList],
) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    let root = item_root(&mut item.attrs, args_root, &mut errors);
    errors.finish()?;
    let mut errors = Errors::new();
    let fn_name = unraw(&item.sig.ident);
    if mentions_self(item.sig.to_token_stream()) && item.sig.receiver().is_none() {
        // The same finding as an ordinary function (see `expand_fn_as`).
        return expand_fn_as(Some(root), item, None);
    }
    let Some(plan) = generic_fn::plan(&item.sig, GenericOn::Function, lists, &mut errors) else {
        errors.finish()?;
        unreachable!("a plan that is not made has said why");
    };

    // The template pass: the signature as written.
    let mut template = Errors::new();
    let analysis = analyze(&mut item.sig, &mut template, Kindred::Callable, None);
    if analysis.has_receiver {
        template.push(has_receiver_error(&item.sig));
    }
    generic_fn::check_parameter_use(&item.sig, &plan.param, &mut template);
    let ret = match map_method_return(&item.sig.output) {
        Ok(ret) => ret,
        Err(err) => {
            template.push(err.into_error());
            KType::Unit
        }
    };
    let mut checks = Checks::for_template(&item.sig.ident, vec![plan.param.to_string()]);
    for p in &analysis.params {
        checks.ty(&p.ty, &p.kty);
    }
    checks.ret(&item.sig.output, &ret);
    ensure_static_streams_in(&mut item.sig.output);
    errors.absorb(template);
    errors.finish()?;
    let template_checks = checks.emit(&root);

    let mut out = TokenStream::new();
    let mut errors = Errors::new();
    for instance in &plan.instances {
        let _guard = in_instance(&instance.name);
        let concrete = ItemFn {
            sig: generic_fn::concrete_signature(&item.sig, &plan.param, &instance.ty),
            ..item.clone()
        };
        let label = Label {
            of: fn_name.clone(),
            param: plan.param.to_string(),
            arg: instance.arg_name.clone(),
            inferred: generic_fn::inferred(&item.sig, &plan.param),
        };
        let fn_instance = FnInstance {
            name: instance.name.clone(),
            suffix: instance.suffix.clone(),
            turbofish: instance.turbofish.clone(),
            label,
        };
        match expand_fn_as(
            Some(root.clone()),
            concrete,
            Some((fn_instance, &plan, instance)),
        ) {
            Ok(tokens) => out.extend(tokens),
            Err(error) => errors.push(error),
        }
    }
    errors.finish()?;
    Ok(quote! {
        #item
        #template_checks
        #out
    })
}

fn has_receiver_error(sig: &Signature) -> syn::Error {
    Diag::new(
        code::E0007,
        format!("`#[undra::api]` on the method `{}`", sig.ident),
        "`#[undra::api]` on a function exposes a free function; the methods of an object are exposed by putting the attribute on the `impl` block they are in",
        "remove `#[undra::api]` from the method and write `#[undra::api]` above `impl Type { .. }`",
    )
    .on(&sig.ident)
}

/// `instance` is `Some` for one instantiation of a generic function: the function itself is
/// emitted once by [`expand_generic_fn`], so only what is particular to the instantiation is.
fn expand_fn_as(
    args_root: Option<Root>,
    mut item: ItemFn,
    instance: Option<(FnInstance, &Plan, &generic_fn::Instance)>,
) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    let root = item_root(&mut item.attrs, args_root, &mut errors);
    if mentions_self(item.sig.to_token_stream()) && item.sig.receiver().is_none() {
        // An associated function (a constructor, usually) of an impl block that is not itself
        // `#[undra::api]`: a free function cannot name `Self`. This is the one finding; what the
        // rest of the signature would add (`Self` is not a wire type either) is only noise.
        return Err(
            Diag::new(
                code::E0007,
                format!(
                    "`#[undra::api]` on `{}`, which is an associated function: its signature uses `Self`",
                    item.sig.ident
                ),
                "`Self` only exists inside an `impl` block, and `#[undra::api]` on a function exposes a free function; the functions of an object are exposed by putting the attribute on the `impl` block they are in",
                format!(
                    "remove `#[undra::api]` from `{}` and write `#[undra::api]` above the `impl` block (it exposes every `pub fn` in it)",
                    item.sig.ident
                ),
            )
            .on(&item.sig.ident),
        );
    }
    let generics = instance.is_none().then_some(GenericOn::Function);
    let analysis = analyze(&mut item.sig, &mut errors, Kindred::Callable, generics);
    if analysis.has_receiver {
        errors.push(has_receiver_error(&item.sig));
    }
    let ret = match map_method_return(&item.sig.output) {
        Ok(ret) => ret,
        Err(err) => {
            errors.push(err.into_error());
            KType::Unit
        }
    };
    let mut checks = match &instance {
        Some(_) => Checks::for_listed_instance(&item.sig.ident),
        None => Checks::new(),
    };
    if let Some((_, plan, listed)) = &instance {
        checks.listed(
            &listed.ty,
            &listed.arg_name,
            &plan.param.to_string(),
            &plan.fn_name,
        );
    }
    for p in &analysis.params {
        checks.ty(&p.ty, &p.kty);
    }
    checks.ret(&item.sig.output, &ret);
    let stream_item = stream_item_type(&item.sig.output);
    let stream_in_result = stream_in_result(&item.sig.output);
    ensure_static_streams_in(&mut item.sig.output);
    errors.finish()?;
    let checks = checks.emit(&root);
    let (name, suffix, turbofish, label) = match instance {
        Some((instance, ..)) => (
            instance.name,
            instance.suffix,
            Some(instance.turbofish),
            Some(instance.label),
        ),
        None => {
            let name = unraw(&item.sig.ident);
            (name.clone(), name, None, None)
        }
    };

    let model = FnModel {
        ident: item.sig.ident.clone(),
        name: name.clone(),
        kind: Kind::Function,
        is_async: analysis.is_async,
        ctx: analysis.ctx,
        params: analysis.params,
        ret,
        ctor_shared: false,
        stream_item,
        stream_in_result,
        docs: docs(&item.attrs),
        suffix: suffix.clone(),
        turbofish,
        generic: label,
    };

    let meta = root.meta();
    let target = Target {
        self_ty: None,
        store: false,
    };
    let mut needs = Needs::default();
    let body = arm_body(&root, &model, &target, &mut needs);
    let helper_items = helpers(&root, &needs);
    let runtime = root.runtime();
    let dispatch_fn = format_ident!("__undra_dispatch_fn_{}", suffix);
    let meta_static = format_ident!("__UNDRA_META_fn_{}", suffix);
    let function_id = quote!(#meta::ids::function_id(#name));
    let params = model
        .params
        .iter()
        .map(|p| param_meta(&meta, &p.name, &p.kty));
    let returns = model.ret.meta(&meta);
    let is_async = model.is_async;
    let takes_ctx = model.ctx.is_some();
    let docs_text = &model.docs;
    let generic = Label::meta(model.generic.as_ref(), &meta);
    let registration = submit(&root, "Function", &meta_static);
    // The function itself is emitted once, by `expand_generic_fn`.
    let emitted = if model.generic.is_some() {
        None
    } else {
        Some(&item)
    };

    Ok(quote! {
        #emitted

        #[doc(hidden)]
        #[allow(non_snake_case, unused_variables, unused_mut, deprecated, clippy::all)]
        fn #dispatch_fn(
            __rt: &dyn ::core::any::Any,
            __call: #meta::DispatchCall<'_>,
        ) -> #meta::DispatchOutcome {
            #helper_items
            let ::core::option::Option::Some(__rt) = __rt.downcast_ref::<#runtime::Runtime>() else {
                return __undra_unknown();
            };
            if __call.method_id != #function_id {
                return __undra_unknown();
            }
            #body
        }

        #[allow(non_upper_case_globals)]
        static #meta_static: #meta::FunctionMeta = #meta::FunctionMeta {
            name: #name,
            method_id: #function_id,
            params: &[ #(#params),* ],
            returns: #returns,
            is_async: #is_async,
            takes_ctx: #takes_ctx,
            generic: #generic,
            docs: #docs_text,
            dispatch: #dispatch_fn,
        };
        #registration

        #checks
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::has;

    const SEND_FN: &str = "_undra_error_E0022_the_future_of_an_async_method_must_be_Send";

    fn impl_result(src: &str, store: bool) -> Result<String, String> {
        let item: ItemImpl = syn::parse_str(src).unwrap();
        expand_impl(None, store, item)
            .map(|t| t.to_string())
            .map_err(|e| e.to_string())
    }

    fn impl_error(src: &str) -> String {
        impl_result(src, false).expect_err("expected a diagnostic")
    }

    #[test]
    fn methods_and_constructors_are_listed_separately() {
        let out = impl_result(
            "impl Calc { pub fn new() -> Self { Calc } pub fn add(&self, a: i32, b: i32) -> i32 { a + b } fn helper(&self) {} }",
            false,
        )
        .unwrap();
        assert!(
            has(
                &out,
                "constructors: &[::undra::meta::MethodMeta { name: \"new\""
            ),
            "{out}"
        );
        assert!(
            has(&out, "methods: &[::undra::meta::MethodMeta { name: \"add\""),
            "{out}"
        );
        assert!(!has(&out, "name: \"helper\""), "{out}");
        assert!(
            has(&out, "impl ::undra::runtime::UndraObject for Calc"),
            "{out}"
        );
        assert!(has(&out, "fn __undra_dispatch_Calc"), "{out}");
        assert!(
            has(&out, "::undra::meta::ids::method_id(\"Calc\", \"add\")"),
            "{out}"
        );
    }

    #[test]
    fn receivers_are_checked() {
        assert!(impl_error("impl C { pub fn f(&mut self) {} }").contains("error[undra::E0020]"));
        assert!(impl_error("impl C { pub fn f(self) {} }").contains("error[undra::E0021]"));
        assert!(impl_error("impl C { pub fn f(mut self) {} }").contains("error[undra::E0021]"));
        assert!(impl_error("impl C { pub fn f(self: Self) {} }").contains("error[undra::E0021]"));
        // Receivers that are neither `&self` nor by value are not callable through a handle.
        for receiver in [
            "Arc<Self>",
            "Box<Self>",
            "Pin<&Self>",
            "&Arc<Self>",
            "Rc<Self>",
        ] {
            let message = impl_error(&format!("impl C {{ pub fn f(self: {receiver}) {{}} }}"));
            assert!(
                message.contains("error[undra::E0007]: the receiver `self: "),
                "{receiver}: {message}"
            );
            assert!(message.contains("write `&self`"), "{receiver}: {message}");
        }
        assert!(
            impl_error("impl C { pub fn f(self: &mut Self) {} }").contains("error[undra::E0020]")
        );
        assert!(impl_error("impl C { pub fn f<'a>(&'a self) {} }").contains("error[undra::E0003]"));
        assert!(impl_result("impl C { pub fn f(self: &Self) {} }", false).is_ok());
    }

    #[test]
    fn ctx_rules() {
        assert!(
            impl_error("impl C { pub fn f(&self, ctx: &Ctx) {} }")
                .contains("`Ctx` parameter on method")
        );
        assert!(
            impl_error("impl C { pub fn new(a: u8, ctx: &Ctx) -> Self { C } }")
                .contains("must be the first")
        );
        assert!(
            impl_error("impl C { pub fn new(ctx: &mut Ctx) -> Self { C } }").contains("&mut Ctx")
        );
        assert!(impl_result("impl C { pub fn new(ctx: &Ctx) -> Self { C } }", false).is_ok());
        assert!(
            impl_result(
                "impl C { pub fn new(ctx: Ctx, a: u8) -> Self { C } }",
                false
            )
            .is_ok()
        );
        let out = impl_result("impl C { pub fn new(ctx: Ctx) -> Self { C } }", false).unwrap();
        assert!(has(&out, "takes_ctx: true"), "{out}");
        assert!(has(&out, "__rt.ctx()"), "{out}");
    }

    #[test]
    fn functions_without_receiver_must_construct() {
        let message = impl_error("impl C { pub fn helper() -> u8 { 1 } }");
        assert!(
            message.contains("error[undra::E0007]: `helper` is neither a method nor a constructor")
        );
    }

    #[test]
    fn constructors_may_be_fallible_but_not_async() {
        let out = impl_result(
            "impl C { pub fn new() -> Result<Self, CError> { Ok(C) } }",
            false,
        )
        .unwrap();
        assert!(
            has(
                &out,
                "TypeRefMeta::Result(&::undra::meta::TypeRefMeta::Named(\"C\"), &::undra::meta::TypeRefMeta::Named(\"CError\"))"
            ),
            "{out}"
        );
        let message = impl_error("impl C { pub async fn new() -> Self { C } }");
        assert!(message.contains("constructor `new` is `async`"));
    }

    #[test]
    fn trait_impls_and_generic_impls_are_rejected() {
        assert!(impl_error("impl Trait for C { pub fn f(&self) {} }").contains("trait impl"));
        assert!(impl_error("impl<T> C<T> { pub fn f(&self) {} }").contains("E0002"));
        assert!(impl_error("impl C { pub fn f<T>(&self, x: T) {} }").contains("E0002"));
    }

    #[test]
    fn parameter_types_are_checked_with_their_codes() {
        assert!(impl_error("impl C { pub fn f(&self, s: &str) {} }").contains("E0001"));
        assert!(impl_error("impl C { pub fn f(&self, f: Box<dyn Fn()>) {} }").contains("E0004"));
        assert!(impl_error("impl C { pub fn f(&self, r: Result<u8, E>) {} }").contains("E0005"));
        assert!(impl_error("impl C { pub fn f(&self, m: HashMap<f64, u8>) {} }").contains("E0006"));
        assert!(
            impl_error("impl C { pub fn f(&self, (a, b): (u8, u8)) {} }").contains("is a pattern")
        );
    }

    #[test]
    fn store_marker_requires_a_constructor() {
        let item = "impl S { pub fn get(&self) -> u8 { 1 } }";
        assert!(
            impl_result(item, true)
                .unwrap_err()
                .contains("error[undra::E0011]: store `S` has no constructor")
        );
    }

    #[test]
    fn store_impls_patch_struct_literals() {
        let out = impl_result(
            "impl S { pub fn new() -> Self { let a = Self { x: 1 }; let b = S { x: 2 }; let c = Other { x: 3 }; a } fn r() -> Self { Self { x: 1, ..base() } } }",
            true,
        )
        .unwrap();
        assert_eq!(
            crate::tests::squash(&out)
                .matches("__undra_cell:::core::default::Default::default()")
                .count(),
            2,
            "{out}"
        );
        assert!(has(&out, "<S>::__UNDRA_STORE_META"), "{out}");
        assert!(has(&out, "__value.__undra_attach_all()"), "{out}");
        // `impl StoreObject` lives next to `impl UndraObject`: a store without an impl block
        // then gets only the branded E0011.
        assert!(
            has(&out, "impl ::undra::runtime::StoreObject for S"),
            "{out}"
        );
    }

    #[test]
    fn non_store_impls_are_left_alone() {
        let out = impl_result("impl S { pub fn new() -> Self { Self { x: 1 } } }", false).unwrap();
        assert!(
            !has(&out, "__undra_cell:"),
            "no cell field is added to literals: {out}"
        );
        assert!(!has(&out, "impl ::undra::runtime::StoreObject"), "{out}");
        assert!(!has(&out, "__value.__undra_attach_all()"), "{out}");
        assert!(has(&out, "store: ::core::option::Option::None"), "{out}");
    }

    #[test]
    fn async_methods_get_a_send_check() {
        let out = impl_result("impl C { pub async fn f(&self) -> u8 { 1 } }", false).unwrap();
        assert!(has(&out, SEND_FN), "{out}");
        assert!(has(&out, &format!("{SEND_FN}(&__fut)")), "{out}");
        assert!(has(&out, "is_async: true"), "{out}");
        let out = impl_result("impl C { pub fn f(&self) -> u8 { 1 } }", false).unwrap();
        assert!(!has(&out, SEND_FN), "{out}");
    }

    #[test]
    fn stream_methods_bring_their_adapters() {
        let out = impl_result(
            "impl C { pub fn f(&self) -> impl Stream<Item = u8> { s() } }",
            false,
        )
        .unwrap();
        assert!(has(&out, "struct __UndraMap"), "{out}");
        assert!(!has(&out, "enum __UndraOpening"), "{out}");
        let out = impl_result(
            "impl C { pub async fn f(&self) -> Result<impl Stream<Item = u8>, E> { s() } }",
            false,
        )
        .unwrap();
        assert!(has(&out, "enum __UndraOpening"), "{out}");
    }

    fn fn_result(src: &str) -> Result<String, String> {
        let item: ItemFn = syn::parse_str(src).unwrap();
        expand_fn(None, item)
            .map(|t| t.to_string())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn free_functions_register_function_meta() {
        let out = fn_result("pub fn greet(name: String) -> String { name }").unwrap();
        assert!(has(&out, "FunctionMeta"), "{out}");
        assert!(has(&out, "function_id(\"greet\")"), "{out}");
        assert!(has(&out, "Registration::Function"), "{out}");
        assert!(has(&out, "fn __undra_dispatch_fn_greet"), "{out}");
    }

    #[test]
    fn free_function_diagnostics() {
        assert!(
            fn_result("pub fn f<T>(x: T) {}")
                .unwrap_err()
                .contains("E0002")
        );
        assert!(
            fn_result("pub fn f(x: usize) {}")
                .unwrap_err()
                .contains("E0001")
        );
        assert!(
            fn_result("pub unsafe fn f() {}")
                .unwrap_err()
                .contains("unsafe")
        );
    }
}
