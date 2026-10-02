//! `#[undra::port]`: traits implemented by the platform (foreign) or by Rust fakes.
//!
//! # The trait
//!
//! The trait is kept as written with two changes, both needed for `dyn Trait` (the port table
//! hands out `Arc<dyn Trait>`):
//!
//! * `Send + Sync` supertraits are added if missing;
//! * every `async fn` becomes a method returning a boxed future,
//!   `fn m(&self, ..) -> Pin<Box<dyn Future<Output = R> + Send + '_>>`, because `async fn` in
//!   traits is not dyn compatible. To keep implementations pleasant to write,
//!   `#[undra::port]` also works on `impl Trait for Type { .. }` blocks, where it rewrites
//!   `async fn` bodies into the boxed form.
//!
//! # Generated for sync and async ports
//!
//! * `impl Port for dyn Trait` (`PORT_ID`, `NAME`, `KIND`);
//! * `pub struct <Trait>Proxy(Ctx)` implementing the trait by encoding the arguments and calling
//!   `ctx.port_call(..).await` / `ctx.port_call_sync(..)`. A port is allowed to be unavailable
//!   (SPEC 6.3: a port nobody registered answers "unavailable"), so the proxy never panics where
//!   the method has an error channel: for a method returning `Result<T, E>`, `PortError::Failed`
//!   carries the encoded `E` and every other outcome (unavailable, cancelled, a reply that does
//!   not decode) becomes `E::from(PortError)`, which `E: From<PortError>` provides (E0033 when
//!   it does not). A method without an error channel has no typed way to say "unavailable": it
//!   panics with a message that names the port and method and says how to bind one (E0062, a
//!   runtime message; the runtime contains the panic at the dispatch boundary, and on wasm it
//!   traps the core). The generated locals are prefixed `__undra_`, so no parameter name can
//!   collide with them;
//! * `pub fn <trait_snake>(ctx: &Ctx) -> Arc<dyn Trait>`: the Rust binding if one is bound
//!   (fakes, built-ins), else the proxy;
//! * `pub fn __undra_port_dispatch_<Trait>(imp: &Arc<dyn Trait>, method_id, args) -> PortDispatch`
//!   for Rust-side bindings called with encoded arguments, and its registration as a
//!   `PortDispatcher`. With the hidden flag `dispatcher_by_use` (the standard ports of
//!   `undra-ports`, which every core links) the dispatcher is not registered: it is
//!   `pub static <TRAIT_SNAKE>_DISPATCHER: PortDispatcher`, which a binding passes to
//!   `Runtime::bind_dyn_port_with`, so a core that binds no Rust implementation does not link it
//!   (ADR-052);
//! * `PortMeta` and its registration.
//!
//! # Generated for event ports (`#[undra::port(event)]`)
//!
//! Events flow host to core. Instead of a proxy, each method gets
//! `pub fn on_<trait_snake>_<method>(ctx, f) -> Subscription` (decodes the payload and calls
//! `f`) and `pub fn encode_<trait_snake>_<method>_event(..) -> Vec<u8>` (what a fake or a test
//! feeds to `Runtime::event`).

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{FnArg, ItemImpl, ItemTrait, Pat, ReturnType, TraitItem, Type};

use super::attrs::{Site, docs, flag, parse_args, root_arg, take};
use super::check::{Checks, on_unimplemented};
use super::common::{GenericOn, check_generics, derived, item_root, param_meta, submit};
use super::diag::{Diag, Errors, code};
use super::naming::{fnv1a32, snake_case, unraw};
use super::object::arg_local;
use super::paths::Root;
use super::types::{Allow, KType, Pos, map_return_at, map_type, ty_string};

/// What kind of port the attribute arguments ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Requested {
    /// No argument: the kind follows from the methods.
    Inferred,
    /// `sync`: every method must be synchronous.
    Sync,
    /// `event`: host to core, fire and forget.
    Event,
    /// `#[undra::callback]`: a host-implemented interface with many instances (ADR-041).
    Callback,
}

struct PortParam {
    ident: syn::Ident,
    name: String,
    ty: Type,
    kty: KType,
}

struct PortMethod {
    ident: syn::Ident,
    name: String,
    is_async: bool,
    /// `#[undra(coalesce)]` (callback interfaces only).
    coalesce: bool,
    params: Vec<PortParam>,
    ret: KType,
    docs: String,
    /// The signature as it appears in the (rewritten) trait.
    sig: syn::Signature,
}

fn shape_error(what: String, node: &impl quote::ToTokens, why: &str, help: &str) -> syn::Error {
    Diag::new(code::E0032, what, why, help).on(node)
}

/// `Pin<Box<dyn Future<Output = ret> + Send + '_>>`.
fn boxed_future(ret: &ReturnType) -> Type {
    let output: Type = match ret {
        ReturnType::Default => syn::parse_quote!(()),
        ReturnType::Type(_, ty) => (**ty).clone(),
    };
    syn::parse_quote! {
        ::core::pin::Pin<::std::boxed::Box<
            dyn ::core::future::Future<Output = #output> + ::core::marker::Send + '_
        >>
    }
}

