/// Talks HTTP.
pub trait Http: ::core::marker::Send + ::core::marker::Sync {
    /// Sends a request.
    fn request(
        &self,
        req: HttpRequest,
    ) -> ::core::pin::Pin<
        ::std::boxed::Box<
            dyn ::core::future::Future<
                Output = Result<HttpResponse, HttpError>,
            > + ::core::marker::Send + '_,
        >,
    >;
    fn ping(
        &self,
        url: String,
    ) -> ::core::pin::Pin<
        ::std::boxed::Box<
            dyn ::core::future::Future<Output = bool> + ::core::marker::Send + '_,
        >,
    >;
}
#[automatically_derived]
impl ::keel::runtime::Port for dyn Http {
    const PORT_ID: u32 = ::keel::meta::ids::port_id("Http");
    const NAME: &'static str = "Http";
    const KIND: ::keel::meta::PortKind = ::keel::meta::PortKind::Async;
}
#[allow(non_upper_case_globals)]
static __KEEL_META_port_Http: ::keel::meta::PortMeta = ::keel::meta::PortMeta {
    name: "Http",
    port_id: ::keel::meta::ids::port_id("Http"),
    kind: ::keel::meta::PortKind::Async,
    methods: &[
        ::keel::meta::MethodMeta {
            name: "request",
            method_id: ::keel::meta::ids::port_method_id("Http", "request"),
            params: &[
                ::keel::meta::ParamMeta {
                    name: "req",
                    ty: ::keel::meta::TypeRefMeta::Named("HttpRequest"),
                },
            ],
            returns: ::keel::meta::TypeRefMeta::Result(
                &::keel::meta::TypeRefMeta::Named("HttpResponse"),
                &::keel::meta::TypeRefMeta::Named("HttpError"),
            ),
            is_async: true,
            takes_ctx: false,
            docs: "Sends a request.",
        },
        ::keel::meta::MethodMeta {
            name: "ping",
            method_id: ::keel::meta::ids::port_method_id("Http", "ping"),
            params: &[
                ::keel::meta::ParamMeta {
                    name: "url",
                    ty: ::keel::meta::TypeRefMeta::String,
                },
            ],
            returns: ::keel::meta::TypeRefMeta::Bool,
            is_async: true,
            takes_ctx: false,
            docs: "",
        },
    ],
    docs: "Talks HTTP.",
};
::keel::meta::inventory::submit! {
    ::keel::meta::Registration::Port(& __KEEL_META_port_Http)
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code)]
#[diagnostic::on_unimplemented(
    message = "error[keel::E0033]: the error type `{Self}` of a method of the `Http` port cannot represent a port that is unavailable\n  = note: a port nobody registered, a cancelled call and a reply that does not decode are ordinary outcomes (SPEC 6.3), and a method that returns `Result<T, E>` reports them as its error instead of panicking; that needs `From<PortError>` for the error type\n  = help: implement `From<keel::runtime::PortError>` for `{Self}`, mapping it to a variant such as `Unavailable`, or to the `Display` text of the `PortError`\n  = docs: https://keel.dev/errors/E0033",
    label = "`From<PortError>` is not implemented for this error type"
)]
trait __KeelPortError_Http: ::core::marker::Sized {
    fn __keel_from_port_error(__keel_e: ::keel::runtime::PortError) -> Self;
}
impl<__KeelE: ::core::convert::From<::keel::runtime::PortError>> __KeelPortError_Http
for __KeelE {
    fn __keel_from_port_error(__keel_e: ::keel::runtime::PortError) -> Self {
        <__KeelE as ::core::convert::From<::keel::runtime::PortError>>::from(__keel_e)
    }
}
#[doc(hidden)]
#[cold]
#[inline(never)]
#[allow(non_snake_case, dead_code)]
fn __keel_port_failure_Http(
    __keel_method: &str,
    __keel_error: ::keel::runtime::PortError,
) -> ! {
    let (__keel_what, __keel_how) = match &__keel_error {
        ::keel::runtime::PortError::Unavailable => {
            (
                ::std::format!(
                    "the `{}` port has no adapter registered (method `{}`)", "Http",
                    __keel_method,
                ),
                "Register one with core.registerPort(..) (TypeScript, Kotlin, Swift) / keel_port_register (C), or bind a Rust implementation (`keel::ports::fakes` in tests)",
            )
        }
        ::keel::runtime::PortError::Cancelled => {
            (
                ::std::format!(
                    "a call to the `{}` port (method `{}`) was cancelled", "Http",
                    __keel_method,
                ),
                "A method without an error type cannot report an abandoned call; give it a `Result<T, E>` return type",
            )
        }
        ::keel::runtime::PortError::Decode(__keel_why) => {
            (
                ::std::format!(
                    "the `{}` port (method `{}`) replied with bytes that do not decode: {}",
                    "Http", __keel_method, __keel_why,
                ),
                "The adapter's reply does not match the schema; check its codec for this method",
            )
        }
        __keel_other => {
            (
                ::std::format!(
                    "a call to the `{}` port (method `{}`) failed: {}", "Http",
                    __keel_method, __keel_other,
                ),
                "A method without an error type cannot report a failed call; give it a `Result<T, E>` return type",
            )
        }
    };
    ::core::panic!(
        "keel: {}. {}. On the web this traps the core. docs: {}", __keel_what,
        __keel_how, "https://keel.dev/errors/E0062",
    )
}
///Calls the `Http` port through the runtime's port table: the platform's binding, or a Rust fake.
pub struct HttpProxy(::keel::runtime::Ctx);
impl HttpProxy {
    /// Creates a proxy that calls through `ctx`.
    pub fn new(ctx: ::keel::runtime::Ctx) -> Self {
        Self(ctx)
    }
}
#[automatically_derived]
impl Http for HttpProxy {
    fn request(
        &self,
        req: HttpRequest,
    ) -> ::core::pin::Pin<
        ::std::boxed::Box<
            dyn ::core::future::Future<
                Output = Result<HttpResponse, HttpError>,
            > + ::core::marker::Send + '_,
        >,
    > {
        let __keel_args = {
            let mut __keel_w = ::keel::wire::Writer::new();
            ::keel::wire::Encode::encode(&req, &mut __keel_w);
            __keel_w.into_vec()
        };
        let __keel_ctx = ::core::clone::Clone::clone(&self.0);
        ::std::boxed::Box::pin(async move {
            let __keel_reply = __keel_ctx
                .port_call(
                    ::keel::meta::ids::port_id("Http"),
                    ::keel::meta::ids::port_method_id("Http", "request"),
                    __keel_args,
                )
                .await;
            fn __keel_port_error(__keel_e: ::keel::runtime::PortError) -> HttpError {
                <HttpError as __KeelPortError_Http>::__keel_from_port_error(__keel_e)
            }
            match __keel_reply {
                ::core::result::Result::Ok(__keel_bytes) => {
                    match <HttpResponse as ::keel::wire::Decode>::decode_exact(
                        &__keel_bytes,
                    ) {
                        ::core::result::Result::Ok(__keel_value) => {
                            ::core::result::Result::Ok(__keel_value)
                        }
                        ::core::result::Result::Err(__keel_error) => {
                            ::core::result::Result::Err(
                                __keel_port_error(
                                    ::keel::runtime::PortError::Decode(__keel_error),
                                ),
                            )
                        }
                    }
                }
                ::core::result::Result::Err(
                    ::keel::runtime::PortError::Failed(__keel_bytes),
                ) => {
                    match <HttpError as ::keel::wire::Decode>::decode_exact(
                        &__keel_bytes,
                    ) {
                        ::core::result::Result::Ok(__keel_value) => {
                            ::core::result::Result::Err(__keel_value)
                        }
                        ::core::result::Result::Err(__keel_error) => {
                            ::core::result::Result::Err(
                                __keel_port_error(
                                    ::keel::runtime::PortError::Decode(__keel_error),
                                ),
                            )
                        }
                    }
                }
                ::core::result::Result::Err(__keel_other) => {
                    ::core::result::Result::Err(__keel_port_error(__keel_other))
                }
            }
        })
    }
    fn ping(
        &self,
        url: String,
    ) -> ::core::pin::Pin<
        ::std::boxed::Box<
            dyn ::core::future::Future<Output = bool> + ::core::marker::Send + '_,
        >,
    > {
        let __keel_args = {
            let mut __keel_w = ::keel::wire::Writer::new();
            ::keel::wire::Encode::encode(&url, &mut __keel_w);
            __keel_w.into_vec()
        };
        let __keel_ctx = ::core::clone::Clone::clone(&self.0);
        ::std::boxed::Box::pin(async move {
            let __keel_reply = __keel_ctx
                .port_call(
                    ::keel::meta::ids::port_id("Http"),
                    ::keel::meta::ids::port_method_id("Http", "ping"),
                    __keel_args,
                )
                .await;
            match __keel_reply {
                ::core::result::Result::Ok(__keel_bytes) => {
                    match <bool as ::keel::wire::Decode>::decode_exact(&__keel_bytes) {
                        ::core::result::Result::Ok(__keel_value) => __keel_value,
                        ::core::result::Result::Err(__keel_error) => {
                            __keel_port_failure_Http(
                                "ping",
                                ::keel::runtime::PortError::Decode(__keel_error),
                            )
                        }
                    }
                }
                ::core::result::Result::Err(__keel_error) => {
                    __keel_port_failure_Http("ping", __keel_error)
                }
            }
        })
    }
}
///The Rust binding of the `Http` port if one is bound (fakes, built-ins), otherwise a proxy to the platform's binding.
pub fn http(ctx: &::keel::runtime::Ctx) -> ::std::sync::Arc<dyn Http> {
    match ctx.rust_port::<dyn Http>(<dyn Http as ::keel::runtime::Port>::PORT_ID) {
        ::core::option::Option::Some(__imp) => __imp,
        ::core::option::Option::None => {
            ::std::sync::Arc::new(HttpProxy::new(::core::clone::Clone::clone(ctx)))
        }
    }
}
///Runs an encoded `Http` call on a Rust implementation. The reply is `status u8` (0 ok, 1 typed error, 2 unavailable) followed by the body.
#[allow(
    non_snake_case,
    non_upper_case_globals,
    unused_mut,
    unused_variables,
    clippy::all
)]
pub fn __keel_port_dispatch_Http(
    __imp: &::std::sync::Arc<dyn Http>,
    __method_id: u32,
    __args: &[u8],
) -> ::keel::runtime::PortDispatch {
    const __KEEL_ID_request: u32 = ::keel::meta::ids::port_method_id("Http", "request");
    const __KEEL_ID_ping: u32 = ::keel::meta::ids::port_method_id("Http", "ping");
    match __method_id {
        __KEEL_ID_request => {
            let mut __r = ::keel::wire::Reader::new(__args);
            let __keel_a0: HttpRequest = match <HttpRequest as ::keel::wire::Decode>::decode(
                &mut __r,
            ) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(_) => {
                    return ::keel::runtime::PortDispatch::Sync(::std::vec![2u8]);
                }
            };
            if __r.finish().is_err() {
                return ::keel::runtime::PortDispatch::Sync(::std::vec![2u8]);
            }
            {
                let __imp = ::std::sync::Arc::clone(__imp);
                ::keel::runtime::PortDispatch::Async(
                    ::std::boxed::Box::pin(async move {
                        match __imp.request(__keel_a0).await {
                            ::core::result::Result::Ok(__v) => {
                                let mut __w = ::keel::wire::Writer::new();
                                __w.write_u8(0u8);
                                ::keel::wire::Encode::encode(&__v, &mut __w);
                                __w.into_vec()
                            }
                            ::core::result::Result::Err(__e) => {
                                let mut __w = ::keel::wire::Writer::new();
                                __w.write_u8(1u8);
                                ::keel::wire::Encode::encode(&__e, &mut __w);
                                __w.into_vec()
                            }
                        }
                    }),
                )
            }
        }
        __KEEL_ID_ping => {
            let mut __r = ::keel::wire::Reader::new(__args);
            let __keel_a0: String = match <String as ::keel::wire::Decode>::decode(
                &mut __r,
            ) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(_) => {
                    return ::keel::runtime::PortDispatch::Sync(::std::vec![2u8]);
                }
            };
            if __r.finish().is_err() {
                return ::keel::runtime::PortDispatch::Sync(::std::vec![2u8]);
            }
            {
                let __imp = ::std::sync::Arc::clone(__imp);
                ::keel::runtime::PortDispatch::Async(
                    ::std::boxed::Box::pin(async move {
                        let __v = __imp.ping(__keel_a0).await;
                        {
                            let mut __w = ::keel::wire::Writer::new();
                            __w.write_u8(0u8);
                            ::keel::wire::Encode::encode(&__v, &mut __w);
                            __w.into_vec()
                        }
                    }),
                )
            }
        }
        _ => ::keel::runtime::PortDispatch::Sync(::std::vec![2u8]),
    }
}
#[doc(hidden)]
#[allow(non_snake_case)]
fn __keel_port_dispatch_erased_Http(
    __imp: &(dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync),
    __method_id: u32,
    __args: &[u8],
) -> ::keel::runtime::PortDispatch {
    match __imp.downcast_ref::<::std::sync::Arc<dyn Http>>() {
        ::core::option::Option::Some(__imp) => {
            __keel_port_dispatch_Http(__imp, __method_id, __args)
        }
        ::core::option::Option::None => {
            ::keel::runtime::PortDispatch::Sync(::std::vec![2u8])
        }
    }
}
::keel::meta::inventory::submit! {
    ::keel::runtime::PortDispatcher { port_id : ::keel::meta::ids::port_id("Http"),
    dispatch : __keel_port_dispatch_erased_Http, }
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code, unused, unused_braces, clippy::all)]
const _: () = {
    #[diagnostic::on_unimplemented(
        message = "error[keel::E0060]: `{Self}` is spelled like the built-in Keel type `{T}`, but it is a different type\n  = note: the schema records this position as the built-in type, so the platforms would read the bytes of `{T}` where the generated code writes `{Self}`\n  = help: rename your type, or import the built-in one (`{T}`) where it is used\n  = docs: https://keel.dev/errors/E0060",
        label = "this is not `{T}`"
    )]
    trait __KeelSameAs<T: ?::core::marker::Sized> {}
    impl<T: ?::core::marker::Sized> __KeelSameAs<T> for T {}
    fn __keel_same<A, B>()
    where
        A: ?::core::marker::Sized + __KeelSameAs<B>,
        B: ?::core::marker::Sized,
    {}
    fn __keel_identity() {
        __keel_same::<
            Result<HttpResponse, HttpError>,
            ::core::result::Result<HttpResponse, HttpError>,
        >();
        __keel_same::<String, ::std::string::String>();
        __keel_same::<bool, ::core::primitive::bool>();
    }
    trait __KeelFallback {
        const KEEL_TYPE_ID: u32 = 0;
        const KEEL_IS_ERROR: bool = false;
        const __KEEL_IS_OBJECT: bool = false;
    }
    impl<T: ?::core::marker::Sized> __KeelFallback for T {}
    const _: () = {
        if <HttpRequest>::__KEEL_IS_OBJECT {
            ::core::panic!(
                "error[keel::E0064]: `HttpRequest` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a parameter or a return value\n  = help: return a record with the data the platform needs, or construct the object from the platform with one of its constructors\n  = docs: https://keel.dev/errors/E0064"
            );
        }
        if <HttpRequest>::KEEL_TYPE_ID != ::keel::meta::ids::type_id("HttpRequest") {
            ::core::panic!(
                "error[keel::E0061]: the schema records this type as `HttpRequest`, but the type written here is not that type\n  = note: Keel describes a type to the platforms by the name it is written with, while the generated code encodes the type the name resolves to; an alias (`type HttpRequest = Other`), a renamed import (`use path::Other as HttpRequest`) or a type that is not declared with `#[keel::api]` makes the two differ, so the platforms would read the wrong layout\n  = help: write the type under its declared name (for an alias or a renamed import, use `Other` here), or declare it with `#[keel::api]` (`#[keel::error]` for errors)\n  = docs: https://keel.dev/errors/E0061"
            );
        }
    };
    const _: () = {
        if <HttpResponse>::__KEEL_IS_OBJECT {
            ::core::panic!(
                "error[keel::E0064]: `HttpResponse` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a parameter or a return value\n  = help: return a record with the data the platform needs, or construct the object from the platform with one of its constructors\n  = docs: https://keel.dev/errors/E0064"
            );
        }
        if <HttpResponse>::KEEL_TYPE_ID != ::keel::meta::ids::type_id("HttpResponse") {
            ::core::panic!(
                "error[keel::E0061]: the schema records this type as `HttpResponse`, but the type written here is not that type\n  = note: Keel describes a type to the platforms by the name it is written with, while the generated code encodes the type the name resolves to; an alias (`type HttpResponse = Other`), a renamed import (`use path::Other as HttpResponse`) or a type that is not declared with `#[keel::api]` makes the two differ, so the platforms would read the wrong layout\n  = help: write the type under its declared name (for an alias or a renamed import, use `Other` here), or declare it with `#[keel::api]` (`#[keel::error]` for errors)\n  = docs: https://keel.dev/errors/E0061"
            );
        }
    };
    const _: () = {
        if <HttpError>::__KEEL_IS_OBJECT {
            ::core::panic!(
                "error[keel::E0064]: `HttpError` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a parameter or a return value\n  = help: return a record with the data the platform needs, or construct the object from the platform with one of its constructors\n  = docs: https://keel.dev/errors/E0064"
            );
        }
        if <HttpError>::KEEL_TYPE_ID != ::keel::meta::ids::type_id("HttpError") {
            ::core::panic!(
                "error[keel::E0061]: the schema records this type as `HttpError`, but the type written here is not that type\n  = note: Keel describes a type to the platforms by the name it is written with, while the generated code encodes the type the name resolves to; an alias (`type HttpError = Other`), a renamed import (`use path::Other as HttpError`) or a type that is not declared with `#[keel::api]` makes the two differ, so the platforms would read the wrong layout\n  = help: write the type under its declared name (for an alias or a renamed import, use `Other` here), or declare it with `#[keel::api]` (`#[keel::error]` for errors)\n  = docs: https://keel.dev/errors/E0061"
            );
        }
        if !<HttpError>::KEEL_IS_ERROR {
            ::core::panic!(
                "error[keel::E0001]: `HttpError` is used as the error type of a `Result`, but it is not a `#[keel::error]` enum\n  = note: the platforms throw the error type by name, and only `#[keel::error]` enums carry the messages they show\n  = help: declare it with `#[keel::error]`, for example `#[keel::error] enum HttpError {{ #[error(\"failed\")] Failed }}`\n  = docs: https://keel.dev/errors/E0001"
            );
        }
    };
};
