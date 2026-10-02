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
    /// The stable Undra type id: `fnv1a32` of the type name.
    pub const UNDRA_TYPE_ID: u32 = ::undra::meta::ids::type_id("TodoError");
    /// Whether this is a `#[undra::error]` enum (what a `Result` may throw).
    #[doc(hidden)]
    pub const UNDRA_IS_ERROR: bool = true;
}
#[automatically_derived]
impl ::undra::wire::Encode for TodoError {
    fn encode(&self, __w: &mut ::undra::wire::Writer) {
        match self {
            Self::EmptyTitle => {
                __w.write_u16(0u16);
            }
            Self::NotFound(__f0) => {
                __w.write_u16(1u16);
                ::undra::wire::Encode::encode(__f0, __w);
            }
            Self::Rejected { code: __f0, reason: __f1 } => {
                __w.write_u16(2u16);
                ::undra::wire::Encode::encode(__f0, __w);
                ::undra::wire::Encode::encode(__f1, __w);
            }
            Self::Http(__f0) => {
                __w.write_u16(3u16);
                ::undra::wire::Encode::encode(__f0, __w);
            }
            Self::Storage(__f0) => {
                __w.write_u16(4u16);
                ::undra::wire::Encode::encode(__f0, __w);
            }
        }
    }
}
#[automatically_derived]
impl ::undra::wire::Decode for TodoError {
    const MIN_ENCODED_LEN: usize = 2;
    fn decode(
        __r: &mut ::undra::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::undra::wire::WireError> {
        let __at = __r.position();
        match __r.read_u16()? {
            0u16 => ::core::result::Result::Ok(Self::EmptyTitle),
            1u16 => {
                ::core::result::Result::Ok(
                    Self::NotFound(<Uuid as ::undra::wire::Decode>::decode(__r)?),
                )
            }
            2u16 => {
                ::core::result::Result::Ok(Self::Rejected {
                    code: <u16 as ::undra::wire::Decode>::decode(__r)?,
                    reason: <String as ::undra::wire::Decode>::decode(__r)?,
                })
            }
            3u16 => {
                ::core::result::Result::Ok(
                    Self::Http(<HttpError as ::undra::wire::Decode>::decode(__r)?),
                )
            }
            4u16 => {
                ::core::result::Result::Ok(
                    Self::Storage(<StorageError as ::undra::wire::Decode>::decode(__r)?),
                )
            }
            __tag => {
                ::core::result::Result::Err(::undra::wire::WireError::InvalidTag {
                    tag: ::core::primitive::u32::from(__tag),
                    at: __at,
                    ty: "TodoError",
                })
            }
        }
    }
}
#[allow(non_upper_case_globals)]
static __UNDRA_META_TodoError: ::undra::meta::EnumMeta = ::undra::meta::EnumMeta {
    name: "TodoError",
    type_id: ::undra::meta::ids::type_id("TodoError"),
    is_error: true,
    variants: &[
        ::undra::meta::VariantMeta {
            name: "EmptyTitle",
            index: 0u16,
            fields: &[],
            tuple: false,
            message: ::core::option::Option::Some("title cannot be empty"),
            docs: "",
        },
        ::undra::meta::VariantMeta {
            name: "NotFound",
            index: 1u16,
            fields: &[
                ::undra::meta::FieldMeta {
                    name: "0",
                    ty: ::undra::meta::TypeRefMeta::Uuid,
                    default: false,
                    docs: "",
                },
            ],
            tuple: true,
            message: ::core::option::Option::Some("todo {0} not found"),
            docs: "",
        },
        ::undra::meta::VariantMeta {
            name: "Rejected",
            index: 2u16,
            fields: &[
                ::undra::meta::FieldMeta {
                    name: "code",
                    ty: ::undra::meta::TypeRefMeta::U16,
                    default: false,
                    docs: "",
                },
                ::undra::meta::FieldMeta {
                    name: "reason",
                    ty: ::undra::meta::TypeRefMeta::String,
                    default: false,
                    docs: "",
                },
            ],
            tuple: false,
            message: ::core::option::Option::Some("rejected with {code}: {reason}"),
            docs: "",
        },
        ::undra::meta::VariantMeta {
            name: "Http",
            index: 3u16,
            fields: &[
                ::undra::meta::FieldMeta {
                    name: "0",
                    ty: ::undra::meta::TypeRefMeta::Named("HttpError"),
                    default: false,
                    docs: "",
                },
            ],
            tuple: true,
            message: ::core::option::Option::None,
            docs: "",
        },
        ::undra::meta::VariantMeta {
            name: "Storage",
            index: 4u16,
            fields: &[
                ::undra::meta::FieldMeta {
                    name: "0",
                    ty: ::undra::meta::TypeRefMeta::Named("StorageError"),
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
::undra::meta::inventory::submit! {
    ::undra::meta::Registration::Enum(& __UNDRA_META_TodoError)
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
        __undra_same::<Uuid, ::undra::wire::Uuid>();
        __undra_same::<u16, ::core::primitive::u16>();
        __undra_same::<String, ::std::string::String>();
    }
    trait __UndraFallback {
        const UNDRA_TYPE_ID: u32 = 0;
        const UNDRA_IS_ERROR: bool = false;
        const __UNDRA_IS_OBJECT: bool = false;
        const __UNDRA_OBJECT_ID: u32 = 0;
    }
    impl<T: ?::core::marker::Sized> __UndraFallback for T {}
    const _: () = {
        if <HttpError>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0064]: `HttpError` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a variant field, a signal value, a map entry or a query value\n  = help: return it as `Arc<HttpError>`, take it as `&HttpError` or `Arc<HttpError>` (a method, constructor or function parameter or return), or use a record with the data the platform needs\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0064"
            );
        }
        let __undra_id = <HttpError>::UNDRA_TYPE_ID;
        if __undra_id == 0 {
            ::core::panic!(
                "error[undra::E0061]: `HttpError` is not a type declared with `#[undra::api]`\n  = note: Undra describes a type to the platforms by the name it is written with, so the name must be a record or enum declared with `#[undra::api]` or an error declared with `#[undra::error]`; anything else, such as a plain struct or an alias (`type Id = u64`), has no definition the platforms could generate\n  = help: add `#[undra::api]` to `HttpError`, or, if it is an alias, write the type it stands for where it is used\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if __undra_id != ::undra::meta::ids::type_id("HttpError") {
            ::core::panic!(
                "error[undra::E0061]: `HttpError` here is an alias or a renamed import of an Undra type that is declared under another name\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type that name resolves to; with `type HttpError = Other` or `use path::Other as HttpError` the platforms would be told `HttpError` and receive the layout of `Other`\n  = help: write the type under the name it is declared with (`Other` in the examples above), or declare a separate `#[undra::api] struct HttpError` if you mean a distinct type\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
    };
    const _: () = {
        if <StorageError>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0064]: `StorageError` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a variant field, a signal value, a map entry or a query value\n  = help: return it as `Arc<StorageError>`, take it as `&StorageError` or `Arc<StorageError>` (a method, constructor or function parameter or return), or use a record with the data the platform needs\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0064"
            );
        }
        let __undra_id = <StorageError>::UNDRA_TYPE_ID;
        if __undra_id == 0 {
            ::core::panic!(
                "error[undra::E0061]: `StorageError` is not a type declared with `#[undra::api]`\n  = note: Undra describes a type to the platforms by the name it is written with, so the name must be a record or enum declared with `#[undra::api]` or an error declared with `#[undra::error]`; anything else, such as a plain struct or an alias (`type Id = u64`), has no definition the platforms could generate\n  = help: add `#[undra::api]` to `StorageError`, or, if it is an alias, write the type it stands for where it is used\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if __undra_id != ::undra::meta::ids::type_id("StorageError") {
            ::core::panic!(
                "error[undra::E0061]: `StorageError` here is an alias or a renamed import of an Undra type that is declared under another name\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type that name resolves to; with `type StorageError = Other` or `use path::Other as StorageError` the platforms would be told `StorageError` and receive the layout of `Other`\n  = help: write the type under the name it is declared with (`Other` in the examples above), or declare a separate `#[undra::api] struct StorageError` if you mean a distinct type\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
    };
};
