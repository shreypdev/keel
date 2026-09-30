/// A shape.
pub enum Shape {
    /// Nothing.
    Empty,
    Circle { radius: f64 },
    Rect(f64, f64),
}
impl Shape {
    /// The stable Keel type id: `fnv1a32` of the type name.
    pub const KEEL_TYPE_ID: u32 = ::keel::meta::ids::type_id("Shape");
}
#[automatically_derived]
impl ::keel::wire::Encode for Shape {
    fn encode(&self, __w: &mut ::keel::wire::Writer) {
        match self {
            Self::Empty => {
                __w.write_u16(0u16);
            }
            Self::Circle { radius: __f0 } => {
                __w.write_u16(1u16);
                ::keel::wire::Encode::encode(__f0, __w);
            }
            Self::Rect(__f0, __f1) => {
                __w.write_u16(2u16);
                ::keel::wire::Encode::encode(__f0, __w);
                ::keel::wire::Encode::encode(__f1, __w);
            }
        }
    }
}
#[automatically_derived]
impl ::keel::wire::Decode for Shape {
    const MIN_ENCODED_LEN: usize = 2;
    fn decode(
        __r: &mut ::keel::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::keel::wire::WireError> {
        let __at = __r.position();
        match __r.read_u16()? {
            0u16 => ::core::result::Result::Ok(Self::Empty),
            1u16 => {
                ::core::result::Result::Ok(Self::Circle {
                    radius: <f64 as ::keel::wire::Decode>::decode(__r)?,
                })
            }
            2u16 => {
                ::core::result::Result::Ok(
                    Self::Rect(
                        <f64 as ::keel::wire::Decode>::decode(__r)?,
                        <f64 as ::keel::wire::Decode>::decode(__r)?,
                    ),
                )
            }
            __tag => {
                ::core::result::Result::Err(::keel::wire::WireError::InvalidTag {
                    tag: ::core::primitive::u32::from(__tag),
                    at: __at,
                    ty: "Shape",
                })
            }
        }
    }
}
#[allow(non_upper_case_globals)]
static __KEEL_META_Shape: ::keel::meta::EnumMeta = ::keel::meta::EnumMeta {
    name: "Shape",
    type_id: ::keel::meta::ids::type_id("Shape"),
    is_error: false,
    variants: &[
        ::keel::meta::VariantMeta {
            name: "Empty",
            index: 0u16,
            fields: &[],
            tuple: false,
            message: ::core::option::Option::None,
            docs: "Nothing.",
        },
        ::keel::meta::VariantMeta {
            name: "Circle",
            index: 1u16,
            fields: &[
                ::keel::meta::FieldMeta {
                    name: "radius",
                    ty: ::keel::meta::TypeRefMeta::F64,
                    default: false,
                    docs: "",
                },
            ],
            tuple: false,
            message: ::core::option::Option::None,
            docs: "",
        },
        ::keel::meta::VariantMeta {
            name: "Rect",
            index: 2u16,
            fields: &[
                ::keel::meta::FieldMeta {
                    name: "0",
                    ty: ::keel::meta::TypeRefMeta::F64,
                    default: false,
                    docs: "",
                },
                ::keel::meta::FieldMeta {
                    name: "1",
                    ty: ::keel::meta::TypeRefMeta::F64,
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
::keel::meta::inventory::submit! {
    ::keel::meta::Registration::Enum(& __KEEL_META_Shape)
}
