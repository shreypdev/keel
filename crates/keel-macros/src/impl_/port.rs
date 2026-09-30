//! `#[keel::port]`: traits implemented by the platform (foreign) or by Rust fakes.
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
//!   `#[keel::port]` also works on `impl Trait for Type { .. }` blocks, where it rewrites
//!   `async fn` bodies into the boxed form.
//!
//! # Generated for sync and async ports
//!
//! * `impl Port for dyn Trait` (`PORT_ID`, `NAME`, `KIND`);
//! * `pub struct <Trait>Proxy(Ctx)` implementing the trait by encoding the arguments and calling
//!   `ctx.port_call(..).await` / `ctx.port_call_sync(..)`; `PortError::Failed(bytes)` is decoded
//!   into the method's `E`. A port that is unavailable, or a reply that cannot be decoded, is a
//!   bug in the host binding and panics with a message naming the port and method (the runtime
//!   turns panics at the dispatch boundary into typed replies);
//! * `pub fn <trait_snake>(ctx: &Ctx) -> Arc<dyn Trait>`: the Rust binding if one is bound
//!   (fakes, built-ins), else the proxy;
//! * `pub fn __keel_port_dispatch_<Trait>(imp: &Arc<dyn Trait>, method_id, args) -> PortDispatch`
//!   for Rust-side bindings called with encoded arguments, and its registration as a
//!   `PortDispatcher`;
//! * `PortMeta` and its registration.
//!
//! # Generated for event ports (`#[keel::port(event)]`)
//!
//! Events flow host to core. Instead of a proxy, each method gets
//! `pub fn on_<trait_snake>_<method>(ctx, f) -> Subscription` (decodes the payload and calls
//! `f`) and `pub fn encode_<trait_snake>_<method>_event(..) -> Vec<u8>` (what a fake or a test
//! feeds to `Runtime::event`).

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{FnArg, ItemImpl, ItemTrait, Pat, ReturnType, TraitItem, Type};

use super::attrs::{Site, docs, parse_args, root_arg, take};
use super::common::{check_generics, derived, item_root, param_meta, submit};
use super::diag::{Diag, Errors, code};
use super::naming::{fnv1a32, snake_case, unraw};
use super::paths::Root;
use super::types::{Allow, KType, Pos, map_return, map_type, ty_string};

/// What kind of port the attribute arguments ask for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Requested {
    /// No argument: the kind follows from the methods.
    Inferred,
    /// `sync`: every method must be synchronous.
    Sync,
    /// `event`: host to core, fire and forget.
    Event,
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

/// Analyses the methods of a port trait, normalising parameter names.
fn analyze_method(
    method: &mut syn::TraitItemFn,
    requested: Requested,
    errors: &mut Errors,
) -> Option<PortMethod> {
    take(&mut method.attrs, Site::NOTHING, errors);
    let name = unraw(&method.sig.ident);
    check_generics(&method.sig.generics, &name, errors);
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
        let kty = match map_type(&pat_type.ty, Pos::Param, Allow::NONE) {
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
    let ret = match map_return(&method.sig.output) {
        Ok(ret) => ret,
        Err(err) => {
            errors.push(err.into_error());
            ok = false;
            KType::Unit
        }
    };
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
                    "remove the return type, or drop `event` from `#[keel::port(event)]` to make it a request/reply port",
                )
                .on(&method.sig.output),
            );
        }
    } else if requested == Requested::Sync && is_async {
        errors.push(
            Diag::new(
                code::E0032,
                format!("`#[keel::port(sync)]` port has the `async` method `{name}`"),
                "`sync` promises that every method answers immediately, so the core can call it without yielding",
                "remove `async`, or remove `sync` from the attribute",
            )
            .on(&method.sig.asyncness),
        );
    }

    ok.then(|| PortMethod {
        ident: method.sig.ident.clone(),
        name,
        is_async,
        params,
        ret,
        docs,
        sig: method.sig.clone(),
    })
}

