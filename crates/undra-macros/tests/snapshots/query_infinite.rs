/// The posts of a filter, newest first.
pub async fn feed(
    ctx: &Ctx,
    filter: Filter,
    cursor: Option<String>,
) -> Result<undra::query::Page<Post, String>, ApiError> {
    todo!()
}
#[doc(hidden)]
macro_rules! _undra_error_E0007_a_query_is_a_free_function_move_it_out_of_the_impl_block {
    () => {
        #[doc =
        "The `feed` query: its identifiers and settings, and the function `undra-query` runs."]
        pub struct FeedQuery; impl FeedQuery { #[doc =
        r" The stable id: `fnv1a32` of `query.<fn>` or `mutation.<fn>`."] pub const
        QUERY_ID : u32 = ::undra::meta::ids::query_id("feed"); #[doc =
        r" The cache key template."] pub const KEY : & 'static str = "feed/{filter}";
        #[doc = r" The staleness window in milliseconds, if any."] pub const STALE_MS :
        ::core::option::Option < u64 > = ::core::option::Option::Some(60000u64); #[doc =
        r" Whether results are persisted."] pub const PERSIST : bool = true; #[doc =
        r" Retry attempts after a failure."] pub const RETRY : u32 = 3u32; #[doc =
        r" Whether the call is safe to replay."] pub const IDEMPOTENT : bool = false;
        #[doc = r" The polling interval in milliseconds, if the query polls by default."]
        pub const INTERVAL_MS : ::core::option::Option < u64 > =
        ::core::option::Option::Some(30000u64); #[doc =
        r" Whether the query keeps polling while the app is in the background."] pub
        const POLL_IN_BACKGROUND : bool = false; #[doc =
        r" The field of a row that identifies it (`item_key`): the handle's list is keyed on it."]
        pub const ITEM_KEY : & 'static str = "id"; #[doc =
        r" How many pages a refetch loads at most (`refetch_pages`); none: every loaded page."]
        pub const REFETCH_PAGES : ::core::option::Option < u32 > =
        ::core::option::Option::Some(3u32); #[doc =
        r" How many of the first pages a persisted entry stores (`persist_pages`)."] pub
        const PERSIST_PAGES : u32 = 2u32; } #[allow(non_upper_case_globals)] static
        __UNDRA_META_FeedQuery : ::undra::meta::QueryMeta = ::undra::meta::QueryMeta {
        name : "feed", query_id : ::undra::meta::ids::query_id("feed"), kind :
        ::undra::meta::QueryKind::Query, key : "feed/{filter}", params : &
        [::undra::meta::ParamMeta { name : "filter", ty :
        ::undra::meta::TypeRefMeta::Named("Filter") }], returns :
        ::undra::meta::TypeRefMeta::Result(& ::undra::meta::TypeRefMeta::Vec(&
        ::undra::meta::TypeRefMeta::Named("Post")), &
        ::undra::meta::TypeRefMeta::Named("ApiError")), stale_ms :
        ::core::option::Option::Some(60000u64), persist : true, idempotent : false,
        interval_ms : ::core::option::Option::Some(30000u64), poll_in_background : false,
        infinite : ::core::option::Option::Some(::undra::meta::InfiniteMeta { cursor :
        ::undra::meta::TypeRefMeta::String, item_key : "id" }), };
        ::undra::meta::inventory::submit! { ::undra::meta::Registration::Query(&
        __UNDRA_META_FeedQuery) } ::undra::meta::inventory::submit! {
        ::undra::query::QueryRegistration::of:: < FeedQuery > () }
        ::undra::meta::inventory::submit! { ::undra::query::__private::HYDRATE }
        ::undra::meta::inventory::submit! { ::undra::query::__private::LAYER }
    };
}
_undra_error_E0007_a_query_is_a_free_function_move_it_out_of_the_impl_block!();
#[automatically_derived]
impl ::undra::query::QueryDef for FeedQuery {
    const ID: u32 = Self::QUERY_ID;
    const KEY: &'static str = Self::KEY;
    const STALE_MS: ::core::option::Option<u64> = Self::STALE_MS;
    const PERSIST: bool = Self::PERSIST;
    const RETRY: u32 = Self::RETRY;
    const INTERVAL_MS: ::core::option::Option<u64> = Self::INTERVAL_MS;
    const POLL_IN_BACKGROUND: bool = Self::POLL_IN_BACKGROUND;
    const PAGED: ::core::option::Option<&'static ::undra::query::PagedVTable> = ::core::option::Option::Some(
        ::undra::query::paged_vtable::<Self>(),
    );
    type Params = (Filter,);
    type Output = Vec<Post>;
    type Error = ApiError;
    fn fetch(
        __ctx: ::undra::runtime::Ctx,
        __params: Self::Params,
    ) -> ::core::pin::Pin<
        ::std::boxed::Box<
            dyn ::core::future::Future<
                Output = ::core::result::Result<Vec<Post>, ApiError>,
            > + ::core::marker::Send,
        >,
    > {
        ::undra::query::fetch_first_page::<Self>(__ctx, __params)
    }
}
#[automatically_derived]
impl ::undra::query::InfiniteQueryDef for FeedQuery {
    type Item = Post;
    type Cursor = String;
    const REFETCH_PAGES: ::core::option::Option<u32> = Self::REFETCH_PAGES;
    const PERSIST_PAGES: u32 = Self::PERSIST_PAGES;
    #[allow(unused_variables)]
    fn fetch_page(
        __ctx: ::undra::runtime::Ctx,
        __params: Self::Params,
        __cursor: ::core::option::Option<String>,
    ) -> ::core::pin::Pin<
        ::std::boxed::Box<
            dyn ::core::future::Future<
                Output = ::core::result::Result<
                    ::undra::query::Page<Post, String>,
                    ApiError,
                >,
            > + ::core::marker::Send,
        >,
    > {
        #[allow(non_snake_case)]
        fn _undra_error_E0022_the_future_of_an_async_method_must_be_Send<
            T: ::core::marker::Send,
        >(_: &T) {}
        let (__undra_a0,) = __params;
        let __fut = async move { feed(&__ctx, __undra_a0, __cursor).await };
        _undra_error_E0022_the_future_of_an_async_method_must_be_Send(&__fut);
        ::std::boxed::Box::pin(__fut)
    }
    #[allow(non_camel_case_types, dead_code)]
    fn item_key(__item: &Post) -> u64 {
        trait __UndraKeyed {
            const __UNDRA_FIELDS: &'static [&'static str] = &[];
        }
        impl<__T: ?::core::marker::Sized> __UndraKeyed for __T {}
        struct __UndraGate<const __OK: bool>;
        trait __UndraPass<__T: ?::core::marker::Sized> {
            type Out: ?::core::marker::Sized;
        }
        impl<__T: ?::core::marker::Sized> __UndraPass<__T> for __UndraGate<true> {
            type Out = __T;
        }
        const __UNDRA_FIELDS: &[&str] = <Post>::__UNDRA_FIELDS;
        const __UNDRA_INDEX: usize = ::undra::meta::keys::index_of(__UNDRA_FIELDS, "id");
        const __UNDRA_MESSAGE_LEN: usize = ::undra::meta::keys::message_len(
            "error[undra::E0073]: `item_key = \"id\"` of the infinite query `feed` names no field of `Post`\n  = note: `item_key` names the field of the rows that identifies them, and `Post` has ",
            __UNDRA_FIELDS,
            "\n  = help: write the name of one of those fields as `item_key`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0073",
        );
        const __UNDRA_MESSAGE: [u8; __UNDRA_MESSAGE_LEN] = ::undra::meta::keys::message::<
            __UNDRA_MESSAGE_LEN,
        >(
            "error[undra::E0073]: `item_key = \"id\"` of the infinite query `feed` names no field of `Post`\n  = note: `item_key` names the field of the rows that identifies them, and `Post` has ",
            __UNDRA_FIELDS,
            "\n  = help: write the name of one of those fields as `item_key`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0073",
        );
        const __UNDRA_MESSAGE_TEXT: &str = ::undra::meta::keys::as_str(&__UNDRA_MESSAGE);
        const __UNDRA_KEY_IS_A_FIELD: bool = if __UNDRA_INDEX == usize::MAX {
            ::core::panic!("{}", __UNDRA_MESSAGE_TEXT)
        } else {
            true
        };
        ::std::thread_local! {
            static __UNDRA_KEY_BUF : ::core::cell::RefCell < ::undra::wire::Writer > =
            ::core::cell::RefCell::new(::undra::wire::Writer::new());
        }
        __UNDRA_KEY_BUF
            .with(|__buf| {
                let mut __buf = __buf.borrow_mut();
                __buf.clear();
                let __row: &<__UndraGate<
                    { __UNDRA_KEY_IS_A_FIELD },
                > as __UndraPass<Post>>::Out = __item;
                ::undra::wire::Encode::encode(&__row.id, &mut __buf);
                ::undra::meta::ids::fnv1a64(__buf.as_slice())
            })
    }
}
#[doc(hidden)]
#[allow(
    non_camel_case_types,
    non_upper_case_globals,
    dead_code,
    unused,
    unused_braces,
    clippy::all
)]
const __UNDRA_CHECKS_Feed: () = {
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
        __undra_same::<Option<String>, ::core::option::Option<String>>();
        __undra_same::<String, ::std::string::String>();
        __undra_same::<
            Result<Vec<Post>, ApiError>,
            ::core::result::Result<Vec<Post>, ApiError>,
        >();
        __undra_same::<Vec<Post>, ::std::vec::Vec<Post>>();
    }
    trait __UndraFallback {
        const UNDRA_TYPE_ID: u32 = 0;
        const UNDRA_IS_ERROR: bool = false;
        const __UNDRA_IS_OBJECT: bool = false;
        const __UNDRA_OBJECT_ID: u32 = 0;
    }
    impl<T: ?::core::marker::Sized> __UndraFallback for T {}
    const _: () = {
        if <Filter>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0064]: `Filter` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a variant field, a signal value, a map entry or a query value\n  = help: return it as `Arc<Filter>`, take it as `&Filter` or `Arc<Filter>` (a method, constructor or function parameter or return), or use a record with the data the platform needs\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0064"
            );
        }
        let __undra_id = <Filter>::UNDRA_TYPE_ID;
        if __undra_id == 0 {
            ::core::panic!(
                "error[undra::E0061]: `Filter` is not a type declared with `#[undra::api]`\n  = note: Undra describes a type to the platforms by the name it is written with, so the name must be a record or enum declared with `#[undra::api]` or an error declared with `#[undra::error]`; anything else, such as a plain struct or an alias (`type Id = u64`), has no definition the platforms could generate\n  = help: add `#[undra::api]` to `Filter`, or, if it is an alias, write the type it stands for where it is used\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if __undra_id != ::undra::meta::ids::type_id("Filter") {
            ::core::panic!(
                "error[undra::E0061]: `Filter` here is an alias or a renamed import of an Undra type that is declared under another name\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type that name resolves to; with `type Filter = Other` or `use path::Other as Filter` the platforms would be told `Filter` and receive the layout of `Other`\n  = help: write the type under the name it is declared with (`Other` in the examples above), or declare a separate `#[undra::api] struct Filter` if you mean a distinct type\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
    };
    const _: () = {
        if <Post>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0064]: `Post` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a variant field, a signal value, a map entry or a query value\n  = help: return it as `Arc<Post>`, take it as `&Post` or `Arc<Post>` (a method, constructor or function parameter or return), or use a record with the data the platform needs\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0064"
            );
        }
        let __undra_id = <Post>::UNDRA_TYPE_ID;
        if __undra_id == 0 {
            ::core::panic!(
                "error[undra::E0061]: `Post` is not a type declared with `#[undra::api]`\n  = note: Undra describes a type to the platforms by the name it is written with, so the name must be a record or enum declared with `#[undra::api]` or an error declared with `#[undra::error]`; anything else, such as a plain struct or an alias (`type Id = u64`), has no definition the platforms could generate\n  = help: add `#[undra::api]` to `Post`, or, if it is an alias, write the type it stands for where it is used\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if __undra_id != ::undra::meta::ids::type_id("Post") {
            ::core::panic!(
                "error[undra::E0061]: `Post` here is an alias or a renamed import of an Undra type that is declared under another name\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type that name resolves to; with `type Post = Other` or `use path::Other as Post` the platforms would be told `Post` and receive the layout of `Other`\n  = help: write the type under the name it is declared with (`Other` in the examples above), or declare a separate `#[undra::api] struct Post` if you mean a distinct type\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
    };
    const _: () = {
        if <ApiError>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0064]: `ApiError` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a variant field, a signal value, a map entry or a query value\n  = help: return it as `Arc<ApiError>`, take it as `&ApiError` or `Arc<ApiError>` (a method, constructor or function parameter or return), or use a record with the data the platform needs\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0064"
            );
        }
        let __undra_id = <ApiError>::UNDRA_TYPE_ID;
        if __undra_id == 0 {
            ::core::panic!(
                "error[undra::E0061]: `ApiError` is not a type declared with `#[undra::api]`\n  = note: Undra describes a type to the platforms by the name it is written with, so the name must be a record or enum declared with `#[undra::api]` or an error declared with `#[undra::error]`; anything else, such as a plain struct or an alias (`type Id = u64`), has no definition the platforms could generate\n  = help: add `#[undra::api]` to `ApiError`, or, if it is an alias, write the type it stands for where it is used\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if __undra_id != ::undra::meta::ids::type_id("ApiError") {
            ::core::panic!(
                "error[undra::E0061]: `ApiError` here is an alias or a renamed import of an Undra type that is declared under another name\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type that name resolves to; with `type ApiError = Other` or `use path::Other as ApiError` the platforms would be told `ApiError` and receive the layout of `Other`\n  = help: write the type under the name it is declared with (`Other` in the examples above), or declare a separate `#[undra::api] struct ApiError` if you mean a distinct type\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if !<ApiError>::UNDRA_IS_ERROR {
            ::core::panic!(
                "error[undra::E0001]: `ApiError` is used as the error type of a `Result`, but it is not a `#[undra::error]` enum\n  = note: the platforms throw the error type by name, and only `#[undra::error]` enums carry the messages they show\n  = help: declare it with `#[undra::error]`, for example `#[undra::error] enum ApiError {{ #[error(\"failed\")] Failed }}`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0001"
            );
        }
    };
};
