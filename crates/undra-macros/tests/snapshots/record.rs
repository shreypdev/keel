/// A todo item.
#[derive(Clone, Debug, PartialEq)]
pub struct Todo {
    /// Stable id.
    pub id: Uuid,
    pub title: String,
    pub done: bool,
    pub tags: Vec<String>,
    pub due: Option<Timestamp>,
    pub scores: HashMap<String, i32>,
}
impl Todo {
    /// The stable Undra type id: `fnv1a32` of the type name.
    pub const UNDRA_TYPE_ID: u32 = ::undra::meta::ids::type_id("Todo");
    /// The names of the fields, in declaration order (see `undra_meta::keys`).
    #[doc(hidden)]
    pub const __UNDRA_FIELDS: &'static [&'static str] = &[
        "id",
        "title",
        "done",
        "tags",
        "due",
        "scores",
    ];
}
#[automatically_derived]
impl ::undra::wire::Encode for Todo {
    #[allow(unused_variables)]
    fn encode(&self, __w: &mut ::undra::wire::Writer) {
        ::undra::wire::Encode::encode(&self.id, __w);
        ::undra::wire::Encode::encode(&self.title, __w);
        ::undra::wire::Encode::encode(&self.done, __w);
        ::undra::wire::Encode::encode(&self.tags, __w);
        ::undra::wire::Encode::encode(&self.due, __w);
        ::undra::wire::Encode::encode(&self.scores, __w);
    }
}
#[automatically_derived]
impl ::undra::wire::Decode for Todo {
    const MIN_ENCODED_LEN: usize = 0usize
        + <Uuid as ::undra::wire::Decode>::MIN_ENCODED_LEN
        + <String as ::undra::wire::Decode>::MIN_ENCODED_LEN
        + <bool as ::undra::wire::Decode>::MIN_ENCODED_LEN
        + <Vec<String> as ::undra::wire::Decode>::MIN_ENCODED_LEN
        + <Option<Timestamp> as ::undra::wire::Decode>::MIN_ENCODED_LEN
        + <HashMap<String, i32> as ::undra::wire::Decode>::MIN_ENCODED_LEN;
    #[allow(unused_variables)]
    fn decode(
        __r: &mut ::undra::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::undra::wire::WireError> {
        ::core::result::Result::Ok(Self {
            id: <Uuid as ::undra::wire::Decode>::decode(__r)?,
            title: <String as ::undra::wire::Decode>::decode(__r)?,
            done: <bool as ::undra::wire::Decode>::decode(__r)?,
            tags: <Vec<String> as ::undra::wire::Decode>::decode(__r)?,
            due: <Option<Timestamp> as ::undra::wire::Decode>::decode(__r)?,
            scores: <HashMap<String, i32> as ::undra::wire::Decode>::decode(__r)?,
        })
    }
}
#[allow(non_upper_case_globals)]
static __UNDRA_META_Todo: ::undra::meta::RecordMeta = ::undra::meta::RecordMeta {
    name: "Todo",
    type_id: ::undra::meta::ids::type_id("Todo"),
    fields: &[
        ::undra::meta::FieldMeta {
            name: "id",
            ty: ::undra::meta::TypeRefMeta::Uuid,
            default: false,
            docs: "Stable id.",
        },
        ::undra::meta::FieldMeta {
            name: "title",
            ty: ::undra::meta::TypeRefMeta::String,
            default: false,
            docs: "",
        },
        ::undra::meta::FieldMeta {
            name: "done",
            ty: ::undra::meta::TypeRefMeta::Bool,
            default: true,
            docs: "",
        },
        ::undra::meta::FieldMeta {
            name: "tags",
            ty: ::undra::meta::TypeRefMeta::Vec(&::undra::meta::TypeRefMeta::String),
            default: false,
            docs: "",
        },
        ::undra::meta::FieldMeta {
            name: "due",
            ty: ::undra::meta::TypeRefMeta::Option(
                &::undra::meta::TypeRefMeta::Timestamp,
            ),
            default: false,
            docs: "",
        },
        ::undra::meta::FieldMeta {
            name: "scores",
            ty: ::undra::meta::TypeRefMeta::Map(
                &::undra::meta::TypeRefMeta::String,
                &::undra::meta::TypeRefMeta::I32,
            ),
            default: false,
            docs: "",
        },
    ],
    transparent: false,
    docs: "A todo item.",
};
::undra::meta::inventory::submit! {
    ::undra::meta::Registration::Record(& __UNDRA_META_Todo)
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
    fn __undra_leaf<A, K>()
    where
        A: ?::core::marker::Sized + ::undra::wire::leaf::WireLeaf<K>,
    {}
    fn __undra_map_key<K>()
    where
        K: ?::core::marker::Sized + ::undra::wire::leaf::MapKey,
    {}
    fn __undra_identity() {
        __undra_leaf::<Uuid, ::undra::wire::leaf::kinds::Uuid>();
        __undra_same::<String, ::std::string::String>();
        __undra_same::<bool, ::core::primitive::bool>();
        __undra_same::<Vec<String>, ::std::vec::Vec<String>>();
        __undra_same::<Option<Timestamp>, ::core::option::Option<Timestamp>>();
        __undra_leaf::<Timestamp, ::undra::wire::leaf::kinds::Timestamp>();
        __undra_same::<HashMap<String, i32>, ::std::collections::HashMap<String, i32>>();
        __undra_same::<i32, ::core::primitive::i32>();
        __undra_map_key::<String>();
    }
};