/// Rewrites `async fn m(..) -> R` into `fn m(..) -> Pin<Box<dyn Future<Output = R> + Send + '_>>`.
/// A default body becomes `Box::pin(async move { .. })`.
fn desugar_async(sig: &mut syn::Signature, body: Option<&mut syn::Block>) {
    if sig.asyncness.take().is_none() {
        return;
    }
    let ty = boxed_future(&sig.output);
    sig.output = ReturnType::Type(syn::token::RArrow::default(), Box::new(ty));
    if let Some(block) = body {
        let original = block.clone();
        *block = syn::parse_quote!({ ::std::boxed::Box::pin(async move #original) });
    }
}

fn port_param_error(err: super::types::TyErr, method: &str, param: &str) -> syn::Error {
    let underlying = err.diag.clone();
    err.with_diag(Diag::new(
        code::E0030,
        format!("parameter `{param}` of port method `{method}` is not a wire type"),
        format!("{} ({})", underlying.what, underlying.why),
        underlying.help,
    ))
    .into_error()
}

/// E0007 for an Undra attribute macro on a method of a port trait.
///
/// The trait's macro expands first and sees the method's attributes unexpanded, so a query,
/// mutation or function placed in the trait can be told where it belongs in Undra's words (the
/// same placement is reported for an `#[undra::api] impl` block in `object.rs`).
fn reject_undra_macros(
    attrs: &[syn::Attribute],
    accessor: &str,
    method: &str,
    errors: &mut Errors,
) {
    for attr in attrs {
        let Some(macro_name) = super::object::undra_macro_name(attr) else {
            continue;
        };
        let help = match macro_name.as_str() {
            "query" | "mutation" => format!(
                "remove the attribute from `{method}`; a `#[undra::{macro_name}]` is a free `async fn(ctx: &Ctx, ..)` that calls the port as `{accessor}(ctx).{method}(..)`, so write it outside the trait"
            ),
            "api" => format!(
                "remove the attribute from `{method}`; a function the core exposes that calls the port is a free `#[undra::api] fn(ctx: &Ctx, ..)` outside the trait, calling `{accessor}(ctx).{method}(..)`"
            ),
            _ => format!("remove the attribute from `{method}`, or move the item out of the trait"),
        };
        errors.push(
            Diag::new(
                code::E0007,
                format!("`#[undra::{macro_name}]` on the method `{method}` of a port trait"),
                "a port is a list of methods the platform implements; the attribute applies to a whole item (a type or a free function), never to a trait method",
                help,
            )
            .on(attr),
        );
    }
}

/// Analyses the methods of a port trait, normalising parameter names.
fn analyze_method(
    accessor: &str,
    method: &mut syn::TraitItemFn,
    requested: Requested,
    checks: &mut Checks,
    errors: &mut Errors,
) -> Option<PortMethod> {
    let attrs = take(
        &mut method.attrs,
        if requested == Requested::Callback {
            Site::CALLBACK_METHOD
        } else {
            Site::NOTHING
        },
        errors,
    );
    let name = unraw(&method.sig.ident);
    reject_undra_macros(&method.attrs, accessor, &name, errors);
    check_generics(&method.sig.generics, &name, GenericOn::PortMethod, errors);
    let docs = docs(&method.attrs);

    match method.sig.receiver() {
        Some(receiver) => {
            if receiver.colon_token.is_none() {
                match (&receiver.reference, receiver.mutability.is_some()) {
                    (None, _) => errors.push(
                        Diag::new(
                            code::E0021,
                            format!("`self` by value on port method `{name}`"),
                            "a port is shared as `Arc<dyn Trait>`; a method cannot consume it",
                            "take `&self`",
                        )
                        .on(receiver),
                    ),
                    (Some(_), true) => errors.push(
                        Diag::new(
                            code::E0020,
                            format!("`&mut self` receiver on port method `{name}`"),
                            "a port is shared as `Arc<dyn Trait>` across threads, so its methods only get `&self`",
                            "take `&self`; implementations use interior mutability",
                        )
                        .on(receiver),
                    ),
                    _ => {}
                }
            } else {
                errors.push(shape_error(
                    format!("port method `{name}` has a typed receiver"),
                    receiver,
                    "the proxy and the dispatcher call port methods on `&dyn Trait`",
                    "write `&self`",
                ));
            }
        }
        None => errors.push(shape_error(
            format!("port method `{name}` has no `&self` receiver"),
            &method.sig.ident,
            "a port method is called on a bound implementation",
            "add `&self`",
        )),
    }

    let mut params = Vec::new();
    let mut ok = true;
    for arg in method.sig.inputs.iter_mut() {
        let FnArg::Typed(pat_type) = arg else {
            continue;
        };
        take(&mut pat_type.attrs, Site::NOTHING, errors);
        let ident = match &*pat_type.pat {
            Pat::Ident(pat) if pat.by_ref.is_none() && pat.subpat.is_none() => pat.ident.clone(),
            other => {
                errors.push(shape_error(
                    format!("parameter `{}` of port method `{name}` is not a plain name", ty_string(other)),
                    other,
                    "the schema names every parameter, and the generated bindings label their arguments with those names",
                    "give the parameter a name: `key: String`",
                ));
                ok = false;
                continue;
            }
        };
        let param_name = unraw(&ident);
        let kty = match map_type(&pat_type.ty, Pos::PortParam, Allow::NONE) {
            Ok(kty) => kty,
            Err(err) => {
                errors.push(port_param_error(err, &name, &param_name));
                ok = false;
                KType::Unit
            }
        };
        params.push(PortParam {
            ident,
            name: param_name,
            ty: (*pat_type.ty).clone(),
            kty,
        });
    }

    let is_async = method.sig.asyncness.is_some();
    let ret = match map_return_at(&method.sig.output, Pos::PortReturn) {
        Ok(ret) => ret,
        Err(err) => {
            errors.push(err.into_error());
            ok = false;
            KType::Unit
        }
    };
    for p in &params {
        checks.ty(&p.ty, &p.kty);
    }
    checks.ret(&method.sig.output, &ret);
    if matches!(ret, KType::Stream(_))
        || matches!(&ret, KType::Result(ok_ty, _) if matches!(**ok_ty, KType::Stream(_)))
    {
        errors.push(shape_error(
            format!("port method `{name}` returns a stream"),
            &method.sig.output,
            "port calls are request/reply; streaming across a port is not supported in v1",
            "return a `Vec<T>` page, or make the platform push events through an event port",
        ));
    }

    if requested == Requested::Event {
        if is_async {
            errors.push(
                Diag::new(
                    code::E0031,
                    format!("event port method `{name}` is `async`"),
                    "events are fire-and-forget notifications from the host; nothing waits for them",
                    "make it a plain `fn` returning `()`",
                )
                .on(&method.sig.asyncness),
            );
        }
        if !ret.is_unit() {
            errors.push(
                Diag::new(
                    code::E0031,
                    format!("event port method `{name}` returns a value"),
                    "events are fire-and-forget notifications from the host; there is no reply to carry a value",
                    "remove the return type, or drop `event` from `#[undra::port(event)]` to make it a request/reply port",
                )
                .on(&method.sig.output),
            );
        }
    } else if requested == Requested::Sync && is_async {
        errors.push(
            Diag::new(
                code::E0032,
                format!("`#[undra::port(sync)]` port has the `async` method `{name}`"),
                "`sync` promises that every method answers immediately, so the core can call it without yielding",
                "remove `async`, or remove `sync` from the attribute",
            )
            .on(&method.sig.asyncness),
        );
    }

    if requested == Requested::Callback {
        callback_shape(&name, is_async, &ret, attrs.coalesce, method, errors);
    }

    ok.then(|| PortMethod {
        ident: method.sig.ident.clone(),
        name,
        is_async,
        coalesce: attrs.coalesce,
        params,
        ret,
        docs,
        sig: method.sig.clone(),
    })
}

/// E0071: a method of a callback interface either reports (a plain `fn` returning nothing) or is
/// `async` and returns a `Result`; its name must not start with `__` (reserved for `__release`
/// and `__cancel`); `#[undra(coalesce)]` is for reporting methods.
fn callback_shape(
    name: &str,
    is_async: bool,
    ret: &KType,
    coalesce: bool,
    method: &syn::TraitItemFn,
    errors: &mut Errors,
) {
    let why = "the host runs a callback outside the core's thread and lock, so the core can never wait for it synchronously, and a host implementation can always fail or be gone";
    if name.starts_with("__") {
        errors.push(
            Diag::new(
                code::E0071,
                format!("callback method `{name}` has a reserved name"),
                "names that start with `__` belong to the protocol (`__release` gives an instance back, `__cancel` cancels a call): a method of that name would be mistaken for one",
                "rename the method",
            )
            .on(&method.sig.ident),
        );
    }
    match (is_async, ret) {
        (false, KType::Unit) => {}
        (true, KType::Result(..)) => {
            if coalesce {
                errors.push(
                    Diag::new(
                        code::E0071,
                        format!("`#[undra(coalesce)]` on the `async` callback method `{name}`"),
                        "`coalesce` drops the older of two pending calls, and an `async` method's caller is waiting for each answer",
                        "remove `#[undra(coalesce)]`, or make the method a plain `fn` that reports progress",
                    )
                    .on(&method.sig.ident),
                );
            }
        }
        (false, _) => errors.push(
            Diag::new(
                code::E0071,
                format!("callback method `{name}` returns a value synchronously"),
                why,
                "make it `async` and return `Result<T, E>`, or make it a plain `fn` that returns nothing",
            )
            .on(&method.sig.output),
        ),
        (true, KType::Unit) => errors.push(
            Diag::new(
                code::E0071,
                format!("callback method `{name}` is `async` and returns nothing"),
                why,
                "return `Result<T, E>` (an `#[undra::error]` enum that implements `From<PortError>`), or remove `async` to make it a fire-and-forget report",
            )
            .on(&method.sig.asyncness),
        ),
        (true, _) => errors.push(
            Diag::new(
                code::E0071,
                format!("callback method `{name}` is `async` but does not return a `Result`"),
                why,
                "return `Result<T, E>` so the method can report that the host failed or is gone",
            )
            .on(&method.sig.output),
        ),
    }
}

/// Expands `#[undra::port]` on a trait.
pub(crate) fn expand_trait(
    args_root: Option<Root>,
    requested: Requested,
    dispatcher_by_use: bool,
    background: bool,
    mut item: ItemTrait,
) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    let root = item_root(&mut item.attrs, args_root, &mut errors);
    let name = item.ident.clone();
    let name_str = unraw(&name);
    let generic_on = if requested == Requested::Callback {
        GenericOn::Callback
    } else {
        GenericOn::Port
    };
    check_generics(&item.generics, &name_str, generic_on, &mut errors);
    let type_docs = docs(&item.attrs);
    let vis = item.vis.clone();

    let accessor = snake_case(&name_str);
    let mut methods: Vec<PortMethod> = Vec::new();
    let mut checks = Checks::new();
    for trait_item in &mut item.items {
        match trait_item {
            TraitItem::Fn(method) => {
                if let Some(model) =
                    analyze_method(&accessor, method, requested, &mut checks, &mut errors)
                {
                    methods.push(model);
                }
            }
            other => errors.push(shape_error(
                format!("port trait `{name_str}` has an associated item that is not a method"),
                other,
                "a port is a list of methods the platform implements; associated types and constants have no wire representation",
                "remove it, or move it to a separate trait",
            )),
        }
    }
    if methods.is_empty() && errors.is_empty() {
        errors.push(shape_error(
            format!("port trait `{name_str}` has no methods"),
            &name,
            "a port without methods can never be called",
            "add at least one method",
        ));
    }
    let mut ids: Vec<(u32, &str)> = Vec::new();
    for m in &methods {
        let id = fnv1a32(&format!("{name_str}.{}", m.name));
        if let Some((_, other)) = ids.iter().find(|(existing, _)| *existing == id) {
            errors.push(shape_error(
                format!("method ids of `{other}` and `{}` collide", m.name),
                &m.ident,
                "ids are 32-bit hashes of `Trait.method`, and two of them are equal",
                "rename one of the methods",
            ));
        }
        ids.push((id, &m.name));
    }
    errors.finish()?;

    // Rewrite the trait: supertraits and boxed futures.
    let has_bound = |item: &ItemTrait, bound: &str| {
        item.supertraits.iter().any(|b| {
            matches!(b, syn::TypeParamBound::Trait(t)
                if t.path.segments.last().is_some_and(|seg| seg.ident == bound))
        })
    };
    if !has_bound(&item, "Send") {
        item.supertraits
            .push(syn::parse_quote!(::core::marker::Send));
    }
    if !has_bound(&item, "Sync") {
        item.supertraits
            .push(syn::parse_quote!(::core::marker::Sync));
    }
    let mut boxed_sigs: Vec<syn::Signature> = Vec::new();
    for trait_item in &mut item.items {
        if let TraitItem::Fn(method) = trait_item {
            desugar_async(&mut method.sig, method.default.as_mut());
            boxed_sigs.push(method.sig.clone());
        }
    }
    for (method, sig) in methods.iter_mut().zip(boxed_sigs) {
        method.sig = sig;
    }

    let kind = match requested {
        Requested::Event => "Event",
        Requested::Callback => "Callback",
        _ if methods.iter().any(|m| m.is_async) => "Async",
        _ => "Sync",
    };

    let meta = root.meta();
    let runtime = root.runtime();
    let snake = snake_case(&name_str);
    let derived = derived();
    let kind_ident = syn::Ident::new(kind, Span::call_site());
    let meta_static = format_ident!("__UNDRA_META_port_{}", name_str);

    let method_metas = methods.iter().map(|m| {
        let mname = &m.name;
        let params = m.params.iter().map(|p| param_meta(&meta, &p.name, &p.kty));
        let returns = m.ret.meta(&meta);
        let is_async = m.is_async;
        let coalesce = m.coalesce;
        let mdocs = &m.docs;
        quote! {
            #meta::MethodMeta {
                name: #mname,
                method_id: #meta::ids::port_method_id(#name_str, #mname),
                params: &[ #(#params),* ],
                returns: #returns,
                is_async: #is_async,
                takes_ctx: false,
                coalesce: #coalesce,
                generic: ::core::option::Option::None,
                docs: #mdocs,
            }
        }
    });

    let port_impl = quote! {
        #derived
        impl #runtime::Port for dyn #name {
            const PORT_ID: u32 = #meta::ids::port_id(#name_str);
            const NAME: &'static str = #name_str;
            const KIND: #meta::PortKind = #meta::PortKind::#kind_ident;
        }
    };

    let registration = submit(&root, "Port", &meta_static);
    let meta_item = quote! {
        #[allow(non_upper_case_globals)]
        static #meta_static: #meta::PortMeta = #meta::PortMeta {
            name: #name_str,
            port_id: #meta::ids::port_id(#name_str),
            kind: #meta::PortKind::#kind_ident,
            background: #background,
            methods: &[ #(#method_metas),* ],
            docs: #type_docs,
        };
        #registration
    };

    let extras = if requested == Requested::Event {
        event_helpers(&root, &vis, &name, &name_str, &snake, &methods)
    } else if requested == Requested::Callback {
        callback_helpers(&root, &vis, &name, &name_str, &methods)
    } else {
        call_helpers(
            &root,
            &vis,
            &name,
            &name_str,
            &snake,
            &methods,
            dispatcher_by_use,
        )
    };
    let checks = checks.emit(&root);
    Ok(quote! {
        #item
        #port_impl
        #meta_item
        #extras
        #checks
    })
}

/// The tokens that turn the reply of a port call (`__undra_reply`, a
/// `Result<Vec<u8>, PortError>`) into the value of method `m`: the typed `Result` of a method
/// with an error channel, the decoded value, or a panic that names how to fix a missing adapter
/// (E0062).
fn reply_tokens(
    m: &PortMethod,
    runtime: &TokenStream,
    wire: &TokenStream,
    port_error_trait: &syn::Ident,
    failure_fn: &syn::Ident,
) -> TokenStream {
    let mname = &m.name;
    // What a method with no error channel does with an outcome it cannot return: a
    // contained panic whose message says how to fix it (E0062).
    let failed = quote!(#failure_fn(#mname, __undra_error));
    let undecodable = |ty: &Type| {
        quote_spanned! {ty.span()=>
            match <#ty as #wire::Decode>::decode_exact(&__undra_bytes) {
                ::core::result::Result::Ok(__undra_value) => __undra_value,
                ::core::result::Result::Err(__undra_error) => #failure_fn(
                    #mname,
                    #runtime::PortError::Decode(__undra_error),
                ),
            }
        }
    };
    let (ok_ty, err_ty) = result_types(&m.sig.output, m.is_async);
    match (&m.ret, ok_ty, err_ty) {
        // A method with an error channel turns every outcome into a value: the encoded `E`
        // the adapter reported, or `E::from(PortError)` for an unavailable port, a
        // cancelled call or a reply that does not decode.
        (KType::Result(ok, _), ok_ty, Some(err_ty)) => {
            let to_err = quote_spanned! {err_ty.span()=>
                fn __undra_port_error(__undra_e: #runtime::PortError) -> #err_ty {
                    <#err_ty as #port_error_trait>::__undra_from_port_error(__undra_e)
                }
            };
            let ok_arm = if ok.is_unit() {
                quote!(::core::result::Result::Ok(_) => ::core::result::Result::Ok(()),)
            } else {
                let ok_ty = ok_ty.expect("result has an ok type");
                quote_spanned! {ok_ty.span()=>
                    ::core::result::Result::Ok(__undra_bytes) => {
                        match <#ok_ty as #wire::Decode>::decode_exact(&__undra_bytes) {
                            ::core::result::Result::Ok(__undra_value) => {
                                ::core::result::Result::Ok(__undra_value)
                            }
                            ::core::result::Result::Err(__undra_error) => {
                                ::core::result::Result::Err(__undra_port_error(
                                    #runtime::PortError::Decode(__undra_error),
                                ))
                            }
                        }
                    }
                }
            };
            let failed_arm = quote_spanned! {err_ty.span()=>
                ::core::result::Result::Err(#runtime::PortError::Failed(__undra_bytes)) => {
                    match <#err_ty as #wire::Decode>::decode_exact(&__undra_bytes) {
                        ::core::result::Result::Ok(__undra_value) => {
                            ::core::result::Result::Err(__undra_value)
                        }
                        ::core::result::Result::Err(__undra_error) => {
                            ::core::result::Result::Err(__undra_port_error(
                                #runtime::PortError::Decode(__undra_error),
                            ))
                        }
                    }
                }
            };
            quote! {
                #to_err
                match __undra_reply {
                    #ok_arm
                    #failed_arm
                    ::core::result::Result::Err(__undra_other) => {
                        ::core::result::Result::Err(__undra_port_error(__undra_other))
                    }
                }
            }
        }
        (ret, ok_ty, _) if ret.is_unit() => {
            let _ = ok_ty;
            quote! {
                match __undra_reply {
                    ::core::result::Result::Ok(_) => (),
                    ::core::result::Result::Err(__undra_error) => #failed,
                }
            }
        }
        (_, ok_ty, _) => {
            let decode = undecodable(&ok_ty.expect("plain return has a type"));
            quote! {
                match __undra_reply {
                    ::core::result::Result::Ok(__undra_bytes) => #decode,
                    ::core::result::Result::Err(__undra_error) => #failed,
                }
            }
        }
    }
}

