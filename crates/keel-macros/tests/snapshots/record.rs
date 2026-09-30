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
