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
        let __args = {
            let mut __w = ::keel::wire::Writer::new();
            ::keel::wire::Encode::encode(&req, &mut __w);
            __w.into_vec()
        };
        let __ctx = ::core::clone::Clone::clone(&self.0);
        ::std::boxed::Box::pin(async move {
            let __reply = __ctx
                .port_call(
                    ::keel::meta::ids::port_id("Http"),
                    ::keel::meta::ids::port_method_id("Http", "request"),
                    __args,
                )
                .await;
            match __reply {
                ::core::result::Result::Ok(__bytes) => {
                    ::core::result::Result::Ok(
                        match <HttpResponse as ::keel::wire::Decode>::decode_exact(
                            &__bytes,
                        ) {
                            ::core::result::Result::Ok(__value) => __value,
                            ::core::result::Result::Err(__error) => {
                                ::core::panic!(
                                    "keel: port `{}.{}` replied with a value that does not decode: {:?}",
                                    "Http", "request", __error,
                                )
                            }
                        },
                    )
                }
                ::core::result::Result::Err(
                    ::keel::runtime::PortError::Failed(__bytes),
                ) => {
                    ::core::result::Result::Err(
                        match <HttpError as ::keel::wire::Decode>::decode_exact(
                            &__bytes,
                        ) {
                            ::core::result::Result::Ok(__value) => __value,
                            ::core::result::Result::Err(__error) => {
                                ::core::panic!(
                                    "keel: port `{}.{}` replied with a value that does not decode: {:?}",
                                    "Http", "request", __error,
                                )
                            }
                        },
                    )
                }
                ::core::result::Result::Err(__error) => {
                    ::core::panic!(
                        "keel: port call `{}.{}` failed: {:?}", "Http", "request",
                        __error,
                    )
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
        let __args = {
            let mut __w = ::keel::wire::Writer::new();
            ::keel::wire::Encode::encode(&url, &mut __w);
            __w.into_vec()
        };
        let __ctx = ::core::clone::Clone::clone(&self.0);
        ::std::boxed::Box::pin(async move {
            let __reply = __ctx
                .port_call(
                    ::keel::meta::ids::port_id("Http"),
                    ::keel::meta::ids::port_method_id("Http", "ping"),
                    __args,
                )
                .await;
            match __reply {
                ::core::result::Result::Ok(__bytes) => {
                    match <bool as ::keel::wire::Decode>::decode_exact(&__bytes) {
                        ::core::result::Result::Ok(__value) => __value,
                        ::core::result::Result::Err(__error) => {
                            ::core::panic!(
                                "keel: port `{}.{}` replied with a value that does not decode: {:?}",
                                "Http", "ping", __error,
                            )
                        }
                    }
                }
                ::core::result::Result::Err(__error) => {
                    ::core::panic!(
                        "keel: port call `{}.{}` failed: {:?}", "Http", "ping", __error,
                    )
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
            let req: HttpRequest = match <HttpRequest as ::keel::wire::Decode>::decode(
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
                        match __imp.request(req).await {
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
            let url: String = match <String as ::keel::wire::Decode>::decode(&mut __r) {
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
                        let __v = __imp.ping(url).await;
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
