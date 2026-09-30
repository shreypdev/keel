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
impl ::undra::runtime::Port for dyn Http {
    const PORT_ID: u32 = ::undra::meta::ids::port_id("Http");
    const NAME: &'static str = "Http";
    const KIND: ::undra::meta::PortKind = ::undra::meta::PortKind::Async;
}
#[allow(non_upper_case_globals)]
static __UNDRA_META_port_Http: ::undra::meta::PortMeta = ::undra::meta::PortMeta {
    name: "Http",
    port_id: ::undra::meta::ids::port_id("Http"),
    kind: ::undra::meta::PortKind::Async,
    methods: &[
        ::undra::meta::MethodMeta {
            name: "request",
            method_id: ::undra::meta::ids::port_method_id("Http", "request"),
            params: &[
                ::undra::meta::ParamMeta {
                    name: "req",
                    ty: ::undra::meta::TypeRefMeta::Named("HttpRequest"),
                },
            ],
            returns: ::undra::meta::TypeRefMeta::Result(
                &::undra::meta::TypeRefMeta::Named("HttpResponse"),
                &::undra::meta::TypeRefMeta::Named("HttpError"),
            ),
            is_async: true,
            takes_ctx: false,
            docs: "Sends a request.",
        },
        ::undra::meta::MethodMeta {
            name: "ping",
            method_id: ::undra::meta::ids::port_method_id("Http", "ping"),
            params: &[
                ::undra::meta::ParamMeta {
                    name: "url",
                    ty: ::undra::meta::TypeRefMeta::String,
                },
            ],
            returns: ::undra::meta::TypeRefMeta::Bool,
            is_async: true,
            takes_ctx: false,
            docs: "",
        },
    ],
    docs: "Talks HTTP.",
};
::undra::meta::inventory::submit! {
    ::undra::meta::Registration::Port(& __UNDRA_META_port_Http)
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code)]
#[diagnostic::on_unimplemented(
    message = "error[undra::E0033]: the error type `{Self}` of a method of the `Http` port cannot represent a port that is unavailable\n  = note: a port nobody registered, a cancelled call and a reply that does not decode are ordinary outcomes (SPEC 6.3), and a method that returns `Result<T, E>` reports them as its error instead of panicking; that needs `From<PortError>` for the error type\n  = help: implement `From<undra::runtime::PortError>` for `{Self}`, mapping it to a variant such as `Unavailable`, or to the `Display` text of the `PortError`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0033",
    label = "`From<PortError>` is not implemented for this error type"
)]
trait __UndraPortError_Http: ::core::marker::Sized {
    fn __undra_from_port_error(__undra_e: ::undra::runtime::PortError) -> Self;
}
impl<__UndraE: ::core::convert::From<::undra::runtime::PortError>> __UndraPortError_Http
for __UndraE {
    fn __undra_from_port_error(__undra_e: ::undra::runtime::PortError) -> Self {
        <__UndraE as ::core::convert::From<::undra::runtime::PortError>>::from(__undra_e)
    }
}
#[doc(hidden)]
#[cold]
#[inline(never)]
#[allow(non_snake_case, dead_code)]
fn __undra_port_failure_Http(
    __undra_method: &str,
    __undra_error: ::undra::runtime::PortError,
) -> ! {
    let (__undra_what, __undra_how) = match &__undra_error {
        ::undra::runtime::PortError::Unavailable => {
            (
                ::std::format!(
                    "the `{}` port has no adapter registered (method `{}`)", "Http",
                    __undra_method,
                ),
                "Register one with core.registerPort(..) (TypeScript, Kotlin, Swift) / undra_port_register (C), or bind a Rust implementation (`undra::ports::fakes` in tests)",
            )
        }
        ::undra::runtime::PortError::Cancelled => {
            (
                ::std::format!(
                    "a call to the `{}` port (method `{}`) was cancelled", "Http",
                    __undra_method,
                ),
                "A method without an error type cannot report an abandoned call; give it a `Result<T, E>` return type",
            )
        }
        ::undra::runtime::PortError::Decode(__undra_why) => {
            (
                ::std::format!(
                    "the `{}` port (method `{}`) replied with bytes that do not decode: {}",
                    "Http", __undra_method, __undra_why,
                ),
                "The adapter's reply does not match the schema; check its codec for this method",
            )
        }
        __undra_other => {
            (
                ::std::format!(
                    "a call to the `{}` port (method `{}`) failed: {}", "Http",
                    __undra_method, __undra_other,
                ),
                "A method without an error type cannot report a failed call; give it a `Result<T, E>` return type",
            )
        }
    };
    ::core::panic!(
        "undra: {}. {}. On the web this traps the core. docs: {}", __undra_what,
        __undra_how, "https://shreypdev.github.io/undra/docs/errors.html#E0062",
    )
}
///Calls the `Http` port through the runtime's port table: the platform's binding, or a Rust fake.
pub struct HttpProxy(::undra::runtime::Ctx);
impl HttpProxy {
    /// Creates a proxy that calls through `ctx`.
    pub fn new(ctx: ::undra::runtime::Ctx) -> Self {
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
        let __undra_args = {
            let mut __undra_w = ::undra::wire::Writer::new();
            ::undra::wire::Encode::encode(&req, &mut __undra_w);
            __undra_w.into_vec()
        };
        let __undra_ctx = ::core::clone::Clone::clone(&self.0);
        ::std::boxed::Box::pin(async move {
            let __undra_reply = __undra_ctx
                .port_call(
                    ::undra::meta::ids::port_id("Http"),
                    ::undra::meta::ids::port_method_id("Http", "request"),
                    __undra_args,
                )
                .await;
            fn __undra_port_error(__undra_e: ::undra::runtime::PortError) -> HttpError {
                <HttpError as __UndraPortError_Http>::__undra_from_port_error(__undra_e)
            }
            match __undra_reply {
                ::core::result::Result::Ok(__undra_bytes) => {
                    match <HttpResponse as ::undra::wire::Decode>::decode_exact(
                        &__undra_bytes,
                    ) {
                        ::core::result::Result::Ok(__undra_value) => {
                            ::core::result::Result::Ok(__undra_value)
                        }
                        ::core::result::Result::Err(__undra_error) => {
                            ::core::result::Result::Err(
                                __undra_port_error(
                                    ::undra::runtime::PortError::Decode(__undra_error),
                                ),
                            )
                        }
                    }
                }
                ::core::result::Result::Err(
                    ::undra::runtime::PortError::Failed(__undra_bytes),
                ) => {
                    match <HttpError as ::undra::wire::Decode>::decode_exact(
                        &__undra_bytes,
                    ) {
                        ::core::result::Result::Ok(__undra_value) => {
                            ::core::result::Result::Err(__undra_value)
                        }
                        ::core::result::Result::Err(__undra_error) => {
                            ::core::result::Result::Err(
                                __undra_port_error(
                                    ::undra::runtime::PortError::Decode(__undra_error),
                                ),
                            )
                        }
                    }
                }
                ::core::result::Result::Err(__undra_other) => {
                    ::core::result::Result::Err(__undra_port_error(__undra_other))
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
        let __undra_args = {
            let mut __undra_w = ::undra::wire::Writer::new();
            ::undra::wire::Encode::encode(&url, &mut __undra_w);
            __undra_w.into_vec()
        };
        let __undra_ctx = ::core::clone::Clone::clone(&self.0);
        ::std::boxed::Box::pin(async move {
            let __undra_reply = __undra_ctx
                .port_call(
                    ::undra::meta::ids::port_id("Http"),
                    ::undra::meta::ids::port_method_id("Http", "ping"),
                    __undra_args,
                )
                .await;
            match __undra_reply {
                ::core::result::Result::Ok(__undra_bytes) => {
                    match <bool as ::undra::wire::Decode>::decode_exact(&__undra_bytes) {
                        ::core::result::Result::Ok(__undra_value) => __undra_value,
                        ::core::result::Result::Err(__undra_error) => {
                            __undra_port_failure_Http(
                                "ping",
                                ::undra::runtime::PortError::Decode(__undra_error),
                            )
                        }
                    }
                }
                ::core::result::Result::Err(__undra_error) => {
                    __undra_port_failure_Http("ping", __undra_error)
                }
            }
        })
    }
}
///The Rust binding of the `Http` port if one is bound (fakes, built-ins), otherwise a proxy to the platform's binding.
pub fn http(ctx: &::undra::runtime::Ctx) -> ::std::sync::Arc<dyn Http> {
    match ctx.rust_port::<dyn Http>(<dyn Http as ::undra::runtime::Port>::PORT_ID) {
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
pub fn __undra_port_dispatch_Http(
    __imp: &::std::sync::Arc<dyn Http>,
    __method_id: u32,
    __args: &[u8],
) -> ::undra::runtime::PortDispatch {
    const __UNDRA_ID_request: u32 = ::undra::meta::ids::port_method_id(
        "Http",
        "request",
    );
    const __UNDRA_ID_ping: u32 = ::undra::meta::ids::port_method_id("Http", "ping");
    match __method_id {
        __UNDRA_ID_request => {
            let mut __r = ::undra::wire::Reader::new(__args);
            let __undra_a0: HttpRequest = match <HttpRequest as ::undra::wire::Decode>::decode(
                &mut __r,
            ) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(_) => {
                    return ::undra::runtime::PortDispatch::Sync(::std::vec![2u8]);
                }
            };
            if __r.finish().is_err() {
                return ::undra::runtime::PortDispatch::Sync(::std::vec![2u8]);
            }
            {
                let __imp = ::std::sync::Arc::clone(__imp);
                ::undra::runtime::PortDispatch::Async(
                    ::std::boxed::Box::pin(async move {
                        match __imp.request(__undra_a0).await {
                            ::core::result::Result::Ok(__v) => {
                                let mut __w = ::undra::wire::Writer::new();
                                __w.write_u8(0u8);
                                ::undra::wire::Encode::encode(&__v, &mut __w);
                                __w.into_vec()
                            }
                            ::core::result::Result::Err(__e) => {
                                let mut __w = ::undra::wire::Writer::new();
                                __w.write_u8(1u8);
                                ::undra::wire::Encode::encode(&__e, &mut __w);
                                __w.into_vec()
                            }
                        }
                    }),
                )
            }
        }
        __UNDRA_ID_ping => {
            let mut __r = ::undra::wire::Reader::new(__args);
            let __undra_a0: String = match <String as ::undra::wire::Decode>::decode(
                &mut __r,
            ) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(_) => {
                    return ::undra::runtime::PortDispatch::Sync(::std::vec![2u8]);
                }
            };
            if __r.finish().is_err() {
                return ::undra::runtime::PortDispatch::Sync(::std::vec![2u8]);
            }
            {
                let __imp = ::std::sync::Arc::clone(__imp);
                ::undra::runtime::PortDispatch::Async(
                    ::std::boxed::Box::pin(async move {
                        let __v = __imp.ping(__undra_a0).await;
                        {
                            let mut __w = ::undra::wire::Writer::new();
                            __w.write_u8(0u8);
                            ::undra::wire::Encode::encode(&__v, &mut __w);
                            __w.into_vec()
                        }
                    }),
                )
            }
        }
        _ => ::undra::runtime::PortDispatch::Sync(::std::vec![2u8]),
    }
}
#[doc(hidden)]
#[allow(non_snake_case)]
fn __undra_port_dispatch_erased_Http(
    __imp: &(dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync),
    __method_id: u32,
    __args: &[u8],
) -> ::undra::runtime::PortDispatch {
    match __imp.downcast_ref::<::std::sync::Arc<dyn Http>>() {
        ::core::option::Option::Some(__imp) => {
            __undra_port_dispatch_Http(__imp, __method_id, __args)
        }
        ::core::option::Option::None => {
            ::undra::runtime::PortDispatch::Sync(::std::vec![2u8])
        }
    }
}
::undra::meta::inventory::submit! {
    ::undra::runtime::PortDispatcher { port_id : ::undra::meta::ids::port_id("Http"),
    dispatch : __undra_port_dispatch_erased_Http, }
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code, unused, unused_braces, clippy::all)]
const _: () = {
    #[diagnostic::on_unimplemented(
        message = "error[undra::E0060]: `{Self}` is spelled like the built-in Undra type `{T}`, but it is a different type\n  = note: the schema records this position as the built-in type, so the platforms would read the bytes of `{T}` where the generated code writes `{Self}`\n  = help: rename your type, or import the built-in one (`{T}`) where it is used\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0060",
        label = "this is not `{T}`"
    )]
    trait __UndraSameAs<T: ?::core::marker::Sized> {}
    impl<T: ?::core::marker::Sized> __UndraSameAs<T> for T {}
    fn __undra_same<A, B>()
    where
        A: ?::core::marker::Sized + __UndraSameAs<B>,
        B: ?::core::marker::Sized,
    {}
    fn __undra_identity() {
        __undra_same::<
            Result<HttpResponse, HttpError>,
            ::core::result::Result<HttpResponse, HttpError>,
        >();
        __undra_same::<String, ::std::string::String>();
        __undra_same::<bool, ::core::primitive::bool>();
    }
    trait __UndraFallback {
        const UNDRA_TYPE_ID: u32 = 0;
        const UNDRA_IS_ERROR: bool = false;
        const __UNDRA_IS_OBJECT: bool = false;
    }
    impl<T: ?::core::marker::Sized> __UndraFallback for T {}
    const _: () = {
        if <HttpRequest>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0064]: `HttpRequest` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a parameter or a return value\n  = help: return a record with the data the platform needs, or construct the object from the platform with one of its constructors\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0064"
            );
        }
        if <HttpRequest>::UNDRA_TYPE_ID != ::undra::meta::ids::type_id("HttpRequest") {
            ::core::panic!(
                "error[undra::E0061]: the schema records this type as `HttpRequest`, but the type written here is not that type\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type the name resolves to; an alias (`type HttpRequest = Other`), a renamed import (`use path::Other as HttpRequest`) or a type that is not declared with `#[undra::api]` makes the two differ, so the platforms would read the wrong layout\n  = help: write the type under its declared name (for an alias or a renamed import, use `Other` here), or declare it with `#[undra::api]` (`#[undra::error]` for errors)\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
    };
    const _: () = {
        if <HttpResponse>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0064]: `HttpResponse` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a parameter or a return value\n  = help: return a record with the data the platform needs, or construct the object from the platform with one of its constructors\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0064"
            );
        }
        if <HttpResponse>::UNDRA_TYPE_ID != ::undra::meta::ids::type_id("HttpResponse") {
            ::core::panic!(
                "error[undra::E0061]: the schema records this type as `HttpResponse`, but the type written here is not that type\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type the name resolves to; an alias (`type HttpResponse = Other`), a renamed import (`use path::Other as HttpResponse`) or a type that is not declared with `#[undra::api]` makes the two differ, so the platforms would read the wrong layout\n  = help: write the type under its declared name (for an alias or a renamed import, use `Other` here), or declare it with `#[undra::api]` (`#[undra::error]` for errors)\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
    };
    const _: () = {
        if <HttpError>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0064]: `HttpError` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a parameter or a return value\n  = help: return a record with the data the platform needs, or construct the object from the platform with one of its constructors\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0064"
            );
        }
        if <HttpError>::UNDRA_TYPE_ID != ::undra::meta::ids::type_id("HttpError") {
            ::core::panic!(
                "error[undra::E0061]: the schema records this type as `HttpError`, but the type written here is not that type\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type the name resolves to; an alias (`type HttpError = Other`), a renamed import (`use path::Other as HttpError`) or a type that is not declared with `#[undra::api]` makes the two differ, so the platforms would read the wrong layout\n  = help: write the type under its declared name (for an alias or a renamed import, use `Other` here), or declare it with `#[undra::api]` (`#[undra::error]` for errors)\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if !<HttpError>::UNDRA_IS_ERROR {
            ::core::panic!(
                "error[undra::E0001]: `HttpError` is used as the error type of a `Result`, but it is not a `#[undra::error]` enum\n  = note: the platforms throw the error type by name, and only `#[undra::error]` enums carry the messages they show\n  = help: declare it with `#[undra::error]`, for example `#[undra::error] enum HttpError {{ #[error(\"failed\")] Failed }}`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0001"
            );
        }
    };
};