/// The proxy, the accessor and the Rust-side dispatcher of a request/reply port.
fn call_helpers(
    root: &Root,
    vis: &syn::Visibility,
    name: &syn::Ident,
    name_str: &str,
    snake: &str,
    methods: &[PortMethod],
    dispatcher_by_use: bool,
) -> TokenStream {
    let meta = root.meta();
    let runtime = root.runtime();
    let wire = root.wire();
    let derived = derived();
    let proxy = format_ident!("{}Proxy", name_str);
    let accessor = ident_or_raw(snake);
    let dispatch_fn = format_ident!("__undra_port_dispatch_{}", name_str);
    let erased_fn = format_ident!("__undra_port_dispatch_erased_{}", name_str);
    let proxy_doc = format!(
        "Calls the `{name_str}` port through the runtime's port table: the platform's binding, or a Rust fake."
    );
    let accessor_doc = format!(
        "The Rust binding of the `{name_str}` port if one is bound (fakes, built-ins), otherwise a proxy to the platform's binding."
    );
    let dispatch_doc = format!(
        "Runs an encoded `{name_str}` call on a Rust implementation. The reply is `status u8` (0 ok, 1 typed error, 2 unavailable) followed by the body."
    );

    // --- proxy methods ---------------------------------------------------------------------
    let port_error_trait = format_ident!("__UndraPortError_{}", name_str);
    let failure_fn = format_ident!("__undra_port_failure_{}", name_str);
    let needs_error_trait = methods.iter().any(|m| matches!(m.ret, KType::Result(..)));
    let proxy_methods = methods.iter().map(|m| {
        let sig = &m.sig;
        let mname = &m.name;
        let port_id = quote!(#meta::ids::port_id(#name_str));
        let method_id = quote!(#meta::ids::port_method_id(#name_str, #mname));
        let encodes = m.params.iter().map(|p| {
            let ident = &p.ident;
            quote_spanned!(p.ty.span()=> #wire::Encode::encode(&#ident, &mut __undra_w);)
        });
        let reply = reply_tokens(m, &runtime, &wire, &port_error_trait, &failure_fn);
        let body = if m.is_async {
            quote! {
                let __undra_ctx = ::core::clone::Clone::clone(&self.0);
                ::std::boxed::Box::pin(async move {
                    let __undra_reply = __undra_ctx
                        .port_call(#port_id, #method_id, __undra_args)
                        .await;
                    #reply
                })
            }
        } else {
            quote! {
                let __undra_reply = self.0.port_call_sync(#port_id, #method_id, &__undra_args);
                #reply
            }
        };
        quote! {
            #sig {
                let __undra_args = {
                    let mut __undra_w = #wire::Writer::new();
                    #(#encodes)*
                    __undra_w.into_vec()
                };
                #body
            }
        }
    });
    let proxy_methods: Vec<TokenStream> = proxy_methods.collect();

    // The error channel's bound, with the branded message (E0033), and the panic of a method
    // that has none (E0062).
    let error_trait_attr = on_unimplemented(
        &Diag::new(
            code::E0033,
            format!(
                "the error type `{{Self}}` of a method of the `{name_str}` port cannot represent a port that is unavailable"
            ),
            "a port nobody registered, a cancelled call and a reply that does not decode are ordinary outcomes (SPEC 6.3), and a method that returns `Result<T, E>` reports them as its error instead of panicking; that needs `From<PortError>` for the error type",
            "implement `From<undra::runtime::PortError>` for `{Self}`, mapping it to a variant such as `Unavailable`, or to the `Display` text of the `PortError`",
        ),
        "`From<PortError>` is not implemented for this error type",
    );
    let error_trait = if needs_error_trait {
        quote! {
            #[doc(hidden)]
            #[allow(non_camel_case_types, dead_code)]
            #error_trait_attr
            trait #port_error_trait: ::core::marker::Sized {
                fn __undra_from_port_error(__undra_e: #runtime::PortError) -> Self;
            }
            impl<__UndraE: ::core::convert::From<#runtime::PortError>> #port_error_trait for __UndraE {
                fn __undra_from_port_error(__undra_e: #runtime::PortError) -> Self {
                    <__UndraE as ::core::convert::From<#runtime::PortError>>::from(__undra_e)
                }
            }
        }
    } else {
        TokenStream::new()
    };
    // The message has the shape of every other diagnostic (`Diag::runtime_template`), finished
    // with the port and method of the call: a runtime error reads exactly like a compile error.
    let failure_template = Diag::runtime_template(code::E0062);
    let failure = quote! {
        #[doc(hidden)]
        #[cold]
        #[inline(never)]
        #[allow(non_snake_case, dead_code)]
        fn #failure_fn(__undra_method: &str, __undra_error: #runtime::PortError) -> ! {
            let (__undra_what, __undra_why, __undra_how) = match &__undra_error {
                #runtime::PortError::Unavailable => (
                    ::std::format!(
                        "the `{}` port has no adapter registered (method `{}`)",
                        #name_str,
                        __undra_method,
                    ),
                    "this method has no error channel, so an unavailable port cannot be reported and the call panics; the runtime contains the panic, but on the web it traps the core",
                    "register an adapter (`core.registerPort(..)` in TypeScript, Kotlin and Swift, `undra_port_register` in C), bind a Rust implementation (`undra::ports::fakes` in tests), or give the method a `Result<T, E>` return type so it can report the outage",
                ),
                #runtime::PortError::Cancelled => (
                    ::std::format!(
                        "a call to the `{}` port (method `{}`) was cancelled",
                        #name_str,
                        __undra_method,
                    ),
                    "this method has no error channel, so an abandoned call cannot be reported and the call panics; on the web that traps the core",
                    "give the method a `Result<T, E>` return type so it can report a cancelled call",
                ),
                #runtime::PortError::Decode(__undra_why) => (
                    ::std::format!(
                        "the `{}` port (method `{}`) replied with bytes that do not decode: {}",
                        #name_str,
                        __undra_method,
                        __undra_why,
                    ),
                    "the adapter's reply does not match the schema, and this method has no error channel to report that, so the call panics; on the web that traps the core",
                    "check the adapter's codec for this method against the schema, or give the method a `Result<T, E>` return type so it can report a bad reply",
                ),
                __undra_other => (
                    ::std::format!(
                        "a call to the `{}` port (method `{}`) failed: {}",
                        #name_str,
                        __undra_method,
                        __undra_other,
                    ),
                    "this method has no error channel, so a failed call cannot be reported and the call panics; on the web that traps the core",
                    "give the method a `Result<T, E>` return type so it can report the failure",
                ),
            };
            ::core::panic!(#failure_template, __undra_what, __undra_why, __undra_how)
        }
    };

    // --- Rust-side dispatcher ---------------------------------------------------------------
    let unavailable = quote!(#runtime::PortDispatch::Sync(::std::vec![2u8]));
    let dispatch_consts = methods.iter().map(|m| {
        let id = format_ident!("__UNDRA_ID_{}", m.name);
        let mname = &m.name;
        quote!(const #id: u32 = #meta::ids::port_method_id(#name_str, #mname);)
    });
    let reply_bytes = |status: u8, value: TokenStream| {
        quote! {{
            let mut __w = #wire::Writer::new();
            __w.write_u8(#status);
            #wire::Encode::encode(&#value, &mut __w);
            __w.into_vec()
        }}
    };
    let dispatch_arms = methods.iter().map(|m| {
        let id = format_ident!("__UNDRA_ID_{}", m.name);
        let ident = &m.ident;
        // Positional locals: no parameter name can collide with the generated ones.
        let decodes = m.params.iter().enumerate().map(|(index, p)| {
            let local = arg_local(index);
            let ty = &p.ty;
            quote_spanned! {p.ty.span()=>
                let #local: #ty = match <#ty as #wire::Decode>::decode(&mut __r) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(_) => return #unavailable,
                };
            }
        });
        let call_args = (0..m.params.len()).map(arg_local);
        let call = quote!(#ident( #(#call_args),* ));
        let (ok_bytes, err_bytes) = (reply_bytes(0, quote!(__v)), reply_bytes(1, quote!(__e)));
        let unit_bytes = quote!(::std::vec![0u8]);
        let outcome = match (&m.ret, m.is_async) {
            (KType::Result(..), false) => quote! {
                #runtime::PortDispatch::Sync(match __imp.#call {
                    ::core::result::Result::Ok(__v) => #ok_bytes,
                    ::core::result::Result::Err(__e) => #err_bytes,
                })
            },
            (KType::Result(..), true) => quote! {{
                let __imp = ::std::sync::Arc::clone(__imp);
                #runtime::PortDispatch::Async(::std::boxed::Box::pin(async move {
                    match __imp.#call.await {
                        ::core::result::Result::Ok(__v) => #ok_bytes,
                        ::core::result::Result::Err(__e) => #err_bytes,
                    }
                }))
            }},
            (ret, false) if ret.is_unit() => quote! {{
                __imp.#call;
                #runtime::PortDispatch::Sync(#unit_bytes)
            }},
            (ret, true) if ret.is_unit() => quote! {{
                let __imp = ::std::sync::Arc::clone(__imp);
                #runtime::PortDispatch::Async(::std::boxed::Box::pin(async move {
                    __imp.#call.await;
                    #unit_bytes
                }))
            }},
            (_, false) => quote! {{
                let __v = __imp.#call;
                #runtime::PortDispatch::Sync(#ok_bytes)
            }},
            (_, true) => quote! {{
                let __imp = ::std::sync::Arc::clone(__imp);
                #runtime::PortDispatch::Async(::std::boxed::Box::pin(async move {
                    let __v = __imp.#call.await;
                    #ok_bytes
                }))
            }},
        };
        quote! {
            #id => {
                let mut __r = #wire::Reader::new(__args);
                #(#decodes)*
                if __r.finish().is_err() {
                    return #unavailable;
                }
                #outcome
            }
        }
    });

    let dispatcher_value = quote! {
        #runtime::PortDispatcher {
            port_id: #meta::ids::port_id(#name_str),
            dispatch: #erased_fn,
        }
    };
    let dispatcher = if dispatcher_by_use {
        let name = format_ident!("{}_DISPATCHER", snake.to_uppercase());
        let doc = format!(
            "The Rust-side dispatcher of the `{name_str}` port, for `Runtime::bind_dyn_port_with`: a raw port call (the generated proxy) on a Rust implementation bound with it runs here. Not registered, so a core that binds no Rust implementation of the port does not link it."
        );
        quote! {
            #[doc = #doc]
            #vis static #name: #runtime::PortDispatcher = #dispatcher_value;
        }
    } else {
        quote! {
            #meta::inventory::submit! { #dispatcher_value }
        }
    };

    quote! {
        #error_trait
        #failure

        #[doc = #proxy_doc]
        #vis struct #proxy(#runtime::Ctx);

        impl #proxy {
            /// Creates a proxy that calls through `ctx`.
            #vis fn new(ctx: #runtime::Ctx) -> Self {
                Self(ctx)
            }
        }

        #derived
        impl #name for #proxy {
            #(#proxy_methods)*
        }

        #[doc = #accessor_doc]
        #vis fn #accessor(ctx: &#runtime::Ctx) -> ::std::sync::Arc<dyn #name> {
            match ctx.rust_port::<dyn #name>(<dyn #name as #runtime::Port>::PORT_ID) {
                ::core::option::Option::Some(__imp) => __imp,
                ::core::option::Option::None => ::std::sync::Arc::new(
                    #proxy::new(::core::clone::Clone::clone(ctx))
                ),
            }
        }

        #[doc = #dispatch_doc]
        #[allow(non_snake_case, non_upper_case_globals, unused_mut, unused_variables, clippy::all)]
        #vis fn #dispatch_fn(
            __imp: &::std::sync::Arc<dyn #name>,
            __method_id: u32,
            __args: &[u8],
        ) -> #runtime::PortDispatch {
            #(#dispatch_consts)*
            match __method_id {
                #(#dispatch_arms)*
                _ => #unavailable,
            }
        }

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #erased_fn(
            __imp: &(dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync),
            __method_id: u32,
            __args: &[u8],
        ) -> #runtime::PortDispatch {
            match __imp.downcast_ref::<::std::sync::Arc<dyn #name>>() {
                ::core::option::Option::Some(__imp) => #dispatch_fn(__imp, __method_id, __args),
                ::core::option::Option::None => #unavailable,
            }
        }
        #dispatcher
    }
}

