pub struct Point {
    pub x: f64,
    pub y: f64,
}
impl Point {
    /// The stable Undra type id: `fnv1a32` of the type name.
    pub const UNDRA_TYPE_ID: u32 = ::undra_runtime::meta::ids::type_id("Point");
    /// The names of the fields, in declaration order (see `undra_meta::keys`).
    #[doc(hidden)]
    pub const __UNDRA_FIELDS: &'static [&'static str] = &["x", "y"];
    /// Encodes the field at index `__I` of `__UNDRA_FIELDS`, as `Encode` does.
    #[doc(hidden)]
    #[inline(always)]
    #[allow(unused_variables)]
    pub fn __undra_encode_field<const __I: usize>(
        &self,
        __w: &mut ::undra_runtime::wire::Writer,
    ) {
        match __I {
            0usize => ::undra_runtime::wire::Encode::encode(&self.x, __w),
            1usize => ::undra_runtime::wire::Encode::encode(&self.y, __w),
            _ => {}
        }
    }
}
#[automatically_derived]
impl ::undra_runtime::wire::Encode for Point {
    #[allow(unused_variables)]
    fn encode(&self, __w: &mut ::undra_runtime::wire::Writer) {
        ::undra_runtime::wire::Encode::encode(&self.x, __w);
        ::undra_runtime::wire::Encode::encode(&self.y, __w);
    }
}
#[automatically_derived]
impl ::undra_runtime::wire::Decode for Point {
    const MIN_ENCODED_LEN: usize = 0usize
        + <f64 as ::undra_runtime::wire::Decode>::MIN_ENCODED_LEN
        + <f64 as ::undra_runtime::wire::Decode>::MIN_ENCODED_LEN;
    #[allow(unused_variables)]
    fn decode(
        __r: &mut ::undra_runtime::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::undra_runtime::wire::WireError> {
        ::core::result::Result::Ok(Self {
            x: <f64 as ::undra_runtime::wire::Decode>::decode(__r)?,
            y: <f64 as ::undra_runtime::wire::Decode>::decode(__r)?,
        })
    }
}
#[allow(non_upper_case_globals)]
static __UNDRA_META_Point: ::undra_runtime::meta::RecordMeta = ::undra_runtime::meta::RecordMeta {
    name: "Point",
    type_id: ::undra_runtime::meta::ids::type_id("Point"),
    fields: &[
        ::undra_runtime::meta::FieldMeta {
            name: "x",
            ty: ::undra_runtime::meta::TypeRefMeta::F64,
            default: false,
            docs: "",
        },
        ::undra_runtime::meta::FieldMeta {
            name: "y",
            ty: ::undra_runtime::meta::TypeRefMeta::F64,
            default: false,
            docs: "",
        },
    ],
    docs: "",
};
::undra_runtime::meta::inventory::submit! {
    ::undra_runtime::meta::Registration::Record(& __UNDRA_META_Point)
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
        __undra_same::<f64, ::core::primitive::f64>();
    }
};
