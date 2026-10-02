impl TodoPage {
    #[doc(hidden)]
    #[allow(non_upper_case_globals)]
    pub const _undra_error_E0070_this_instantiation_of_Page_is_declared_twice_keep_one_alias_per_instantiation: () = ();
    /// The stable Undra type id: `fnv1a32` of the type name.
    pub const UNDRA_TYPE_ID: u32 = ::undra::meta::ids::type_id("TodoPage");
    /// The names of the fields, in declaration order (see `undra_meta::keys`).
    #[doc(hidden)]
    pub const __UNDRA_FIELDS: &'static [&'static str] = &["items", "next", "by_name"];
}
#[allow(non_upper_case_globals)]
static __UNDRA_META_TodoPage: ::undra::meta::RecordMeta = ::undra::meta::RecordMeta {
    name: "TodoPage",
    type_id: ::undra::meta::ids::type_id("TodoPage"),
    fields: &[
        ::undra::meta::FieldMeta {
            name: "items",
            ty: ::undra::meta::TypeRefMeta::Vec(
                &::undra::meta::TypeRefMeta::Named("Todo"),
            ),
            default: false,
            docs: "",
        },
        ::undra::meta::FieldMeta {
            name: "next",
            ty: ::undra::meta::TypeRefMeta::Option(&::undra::meta::TypeRefMeta::String),
            default: true,
            docs: "",
        },
        ::undra::meta::FieldMeta {
            name: "by_name",
            ty: ::undra::meta::TypeRefMeta::Map(
                &::undra::meta::TypeRefMeta::String,
                &::undra::meta::TypeRefMeta::Vec(
                    &::undra::meta::TypeRefMeta::Option(
                        &::undra::meta::TypeRefMeta::Named("Todo"),
                    ),
                ),
            ),
            default: false,
            docs: "",
        },
    ],
    transparent: false,
    docs: "One page of a list.",
};
::undra::meta::inventory::submit! {
    ::undra::meta::Registration::Record(& __UNDRA_META_TodoPage)
}