/// `snake` as an identifier, raw (`r#match`) when it is a keyword. `self`, `super` and `crate`
/// cannot be raw identifiers; they get a trailing underscore.
fn ident_or_raw(snake: &str) -> syn::Ident {
    // `gen` is reserved from edition 2024 on, which `syn` does not know; raw is valid in all.
    if snake != "gen" && syn::parse_str::<syn::Ident>(snake).is_ok() {
        return syn::Ident::new(snake, Span::call_site());
    }
    if matches!(snake, "self" | "super" | "crate") {
        return syn::Ident::new(&format!("{snake}_"), Span::call_site());
    }
    syn::Ident::new_raw(snake, Span::call_site())
}

/// `(T)` and `Group` wrappers around a type are transparent.
fn strip_parens(mut ty: &Type) -> &Type {
    loop {
        match ty {
            Type::Paren(inner) => ty = &inner.elem,
            Type::Group(inner) => ty = &inner.elem,
            _ => return ty,
        }
    }
}

/// The types `T` and `E` of a method's declared return type. For an `async fn` rewritten to
/// a boxed future, the original output type is inside the `Future<Output = ..>` bound.
fn result_types(output: &ReturnType, is_async: bool) -> (Option<Type>, Option<Type>) {
    let ReturnType::Type(_, ty) = output else {
        return (None, None);
    };
    let mut ty: &Type = strip_parens(ty);
    if is_async {
        if let Some(inner) = boxed_future_output(ty) {
            ty = strip_parens(inner);
        }
    }
    if let Type::Path(path) = ty {
        if let Some(seg) = path.path.segments.last() {
            if seg.ident == "Result" {
                if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                    let mut types = args.args.iter().filter_map(|a| match a {
                        syn::GenericArgument::Type(t) => Some(t.clone()),
                        _ => None,
                    });
                    let ok = types.next();
                    let err = types.next();
                    return (ok, err);
                }
            }
        }
    }
    (Some(ty.clone()), None)
}

