/// A user's id.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserId(pub Uuid);
impl UserId {
    /// The stable Undra type id: `fnv1a32` of the type name.
    pub const UNDRA_TYPE_ID: u32 = ::undra::meta::ids::type_id("UserId");
    /// The names of the fields, in declaration order (see `undra_meta::keys`).
    #[doc(hidden)]
    pub const __UNDRA_FIELDS: &'static [&'static str] = &[];
}
#[automatically_derived]
impl ::undra::wire::Encode for UserId {
    #[allow(unused_variables)]
    fn encode(&self, __w: &mut ::undra::wire::Writer) {
        ::undra::wire::Encode::encode(&self.0, __w);
    }
}
#[automatically_derived]
impl ::undra::wire::Decode for UserId {
    const MIN_ENCODED_LEN: usize = <Uuid as ::undra::wire::Decode>::MIN_ENCODED_LEN;
    #[allow(unused_variables)]
    fn decode(
        __r: &mut ::undra::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::undra::wire::WireError> {
        ::core::result::Result::Ok(Self(<Uuid as ::undra::wire::Decode>::decode(__r)?))
    }
}
#[automatically_derived]
impl ::undra::wire::leaf::Newtype for UserId {
    type Inner = Uuid;
}
#[allow(non_upper_case_globals)]
static __UNDRA_META_UserId: ::undra::meta::RecordMeta = ::undra::meta::RecordMeta {
    name: "UserId",
    type_id: ::undra::meta::ids::type_id("UserId"),
    fields: &[
        ::undra::meta::FieldMeta {
            name: "value",
            ty: ::undra::meta::TypeRefMeta::Uuid,
            default: false,
            docs: "",
        },
    ],
    transparent: true,
    docs: "A user's id.",
};
::undra::meta::inventory::submit! {
    ::undra::meta::Registration::Record(& __UNDRA_META_UserId)
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code, unused, unused_braces, clippy::all)]
const _: () = {
    fn __undra_leaf<A, K>()
    where
        A: ?::core::marker::Sized + ::undra::wire::leaf::WireLeaf<K>,
    {}
    fn __undra_identity() {
        __undra_leaf::<Uuid, ::undra::wire::leaf::kinds::Uuid>();
    }
};
