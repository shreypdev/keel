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
        let __args = {
            let mut __w = ::keel::wire::Writer::new();
            __w.into_vec()
        };
        let __reply = self
            .0
            .port_call_sync(
                ::keel::meta::ids::port_id("Clock"),
                ::keel::meta::ids::port_method_id("Clock", "now_ms"),
                &__args,
            );
        match __reply {
            ::core::result::Result::Ok(__bytes) => {
                match <i64 as ::keel::wire::Decode>::decode_exact(&__bytes) {
                    ::core::result::Result::Ok(__value) => __value,
                    ::core::result::Result::Err(__error) => {
                        ::core::panic!(
                            "keel: port `{}.{}` replied with a value that does not decode: {:?}",
                            "Clock", "now_ms", __error,
                        )
                    }
                }
            }
            ::core::result::Result::Err(__error) => {
                ::core::panic!(
                    "keel: port call `{}.{}` failed: {:?}", "Clock", "now_ms", __error,
                )
            }
        }
    }
    fn log(&self, level: u8, message: String) {
        let __args = {
            let mut __w = ::keel::wire::Writer::new();
            ::keel::wire::Encode::encode(&level, &mut __w);
            ::keel::wire::Encode::encode(&message, &mut __w);
            __w.into_vec()
        };
        let __reply = self
            .0
            .port_call_sync(
                ::keel::meta::ids::port_id("Clock"),
                ::keel::meta::ids::port_method_id("Clock", "log"),
                &__args,
            );
        match __reply {
            ::core::result::Result::Ok(_) => {}
            ::core::result::Result::Err(__error) => {
                ::core::panic!(
                    "keel: port call `{}.{}` failed: {:?}", "Clock", "log", __error,
                )
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
            let level: u8 = match <u8 as ::keel::wire::Decode>::decode(&mut __r) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(_) => {
                    return ::keel::runtime::PortDispatch::Sync(::std::vec![2u8]);
                }
            };
            let message: String = match <String as ::keel::wire::Decode>::decode(
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
                __imp.log(level, message);
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
