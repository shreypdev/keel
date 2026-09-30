//! Objects (`#[keel::api] impl Type { .. }`) and free functions (`#[keel::api] fn`).
//!
//! For an impl block the macro keeps the impl as written and adds:
//!
//! * `impl KeelObject for Type { TYPE_ID, NAME }`;
//! * `fn __keel_dispatch_Type(rt: &dyn Any, call: DispatchCall) -> DispatchOutcome`, which
//!   downcasts `rt` to `&Runtime`, matches `call.method_id` against per-method constants
//!   (`ids::method_id`), decodes the arguments in order, resolves `self` with
//!   `rt.object::<Type>(call.handle)`, runs the method and encodes the outcome as
//!   `DispatchResult::{Sync, Async, Stream}`; malformed requests and stale handles answer
//!   `DispatchResult::BadRequest` with a reason, and only an unknown method id answers
//!   `DispatchResult::Unknown`. The decoded arguments are bound to positional locals
//!   (`__keel_a0`, ..), never to the user's parameter names, so no parameter name can collide
//!   with a generated local;
//! * `static __KEEL_META_Type: ObjectMeta` and its registration;
//! * the identity checks of `check.rs` for every parameter and return type, and the hidden
//!   `__KEEL_IS_OBJECT` marker that lets them say "an object cannot be a value" (E0064).
//!
//! A second `#[keel::api] impl` block for the same type defines the dispatcher twice: the
//! expansion carries a constant named after the rule, so the duplicate-definition error says
//! what is wrong.
//!
//! Constructors (a `pub fn` without receiver that returns `Self` or `Result<Self, E>`) are
//! listed separately in the meta. Their dispatch arm builds the value, inserts it into the
//! object table (`rt.insert_object(Arc::new(value))`) and replies with the handle. In a
//! store's impl block (`#[keel::api(store)]`) the arm also attaches the signals first (see
//! `store.rs`), and struct literals of the store inside the block get the hidden cell field
//! added.

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::visit_mut::{self, VisitMut};
use syn::{FnArg, ImplItem, ItemFn, ItemImpl, Pat, ReturnType, Signature, Type, Visibility};

use super::attrs::{Site, docs, is_keel_macro_path, take};
use super::check::Checks;
use super::common::{check_generics, derived, item_root, param_meta, submit};
use super::diag::{Diag, Errors, code};
use super::naming::{fnv1a32, unraw};
use super::paths::Root;
use super::types::{Allow, KType, Pos, map_error_type, map_return, map_type, ty_string};

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
    /// The `T` of a returned `impl Stream<Item = T>` (also inside `Result<.., E>`).
    pub(crate) stream_item: Option<Type>,
    pub(crate) docs: String,
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