/// Expands `#[keel::port]` on a trait.
pub(crate) fn expand_trait(
    args_root: Option<Root>,
    requested: Requested,
    mut item: ItemTrait,
) -> syn::Result<TokenStream> {
    let mut errors = Errors::new();
    let root = item_root(&mut item.attrs, args_root, &mut errors);
    let name = item.ident.clone();
    let name_str = unraw(&name);
    check_generics(&item.generics, &name_str, &mut errors);
    let type_docs = docs(&item.attrs);
    let vis = item.vis.clone();

    let mut methods: Vec<PortMethod> = Vec::new();
    for trait_item in &mut item.items {
        match trait_item {
            TraitItem::Fn(method) => {
                if let Some(model) = analyze_method(method, requested, &mut errors) {
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
        _ if methods.iter().any(|m| m.is_async) => "Async",
        _ => "Sync",
    };

    let meta = root.meta();
    let runtime = root.runtime();
    let snake = snake_case(&name_str);
    let derived = derived();
    let kind_ident = syn::Ident::new(kind, Span::call_site());
    let meta_static = format_ident!("__KEEL_META_port_{}", name_str);

    let method_metas = methods.iter().map(|m| {
        let mname = &m.name;
        let params = m.params.iter().map(|p| param_meta(&meta, &p.name, &p.kty));
        let returns = m.ret.meta(&meta);
        let is_async = m.is_async;
        let mdocs = &m.docs;
        quote! {
            #meta::MethodMeta {
                name: #mname,
                method_id: #meta::ids::port_method_id(#name_str, #mname),
                params: &[ #(#params),* ],
                returns: #returns,
                is_async: #is_async,
                takes_ctx: false,
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
            methods: &[ #(#method_metas),* ],
            docs: #type_docs,
        };
        #registration
    };

    let extras = if requested == Requested::Event {
        event_helpers(&root, &vis, &name, &name_str, &snake, &methods)
    } else {
        call_helpers(&root, &vis, &name, &name_str, &snake, &methods)
    };
    Ok(quote! {
        #item
        #port_impl
        #meta_item
        #extras
    })
}

/// The proxy, the accessor and the Rust-side dispatcher of a request/reply port.
fn call_helpers(
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
    let derived = derived();
    let proxy = format_ident!("{}Proxy", name_str);
    let accessor = format_ident!("{}", snake);
    let dispatch_fn = format_ident!("__keel_port_dispatch_{}", name_str);
    let erased_fn = format_ident!("__keel_port_dispatch_erased_{}", name_str);
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
    let proxy_methods = methods.iter().map(|m| {
        let sig = &m.sig;
        let mname = &m.name;
        let port_id = quote!(#meta::ids::port_id(#name_str));
        let method_id = quote!(#meta::ids::port_method_id(#name_str, #mname));
        let encodes = m.params.iter().map(|p| {
            let ident = &p.ident;
            quote_spanned!(p.ty.span()=> #wire::Encode::encode(&#ident, &mut __w);)
        });
        let failed = quote! {
            ::core::panic!(
                "keel: port call `{}.{}` failed: {:?}",
                #name_str,
                #mname,
                __error,
            )
        };
        let undecodable = |ty: &Type| {
            quote_spanned! {ty.span()=>
                match <#ty as #wire::Decode>::decode_exact(&__bytes) {
                    ::core::result::Result::Ok(__value) => __value,
                    ::core::result::Result::Err(__error) => ::core::panic!(
                        "keel: port `{}.{}` replied with a value that does not decode: {:?}",
                        #name_str,
                        #mname,
                        __error,
                    ),
                }
            }
        };
        let (ok_ty, err_ty) = result_types(&m.sig.output, m.is_async);
        let reply = match (&m.ret, ok_ty, err_ty) {
            (KType::Result(ok, _), ok_ty, Some(err_ty)) => {
                let ok_value = if ok.is_unit() {
                    quote!(::core::result::Result::Ok(()))
                } else {
                    let decode = undecodable(&ok_ty.expect("result has an ok type"));
                    quote!(::core::result::Result::Ok(#decode))
                };
                let err_decode = undecodable(&err_ty);
                quote! {
                    match __reply {
                        ::core::result::Result::Ok(__bytes) => #ok_value,
                        ::core::result::Result::Err(#runtime::PortError::Failed(__bytes)) => {
                            ::core::result::Result::Err(#err_decode)
                        }
                        ::core::result::Result::Err(__error) => #failed,
                    }
                }
            }
            (ret, ok_ty, _) if ret.is_unit() => {
                let _ = ok_ty;
                quote! {
                    match __reply {
                        ::core::result::Result::Ok(_) => (),
                        ::core::result::Result::Err(__error) => #failed,
                    }
                }
            }
            (_, ok_ty, _) => {
                let decode = undecodable(&ok_ty.expect("plain return has a type"));
                quote! {
                    match __reply {
                        ::core::result::Result::Ok(__bytes) => #decode,
                        ::core::result::Result::Err(__error) => #failed,
                    }
                }
            }
        };
        let body = if m.is_async {
            quote! {
                let __ctx = ::core::clone::Clone::clone(&self.0);
                ::std::boxed::Box::pin(async move {
                    let __reply = __ctx.port_call(#port_id, #method_id, __args).await;
                    #reply
                })
            }
        } else {
            quote! {
                let __reply = self.0.port_call_sync(#port_id, #method_id, &__args);
                #reply
            }
        };
        quote! {
            #sig {
                let __args = {
                    let mut __w = #wire::Writer::new();
                    #(#encodes)*
                    __w.into_vec()
                };
                #body
            }
        }
    });

    // --- Rust-side dispatcher ---------------------------------------------------------------
    let unavailable = quote!(#runtime::PortDispatch::Sync(::std::vec![2u8]));
    let dispatch_consts = methods.iter().map(|m| {
        let id = format_ident!("__KEEL_ID_{}", m.name);
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
        let id = format_ident!("__KEEL_ID_{}", m.name);
        let ident = &m.ident;
        let decodes = m.params.iter().map(|p| {
            let pident = &p.ident;
            let ty = &p.ty;
            quote_spanned! {p.ty.span()=>
                let #pident: #ty = match <#ty as #wire::Decode>::decode(&mut __r) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(_) => return #unavailable,
                };
            }
        });
        let call_args = m.params.iter().map(|p| &p.ident);
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

    quote! {
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
        #meta::inventory::submit! {
            #runtime::PortDispatcher {
                port_id: #meta::ids::port_id(#name_str),
                dispatch: #erased_fn,
            }
        }
    }
}

/// The types `T` and `E` of a method's declared return type. For an `async fn` rewritten to
/// a boxed future, the original output type is inside the `Future<Output = ..>` bound.
fn result_types(output: &ReturnType, is_async: bool) -> (Option<Type>, Option<Type>) {
    let ReturnType::Type(_, ty) = output else {
        return (None, None);
    };
    let mut ty: &Type = ty;
    if is_async {
        if let Some(inner) = boxed_future_output(ty) {
            ty = inner;
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
        let on_doc = format!("Calls `f` with the decoded arguments of every `{name_str}.{mname}` event.");
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
            quote_spanned!(ty.span()=> #wire::Encode::encode(&#ident, &mut __w);)
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
                f: impl ::core::ops::Fn( #(#tys),* ) + ::core::marker::Send + ::core::marker::Sync + 'static,
            ) -> #runtime::Subscription {
                ctx.events().subscribe(
                    <dyn #name as #runtime::Port>::PORT_ID,
                    #meta::ids::port_method_id(#name_str, #mname),
                    ::std::boxed::Box::new(move |__payload: &[u8]| {
                        let mut __r = #wire::Reader::new(__payload);
                        #(#decodes)*
                        if __r.finish().is_err() {
                            return;
                        }
                        f( #(#locals),* )
                    }),
                )
            }

            #[doc = #encode_doc]
            #vis fn #encode_fn( #(#params),* ) -> ::std::vec::Vec<u8> {
                let mut __w = #wire::Writer::new();
                #(#encodes)*
                __w.into_vec()
            }
        }
    });
    quote! { #(#helpers)* }
}

/// Expands `#[keel::port]` on `impl Trait for Type`: `async fn` becomes a boxed future.
pub(crate) fn expand_impl(mut item: ItemImpl) -> syn::Result<TokenStream> {
    if item.trait_.is_none() {
        return Err(Diag::new(
            code::E0007,
            "`#[keel::port]` on an inherent impl",
            "`#[keel::port]` applies to port trait definitions and to `impl Trait for Type` blocks that implement one",
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

/// Parses the arguments of `#[keel::port(..)]`.
pub(crate) fn parse_port_args(attr: TokenStream) -> syn::Result<(Option<Root>, Requested)> {
    let mut root = None;
    let mut requested = Requested::Inferred;
    let mut conflict: Option<syn::Error> = None;
    parse_args(
        attr,
        "port",
        "`sync`, `event` and `crate = \"path\"`",
        |meta| {
            if meta.path.is_ident("crate") {
                root = Some(root_arg(meta)?);
                Ok(true)
            } else if meta.path.is_ident("sync") || meta.path.is_ident("event") {
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
    Ok((root, requested))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::has;

    fn trait_result(src: &str, requested: Requested) -> Result<String, String> {
        let item: ItemTrait = syn::parse_str(src).unwrap();
        expand_trait(None, requested, item)
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
            "impl ::keel::runtime::Port for dyn Http",
            "const KIND: ::keel::meta::PortKind = ::keel::meta::PortKind::Async",
            "pub struct HttpProxy(::keel::runtime::Ctx)",
            "impl Http for HttpProxy",
            "pub fn http(ctx: &::keel::runtime::Ctx) -> ::std::sync::Arc<dyn Http>",
            "pub fn __keel_port_dispatch_Http(",
            "::keel::runtime::PortDispatcher",
            "::keel::meta::Registration::Port",
            "__KEEL_META_port_Http",
        ] {
            assert!(has(&out, needle), "missing `{needle}` in {out}");
        }
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
                .contains("error[keel::E0030]")
        );
        assert!(
            e(
                "trait P { fn f(&self, s: Box<dyn Fn()>); }",
                Requested::Inferred
            )
            .contains("error[keel::E0030]")
        );
        assert!(
            e("trait P { fn f(&self) -> u8; }", Requested::Event).contains("error[keel::E0031]")
        );
        assert!(
            e("trait P { async fn f(&self); }", Requested::Event).contains("error[keel::E0031]")
        );
        assert!(
            e("trait P { async fn f(&self); }", Requested::Sync).contains("error[keel::E0032]")
        );
        assert!(
            e("trait P { fn f(&mut self); }", Requested::Inferred).contains("error[keel::E0020]")
        );
        assert!(e("trait P { fn f(self); }", Requested::Inferred).contains("error[keel::E0021]"));
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
        assert!(message.contains("error[keel::E0032]"), "{message}");
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
    fn method_ids_use_the_port_formula() {
        let out = trait_result(
            "pub trait Clock { fn now_ms(&self) -> i64; }",
            Requested::Sync,
        )
        .unwrap();
        assert!(
            has(
                &out,
                "::keel::meta::ids::port_method_id(\"Clock\", \"now_ms\")"
            ),
            "{out}"
        );
        assert!(has(&out, "::keel::meta::ids::port_id(\"Clock\")"), "{out}");
    }
}
