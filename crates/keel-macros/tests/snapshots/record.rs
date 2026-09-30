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
    /// The stable Keel type id: `fnv1a32` of the type name.
    pub const KEEL_TYPE_ID: u32 = ::keel::meta::ids::type_id("Todo");
}
#[automatically_derived]
impl ::keel::wire::Encode for Todo {
    #[allow(unused_variables)]
    fn encode(&self, __w: &mut ::keel::wire::Writer) {
        ::keel::wire::Encode::encode(&self.id, __w);
        ::keel::wire::Encode::encode(&self.title, __w);
        ::keel::wire::Encode::encode(&self.done, __w);
        ::keel::wire::Encode::encode(&self.tags, __w);
        ::keel::wire::Encode::encode(&self.due, __w);
        ::keel::wire::Encode::encode(&self.scores, __w);
    }
}
#[automatically_derived]
impl ::keel::wire::Decode for Todo {
    const MIN_ENCODED_LEN: usize = 0usize
        + <Uuid as ::keel::wire::Decode>::MIN_ENCODED_LEN
        + <String as ::keel::wire::Decode>::MIN_ENCODED_LEN
        + <bool as ::keel::wire::Decode>::MIN_ENCODED_LEN
        + <Vec<String> as ::keel::wire::Decode>::MIN_ENCODED_LEN
        + <Option<Timestamp> as ::keel::wire::Decode>::MIN_ENCODED_LEN
        + <HashMap<String, i32> as ::keel::wire::Decode>::MIN_ENCODED_LEN;
    #[allow(unused_variables)]
    fn decode(
        __r: &mut ::keel::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::keel::wire::WireError> {
        ::core::result::Result::Ok(Self {
            id: <Uuid as ::keel::wire::Decode>::decode(__r)?,
            title: <String as ::keel::wire::Decode>::decode(__r)?,
            done: <bool as ::keel::wire::Decode>::decode(__r)?,
            tags: <Vec<String> as ::keel::wire::Decode>::decode(__r)?,
            due: <Option<Timestamp> as ::keel::wire::Decode>::decode(__r)?,
            scores: <HashMap<String, i32> as ::keel::wire::Decode>::decode(__r)?,
        })
    }
}
#[allow(non_upper_case_globals)]
static __KEEL_META_Todo: ::keel::meta::RecordMeta = ::keel::meta::RecordMeta {
    name: "Todo",
    type_id: ::keel::meta::ids::type_id("Todo"),
    fields: &[
        ::keel::meta::FieldMeta {
            name: "id",
            ty: ::keel::meta::TypeRefMeta::Uuid,
            default: false,
            docs: "Stable id.",
        },
        ::keel::meta::FieldMeta {
            name: "title",
            ty: ::keel::meta::TypeRefMeta::String,
            default: false,
            docs: "",
        },
        ::keel::meta::FieldMeta {
            name: "done",
            ty: ::keel::meta::TypeRefMeta::Bool,
            default: true,
            docs: "",
        },
        ::keel::meta::FieldMeta {
            name: "tags",
            ty: ::keel::meta::TypeRefMeta::Vec(&::keel::meta::TypeRefMeta::String),
            default: false,
            docs: "",
        },
        ::keel::meta::FieldMeta {
            name: "due",
            ty: ::keel::meta::TypeRefMeta::Option(&::keel::meta::TypeRefMeta::Timestamp),
            default: false,
            docs: "",
        },
        ::keel::meta::FieldMeta {
            name: "scores",
            ty: ::keel::meta::TypeRefMeta::Map(
                &::keel::meta::TypeRefMeta::String,
                &::keel::meta::TypeRefMeta::I32,
            ),
            default: false,
            docs: "",
        },
    ],
    docs: "A todo item.",
};
::keel::meta::inventory::submit! {
    ::keel::meta::Registration::Record(& __KEEL_META_Todo)
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code, unused, unused_braces, clippy::all)]
const _: () = {
    #[diagnostic::on_unimplemented(
        message = "error[keel::E0060]: `{Self}` is spelled like the built-in Keel type `{T}`, but it is a different type\n  = note: the schema records this position as the built-in type, so the platforms would read the bytes of `{T}` where the generated code writes `{Self}`\n  = help: rename your type, or import the built-in one (`{T}`) where it is used\n  = docs: https://keel.dev/errors/E0060",
        label = "this is not `{T}`"
    )]
    trait __KeelSameAs<T: ?::core::marker::Sized> {}
    impl<T: ?::core::marker::Sized> __KeelSameAs<T> for T {}
    fn __keel_same<A, B>()
    where
        A: ?::core::marker::Sized + __KeelSameAs<B>,
        B: ?::core::marker::Sized,
    {}
    fn __keel_identity() {
        __keel_same::<Uuid, ::keel::wire::Uuid>();
        __keel_same::<String, ::std::string::String>();
        __keel_same::<bool, ::core::primitive::bool>();
        __keel_same::<Vec<String>, ::std::vec::Vec<String>>();
        __keel_same::<Option<Timestamp>, ::core::option::Option<Timestamp>>();
        __keel_same::<Timestamp, ::keel::wire::Timestamp>();
        __keel_same::<HashMap<String, i32>, ::std::collections::HashMap<String, i32>>();
        __keel_same::<i32, ::core::primitive::i32>();
    }
};
