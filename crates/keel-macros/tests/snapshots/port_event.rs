pub trait Connectivity: ::core::marker::Send + ::core::marker::Sync {
    fn changed(&self, online: bool, kind: NetKind);
}
#[automatically_derived]
impl ::keel::runtime::Port for dyn Connectivity {
    const PORT_ID: u32 = ::keel::meta::ids::port_id("Connectivity");
    const NAME: &'static str = "Connectivity";
    const KIND: ::keel::meta::PortKind = ::keel::meta::PortKind::Event;
}
#[allow(non_upper_case_globals)]
static __KEEL_META_port_Connectivity: ::keel::meta::PortMeta = ::keel::meta::PortMeta {
    name: "Connectivity",
    port_id: ::keel::meta::ids::port_id("Connectivity"),
    kind: ::keel::meta::PortKind::Event,
    methods: &[
        ::keel::meta::MethodMeta {
            name: "changed",
            method_id: ::keel::meta::ids::port_method_id("Connectivity", "changed"),
            params: &[
                ::keel::meta::ParamMeta {
                    name: "online",
                    ty: ::keel::meta::TypeRefMeta::Bool,
                },
                ::keel::meta::ParamMeta {
                    name: "kind",
                    ty: ::keel::meta::TypeRefMeta::Named("NetKind"),
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
    ::keel::meta::Registration::Port(& __KEEL_META_port_Connectivity)
}
///Calls `f` with the decoded arguments of every `Connectivity.changed` event.
pub fn on_connectivity_changed(
    ctx: &::keel::runtime::Ctx,
    f: impl ::core::ops::Fn(
        bool,
        NetKind,
    ) + ::core::marker::Send + ::core::marker::Sync + 'static,
) -> ::keel::runtime::Subscription {
    ctx.events()
        .subscribe(
            <dyn Connectivity as ::keel::runtime::Port>::PORT_ID,
            ::keel::meta::ids::port_method_id("Connectivity", "changed"),
            ::std::boxed::Box::new(move |__payload: &[u8]| {
                let mut __r = ::keel::wire::Reader::new(__payload);
                let __a0: bool = match <bool as ::keel::wire::Decode>::decode(&mut __r) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(_) => return,
                };
                let __a1: NetKind = match <NetKind as ::keel::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(_) => return,
                };
                if __r.finish().is_err() {
                    return;
                }
                f(__a0, __a1)
            }),
        )
}
///Encodes the payload of a `Connectivity.changed` event, as the host would send it to `Runtime::event`.
pub fn encode_connectivity_changed_event(
    online: bool,
    kind: NetKind,
) -> ::std::vec::Vec<u8> {
    let mut __keel_w = ::keel::wire::Writer::new();
    ::keel::wire::Encode::encode(&online, &mut __keel_w);
    ::keel::wire::Encode::encode(&kind, &mut __keel_w);
    __keel_w.into_vec()
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
        __keel_same::<bool, ::core::primitive::bool>();
    }
    trait __KeelFallback {
        const KEEL_TYPE_ID: u32 = 0;
        const KEEL_IS_ERROR: bool = false;
        const __KEEL_IS_OBJECT: bool = false;
    }
    impl<T: ?::core::marker::Sized> __KeelFallback for T {}
    const _: () = {
        if <NetKind>::__KEEL_IS_OBJECT {
            ::core::panic!(
                "error[keel::E0064]: `NetKind` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a parameter or a return value\n  = help: return a record with the data the platform needs, or construct the object from the platform with one of its constructors\n  = docs: https://keel.dev/errors/E0064"
            );
        }
        if <NetKind>::KEEL_TYPE_ID != ::keel::meta::ids::type_id("NetKind") {
            ::core::panic!(
                "error[keel::E0061]: the schema records this type as `NetKind`, but the type written here is not that type\n  = note: Keel describes a type to the platforms by the name it is written with, while the generated code encodes the type the name resolves to; an alias (`type NetKind = Other`), a renamed import (`use path::Other as NetKind`) or a type that is not declared with `#[keel::api]` makes the two differ, so the platforms would read the wrong layout\n  = help: write the type under its declared name (for an alias or a renamed import, use `Other` here), or declare it with `#[keel::api]` (`#[keel::error]` for errors)\n  = docs: https://keel.dev/errors/E0061"
            );
        }
    };
};
