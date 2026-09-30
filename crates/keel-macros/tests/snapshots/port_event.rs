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
    let mut __w = ::keel::wire::Writer::new();
    ::keel::wire::Encode::encode(&online, &mut __w);
    ::keel::wire::Encode::encode(&kind, &mut __w);
    __w.into_vec()
}
