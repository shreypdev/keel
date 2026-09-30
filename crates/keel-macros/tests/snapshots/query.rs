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
        let (__keel_a0,) = __params;
        let __fut = async move { todos(&__ctx, __keel_a0).await };
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
        __keel_same::<u32, ::core::primitive::u32>();
        __keel_same::<
            Result<Vec<Todo>, HttpError>,
            ::core::result::Result<Vec<Todo>, HttpError>,
        >();
        __keel_same::<Vec<Todo>, ::std::vec::Vec<Todo>>();
    }
    trait __KeelFallback {
        const KEEL_TYPE_ID: u32 = 0;
        const KEEL_IS_ERROR: bool = false;
        const __KEEL_IS_OBJECT: bool = false;
    }
    impl<T: ?::core::marker::Sized> __KeelFallback for T {}
    const _: () = {
        if <Todo>::__KEEL_IS_OBJECT {
            ::core::panic!(
                "error[keel::E0064]: `Todo` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a parameter or a return value\n  = help: return a record with the data the platform needs, or construct the object from the platform with one of its constructors\n  = docs: https://keel.dev/errors/E0064"
            );
        }
        if <Todo>::KEEL_TYPE_ID != ::keel::meta::ids::type_id("Todo") {
            ::core::panic!(
                "error[keel::E0061]: the schema records this type as `Todo`, but the type written here is not that type\n  = note: Keel describes a type to the platforms by the name it is written with, while the generated code encodes the type the name resolves to; an alias (`type Todo = Other`), a renamed import (`use path::Other as Todo`) or a type that is not declared with `#[keel::api]` makes the two differ, so the platforms would read the wrong layout\n  = help: write the type under its declared name (for an alias or a renamed import, use `Other` here), or declare it with `#[keel::api]` (`#[keel::error]` for errors)\n  = docs: https://keel.dev/errors/E0061"
            );
        }
    };
    const _: () = {
        if <HttpError>::__KEEL_IS_OBJECT {
            ::core::panic!(
                "error[keel::E0064]: `HttpError` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a parameter or a return value\n  = help: return a record with the data the platform needs, or construct the object from the platform with one of its constructors\n  = docs: https://keel.dev/errors/E0064"
            );
        }
        if <HttpError>::KEEL_TYPE_ID != ::keel::meta::ids::type_id("HttpError") {
            ::core::panic!(
                "error[keel::E0061]: the schema records this type as `HttpError`, but the type written here is not that type\n  = note: Keel describes a type to the platforms by the name it is written with, while the generated code encodes the type the name resolves to; an alias (`type HttpError = Other`), a renamed import (`use path::Other as HttpError`) or a type that is not declared with `#[keel::api]` makes the two differ, so the platforms would read the wrong layout\n  = help: write the type under its declared name (for an alias or a renamed import, use `Other` here), or declare it with `#[keel::api]` (`#[keel::error]` for errors)\n  = docs: https://keel.dev/errors/E0061"
            );
        }
        if !<HttpError>::KEEL_IS_ERROR {
            ::core::panic!(
                "error[keel::E0001]: `HttpError` is used as the error type of a `Result`, but it is not a `#[keel::error]` enum\n  = note: the platforms throw the error type by name, and only `#[keel::error]` enums carry the messages they show\n  = help: declare it with `#[keel::error]`, for example `#[keel::error] enum HttpError {{ #[error(\"failed\")] Failed }}`\n  = docs: https://keel.dev/errors/E0001"
            );
        }
    };
};
