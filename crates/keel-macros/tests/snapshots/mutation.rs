pub async fn add_todo(ctx: Ctx, title: String) -> Result<Todo, HttpError> {
    todo!()
}
///The `add_todo` mutation: its identifiers and settings, and the function `keel-query` runs.
pub struct AddTodoMutation;
impl AddTodoMutation {
    /// The stable id: `fnv1a32` of `query.<fn>` or `mutation.<fn>`.
    pub const MUTATION_ID: u32 = ::keel::meta::ids::mutation_id("add_todo");
    /// The cache key template.
    pub const KEY: &'static str = "todos";
    /// The staleness window in milliseconds, if any.
    pub const STALE_MS: ::core::option::Option<u64> = ::core::option::Option::None;
    /// Whether results are persisted.
    pub const PERSIST: bool = false;
    /// Retry attempts after a failure.
    pub const RETRY: u32 = 2u32;
    /// Whether the call is safe to replay.
    pub const IDEMPOTENT: bool = true;
}
#[automatically_derived]
impl ::keel::query::MutationDef for AddTodoMutation {
    const ID: u32 = Self::MUTATION_ID;
    const KEY: &'static str = Self::KEY;
    const RETRY: u32 = Self::RETRY;
    const IDEMPOTENT: bool = Self::IDEMPOTENT;
    type Input = (String,);
    type Output = Todo;
    type Error = HttpError;
    #[allow(unused_variables)]
    fn execute(
        __ctx: ::keel::runtime::Ctx,
        __params: Self::Input,
    ) -> ::core::pin::Pin<
        ::std::boxed::Box<
            dyn ::core::future::Future<
                Output = ::core::result::Result<Todo, HttpError>,
            > + ::core::marker::Send,
        >,
    > {
        fn __keel_assert_send<T: ::core::marker::Send>(_: &T) {}
        let (title,) = __params;
        let __fut = async move { add_todo(__ctx, title).await };
        __keel_assert_send(&__fut);
        ::std::boxed::Box::pin(__fut)
    }
}
#[allow(non_upper_case_globals)]
static __KEEL_META_AddTodoMutation: ::keel::meta::QueryMeta = ::keel::meta::QueryMeta {
    name: "add_todo",
    query_id: ::keel::meta::ids::mutation_id("add_todo"),
    kind: ::keel::meta::QueryKind::Mutation,
    key: "todos",
    params: &[
        ::keel::meta::ParamMeta {
            name: "title",
            ty: ::keel::meta::TypeRefMeta::String,
        },
    ],
    returns: ::keel::meta::TypeRefMeta::Result(
        &::keel::meta::TypeRefMeta::Named("Todo"),
        &::keel::meta::TypeRefMeta::Named("HttpError"),
    ),
    stale_ms: ::core::option::Option::None,
    persist: false,
    idempotent: true,
};
::keel::meta::inventory::submit! {
    ::keel::meta::Registration::Query(& __KEEL_META_AddTodoMutation)
}
::keel::meta::inventory::submit! {
    ::keel::query::MutationRegistration::of:: < AddTodoMutation > ()
}
