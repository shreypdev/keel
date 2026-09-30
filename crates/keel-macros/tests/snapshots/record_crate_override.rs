pub struct Point {
    pub x: f64,
    pub y: f64,
}
impl Point {
    /// The stable Keel type id: `fnv1a32` of the type name.
    pub const KEEL_TYPE_ID: u32 = ::keel_runtime::meta::ids::type_id("Point");
}
#[automatically_derived]
impl ::keel_runtime::wire::Encode for Point {
    #[allow(unused_variables)]
    fn encode(&self, __w: &mut ::keel_runtime::wire::Writer) {
        ::keel_runtime::wire::Encode::encode(&self.x, __w);
        ::keel_runtime::wire::Encode::encode(&self.y, __w);
    }
}
#[automatically_derived]
impl ::keel_runtime::wire::Decode for Point {
    const MIN_ENCODED_LEN: usize = 0usize
        + <f64 as ::keel_runtime::wire::Decode>::MIN_ENCODED_LEN
        + <f64 as ::keel_runtime::wire::Decode>::MIN_ENCODED_LEN;
    #[allow(unused_variables)]
    fn decode(
        __r: &mut ::keel_runtime::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::keel_runtime::wire::WireError> {
        ::core::result::Result::Ok(Self {
            x: <f64 as ::keel_runtime::wire::Decode>::decode(__r)?,
            y: <f64 as ::keel_runtime::wire::Decode>::decode(__r)?,
        })
    }
}
#[allow(non_upper_case_globals)]
static __KEEL_META_Point: ::keel_runtime::meta::RecordMeta = ::keel_runtime::meta::RecordMeta {
    name: "Point",
    type_id: ::keel_runtime::meta::ids::type_id("Point"),
    fields: &[
        ::keel_runtime::meta::FieldMeta {
            name: "x",
            ty: ::keel_runtime::meta::TypeRefMeta::F64,
            default: false,
            docs: "",
        },
        ::keel_runtime::meta::FieldMeta {
            name: "y",
            ty: ::keel_runtime::meta::TypeRefMeta::F64,
            default: false,
            docs: "",
        },
    ],
    docs: "",
};
::keel_runtime::meta::inventory::submit! {
    ::keel_runtime::meta::Registration::Record(& __KEEL_META_Point)
}
