pub trait Connectivity: ::core::marker::Send + ::core::marker::Sync {
    fn changed(&self, online: bool, kind: NetKind);
}
#[automatically_derived]
impl ::undra::runtime::Port for dyn Connectivity {
    const PORT_ID: u32 = ::undra::meta::ids::port_id("Connectivity");
    const NAME: &'static str = "Connectivity";
    const KIND: ::undra::meta::PortKind = ::undra::meta::PortKind::Event;
}
#[allow(non_upper_case_globals)]
static __UNDRA_META_port_Connectivity: ::undra::meta::PortMeta = ::undra::meta::PortMeta {
    name: "Connectivity",
    port_id: ::undra::meta::ids::port_id("Connectivity"),
    kind: ::undra::meta::PortKind::Event,
    methods: &[
        ::undra::meta::MethodMeta {
            name: "changed",
            method_id: ::undra::meta::ids::port_method_id("Connectivity", "changed"),
            params: &[
                ::undra::meta::ParamMeta {
                    name: "online",
                    ty: ::undra::meta::TypeRefMeta::Bool,
                },
                ::undra::meta::ParamMeta {
                    name: "kind",
                    ty: ::undra::meta::TypeRefMeta::Named("NetKind"),
                },
            ],
            returns: ::undra::meta::TypeRefMeta::Unit,
            is_async: false,
            takes_ctx: false,
            docs: "",
        },
    ],
    docs: "",
};
::undra::meta::inventory::submit! {
    ::undra::meta::Registration::Port(& __UNDRA_META_port_Connectivity)
}
///Calls `f` with the runtime's `Ctx` and the decoded arguments of every `Connectivity.changed` event. Use the `Ctx` it is given: a captured one would keep the runtime alive (ADR-034).
pub fn on_connectivity_changed(
    ctx: &::undra::runtime::Ctx,
    f: impl ::core::ops::Fn(
        &::undra::runtime::Ctx,
        bool,
        NetKind,
    ) + ::core::marker::Send + ::core::marker::Sync + 'static,
) -> ::undra::runtime::Subscription {
    ctx.events()
        .subscribe(
            <dyn Connectivity as ::undra::runtime::Port>::PORT_ID,
            ::undra::meta::ids::port_method_id("Connectivity", "changed"),
            ::std::boxed::Box::new(move |
                __ctx: &::undra::runtime::Ctx,
                __payload: &[u8]|
            {
                let mut __r = ::undra::wire::Reader::new(__payload);
                let __a0: bool = match <bool as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(_) => return,
                };
                let __a1: NetKind = match <NetKind as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(_) => return,
                };
                if __r.finish().is_err() {
                    return;
                }
                f(__ctx, __a0, __a1)
            }),
        )
}
///Encodes the payload of a `Connectivity.changed` event, as the host would send it to `Runtime::event`.
pub fn encode_connectivity_changed_event(
    online: bool,
    kind: NetKind,
) -> ::std::vec::Vec<u8> {
    let mut __undra_w = ::undra::wire::Writer::new();
    ::undra::wire::Encode::encode(&online, &mut __undra_w);
    ::undra::wire::Encode::encode(&kind, &mut __undra_w);
    __undra_w.into_vec()
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
        __undra_same::<bool, ::core::primitive::bool>();
    }
    trait __UndraFallback {
        const UNDRA_TYPE_ID: u32 = 0;
        const UNDRA_IS_ERROR: bool = false;
        const __UNDRA_IS_OBJECT: bool = false;
    }
    impl<T: ?::core::marker::Sized> __UndraFallback for T {}
    const _: () = {
        if <NetKind>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0064]: `NetKind` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a parameter or a return value\n  = help: return a record with the data the platform needs, or construct the object from the platform with one of its constructors\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0064"
            );
        }
        if <NetKind>::UNDRA_TYPE_ID != ::undra::meta::ids::type_id("NetKind") {
            ::core::panic!(
                "error[undra::E0061]: the schema records this type as `NetKind`, but the type written here is not that type\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type the name resolves to; an alias (`type NetKind = Other`), a renamed import (`use path::Other as NetKind`) or a type that is not declared with `#[undra::api]` makes the two differ, so the platforms would read the wrong layout\n  = help: write the type under its declared name (for an alias or a renamed import, use `Other` here), or declare it with `#[undra::api]` (`#[undra::error]` for errors)\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
    };
};
