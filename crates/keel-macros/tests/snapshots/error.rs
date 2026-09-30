#[derive(Clone)]
#[derive(::core::fmt::Debug)]
pub enum TodoError {
    EmptyTitle,
    NotFound(Uuid),
    Rejected { code: u16, reason: String },
    Http(HttpError),
    Storage(StorageError),
}
impl TodoError {
    /// The stable Keel type id: `fnv1a32` of the type name.
    pub const KEEL_TYPE_ID: u32 = ::keel::meta::ids::type_id("TodoError");
}
#[automatically_derived]
impl ::keel::wire::Encode for TodoError {
    fn encode(&self, __w: &mut ::keel::wire::Writer) {
        match self {
            Self::EmptyTitle => {
                __w.write_u16(0u16);
            }
            Self::NotFound(__f0) => {
                __w.write_u16(1u16);
                ::keel::wire::Encode::encode(__f0, __w);
            }
            Self::Rejected { code: __f0, reason: __f1 } => {
                __w.write_u16(2u16);
                ::keel::wire::Encode::encode(__f0, __w);
                ::keel::wire::Encode::encode(__f1, __w);
            }
            Self::Http(__f0) => {
                __w.write_u16(3u16);
                ::keel::wire::Encode::encode(__f0, __w);
            }
            Self::Storage(__f0) => {
                __w.write_u16(4u16);
                ::keel::wire::Encode::encode(__f0, __w);
            }
        }
    }
}
#[automatically_derived]
impl ::keel::wire::Decode for TodoError {
    const MIN_ENCODED_LEN: usize = 2;
    fn decode(
        __r: &mut ::keel::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::keel::wire::WireError> {
        let __at = __r.position();
        match __r.read_u16()? {
            0u16 => ::core::result::Result::Ok(Self::EmptyTitle),
            1u16 => {
                ::core::result::Result::Ok(
                    Self::NotFound(<Uuid as ::keel::wire::Decode>::decode(__r)?),
                )
            }
            2u16 => {
                ::core::result::Result::Ok(Self::Rejected {
                    code: <u16 as ::keel::wire::Decode>::decode(__r)?,
                    reason: <String as ::keel::wire::Decode>::decode(__r)?,
                })
            }
            3u16 => {
                ::core::result::Result::Ok(
                    Self::Http(<HttpError as ::keel::wire::Decode>::decode(__r)?),
                )
            }
            4u16 => {
                ::core::result::Result::Ok(
                    Self::Storage(<StorageError as ::keel::wire::Decode>::decode(__r)?),
                )
            }
            __tag => {
                ::core::result::Result::Err(::keel::wire::WireError::InvalidTag {
                    tag: ::core::primitive::u32::from(__tag),
                    at: __at,
                    ty: "TodoError",
                })
            }
        }
    }
}
#[allow(non_upper_case_globals)]
static __KEEL_META_TodoError: ::keel::meta::EnumMeta = ::keel::meta::EnumMeta {
    name: "TodoError",
    type_id: ::keel::meta::ids::type_id("TodoError"),
    is_error: true,
    variants: &[
        ::keel::meta::VariantMeta {
            name: "EmptyTitle",
            index: 0u16,
            fields: &[],
            tuple: false,
            message: ::core::option::Option::Some("title cannot be empty"),
            docs: "",
        },
        ::keel::meta::VariantMeta {
            name: "NotFound",
            index: 1u16,
            fields: &[
                ::keel::meta::FieldMeta {
                    name: "0",
                    ty: ::keel::meta::TypeRefMeta::Uuid,
                    default: false,
                    docs: "",
                },
            ],
            tuple: true,
            message: ::core::option::Option::Some("todo {0} not found"),
            docs: "",
        },
        ::keel::meta::VariantMeta {
            name: "Rejected",
            index: 2u16,
            fields: &[
                ::keel::meta::FieldMeta {
                    name: "code",
                    ty: ::keel::meta::TypeRefMeta::U16,
                    default: false,
                    docs: "",
                },
                ::keel::meta::FieldMeta {
                    name: "reason",
                    ty: ::keel::meta::TypeRefMeta::String,
                    default: false,
                    docs: "",
                },
            ],
            tuple: false,
            message: ::core::option::Option::Some("rejected with {code}: {reason}"),
            docs: "",
        },
        ::keel::meta::VariantMeta {
            name: "Http",
            index: 3u16,
            fields: &[
                ::keel::meta::FieldMeta {
                    name: "0",
                    ty: ::keel::meta::TypeRefMeta::Named("HttpError"),
                    default: false,
                    docs: "",
                },
            ],
            tuple: true,
            message: ::core::option::Option::None,
            docs: "",
        },
        ::keel::meta::VariantMeta {
            name: "Storage",
            index: 4u16,
            fields: &[
                ::keel::meta::FieldMeta {
                    name: "0",
                    ty: ::keel::meta::TypeRefMeta::Named("StorageError"),
                    default: false,
                    docs: "",
                },
            ],
            tuple: true,
            message: ::core::option::Option::Some("storage failed"),
            docs: "",
        },
    ],
    docs: "",
};
::keel::meta::inventory::submit! {
    ::keel::meta::Registration::Enum(& __KEEL_META_TodoError)
}
#[automatically_derived]
impl ::core::fmt::Display for TodoError {
    fn fmt(&self, __fmt: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        match self {
            Self::EmptyTitle => ::core::write!(__fmt, "title cannot be empty"),
            Self::NotFound(__f0) => ::core::write!(__fmt, "todo {__f0} not found"),
            Self::Rejected { code: __f0, reason: __f1, .. } => {
                ::core::write!(__fmt, "rejected with {__f0}: {__f1}")
            }
            Self::Http(__f0) => ::core::fmt::Display::fmt(__f0, __fmt),
            Self::Storage(_) => ::core::write!(__fmt, "storage failed"),
        }
    }
}
#[automatically_derived]
impl ::std::error::Error for TodoError {
    fn source(&self) -> ::core::option::Option<&(dyn ::std::error::Error + 'static)> {
        match self {
            Self::Http(__f0) => ::std::error::Error::source(__f0),
            Self::Storage(__f0) => {
                ::core::option::Option::Some(
                    __f0 as &(dyn ::std::error::Error + 'static),
                )
            }
            _ => ::core::option::Option::None,
        }
    }
}
#[automatically_derived]
impl ::core::convert::From<HttpError> for TodoError {
    fn from(__source: HttpError) -> Self {
        Self::Http(__source)
    }
}
