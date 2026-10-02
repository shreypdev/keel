pub trait Clock: ::core::marker::Send + ::core::marker::Sync {
    fn now_ms(&self) -> i64;
    fn log(&self, level: u8, message: String);
}
#[automatically_derived]
impl ::undra::runtime::Port for dyn Clock {
    const PORT_ID: u32 = ::undra::meta::ids::port_id("Clock");
    const NAME: &'static str = "Clock";
    const KIND: ::undra::meta::PortKind = ::undra::meta::PortKind::Sync;
}
#[allow(non_upper_case_globals)]
static __UNDRA_META_port_Clock: ::undra::meta::PortMeta = ::undra::meta::PortMeta {
    name: "Clock",
    port_id: ::undra::meta::ids::port_id("Clock"),
    kind: ::undra::meta::PortKind::Sync,
    background: false,
    methods: &[
        ::undra::meta::MethodMeta {
            name: "now_ms",
            method_id: ::undra::meta::ids::port_method_id("Clock", "now_ms"),
            params: &[],
            returns: ::undra::meta::TypeRefMeta::I64,
            is_async: false,
            takes_ctx: false,
            coalesce: false,
            generic: ::core::option::Option::None,
            docs: "",
        },
        ::undra::meta::MethodMeta {
            name: "log",
            method_id: ::undra::meta::ids::port_method_id("Clock", "log"),
            params: &[
                ::undra::meta::ParamMeta {
                    name: "level",
                    ty: ::undra::meta::TypeRefMeta::U8,
                },
                ::undra::meta::ParamMeta {
                    name: "message",
                    ty: ::undra::meta::TypeRefMeta::String,
                },
            ],
            returns: ::undra::meta::TypeRefMeta::Unit,
            is_async: false,
            takes_ctx: false,
            coalesce: false,
            generic: ::core::option::Option::None,
            docs: "",
        },
    ],
    docs: "",
};
::undra::meta::inventory::submit! {
    ::undra::meta::Registration::Port(& __UNDRA_META_port_Clock)
}
#[doc(hidden)]
#[cold]
#[inline(never)]
#[allow(non_snake_case, dead_code)]
fn __undra_port_failure_Clock(
    __undra_method: &str,
    __undra_error: ::undra::runtime::PortError,
) -> ! {
    let (__undra_what, __undra_why, __undra_how) = match &__undra_error {
        ::undra::runtime::PortError::Unavailable => {
            (
                ::std::format!(
                    "the `{}` port has no adapter registered (method `{}`)", "Clock",
                    __undra_method,
                ),
                "this method has no error channel, so an unavailable port cannot be reported and the call panics; the runtime contains the panic, but on the web it traps the core",
                "register an adapter (`core.registerPort(..)` in TypeScript, Kotlin and Swift, `undra_port_register` in C), bind a Rust implementation (`undra::ports::fakes` in tests), or give the method a `Result<T, E>` return type so it can report the outage",
            )
        }
        ::undra::runtime::PortError::Cancelled => {
            (
                ::std::format!(
                    "a call to the `{}` port (method `{}`) was cancelled", "Clock",
                    __undra_method,
                ),
                "this method has no error channel, so an abandoned call cannot be reported and the call panics; on the web that traps the core",
                "give the method a `Result<T, E>` return type so it can report a cancelled call",
            )
        }
        ::undra::runtime::PortError::Decode(__undra_why) => {
            (
                ::std::format!(
                    "the `{}` port (method `{}`) replied with bytes that do not decode: {}",
                    "Clock", __undra_method, __undra_why,
                ),
                "the adapter's reply does not match the schema, and this method has no error channel to report that, so the call panics; on the web that traps the core",
                "check the adapter's codec for this method against the schema, or give the method a `Result<T, E>` return type so it can report a bad reply",
            )
        }
        __undra_other => {
            (
                ::std::format!(
                    "a call to the `{}` port (method `{}`) failed: {}", "Clock",
                    __undra_method, __undra_other,
                ),
                "this method has no error channel, so a failed call cannot be reported and the call panics; on the web that traps the core",
                "give the method a `Result<T, E>` return type so it can report the failure",
            )
        }
    };
    ::core::panic!(
        "error[undra::E0062]: {}\n  = note: {}\n  = help: {}\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0062",
        __undra_what, __undra_why, __undra_how
    )
}
///Calls the `Clock` port through the runtime's port table: the platform's binding, or a Rust fake.
pub struct ClockProxy(::undra::runtime::Ctx);
impl ClockProxy {
    /// Creates a proxy that calls through `ctx`.
    pub fn new(ctx: ::undra::runtime::Ctx) -> Self {
        Self(ctx)
    }
}
#[automatically_derived]
impl Clock for ClockProxy {
    fn now_ms(&self) -> i64 {
        let __undra_args = {
            let mut __undra_w = ::undra::wire::Writer::new();
            __undra_w.into_vec()
        };
        let __undra_reply = self
            .0
            .port_call_sync(
                ::undra::meta::ids::port_id("Clock"),
                ::undra::meta::ids::port_method_id("Clock", "now_ms"),
                &__undra_args,
            );
        match __undra_reply {
            ::core::result::Result::Ok(__undra_bytes) => {
                match <i64 as ::undra::wire::Decode>::decode_exact(&__undra_bytes) {
                    ::core::result::Result::Ok(__undra_value) => __undra_value,
                    ::core::result::Result::Err(__undra_error) => {
                        __undra_port_failure_Clock(
                            "now_ms",
                            ::undra::runtime::PortError::Decode(__undra_error),
                        )
                    }
                }
            }
            ::core::result::Result::Err(__undra_error) => {
                __undra_port_failure_Clock("now_ms", __undra_error)
            }
        }
    }
    fn log(&self, level: u8, message: String) {
        let __undra_args = {
            let mut __undra_w = ::undra::wire::Writer::new();
            ::undra::wire::Encode::encode(&level, &mut __undra_w);
            ::undra::wire::Encode::encode(&message, &mut __undra_w);
            __undra_w.into_vec()
        };
        let __undra_reply = self
            .0
            .port_call_sync(
                ::undra::meta::ids::port_id("Clock"),
                ::undra::meta::ids::port_method_id("Clock", "log"),
                &__undra_args,
            );
        match __undra_reply {
            ::core::result::Result::Ok(_) => {}
            ::core::result::Result::Err(__undra_error) => {
                __undra_port_failure_Clock("log", __undra_error)
            }
        }
    }
}
///The Rust binding of the `Clock` port if one is bound (fakes, built-ins), otherwise a proxy to the platform's binding.
pub fn clock(ctx: &::undra::runtime::Ctx) -> ::std::sync::Arc<dyn Clock> {
    match ctx.rust_port::<dyn Clock>(<dyn Clock as ::undra::runtime::Port>::PORT_ID) {
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
pub fn __undra_port_dispatch_Clock(
    __imp: &::std::sync::Arc<dyn Clock>,
    __method_id: u32,
    __args: &[u8],
) -> ::undra::runtime::PortDispatch {
    const __UNDRA_ID_now_ms: u32 = ::undra::meta::ids::port_method_id("Clock", "now_ms");
    const __UNDRA_ID_log: u32 = ::undra::meta::ids::port_method_id("Clock", "log");
    match __method_id {
        __UNDRA_ID_now_ms => {
            let mut __r = ::undra::wire::Reader::new(__args);
            if __r.finish().is_err() {
                return ::undra::runtime::PortDispatch::Sync(::std::vec![2u8]);
            }
            {
                let __v = __imp.now_ms();
                ::undra::runtime::PortDispatch::Sync({
                    let mut __w = ::undra::wire::Writer::new();
                    __w.write_u8(0u8);
                    ::undra::wire::Encode::encode(&__v, &mut __w);
                    __w.into_vec()
                })
            }
        }
        __UNDRA_ID_log => {
            let mut __r = ::undra::wire::Reader::new(__args);
            let __undra_a0: u8 = match <u8 as ::undra::wire::Decode>::decode(&mut __r) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(_) => {
                    return ::undra::runtime::PortDispatch::Sync(::std::vec![2u8]);
                }
            };
            let __undra_a1: String = match <String as ::undra::wire::Decode>::decode(
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
                __imp.log(__undra_a0, __undra_a1);
                ::undra::runtime::PortDispatch::Sync(::std::vec![0u8])
            }
        }
        _ => ::undra::runtime::PortDispatch::Sync(::std::vec![2u8]),
    }
}
#[doc(hidden)]
#[allow(non_snake_case)]
fn __undra_port_dispatch_erased_Clock(
    __imp: &(dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync),
    __method_id: u32,
    __args: &[u8],
) -> ::undra::runtime::PortDispatch {
    match __imp.downcast_ref::<::std::sync::Arc<dyn Clock>>() {
        ::core::option::Option::Some(__imp) => {
            __undra_port_dispatch_Clock(__imp, __method_id, __args)
        }
        ::core::option::Option::None => {
            ::undra::runtime::PortDispatch::Sync(::std::vec![2u8])
        }
    }
}
::undra::meta::inventory::submit! {
    ::undra::runtime::PortDispatcher { port_id : ::undra::meta::ids::port_id("Clock"),
    dispatch : __undra_port_dispatch_erased_Clock, }
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
        __undra_same::<i64, ::core::primitive::i64>();
        __undra_same::<u8, ::core::primitive::u8>();
        __undra_same::<String, ::std::string::String>();
    }
};