/// `Pin<Box<dyn Future<Output = X> + ..>>` -> `X`.
fn boxed_future_output(ty: &Type) -> Option<&Type> {
    let Type::Path(pin) = ty else {
        return None;
    };
    let syn::PathArguments::AngleBracketed(pin_args) = &pin.path.segments.last()?.arguments else {
        return None;
    };
    let syn::GenericArgument::Type(Type::Path(boxed)) = pin_args.args.first()? else {
        return None;
    };
    let syn::PathArguments::AngleBracketed(box_args) = &boxed.path.segments.last()?.arguments
    else {
        return None;
    };
    let syn::GenericArgument::Type(Type::TraitObject(object)) = box_args.args.first()? else {
        return None;
    };
    object.bounds.iter().find_map(|bound| {
        let syn::TypeParamBound::Trait(bound) = bound else {
            return None;
        };
        let syn::PathArguments::AngleBracketed(args) = &bound.path.segments.last()?.arguments
        else {
            return None;
        };
        args.args.iter().find_map(|arg| match arg {
            syn::GenericArgument::AssocType(assoc) if assoc.ident == "Output" => Some(&assoc.ty),
            _ => None,
        })
    })
}

/// The proxy of a callback interface (ADR-041): `<Trait>Proxy` over a
/// [`CallbackHandle`](undra_runtime::CallbackHandle), implementing the trait by calling the host
/// instance, and `impl CallbackInterface for dyn Trait`, which is how a dispatcher makes one from
/// an instance handle. A reporting method is a fire-and-forget call (`port_call_id 0`); an `async`
/// one is a port call whose future sends `__cancel` when it is dropped before the host answered.
fn callback_helpers(
    root: &Root,
    vis: &syn::Visibility,
    name: &syn::Ident,
    name_str: &str,
    methods: &[PortMethod],
) -> TokenStream {
    let meta = root.meta();
    let runtime = root.runtime();
    let wire = root.wire();
    let derived = derived();
    let proxy = format_ident!("{}Proxy", name_str);
    let proxy_doc = format!(
        "The core's handle on one host instance of the `{name_str}` callback interface: its methods call the host (ADR-041). Dropping it gives the instance's reference back."
    );
    let port_error_trait = format_ident!("__UndraPortError_{}", name_str);
    let failure_fn = format_ident!("__undra_port_failure_{}", name_str);
    let needs_error_trait = methods.iter().any(|m| matches!(m.ret, KType::Result(..)));

    let proxy_methods = methods.iter().map(|m| {
        let sig = &m.sig;
        let mname = &m.name;
        let method_id = quote!(#meta::ids::port_method_id(#name_str, #mname));
        let encodes = m.params.iter().map(|p| {
            let ident = &p.ident;
            quote_spanned!(p.ty.span()=> #wire::Encode::encode(&#ident, __undra_w);)
        });
        if m.is_async {
            let reply = reply_tokens(m, &runtime, &wire, &port_error_trait, &failure_fn);
            quote! {
                #sig {
                    // Sent now, in call order; dropping the future before it completes cancels.
                    let __undra_call = self.0.call(#method_id, |__undra_w| { #(#encodes)* });
                    ::std::boxed::Box::pin(async move {
                        let __undra_reply = __undra_call.await;
                        #reply
                    })
                }
            }
        } else {
            quote! {
                #sig {
                    self.0.notify(#method_id, |__undra_w| { #(#encodes)* });
                }
            }
        }
    });
    let proxy_methods: Vec<TokenStream> = proxy_methods.collect();

    let error_trait_attr = on_unimplemented(
        &Diag::new(
            code::E0033,
            format!(
                "the error type `{{Self}}` of an async method of the `{name_str}` callback interface cannot represent a host that is gone"
            ),
            "a host instance that was closed, a call that was cancelled and a reply that does not decode are ordinary outcomes, and an async callback method reports them as its error instead of panicking; that needs `From<PortError>` for the error type",
            "implement `From<undra::runtime::PortError>` for `{Self}`, mapping it to a variant such as `Unavailable`, or to the `Display` text of the `PortError`",
        ),
        "`From<PortError>` is not implemented for this error type",
    );
    let error_trait = if needs_error_trait {
        quote! {
            #[doc(hidden)]
            #[allow(non_camel_case_types, dead_code)]
            #error_trait_attr
            trait #port_error_trait: ::core::marker::Sized {
                fn __undra_from_port_error(__undra_e: #runtime::PortError) -> Self;
            }
            impl<__UndraE: ::core::convert::From<#runtime::PortError>> #port_error_trait for __UndraE {
                fn __undra_from_port_error(__undra_e: #runtime::PortError) -> Self {
                    <__UndraE as ::core::convert::From<#runtime::PortError>>::from(__undra_e)
                }
            }
        }
    } else {
        TokenStream::new()
    };
    // `reply_tokens` names the failure function of a method without an error channel; a callback
    // method never is one (E0071), so it is never called.
    let failure = quote! {
        #[doc(hidden)]
        #[allow(non_snake_case, dead_code)]
        fn #failure_fn(__undra_method: &str, __undra_error: #runtime::PortError) -> ! {
            ::core::panic!("callback `{}` method `{}`: {}", #name_str, __undra_method, __undra_error)
        }
    };

    quote! {
        #error_trait
        #failure

        #[doc = #proxy_doc]
        #vis struct #proxy(#runtime::CallbackHandle);

        impl #proxy {
            /// Wraps a host instance.
            #vis fn new(handle: #runtime::CallbackHandle) -> Self {
                Self(handle)
            }
        }

        #derived
        impl #name for #proxy {
            #(#proxy_methods)*
        }

        #derived
        impl #runtime::CallbackInterface for dyn #name {
            fn proxy(handle: #runtime::CallbackHandle) -> ::std::sync::Arc<dyn #name> {
                ::std::sync::Arc::new(#proxy::new(handle))
            }
        }
    }
}

