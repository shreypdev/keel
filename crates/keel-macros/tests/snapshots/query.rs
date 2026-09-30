pub async fn todos(ctx: &Ctx, page: u32) -> Result<Vec<Todo>, HttpError> {
    todo!()
}
///The `todos` query: its identifiers and settings, and the function `keel-query` runs.
pub struct TodosQuery;
impl TodosQuery {
    /// The stable id: `fnv1a32` of `query.<fn>` or `mutation.<fn>`.
    pub const QUERY_ID: u32 = ::keel::meta::ids::query_id("todos");
    /// The cache key template.
    pub const KEY: &'static str = "todos:{page}";
    /// The staleness window in milliseconds, if any.
    pub const STALE_MS: ::core::option::Option<u64> = ::core::option::Option::Some(
        30000u64,
    );
    /// Whether results are persisted.
    pub const PERSIST: bool = true;
    /// Retry attempts after a failure.
    pub const RETRY: u32 = 5u32;
    /// Whether the call is safe to replay.
    pub const IDEMPOTENT: bool = false;
}
#[automatically_derived]
impl ::keel::query::QueryDef for TodosQuery {
    const ID: u32 = Self::QUERY_ID;
    const KEY: &'static str = Self::KEY;
    const STALE_MS: ::core::option::Option<u64> = Self::STALE_MS;
    const PERSIST: bool = Self::PERSIST;
    const RETRY: u32 = Self::RETRY;
    type Params = (u32,);
    type Output = Vec<Todo>;
    type Error = HttpError;
    #[allow(unused_variables)]
    fn fetch(
        __ctx: ::keel::runtime::Ctx,
        __params: Self::Params,
    ) -> ::core::pin::Pin<
        ::std::boxed::Box<
            dyn ::core::future::Future<
                Output = ::core::result::Result<Vec<Todo>, HttpError>,
            > + ::core::marker::Send,
        >,
    > {
        fn __keel_assert_send<T: ::core::marker::Send>(_: &T) {}
        let (page,) = __params;
        let __fut = async move { todos(&__ctx, page).await };
        __keel_assert_send(&__fut);
        ::std::boxed::Box::pin(__fut)
    }
}
#[allow(non_upper_case_globals)]
static __KEEL_META_TodosQuery: ::keel::meta::QueryMeta = ::keel::meta::QueryMeta {
    name: "todos",
    query_id: ::keel::meta::ids::query_id("todos"),
    kind: ::keel::meta::QueryKind::Query,
    key: "todos:{page}",
    params: &[
        ::keel::meta::ParamMeta {
            name: "page",
            ty: ::keel::meta::TypeRefMeta::U32,
        },
    ],
    returns: ::keel::meta::TypeRefMeta::Result(
        &::keel::meta::TypeRefMeta::Vec(&::keel::meta::TypeRefMeta::Named("Todo")),
        &::keel::meta::TypeRefMeta::Named("HttpError"),
    ),
    stale_ms: ::core::option::Option::Some(30000u64),
    persist: true,
    idempotent: false,
};
::keel::meta::inventory::submit! {
    ::keel::meta::Registration::Query(& __KEEL_META_TodosQuery)
}
::keel::meta::inventory::submit! {
    ::keel::query::QueryRegistration::of:: < TodosQuery > ()
}
