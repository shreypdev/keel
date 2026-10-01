/// A shape.
pub enum Shape {
    /// Nothing.
    Empty,
    Circle { radius: f64 },
    Rect(f64, f64),
}
impl Shape {
    /// The stable Undra type id: `fnv1a32` of the type name.
    pub const UNDRA_TYPE_ID: u32 = ::undra::meta::ids::type_id("Shape");
    /// Whether this is a `#[undra::error]` enum (what a `Result` may throw).
    #[doc(hidden)]
    pub const UNDRA_IS_ERROR: bool = false;
}
#[automatically_derived]
impl ::undra::wire::Encode for Shape {
    fn encode(&self, __w: &mut ::undra::wire::Writer) {
        match self {
            Self::Empty => {
                __w.write_u16(0u16);
            }
            Self::Circle { radius: __f0 } => {
                __w.write_u16(1u16);
                ::undra::wire::Encode::encode(__f0, __w);
            }
            Self::Rect(__f0, __f1) => {
                __w.write_u16(2u16);
                ::undra::wire::Encode::encode(__f0, __w);
                ::undra::wire::Encode::encode(__f1, __w);
            }
        }
    }
}
#[automatically_derived]
impl ::undra::wire::Decode for Shape {
    const MIN_ENCODED_LEN: usize = 2;
    fn decode(
        __r: &mut ::undra::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::undra::wire::WireError> {
        let __at = __r.position();
        match __r.read_u16()? {
            0u16 => ::core::result::Result::Ok(Self::Empty),
            1u16 => {
                ::core::result::Result::Ok(Self::Circle {
                    radius: <f64 as ::undra::wire::Decode>::decode(__r)?,
                })
            }
            2u16 => {
                ::core::result::Result::Ok(
                    Self::Rect(
                        <f64 as ::undra::wire::Decode>::decode(__r)?,
                        <f64 as ::undra::wire::Decode>::decode(__r)?,
                    ),
                )
            }
            __tag => {
                ::core::result::Result::Err(::undra::wire::WireError::InvalidTag {
                    tag: ::core::primitive::u32::from(__tag),
                    at: __at,
                    ty: "Shape",
                })
            }
        }
    }
}
#[allow(non_upper_case_globals)]
static __UNDRA_META_Shape: ::undra::meta::EnumMeta = ::undra::meta::EnumMeta {
    name: "Shape",
    type_id: ::undra::meta::ids::type_id("Shape"),
    is_error: false,
    variants: &[
        ::undra::meta::VariantMeta {
            name: "Empty",
            index: 0u16,
            fields: &[],
            tuple: false,
            message: ::core::option::Option::None,
            docs: "Nothing.",
        },
        ::undra::meta::VariantMeta {
            name: "Circle",
            index: 1u16,
            fields: &[
                ::undra::meta::FieldMeta {
                    name: "radius",
                    ty: ::undra::meta::TypeRefMeta::F64,
                    default: false,
                    docs: "",
                },
            ],
            tuple: false,
            message: ::core::option::Option::None,
            docs: "",
        },
        ::undra::meta::VariantMeta {
            name: "Rect",
            index: 2u16,
            fields: &[
                ::undra::meta::FieldMeta {
                    name: "0",
                    ty: ::undra::meta::TypeRefMeta::F64,
                    default: false,
                    docs: "",
                },
                ::undra::meta::FieldMeta {
                    name: "1",
                    ty: ::undra::meta::TypeRefMeta::F64,
                    default: false,
                    docs: "",
                },
            ],
            tuple: true,
            message: ::core::option::Option::None,
            docs: "",
        },
    ],
    docs: "A shape.",
};
::undra::meta::inventory::submit! {
    ::undra::meta::Registration::Enum(& __UNDRA_META_Shape)
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
