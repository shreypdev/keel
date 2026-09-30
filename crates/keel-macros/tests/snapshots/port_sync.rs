pub trait Clock: ::core::marker::Send + ::core::marker::Sync {
    fn now_ms(&self) -> i64;
    fn log(&self, level: u8, message: String);
}
#[automatically_derived]
impl ::keel::runtime::Port for dyn Clock {
    const PORT_ID: u32 = ::keel::meta::ids::port_id("Clock");
    const NAME: &'static str = "Clock";
    const KIND: ::keel::meta::PortKind = ::keel::meta::PortKind::Sync;
}
#[allow(non_upper_case_globals)]
static __KEEL_META_port_Clock: ::keel::meta::PortMeta = ::keel::meta::PortMeta {
    name: "Clock",
    port_id: ::keel::meta::ids::port_id("Clock"),
    kind: ::keel::meta::PortKind::Sync,
    methods: &[
        ::keel::meta::MethodMeta {
            name: "now_ms",
            method_id: ::keel::meta::ids::port_method_id("Clock", "now_ms"),
            params: &[],
            returns: ::keel::meta::TypeRefMeta::I64,
            is_async: false,
            takes_ctx: false,
            docs: "",
        },
        ::keel::meta::MethodMeta {
            name: "log",
            method_id: ::keel::meta::ids::port_method_id("Clock", "log"),
            params: &[
                ::keel::meta::ParamMeta {
                    name: "level",
                    ty: ::keel::meta::TypeRefMeta::U8,
                },
                ::keel::meta::ParamMeta {
                    name: "message",
                    ty: ::keel::meta::TypeRefMeta::String,
                },
            ],
            returns: ::keel::meta::TypeRefMeta::Unit,
            is_async: false,
            takes_ctx: false,
            docs: "",
        },
    ],
    docs: "",
};
::keel::meta::inventory::submit! {
    ::keel::meta::Registration::Port(& __KEEL_META_port_Clock)
}
#[doc(hidden)]
#[cold]
#[inline(never)]
#[allow(non_snake_case, dead_code)]
fn __keel_port_failure_Clock(
    __keel_method: &str,
    __keel_error: ::keel::runtime::PortError,
) -> ! {
    let (__keel_what, __keel_how) = match &__keel_error {
        ::keel::runtime::PortError::Unavailable => {
            (
                ::std::format!(
                    "the `{}` port has no adapter registered (method `{}`)", "Clock",
                    __keel_method,
                ),
                "Register one with core.registerPort(..) (TypeScript, Kotlin, Swift) / keel_port_register (C), or bind a Rust implementation (`keel::ports::fakes` in tests)",
            )
        }
        ::keel::runtime::PortError::Cancelled => {
            (
                ::std::format!(
                    "a call to the `{}` port (method `{}`) was cancelled", "Clock",
                    __keel_method,
                ),
                "A method without an error type cannot report an abandoned call; give it a `Result<T, E>` return type",
            )
        }
        ::keel::runtime::PortError::Decode(__keel_why) => {
            (
                ::std::format!(
                    "the `{}` port (method `{}`) replied with bytes that do not decode: {}",
                    "Clock", __keel_method, __keel_why,
                ),
                "The adapter's reply does not match the schema; check its codec for this method",
            )
        }
        __keel_other => {
            (
                ::std::format!(
                    "a call to the `{}` port (method `{}`) failed: {}", "Clock",
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
///Calls the `Clock` port through the runtime's port table: the platform's binding, or a Rust fake.
pub struct ClockProxy(::keel::runtime::Ctx);
impl ClockProxy {
    /// Creates a proxy that calls through `ctx`.
    pub fn new(ctx: ::keel::runtime::Ctx) -> Self {
        Self(ctx)
    }
}
#[automatically_derived]
impl Clock for ClockProxy {
    fn now_ms(&self) -> i64 {
        let __keel_args = {
            let mut __keel_w = ::keel::wire::Writer::new();
            __keel_w.into_vec()
        };
        let __keel_reply = self
            .0
            .port_call_sync(
                ::keel::meta::ids::port_id("Clock"),
                ::keel::meta::ids::port_method_id("Clock", "now_ms"),
                &__keel_args,
            );
        match __keel_reply {
            ::core::result::Result::Ok(__keel_bytes) => {
                match <i64 as ::keel::wire::Decode>::decode_exact(&__keel_bytes) {
                    ::core::result::Result::Ok(__keel_value) => __keel_value,
                    ::core::result::Result::Err(__keel_error) => {
                        __keel_port_failure_Clock(
                            "now_ms",
                            ::keel::runtime::PortError::Decode(__keel_error),
                        )
                    }
                }
            }
            ::core::result::Result::Err(__keel_error) => {
                __keel_port_failure_Clock("now_ms", __keel_error)
            }
        }
    }
    fn log(&self, level: u8, message: String) {
        let __keel_args = {
            let mut __keel_w = ::keel::wire::Writer::new();
            ::keel::wire::Encode::encode(&level, &mut __keel_w);
            ::keel::wire::Encode::encode(&message, &mut __keel_w);
            __keel_w.into_vec()
        };
        let __keel_reply = self
            .0
            .port_call_sync(
                ::keel::meta::ids::port_id("Clock"),
                ::keel::meta::ids::port_method_id("Clock", "log"),
                &__keel_args,
            );
        match __keel_reply {
            ::core::result::Result::Ok(_) => {}
            ::core::result::Result::Err(__keel_error) => {
                __keel_port_failure_Clock("log", __keel_error)
            }
        }
    }
}
///The Rust binding of the `Clock` port if one is bound (fakes, built-ins), otherwise a proxy to the platform's binding.
pub fn clock(ctx: &::keel::runtime::Ctx) -> ::std::sync::Arc<dyn Clock> {
    match ctx.rust_port::<dyn Clock>(<dyn Clock as ::keel::runtime::Port>::PORT_ID) {
        ::core::option::Option::Some(__imp) => __imp,
        ::core::option::Option::None => {
            ::std::sync::Arc::new(ClockProxy::new(::core::clone::Clone::clone(ctx)))
        }
    }
}
///Runs an encoded `Clock` call on a Rust implementation. The reply is `status u8` (0 ok, 1 typed error, 2 unavailable) followed by the body.
#[allow(
    non_snake_case,
    non_upper_case_globals,
    unused_mut,
    unused_variables,
    clippy::all
)]
pub fn __keel_port_dispatch_Clock(
    __imp: &::std::sync::Arc<dyn Clock>,
    __method_id: u32,
    __args: &[u8],
) -> ::keel::runtime::PortDispatch {
    const __KEEL_ID_now_ms: u32 = ::keel::meta::ids::port_method_id("Clock", "now_ms");
    const __KEEL_ID_log: u32 = ::keel::meta::ids::port_method_id("Clock", "log");
    match __method_id {
        __KEEL_ID_now_ms => {
            let mut __r = ::keel::wire::Reader::new(__args);
            if __r.finish().is_err() {
                return ::keel::runtime::PortDispatch::Sync(::std::vec![2u8]);
            }
            {
                let __v = __imp.now_ms();
                ::keel::runtime::PortDispatch::Sync({
                    let mut __w = ::keel::wire::Writer::new();
                    __w.write_u8(0u8);
                    ::keel::wire::Encode::encode(&__v, &mut __w);
                    __w.into_vec()
                })
            }
        }
        __KEEL_ID_log => {
            let mut __r = ::keel::wire::Reader::new(__args);
            let __keel_a0: u8 = match <u8 as ::keel::wire::Decode>::decode(&mut __r) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(_) => {
                    return ::keel::runtime::PortDispatch::Sync(::std::vec![2u8]);
                }
            };
            let __keel_a1: String = match <String as ::keel::wire::Decode>::decode(
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
                __imp.log(__keel_a0, __keel_a1);
                ::keel::runtime::PortDispatch::Sync(::std::vec![0u8])
            }
        }
        _ => ::keel::runtime::PortDispatch::Sync(::std::vec![2u8]),
    }
}
#[doc(hidden)]
#[allow(non_snake_case)]
fn __keel_port_dispatch_erased_Clock(
    __imp: &(dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync),
    __method_id: u32,
    __args: &[u8],
) -> ::keel::runtime::PortDispatch {
    match __imp.downcast_ref::<::std::sync::Arc<dyn Clock>>() {
        ::core::option::Option::Some(__imp) => {
            __keel_port_dispatch_Clock(__imp, __method_id, __args)
        }
        ::core::option::Option::None => {
            ::keel::runtime::PortDispatch::Sync(::std::vec![2u8])
        }
    }
}
::keel::meta::inventory::submit! {
    ::keel::runtime::PortDispatcher { port_id : ::keel::meta::ids::port_id("Clock"),
    dispatch : __keel_port_dispatch_erased_Clock, }
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
        __keel_same::<i64, ::core::primitive::i64>();
        __keel_same::<u8, ::core::primitive::u8>();
        __keel_same::<String, ::std::string::String>();
    }
};
