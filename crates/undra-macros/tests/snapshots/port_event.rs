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
///Calls `f` with the decoded arguments of every `Connectivity.changed` event.
pub fn on_connectivity_changed(
    ctx: &::undra::runtime::Ctx,
    f: impl ::core::ops::Fn(
        bool,
        NetKind,
    ) + ::core::marker::Send + ::core::marker::Sync + 'static,
) -> ::undra::runtime::Subscription {
    ctx.events()
        .subscribe(
            <dyn Connectivity as ::undra::runtime::Port>::PORT_ID,
            ::undra::meta::ids::port_method_id("Connectivity", "changed"),
            ::std::boxed::Box::new(move |__payload: &[u8]| {
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
                f(__a0, __a1)
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
        let __undra_id = <NetKind>::UNDRA_TYPE_ID;
        if __undra_id == 0 {
            ::core::panic!(
                "error[undra::E0061]: `NetKind` is not a type declared with `#[undra::api]`\n  = note: Undra describes a type to the platforms by the name it is written with, so the name must be a record or enum declared with `#[undra::api]` or an error declared with `#[undra::error]`; anything else, such as a plain struct or an alias (`type Id = u64`), has no definition the platforms could generate\n  = help: add `#[undra::api]` to `NetKind`, or, if it is an alias, write the type it stands for where it is used\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if __undra_id != ::undra::meta::ids::type_id("NetKind") {
            ::core::panic!(
                "error[undra::E0061]: `NetKind` here is an alias or a renamed import of an Undra type that is declared under another name\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type that name resolves to; with `type NetKind = Other` or `use path::Other as NetKind` the platforms would be told `NetKind` and receive the layout of `Other`\n  = help: write the type under the name it is declared with (`Other` in the examples above), or declare a separate `#[undra::api] struct NetKind` if you mean a distinct type\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
    };
};