/// The arguments of `#[undra::callback]`: `background` and `crate = "path"`.
pub(crate) fn parse_callback_args(attr: TokenStream) -> syn::Result<(Option<Root>, bool)> {
    let mut root = None;
    let mut background = false;
    parse_args(
        attr,
        "callback",
        "`background` and `crate = \"path\"`",
        |meta| {
            if meta.path.is_ident("crate") {
                root = Some(root_arg(meta)?);
                Ok(true)
            } else if meta.path.is_ident("background") {
                flag(meta, code::E0008, "background")?;
                background = true;
                Ok(true)
            } else {
                Ok(false)
            }
        },
    )?;
    Ok((root, background))
}

/// Subscription helpers and payload encoders of an event port.
fn event_helpers(
    root: &Root,
    vis: &syn::Visibility,
    name: &syn::Ident,
    name_str: &str,
    snake: &str,
    methods: &[PortMethod],
) -> TokenStream {
    let meta = root.meta();
    let runtime = root.runtime();
    let wire = root.wire();
    let helpers = methods.iter().map(|m| {
        let mname = &m.name;
        let on_fn = format_ident!("on_{}_{}", snake, mname);
        let encode_fn = format_ident!("encode_{}_{}_event", snake, mname);
        let on_doc = format!(
            "Calls `f` with the runtime's `Ctx` and the decoded arguments of every `{name_str}.{mname}` event. Use the `Ctx` it is given: a captured one would keep the runtime alive (ADR-034)."
        );
        let encode_doc = format!(
            "Encodes the payload of a `{name_str}.{mname}` event, as the host would send it to `Runtime::event`."
        );
        let tys: Vec<&Type> = m.params.iter().map(|p| &p.ty).collect();
        let idents: Vec<&syn::Ident> = m.params.iter().map(|p| &p.ident).collect();
        let locals: Vec<syn::Ident> = (0..m.params.len()).map(|i| format_ident!("__a{}", i)).collect();
        let decodes = tys.iter().zip(&locals).map(|(ty, local)| {
            quote_spanned! {ty.span()=>
                let #local: #ty = match <#ty as #wire::Decode>::decode(&mut __r) {
                    ::core::result::Result::Ok(__v) => __v,
                    // A malformed payload is dropped: the host is trusted to send what the
                    // schema says, and the runtime has already checked the schema hash.
                    ::core::result::Result::Err(_) => return,
                };
            }
        });
        let encodes = tys.iter().zip(&idents).map(|(ty, ident)| {
            quote_spanned!(ty.span()=> #wire::Encode::encode(&#ident, &mut __undra_w);)
        });
        let params = m.params.iter().map(|p| {
            let ident = &p.ident;
            let ty = &p.ty;
            quote!(#ident: #ty)
        });
        quote! {
            #[doc = #on_doc]
            #vis fn #on_fn(
                ctx: &#runtime::Ctx,
                f: impl ::core::ops::Fn( &#runtime::Ctx, #(#tys),* ) + ::core::marker::Send + ::core::marker::Sync + 'static,
            ) -> #runtime::Subscription {
                ctx.events().subscribe(
                    <dyn #name as #runtime::Port>::PORT_ID,
                    #meta::ids::port_method_id(#name_str, #mname),
                    ::std::boxed::Box::new(move |__ctx: &#runtime::Ctx, __payload: &[u8]| {
                        let mut __r = #wire::Reader::new(__payload);
                        #(#decodes)*
                        if __r.finish().is_err() {
                            return;
                        }
                        f( __ctx, #(#locals),* )
                    }),
                )
            }

            #[doc = #encode_doc]
            #vis fn #encode_fn( #(#params),* ) -> ::std::vec::Vec<u8> {
                let mut __undra_w = #wire::Writer::new();
                #(#encodes)*
                __undra_w.into_vec()
            }
        }
    });
    quote! { #(#helpers)* }
}