/// Checks the generics, receiver and parameters of `sig` and collects the parameters.
pub(crate) fn analyze(sig: &mut Signature, errors: &mut Errors) -> Analysis {
    let fn_name = sig.ident.to_string();
    check_generics(&sig.generics, &fn_name, errors);
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
                        "store a `Ctx` in the object when it is constructed, or use `Ctx::current()`",
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
        let kty = match map_type(&pat_type.ty, Pos::Param, Allow::NONE) {
            Ok(kty) => kty,
            Err(err) => {
                errors.push(err.into_error());
                KType::Unit
            }
        };
        params.push(ParamModel {
            name: unraw(&ident),
            ty: (*pat_type.ty).clone(),
            kty,
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
}

/// The local that holds the decoded argument at `index`.
pub(crate) fn arg_local(index: usize) -> syn::Ident {
    format_ident!("__keel_a{}", index)
}

fn enc(wire: &TokenStream, value: &TokenStream) -> TokenStream {
    quote!(#wire::Encode::encode_to_vec(&#value))
}

fn is_stream_ok(ret: &KType) -> bool {
    matches!(ret, KType::Result(ok, _) if matches!(**ok, KType::Stream(_)))
}

/// The expression of the outcome of a method, function or query-like call as a
/// `DispatchResult`.
fn call_result(root: &Root, m: &FnModel, call: &TokenStream, needs: &mut Needs) -> TokenStream {
    let wire = root.wire();
    let runtime = root.runtime();
    let span = m.ident.span();
    let sync_ok = |value: TokenStream| {
        let bytes = enc(&wire, &value);
        quote!(#runtime::DispatchResult::Sync(::core::result::Result::Ok(#bytes)))
    };
    let sync_err = |value: TokenStream| {
        let bytes = enc(&wire, &value);
        quote!(#runtime::DispatchResult::Sync(::core::result::Result::Err(#bytes)))
    };
    let assert_send = |what: TokenStream| quote_spanned!(span=> __keel_assert_send(&#what););
    let out_ok = |value: TokenStream| {
        let bytes = enc(&wire, &value);
        quote!(::core::result::Result::<_, ::std::vec::Vec<u8>>::Ok(#bytes))
    };
    let out_err = |value: TokenStream| {
        let bytes = enc(&wire, &value);
        quote!(::core::result::Result::<::std::vec::Vec<u8>, _>::Err(#bytes))
    };

    match (&m.ret, m.is_async) {
        // A stream.
        (KType::Stream(_), false) => {
            needs.map_stream = true;
            needs.send_assert = true;
            let check = assert_send(quote!(__stream));
            quote! {{
                let __stream = __KeelMap(::std::boxed::Box::pin(#call));
                #check
                #runtime::DispatchResult::Stream(::std::boxed::Box::pin(__stream))
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
                let __stream = __KeelOpening::new(async move {
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
                #runtime::DispatchResult::Stream(::std::boxed::Box::pin(__stream))
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
                        let __stream = __KeelMap(::std::boxed::Box::pin(__s));
                        #check
                        #runtime::DispatchResult::Stream(::std::boxed::Box::pin(__stream))
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
                let __stream = __KeelOpening::new(async move {
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
                #runtime::DispatchResult::Stream(::std::boxed::Box::pin(__stream))
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
                #runtime::DispatchResult::Async(::std::boxed::Box::pin(__fut))
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
                #runtime::DispatchResult::Async(::std::boxed::Box::pin(__fut))
            }}
        }
    }
}

/// The outcome of a constructor: insert the new object and reply with its handle.
fn constructor_result(
    root: &Root,
    m: &FnModel,
    target: &Target<'_>,
    call: &TokenStream,
) -> TokenStream {
    let wire = root.wire();
    let runtime = root.runtime();
    let reply = enc(&wire, &quote!(__handle));
    let ok = quote!(#runtime::DispatchResult::Sync(::core::result::Result::Ok(#reply)));
    // What to do with the constructed `__value`: publish it and answer its handle. A store
    // first attaches its signals; if that fails the store is never published and the caller
    // gets a bad request carrying the reason (nothing panics).
    let finish = if target.store {
        let type_name = target.self_ty.map(ty_string).unwrap_or_default();
        quote! {
            match __value.__keel_attach_all() {
                ::core::result::Result::Ok(()) => {
                    let __arc = ::std::sync::Arc::new(__value);
                    let __handle = __rt.insert_object(::std::sync::Arc::clone(&__arc));
                    (*__arc).__keel_set_handle(__handle.0);
                    #ok
                }
                ::core::result::Result::Err(__why) => #runtime::DispatchResult::BadRequest(
                    ::std::format!("store `{}` could not attach its signals: {}", #type_name, __why),
                ),
            }
        }
    } else {
        quote! {{
            let __handle = __rt.insert_object(::std::sync::Arc::new(__value));
            #ok
        }}
    };
    let err = enc(&wire, &quote!(__e));
    if matches!(m.ret, KType::Result(..)) {
        quote! {
            match #call {
                ::core::result::Result::Ok(__value) => #finish,
                ::core::result::Result::Err(__e) => {
                    #runtime::DispatchResult::Sync(::core::result::Result::Err(#err))
                }
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
    let lets = m.params.iter().enumerate().map(|(index, p)| {
        let local = arg_local(index);
        let ty = &p.ty;
        let param = &p.name;
        quote_spanned! {p.ty.span()=>
            let #local: #ty = match <#ty as #wire::Decode>::decode(&mut __r) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(::std::format!(
                        "cannot decode argument `{}` of `{}`: {}", #param, #what, __e
                    ));
                }
            };
        }
    });
    let decode = quote! {
        let mut __r = #wire::Reader::new(__call.args);
        #(#lets)*
        if let ::core::result::Result::Err(__e) = __r.finish() {
            return __keel_bad_request(::std::format!(
                "cannot decode the arguments of `{}`: {}", #what, __e
            ));
        }
    };

    // Resolve `self` and the context.
    let receiver = match (m.kind, target.self_ty) {
        (Kind::Method, Some(self_ty)) => quote! {
            let __obj = match __rt.object::<#self_ty>(__call.handle) {
                ::core::result::Result::Ok(__o) => __o,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(::std::format!(
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
    call_args.extend((0..m.params.len()).map(|index| {
        let local = arg_local(index);
        quote!(#local)
    }));
    let ident = &m.ident;
    let call = match target.self_ty {
        Some(self_ty) if m.kind != Kind::Function => {
            quote_spanned!(ident.span()=> #self_ty::#ident( #(#call_args),* ))
        }
        _ => quote_spanned!(ident.span()=> #ident( #(#call_args),* )),
    };

    let result = match m.kind {
        Kind::Constructor => constructor_result(root, m, target, &call),
        _ => call_result(root, m, &call, needs),
    };

    quote! {
        #decode
        #receiver
        #ctx_binding
        __keel_out(#result)
    }
}

/// The helper items every dispatcher carries.
fn helpers(root: &Root, needs: &Needs) -> TokenStream {
    let meta = root.meta();
    let runtime = root.runtime();
    let wire = root.wire();

    let send_assert = if needs.send_assert {
        // E0022: the runtime polls futures and streams on its executor thread, so they must be
        // `Send`. `rustc` cannot carry a Keel code, but this assertion, called with the
        // method's span, makes its own "future cannot be sent between threads safely"
        // error (with the offending value and the `.await` it lives across) point at the
        // method instead of at generated code.
        quote! {
            fn __keel_assert_send<T: ::core::marker::Send>(_: &T) {}
        }
    } else {
        TokenStream::new()
    };

    let map_stream = if needs.map_stream {
        quote! {
            struct __KeelMap<S>(::core::pin::Pin<::std::boxed::Box<S>>);
            impl<S> #runtime::Stream for __KeelMap<S>
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
            enum __KeelOpening<F, S> {
                Opening(::core::pin::Pin<::std::boxed::Box<F>>),
                Open(::core::pin::Pin<::std::boxed::Box<S>>),
                Done,
            }
            impl<F, S> __KeelOpening<F, S> {
                fn new(__fut: F) -> Self {
                    Self::Opening(::std::boxed::Box::pin(__fut))
                }
            }
            impl<F, S> #runtime::Stream for __KeelOpening<F, S>
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

    quote! {
        fn __keel_out(__result: #runtime::DispatchResult) -> #meta::DispatchOutcome {
            #meta::DispatchOutcome::new(__result)
        }
        fn __keel_unknown() -> #meta::DispatchOutcome {
            __keel_out(#runtime::DispatchResult::Unknown)
        }
        #[allow(dead_code)]
        fn __keel_bad_request(__reason: ::std::string::String) -> #meta::DispatchOutcome {
            __keel_out(#runtime::DispatchResult::BadRequest(__reason))
        }
        #send_assert
        #map_stream
        #opening_stream
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
    quote! {
        #meta::MethodMeta {
            name: #name,
            method_id: #method_id,
            params: &[ #(#params),* ],
            returns: #returns,
            is_async: #is_async,
            takes_ctx: #takes_ctx,
            docs: #docs,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Struct literal patching for stores
// ---------------------------------------------------------------------------------------------

/// Adds the hidden `__keel_cell` field to struct literals of the store type inside its
/// `#[keel::api(store)]` impl block (`Self { .. }` and `Type { .. }`).
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
            .any(|f| matches!(&f.member, syn::Member::Named(id) if id == "__keel_cell"));
        if is_store && node.rest.is_none() && !has_cell {
            node.fields.push(syn::parse_quote!(
                __keel_cell: ::core::default::Default::default()
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

fn self_type_name(ty: &Type) -> syn::Result<String> {
    match ty {
        Type::Path(path) if path.qself.is_none() => {
            let seg = path.path.segments.last().expect("a path has a segment");
            if !seg.arguments.is_none() {
                return Err(Diag::new(
                    code::E0002,
                    format!("generic type `{}` in `#[keel::api] impl`", ty_string(ty)),
                    "the schema describes concrete types; every target language would need one instantiation per use",
                    "implement the object for a concrete, non-generic type",
                )
                .on(ty));
            }
            Ok(unraw(&seg.ident))
        }
        other => Err(shape_error(
            format!("`#[keel::api]` on an impl of `{}`", ty_string(other)),
            other,
            "an object is a named struct or enum; the impl block must name it",
            "write `impl TypeName { .. }`",
        )),
    }
}

/// Expands `#[keel::api]` on an inherent `impl` block.
pub(crate) fn expand_impl(
    args_root: Option<Root>,
    store: bool,
    mut item: ItemImpl,
) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    let root = item_root(&mut item.attrs, args_root, &mut errors);
    if let Some((_, path, _)) = &item.trait_ {
        errors.push(shape_error(
            format!("`#[keel::api]` on a trait impl (`impl {} for ..`)", ty_string(path)),
            path,
            "the attribute exposes the inherent methods of an object; trait methods have no schema representation",
            "move the methods you want to expose into an inherent `impl Type { .. }` block",
        ));
    }
    check_generics(&item.generics, "impl", &mut errors);
    let self_ty = (*item.self_ty).clone();
    let type_name = match self_type_name(&self_ty) {
        Ok(name) => name,
        Err(error) => {
            errors.push(error);
            String::new()
        }
    };
    let type_docs = docs(&item.attrs);

    let mut constructors: Vec<FnModel> = Vec::new();
    let mut methods: Vec<FnModel> = Vec::new();
    let mut checks = Checks::new();
    for impl_item in &mut item.items {
        let ImplItem::Fn(func) = impl_item else {
            continue;
        };
        // A query or mutation inside the block is reported once; the function is not also
        // "neither a method nor a constructor".
        if reject_keel_macros(&func.attrs, &mut errors) {
            continue;
        }
        let is_public = matches!(func.vis, Visibility::Public(_));
        take(
            &mut func.attrs,
            if is_public {
                Site::NOTHING
            } else {
                Site::PRIVATE
            },
            &mut errors,
        );
        if !is_public {
            continue; // private helpers are not part of the API
        }
        let fn_docs = docs(&func.attrs);
        let analysis = analyze(&mut func.sig, &mut errors);
        let name = unraw(&func.sig.ident);

        for p in &analysis.params {
            checks.ty(&p.ty, &p.kty);
        }
        if analysis.has_receiver {
            let ret = match map_return(&func.sig.output) {
                Ok(ret) => ret,
                Err(err) => {
                    errors.push(err.into_error());
                    KType::Unit
                }
            };
            checks.ret(&func.sig.output, &ret);
            let stream_item = stream_item_type(&func.sig.output);
            ensure_static_streams_in(&mut func.sig.output);
            methods.push(FnModel {
                ident: func.sig.ident.clone(),
                name,
                kind: Kind::Method,
                is_async: analysis.is_async,
                ctx: None,
                params: analysis.params,
                ret,
                stream_item,
                docs: fn_docs,
            });
        } else if let Some(returns) = ctor_return(&func.sig.output, &type_name) {
            if analysis.is_async {
                errors.push(shape_error(
                    format!("constructor `{name}` is `async`"),
                    &func.sig.asyncness,
                    "a constructor inserts the new object into the object table before it replies, so it must finish synchronously",
                    "construct synchronously and start asynchronous work with `ctx.spawn(..)`",
                ));
            }
            let named = KType::Named(type_name.clone());
            let ret = match returns {
                CtorReturn::Plain => named,
                CtorReturn::Fallible(err_ty) => match map_error_type(&err_ty, Pos::Return) {
                    Ok(err) => {
                        checks.error_ty(&err_ty, &err);
                        KType::Result(Box::new(named), Box::new(err))
                    }
                    Err(err) => {
                        errors.push(err.into_error());
                        named
                    }
                },
            };
            constructors.push(FnModel {
                ident: func.sig.ident.clone(),
                name,
                kind: Kind::Constructor,
                is_async: false,
                ctx: analysis.ctx,
                params: analysis.params,
                ret,
                stream_item: None,
                docs: fn_docs,
            });
        } else {
            errors.push(shape_error(
                format!("`{name}` is neither a method nor a constructor"),
                &func.sig.ident,
                "a `pub fn` in a `#[keel::api] impl` block must take `&self` (a method) or return `Self` / `Result<Self, E>` (a constructor); the return type is read as written, so an alias such as `type R<T> = Result<T, E>` is not followed",
                "add `&self`, make it return `Self` or spell out `Result<Self, E>`, or make it private / move it to a free `#[keel::api] fn`",
            ));
        }
    }

    if store && constructors.is_empty() {
        errors.push(
            Diag::new(
                code::E0011,
                format!("store `{type_name}` has no constructor"),
                "the platforms create a store by calling one of its constructors; without one it can never be instantiated",
                "add `pub fn new(..) -> Self` to the `#[keel::api(store)]` impl block",
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

    if store {
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
        .map(|m| format_ident!("__KEEL_ID_{}", m.name))
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

    let dispatch_fn = format_ident!("__keel_dispatch_{}", type_name);
    let meta_static = format_ident!("__KEEL_META_{}", type_name);
    let ctor_metas = constructors.iter().map(|m| {
        let name = &m.name;
        method_meta(&root, m, &quote!(#meta::ids::method_id(#type_name, #name)))
    });
    let method_metas = methods.iter().map(|m| {
        let name = &m.name;
        method_meta(&root, m, &quote!(#meta::ids::method_id(#type_name, #name)))
    });
    let store_meta = if store {
        quote!(::core::option::Option::Some(<#self_ty>::__KEEL_STORE_META))
    } else {
        quote!(::core::option::Option::None)
    };

    // The impl block and the struct must agree on whether this is a store.
    let probe_message = if store {
        format!(
            "error[keel::E0011]: `{type_name}` is implemented with `#[keel::api(store)]` but the struct has no `#[keel::store]`\n  = note: the `store` marker wires the constructors to the struct's signals, which only `#[keel::store]` sets up\n  = help: add `#[keel::store]` to `struct {type_name}`, or remove `store` from the impl attribute\n  = docs: https://keel.dev/errors/E0011"
        )
    } else {
        format!(
            "error[keel::E0011]: `{type_name}` is a `#[keel::store]` but its `#[keel::api]` impl block is not marked as a store\n  = note: the impl block of a store must say so, so its constructors can attach the store's signals and its struct literals get the hidden cell field\n  = help: write `#[keel::api(store)]` on the impl block\n  = docs: https://keel.dev/errors/E0011"
        )
    };
    let probe_assert = if store {
        quote!(::core::assert!(<#self_ty>::__KEEL_IS_STORE, #probe_message);)
    } else {
        quote!(::core::assert!(!<#self_ty>::__KEEL_IS_STORE, #probe_message);)
    };

    let derived = derived();
    let registration = submit(&root, "Object", &meta_static);
    let checks = checks.emit(&root);
    // What the runtime calls on a store, forwarding to the members `#[keel::store]` defines.
    let store_object = if store {
        quote! {
            #derived
            impl #runtime::StoreObject for #self_ty {
                fn cell(&self) -> &::std::sync::Arc<#signals::StoreCell> {
                    self.__keel_cell_ref()
                }

                fn restore(
                    __ctx: #runtime::Ctx,
                    __r: &mut #wire::Reader<'_>,
                ) -> ::core::result::Result<Self, #wire::WireError> {
                    Self::__keel_restore(__ctx, __r)
                }
            }
        }
    } else {
        TokenStream::new()
    };
    // A store's docs are its struct's docs, then its impl block's (a plain object's struct has
    // no Keel attribute, so its docs are not visible here: document it on the impl block).
    let object_docs = if store {
        quote! {{
            const __KEEL_A: &str = <#self_ty>::__KEEL_DOCS;
            const __KEEL_B: &str = #type_docs;
            const __KEEL_SEP: usize = if __KEEL_A.is_empty() || __KEEL_B.is_empty() { 0 } else { 2 };
            const __KEEL_N: usize = __KEEL_A.len() + __KEEL_SEP + __KEEL_B.len();
            const __KEEL_BYTES: [u8; __KEEL_N] = {
                let (__a, __b) = (__KEEL_A.as_bytes(), __KEEL_B.as_bytes());
                let mut __out = [0u8; __KEEL_N];
                let mut __i = 0;
                while __i < __a.len() {
                    __out[__i] = __a[__i];
                    __i += 1;
                }
                if __KEEL_SEP == 2 {
                    __out[__a.len()] = b'\n';
                    __out[__a.len() + 1] = b'\n';
                }
                let mut __j = 0;
                while __j < __b.len() {
                    __out[__a.len() + __KEEL_SEP + __j] = __b[__j];
                    __j += 1;
                }
                __out
            };
            match ::core::str::from_utf8(&__KEEL_BYTES) {
                ::core::result::Result::Ok(__s) => __s,
                ::core::result::Result::Err(_) => "",
            }
        }}
    } else {
        quote!(#type_docs)
    };
    // One `#[keel::api] impl` block per type: the dispatcher, the registration and the object
    // impl are named after the type. This constant repeats in a second block and its
    // duplicate-definition error then reads as the rule.
    let one_block = format_ident!(
        "_keel_error_E0007_a_type_takes_one_keel_api_impl_block_{}",
        type_name
    );

    // The members `#[keel::store]` defines inherently on the struct, with harmless fallbacks
    // for every type that is not one. The fallbacks live in a trait of their own per type
    // (implemented for that type only, so two objects in one module do not clash). Inherent
    // items win over trait items in method and path resolution, so a real store resolves to
    // its own members, anything else to the fallback, and a
    // mismatch between the struct and the impl block's `store` marker yields the single
    // branded E0011 below instead of a cascade of "no method named .." errors.
    let probe_trait = format_ident!("__KeelStoreProbe_{}", type_name);

    Ok(quote! {
        #item

        #[doc(hidden)]
        #[allow(non_upper_case_globals, dead_code)]
        const #one_block: () = ();

        impl #self_ty {
            /// Marks the type as an object, so a signature that uses it as a value can say so.
            #[doc(hidden)]
            pub const __KEEL_IS_OBJECT: bool = true;
        }

        #[doc(hidden)]
        #[allow(non_camel_case_types, dead_code)]
        trait #probe_trait {
            const __KEEL_IS_STORE: bool = false;
            const __KEEL_DOCS: &'static str = "";
            fn __keel_cell_ref(&self) -> &::std::sync::Arc<#signals::StoreCell> {
                ::core::unreachable!("not a `#[keel::store]`: E0011 stops the build first")
            }
            fn __keel_restore(
                _ctx: #runtime::Ctx,
                _r: &mut #wire::Reader<'_>,
            ) -> ::core::result::Result<Self, #wire::WireError>
            where
                Self: ::core::marker::Sized,
            {
                ::core::unreachable!("not a `#[keel::store]`: E0011 stops the build first")
            }
            const __KEEL_STORE_META: #meta::StoreMeta = #meta::StoreMeta { signals: &[] };
            fn __keel_attach_all(&self) -> ::core::result::Result<(), #signals::SignalsError> {
                ::core::result::Result::Ok(())
            }
            fn __keel_set_handle(&self, _handle: u64) {}
        }
        impl #probe_trait for #self_ty {}

        const _: () = {
            #probe_assert
        };

        #derived
        impl #runtime::KeelObject for #self_ty {
            const TYPE_ID: u32 = #meta::ids::type_id(#type_name);
            const NAME: &'static str = #type_name;
        }

        #store_object

        #[doc(hidden)]
        #[allow(non_snake_case, non_upper_case_globals, unused_variables, unused_mut, deprecated, clippy::all)]
        fn #dispatch_fn(
            __rt: &dyn ::core::any::Any,
            __call: #meta::DispatchCall<'_>,
        ) -> #meta::DispatchOutcome {
            #helper_items
            let ::core::option::Option::Some(__rt) = __rt.downcast_ref::<#runtime::Runtime>() else {
                return __keel_unknown();
            };
            #( const #id_consts: u32 = #id_values; )*
            match __call.method_id {
                #(#arms)*
                _ => __keel_unknown(),
            }
        }

        #[allow(non_upper_case_globals)]
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

        #checks
    })
}

/// E0007 for a Keel attribute macro on a method of an `#[keel::api] impl` block.
///
/// The impl block's macro expands first and sees the method's attributes unexpanded, so this
/// is where "put `#[keel::api]` on the block, not on the method" and "a query is not a method"
/// can be said in Keel's words instead of `rustc`'s.
fn reject_keel_macros(attrs: &[syn::Attribute], errors: &mut Errors) -> bool {
    let mut found = false;
    for attr in attrs {
        let Some(name) = keel_macro_name(attr) else {
            continue;
        };
        let (what, why, help) = match name.as_str() {
            "api" => (
                "`#[keel::api]` on a method of an `#[keel::api] impl` block".to_owned(),
                "the attribute on the impl block already exposes every `pub fn` of it; a method is not exposed one by one",
                "remove the attribute from the method",
            ),
            "query" | "mutation" => (
                format!("`#[keel::{name}]` inside an `impl` block"),
                "a query or mutation is a free function: the macro generates a struct next to it, which an impl block cannot hold",
                "move the function out of the impl block; it takes `ctx: &Ctx` first, so it does not need `self`",
            ),
            other => (
                format!("`#[keel::{other}]` on a method"),
                "this macro applies to a whole item (a type, a trait or a free function), not to a method of an impl block",
                "remove the attribute, or move the item out of the impl block",
            ),
        };
        errors.push(Diag::new(code::E0007, what, why, help).on(attr));
        found = true;
    }
    found
}

/// The macro named by `#[keel::name]` / `#[keel_macros::name]`.
fn keel_macro_name(attr: &syn::Attribute) -> Option<String> {
    let path = attr.path();
    if !is_keel_macro_path(path) {
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

/// Expands `#[keel::api]` on a free function.
pub(crate) fn expand_fn(args_root: Option<Root>, mut item: ItemFn) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    let root = item_root(&mut item.attrs, args_root, &mut errors);
    let analysis = analyze(&mut item.sig, &mut errors);
    if analysis.has_receiver {
        errors.push(
            Diag::new(
                code::E0007,
                format!("`#[keel::api]` on the method `{}`", item.sig.ident),
                "`#[keel::api]` on a function exposes a free function; the methods of an object are exposed by putting the attribute on the `impl` block they are in",
                "remove `#[keel::api]` from the method and write `#[keel::api]` above `impl Type { .. }`",
            )
            .on(&item.sig.ident),
        );
    }
    let ret = match map_return(&item.sig.output) {
        Ok(ret) => ret,
        Err(err) => {
            errors.push(err.into_error());
            KType::Unit
        }
    };
    let mut checks = Checks::new();
    for p in &analysis.params {
        checks.ty(&p.ty, &p.kty);
    }
    checks.ret(&item.sig.output, &ret);
    let stream_item = stream_item_type(&item.sig.output);
    ensure_static_streams_in(&mut item.sig.output);
    errors.finish()?;
    let checks = checks.emit(&root);

    let name = unraw(&item.sig.ident);
    let model = FnModel {
        ident: item.sig.ident.clone(),
        name: name.clone(),
        kind: Kind::Function,
        is_async: analysis.is_async,
        ctx: analysis.ctx,
        params: analysis.params,
        ret,
        stream_item,
        docs: docs(&item.attrs),
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
    let dispatch_fn = format_ident!("__keel_dispatch_fn_{}", name);
    let meta_static = format_ident!("__KEEL_META_fn_{}", name);
    let function_id = quote!(#meta::ids::function_id(#name));
    let params = model
        .params
        .iter()
        .map(|p| param_meta(&meta, &p.name, &p.kty));
    let returns = model.ret.meta(&meta);
    let is_async = model.is_async;
    let takes_ctx = model.ctx.is_some();
    let docs_text = &model.docs;
    let registration = submit(&root, "Function", &meta_static);

    Ok(quote! {
        #item

        #[doc(hidden)]
        #[allow(non_snake_case, unused_variables, unused_mut, deprecated, clippy::all)]
        fn #dispatch_fn(
            __rt: &dyn ::core::any::Any,
            __call: #meta::DispatchCall<'_>,
        ) -> #meta::DispatchOutcome {
            #helper_items
            let ::core::option::Option::Some(__rt) = __rt.downcast_ref::<#runtime::Runtime>() else {
                return __keel_unknown();
            };
            if __call.method_id != #function_id {
                return __keel_unknown();
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
                "constructors: &[::keel::meta::MethodMeta { name: \"new\""
            ),
            "{out}"
        );
        assert!(
            has(&out, "methods: &[::keel::meta::MethodMeta { name: \"add\""),
            "{out}"
        );
        assert!(!has(&out, "name: \"helper\""), "{out}");
        assert!(
            has(&out, "impl ::keel::runtime::KeelObject for Calc"),
            "{out}"
        );
        assert!(has(&out, "fn __keel_dispatch_Calc"), "{out}");
        assert!(
            has(&out, "::keel::meta::ids::method_id(\"Calc\", \"add\")"),
            "{out}"
        );
    }

    #[test]
    fn receivers_are_checked() {
        assert!(impl_error("impl C { pub fn f(&mut self) {} }").contains("error[keel::E0020]"));
        assert!(impl_error("impl C { pub fn f(self) {} }").contains("error[keel::E0021]"));
        assert!(impl_error("impl C { pub fn f(mut self) {} }").contains("error[keel::E0021]"));
        assert!(impl_error("impl C { pub fn f(self: Self) {} }").contains("error[keel::E0021]"));
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
                message.contains("error[keel::E0007]: the receiver `self: "),
                "{receiver}: {message}"
            );
            assert!(message.contains("write `&self`"), "{receiver}: {message}");
        }
        assert!(
            impl_error("impl C { pub fn f(self: &mut Self) {} }").contains("error[keel::E0020]")
        );
        assert!(impl_error("impl C { pub fn f<'a>(&'a self) {} }").contains("error[keel::E0003]"));
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
            message.contains("error[keel::E0007]: `helper` is neither a method nor a constructor")
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
                "TypeRefMeta::Result(&::keel::meta::TypeRefMeta::Named(\"C\"), &::keel::meta::TypeRefMeta::Named(\"CError\"))"
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
                .contains("error[keel::E0011]: store `S` has no constructor")
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
                .matches("__keel_cell:::core::default::Default::default()")
                .count(),
            2,
            "{out}"
        );
        assert!(has(&out, "<S>::__KEEL_STORE_META"), "{out}");
        assert!(has(&out, "__value.__keel_attach_all()"), "{out}");
        // `impl StoreObject` lives next to `impl KeelObject`: a store without an impl block
        // then gets only the branded E0011.
        assert!(
            has(&out, "impl ::keel::runtime::StoreObject for S"),
            "{out}"
        );
    }

    #[test]
    fn non_store_impls_are_left_alone() {
        let out = impl_result("impl S { pub fn new() -> Self { Self { x: 1 } } }", false).unwrap();
        assert!(
            !has(&out, "__keel_cell:"),
            "no cell field is added to literals: {out}"
        );
        assert!(!has(&out, "impl ::keel::runtime::StoreObject"), "{out}");
        assert!(!has(&out, "__value.__keel_attach_all()"), "{out}");
        assert!(has(&out, "store: ::core::option::Option::None"), "{out}");
    }

    #[test]
    fn async_methods_get_a_send_check() {
        let out = impl_result("impl C { pub async fn f(&self) -> u8 { 1 } }", false).unwrap();
        assert!(has(&out, "__keel_assert_send"), "{out}");
        assert!(has(&out, "__keel_assert_send(&__fut)"), "{out}");
        assert!(has(&out, "is_async: true"), "{out}");
        let out = impl_result("impl C { pub fn f(&self) -> u8 { 1 } }", false).unwrap();
        assert!(!has(&out, "__keel_assert_send"), "{out}");
    }

    #[test]
    fn stream_methods_bring_their_adapters() {
        let out = impl_result(
            "impl C { pub fn f(&self) -> impl Stream<Item = u8> { s() } }",
            false,
        )
        .unwrap();
        assert!(has(&out, "struct __KeelMap"), "{out}");
        assert!(!has(&out, "enum __KeelOpening"), "{out}");
        let out = impl_result(
            "impl C { pub async fn f(&self) -> Result<impl Stream<Item = u8>, E> { s() } }",
            false,
        )
        .unwrap();
        assert!(has(&out, "enum __KeelOpening"), "{out}");
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
        assert!(has(&out, "fn __keel_dispatch_fn_greet"), "{out}");
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