/// Expands `#[undra::port]` on `impl Trait for Type`: `async fn` becomes a boxed future.
pub(crate) fn expand_impl(mut item: ItemImpl) -> syn::Result<TokenStream> {
    if item.trait_.is_none() {
        return Err(Diag::new(
            code::E0007,
            "`#[undra::port]` on an inherent impl",
            "`#[undra::port]` applies to port trait definitions and to `impl Trait for Type` blocks that implement one",
            "apply it to the trait, or to a trait impl",
        )
        .on(&item.self_ty));
    }
    for impl_item in &mut item.items {
        if let syn::ImplItem::Fn(method) = impl_item {
            desugar_async(&mut method.sig, Some(&mut method.block));
        }
    }
    Ok(quote!(#item))
}

/// Parses the arguments of `#[undra::port(..)]`.
pub(crate) fn parse_port_args(attr: TokenStream) -> syn::Result<(Option<Root>, Requested, bool)> {
    let mut root = None;
    let mut requested = Requested::Inferred;
    let mut dispatcher_by_use = false;
    let mut conflict: Option<syn::Error> = None;
    parse_args(
        attr,
        "port",
        "`sync`, `event` and `crate = \"path\"`",
        |meta| {
            if meta.path.is_ident("crate") {
                root = Some(root_arg(meta)?);
                Ok(true)
            } else if meta.path.is_ident("dispatcher_by_use") {
                // Hidden: the standard ports of `undra-ports` (see the module docs).
                flag(meta, code::E0008, "dispatcher_by_use")?;
                dispatcher_by_use = true;
                Ok(true)
            } else if meta.path.is_ident("sync") || meta.path.is_ident("event") {
                flag(
                    meta,
                    code::E0008,
                    if meta.path.is_ident("sync") {
                        "sync"
                    } else {
                        "event"
                    },
                )?;
                let this = if meta.path.is_ident("sync") {
                    Requested::Sync
                } else {
                    Requested::Event
                };
                if requested != Requested::Inferred && requested != this {
                    conflict = Some(
                        Diag::new(
                            code::E0008,
                            "`sync` and `event` cannot be combined",
                            "a port is either request/reply (`sync` promises no `async` methods) or host-to-core events",
                            "keep one of them",
                        )
                        .on(&meta.path),
                    );
                }
                requested = this;
                Ok(true)
            } else {
                Ok(false)
            }
        },
    )?;
    if let Some(error) = conflict {
        return Err(error);
    }
    Ok((root, requested, dispatcher_by_use))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::has;

    fn trait_result(src: &str, requested: Requested) -> Result<String, String> {
        let item: ItemTrait = syn::parse_str(src).unwrap();
        expand_trait(None, requested, false, false, item)
            .map(|t| t.to_string())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn async_ports_get_a_proxy_accessor_and_dispatcher() {
        let out = trait_result(
            "pub trait Http { async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>; }",
            Requested::Inferred,
        )
        .unwrap();
        for needle in [
            "pub trait Http: ::core::marker::Send + ::core::marker::Sync",
            "fn request(&self, req: HttpRequest) -> ::core::pin::Pin<::std::boxed::Box<dyn ::core::future::Future<Output = Result<HttpResponse, HttpError>> + ::core::marker::Send + '_>>",
            "impl ::undra::runtime::Port for dyn Http",
            "const KIND: ::undra::meta::PortKind = ::undra::meta::PortKind::Async",
            "pub struct HttpProxy(::undra::runtime::Ctx)",
            "impl Http for HttpProxy",
            "pub fn http(ctx: &::undra::runtime::Ctx) -> ::std::sync::Arc<dyn Http>",
            "pub fn __undra_port_dispatch_Http(",
            "::undra::runtime::PortDispatcher",
            "::undra::meta::Registration::Port",
            "__UNDRA_META_port_Http",
        ] {
            assert!(has(&out, needle), "missing `{needle}` in {out}");
        }
    }

    #[test]
    fn a_dispatcher_by_use_is_a_static_and_not_registered() {
        let item: ItemTrait = syn::parse_str(
            "pub trait SecureStore { async fn get(&self, key: String) -> Option<Bytes>; }",
        )
        .unwrap();
        let out = expand_trait(None, Requested::Inferred, true, false, item)
            .unwrap()
            .to_string();
        assert!(
            has(
                &out,
                "pub static SECURE_STORE_DISPATCHER: ::undra::runtime::PortDispatcher = ::undra::runtime::PortDispatcher"
            ),
            "{out}"
        );
        assert!(
            has(&out, "dispatch: __undra_port_dispatch_erased_SecureStore"),
            "{out}"
        );
        assert!(
            !has(
                &out,
                "inventory::submit! { ::undra::runtime::PortDispatcher"
            ),
            "a dispatcher by use is never submitted: {out}"
        );
        let (_, requested, by_use) = parse_port_args(quote!(sync, dispatcher_by_use)).unwrap();
        assert_eq!((requested, by_use), (Requested::Sync, true));
        assert!(parse_port_args(quote!(dispatcher_by_use = 1)).is_err());
    }

    #[test]
    fn sync_ports_use_port_call_sync() {
        let out = trait_result(
            "pub trait Clock { fn now_ms(&self) -> i64; }",
            Requested::Sync,
        )
        .unwrap();
        assert!(has(&out, "port_call_sync"), "{out}");
        assert!(has(&out, "PortKind::Sync"), "{out}");
        assert!(!has(&out, "Box::pin(async move"), "{out}");
    }

    #[test]
    fn existing_supertraits_are_not_duplicated() {
        let out = trait_result(
            "pub trait Kv: Send + Sync + 'static { fn get(&self, k: String) -> u8; }",
            Requested::Inferred,
        )
        .unwrap();
        assert!(has(&out, "pub trait Kv: Send + Sync + 'static {"), "{out}");
    }

    #[test]
    fn event_ports_get_subscriptions_not_proxies() {
        let out = trait_result(
            "pub trait Connectivity { fn changed(&self, online: bool, kind: NetKind); }",
            Requested::Event,
        )
        .unwrap();
        assert!(has(&out, "pub fn on_connectivity_changed"), "{out}");
        assert!(
            has(
                &out,
                "pub fn encode_connectivity_changed_event(online: bool, kind: NetKind)"
            ),
            "{out}"
        );
        assert!(has(&out, "PortKind::Event"), "{out}");
        assert!(!has(&out, "ConnectivityProxy"), "{out}");
    }

    #[test]
    fn port_diagnostics() {
        let e = |src: &str, req: Requested| trait_result(src, req).unwrap_err();
        assert!(
            e("trait P { fn f(&self, s: &str); }", Requested::Inferred)
                .contains("error[undra::E0030]")
        );
        assert!(
            e(
                "trait P { fn f(&self, s: Box<dyn Fn()>); }",
                Requested::Inferred
            )
            .contains("error[undra::E0030]")
        );
        assert!(
            e("trait P { fn f(&self) -> u8; }", Requested::Event).contains("error[undra::E0031]")
        );
        assert!(
            e("trait P { async fn f(&self); }", Requested::Event).contains("error[undra::E0031]")
        );
        assert!(
            e("trait P { async fn f(&self); }", Requested::Sync).contains("error[undra::E0032]")
        );
        assert!(
            e("trait P { fn f(&mut self); }", Requested::Inferred).contains("error[undra::E0020]")
        );
        assert!(e("trait P { fn f(self); }", Requested::Inferred).contains("error[undra::E0021]"));
        assert!(e("trait P { fn f(); }", Requested::Inferred).contains("no `&self` receiver"));
        assert!(
            e("trait P { type X; fn f(&self); }", Requested::Inferred).contains("not a method")
        );
        assert!(e("trait P<T> { fn f(&self); }", Requested::Inferred).contains("E0002"));
        assert!(e("trait P { fn f<T>(&self, x: T); }", Requested::Inferred).contains("E0002"));
        assert!(e("trait P { }", Requested::Inferred).contains("no methods"));
        assert!(
            e(
                "trait P { fn f(&self) -> impl Stream<Item = u8>; }",
                Requested::Inferred
            )
            .contains("returns a stream")
        );
    }

    #[test]
    fn e0030_keeps_the_underlying_reason() {
        let message =
            trait_result("trait P { fn f(&self, s: &str); }", Requested::Inferred).unwrap_err();
        assert!(
            message.contains("parameter `s` of port method `f` is not a wire type"),
            "{message}"
        );
        assert!(
            message.contains("references have no wire representation"),
            "{message}"
        );
        assert!(message.contains("use an owned `String`"), "{message}");
    }

    #[test]
    fn wildcard_and_pattern_parameters_are_rejected() {
        let message =
            trait_result("pub trait P { fn f(&self, _: u8); }", Requested::Inferred).unwrap_err();
        assert!(message.contains("error[undra::E0032]"), "{message}");
        assert!(message.contains("is not a plain name"), "{message}");
        let message = trait_result(
            "pub trait P { fn f(&self, (a, b): (u8, u8)); }",
            Requested::Inferred,
        )
        .unwrap_err();
        assert!(message.contains("is not a plain name"), "{message}");
    }

    #[test]
    fn args_parse() {
        let parse = |src: &str| parse_port_args(src.parse().unwrap());
        assert_eq!(parse("").unwrap().1, Requested::Inferred);
        assert_eq!(parse("sync").unwrap().1, Requested::Sync);
        assert_eq!(parse("event").unwrap().1, Requested::Event);
        assert!(
            parse("sync, event")
                .unwrap_err()
                .to_string()
                .contains("cannot be combined")
        );
        assert!(parse("bogus").unwrap_err().to_string().contains("E0008"));
        assert!(parse("crate = \"::k\"").unwrap().0.is_some());
    }

    #[test]
    fn impl_blocks_get_boxed_futures() {
        let item: ItemImpl = syn::parse_str(
            "impl Http for Fake { async fn request(&self, req: HttpRequest) -> Result<u8, E> { Ok(1) } fn sync(&self) {} }",
        )
        .unwrap();
        let out = expand_impl(item).unwrap().to_string();
        assert!(
            has(
                &out,
                "fn request(&self, req: HttpRequest) -> ::core::pin::Pin<::std::boxed::Box<dyn ::core::future::Future<Output = Result<u8, E>> + ::core::marker::Send + '_>> { ::std::boxed::Box::pin(async move { Ok(1) }) }"
            ),
            "{out}"
        );
        assert!(has(&out, "fn sync(&self) {}"), "{out}");
        let inherent: ItemImpl = syn::parse_str("impl Fake { fn f(&self) {} }").unwrap();
        assert!(
            expand_impl(inherent)
                .unwrap_err()
                .to_string()
                .contains("E0007")
        );
    }

    #[test]
    fn result_methods_map_every_port_outcome_to_their_error() {
        let out = trait_result(
            "pub trait Http { async fn request(&self, req: Req) -> Result<Resp, HttpError>; async fn ping(&self) -> bool; }",
            Requested::Inferred,
        )
        .unwrap();
        // The error channel: `E::from(PortError)` through a bound that carries E0033.
        for needle in [
            "trait __UndraPortError_Http: ::core::marker::Sized",
            "error[undra::E0033]",
            "<HttpError as __UndraPortError_Http>::__undra_from_port_error(__undra_e)",
            "::undra::runtime::PortError::Failed(__undra_bytes)",
            "::undra::runtime::PortError::Decode(__undra_error)",
        ] {
            assert!(has(&out, needle), "missing `{needle}` in {out}");
        }
        // A method without one panics with the teaching message of E0062.
        for needle in [
            "fn __undra_port_failure_Http(__undra_method: &str, __undra_error: ::undra::runtime::PortError) -> !",
            "has no adapter registered (method `{}`)",
            "`core.registerPort(..)` in TypeScript, Kotlin and Swift, `undra_port_register` in C",
            "https://shreypdev.github.io/undra/docs/errors.html#E0062",
            "__undra_port_failure_Http(\"ping\", __undra_error)",
        ] {
            assert!(has(&out, needle), "missing `{needle}` in {out}");
        }
        // No error channel anywhere: no bound to satisfy.
        let out = trait_result(
            "pub trait Clock { fn now_ms(&self) -> i64; }",
            Requested::Sync,
        )
        .unwrap();
        assert!(!has(&out, "__UndraPortError_Clock"), "{out}");
        assert!(has(&out, "__undra_port_failure_Clock"), "{out}");
    }

    #[test]
    fn generated_locals_never_use_the_users_parameter_names() {
        let out = trait_result(
            "pub trait P { async fn f(&self, __w: u32, __args: u32, __ctx: u32) -> u32; }",
            Requested::Inferred,
        )
        .unwrap();
        // The user's names appear in the signature and the encodes only.
        assert!(has(&out, "Encode::encode(&__w, &mut __undra_w)"), "{out}");
        assert!(has(&out, "let __undra_a0: u32 = match"), "{out}");
        assert!(
            has(
                &out,
                "let __undra_ctx = ::core::clone::Clone::clone(&self.0)"
            ),
            "{out}"
        );
    }

    #[test]
    fn keyword_snake_names_become_raw_accessors() {
        for (snake, expected) in [
            ("http", "http"),
            ("match", "r#match"),
            ("type", "r#type"),
            ("loop", "r#loop"),
            ("gen", "r#gen"),
            ("super", "super_"),
            ("self", "self_"),
            ("crate", "crate_"),
        ] {
            assert_eq!(ident_or_raw(snake).to_string(), expected, "{snake}");
        }
        let out = trait_result("pub trait Match { fn go(&self) -> u8; }", Requested::Sync).unwrap();
        assert!(
            has(&out, "pub fn r#match(ctx: &::undra::runtime::Ctx)"),
            "{out}"
        );
    }

    #[test]
    fn parenthesised_returns_are_read_through() {
        let (ok, err) = result_types(&syn::parse_quote!(-> (Result<u8, E>)), false);
        assert_eq!(ok.map(|t| ty_string(&t)), Some("u8".to_owned()));
        assert_eq!(err.map(|t| ty_string(&t)), Some("E".to_owned()));
        let out = trait_result(
            "pub trait P { fn f(&self) -> (Result<u8, E>); }",
            Requested::Sync,
        )
        .unwrap();
        assert!(has(&out, "__undra_port_error(__undra_other)"), "{out}");
    }

    #[test]
    fn method_ids_use_the_port_formula() {
        let out = trait_result(
            "pub trait Clock { fn now_ms(&self) -> i64; }",
            Requested::Sync,
        )
        .unwrap();
        assert!(
            has(
                &out,
                "::undra::meta::ids::port_method_id(\"Clock\", \"now_ms\")"
            ),
            "{out}"
        );
        assert!(has(&out, "::undra::meta::ids::port_id(\"Clock\")"), "{out}");
    }
}
