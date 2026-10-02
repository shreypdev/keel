/// The latest quote.
pub async fn ticker(ctx: &Ctx, symbol: String) -> Result<Quote, HttpError> {
    todo!()
}
#[doc(hidden)]
macro_rules! _undra_error_E0007_a_query_is_a_free_function_move_it_out_of_the_impl_block {
    () => {
        #[doc =
        "The `ticker` query: its identifiers and settings, and the function `undra-query` runs."]
        pub struct TickerQuery; impl TickerQuery { #[doc =
        r" The stable id: `fnv1a32` of `query.<fn>` or `mutation.<fn>`."] pub const
        QUERY_ID : u32 = ::undra::meta::ids::query_id("ticker"); #[doc =
        r" The cache key template."] pub const KEY : & 'static str = "ticker/{symbol}";
        #[doc = r" The staleness window in milliseconds, if any."] pub const STALE_MS :
        ::core::option::Option < u64 > = ::core::option::Option::Some(10000u64); #[doc =
        r" Whether results are persisted."] pub const PERSIST : bool = false; #[doc =
        r" Retry attempts after a failure."] pub const RETRY : u32 = 3u32; #[doc =
        r" Whether the call is safe to replay."] pub const IDEMPOTENT : bool = false;
        #[doc = r" The polling interval in milliseconds, if the query polls by default."]
        pub const INTERVAL_MS : ::core::option::Option < u64 > =
        ::core::option::Option::Some(30000u64); #[doc =
        r" Whether the query keeps polling while the app is in the background."] pub
        const POLL_IN_BACKGROUND : bool = true; } #[allow(non_upper_case_globals)] static
        __UNDRA_META_TickerQuery : ::undra::meta::QueryMeta = ::undra::meta::QueryMeta {
        name : "ticker", query_id : ::undra::meta::ids::query_id("ticker"), kind :
        ::undra::meta::QueryKind::Query, key : "ticker/{symbol}", params : &
        [::undra::meta::ParamMeta { name : "symbol", ty :
        ::undra::meta::TypeRefMeta::String }], returns :
        ::undra::meta::TypeRefMeta::Result(& ::undra::meta::TypeRefMeta::Named("Quote"),
        & ::undra::meta::TypeRefMeta::Named("HttpError")), stale_ms :
        ::core::option::Option::Some(10000u64), persist : false, idempotent : false,
        interval_ms : ::core::option::Option::Some(30000u64), poll_in_background : true,
        infinite : ::core::option::Option::None, }; ::undra::meta::inventory::submit! {
        ::undra::meta::Registration::Query(& __UNDRA_META_TickerQuery) }
        ::undra::meta::inventory::submit! { ::undra::query::QueryRegistration::of:: <
        TickerQuery > () } ::undra::meta::inventory::submit! {
        ::undra::query::__private::HYDRATE } ::undra::meta::inventory::submit! {
        ::undra::query::__private::LAYER }
    };
}
_undra_error_E0007_a_query_is_a_free_function_move_it_out_of_the_impl_block!();
#[automatically_derived]
impl ::undra::query::QueryDef for TickerQuery {
    const ID: u32 = Self::QUERY_ID;
    const KEY: &'static str = Self::KEY;
    const STALE_MS: ::core::option::Option<u64> = Self::STALE_MS;
    const PERSIST: bool = Self::PERSIST;
    const RETRY: u32 = Self::RETRY;
    const INTERVAL_MS: ::core::option::Option<u64> = Self::INTERVAL_MS;
    const POLL_IN_BACKGROUND: bool = Self::POLL_IN_BACKGROUND;
    type Params = (String,);
    type Output = Quote;
    type Error = HttpError;
    #[allow(unused_variables)]
    fn fetch(
        __ctx: ::undra::runtime::Ctx,
        __params: Self::Params,
    ) -> ::core::pin::Pin<
        ::std::boxed::Box<
            dyn ::core::future::Future<
                Output = ::core::result::Result<Quote, HttpError>,
            > + ::core::marker::Send,
        >,
    > {
        #[allow(non_snake_case)]
        fn _undra_error_E0022_the_future_of_an_async_method_must_be_Send<
            T: ::core::marker::Send,
        >(_: &T) {}
        let (__undra_a0,) = __params;
        let __fut = async move { ticker(&__ctx, __undra_a0).await };
        _undra_error_E0022_the_future_of_an_async_method_must_be_Send(&__fut);
        ::std::boxed::Box::pin(__fut)
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
const __UNDRA_CHECKS_Ticker: () = {
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
        __undra_same::<String, ::std::string::String>();
        __undra_same::<
            Result<Quote, HttpError>,
            ::core::result::Result<Quote, HttpError>,
        >();
    }
    trait __UndraFallback {
        const UNDRA_TYPE_ID: u32 = 0;
        const UNDRA_IS_ERROR: bool = false;
        const __UNDRA_IS_OBJECT: bool = false;
        const __UNDRA_OBJECT_ID: u32 = 0;
    }
    impl<T: ?::core::marker::Sized> __UndraFallback for T {}
    const _: () = {
        if <Quote>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0064]: `Quote` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a variant field, a signal value, a map entry or a query value\n  = help: return it as `Arc<Quote>`, take it as `&Quote` or `Arc<Quote>` (a method, constructor or function parameter or return), or use a record with the data the platform needs\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0064"
            );
        }
        let __undra_id = <Quote>::UNDRA_TYPE_ID;
        if __undra_id == 0 {
            ::core::panic!(
                "error[undra::E0061]: `Quote` is not a type declared with `#[undra::api]`\n  = note: Undra describes a type to the platforms by the name it is written with, so the name must be a record or enum declared with `#[undra::api]` or an error declared with `#[undra::error]`; anything else, such as a plain struct or an alias (`type Id = u64`), has no definition the platforms could generate\n  = help: add `#[undra::api]` to `Quote`, or, if it is an alias, write the type it stands for where it is used\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if __undra_id != ::undra::meta::ids::type_id("Quote") {
            ::core::panic!(
                "error[undra::E0061]: `Quote` here is an alias or a renamed import of an Undra type that is declared under another name\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type that name resolves to; with `type Quote = Other` or `use path::Other as Quote` the platforms would be told `Quote` and receive the layout of `Other`\n  = help: write the type under the name it is declared with (`Other` in the examples above), or declare a separate `#[undra::api] struct Quote` if you mean a distinct type\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
    };
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
        if !<HttpError>::UNDRA_IS_ERROR {
            ::core::panic!(
                "error[undra::E0001]: `HttpError` is used as the error type of a `Result`, but it is not a `#[undra::error]` enum\n  = note: the platforms throw the error type by name, and only `#[undra::error]` enums carry the messages they show\n  = help: declare it with `#[undra::error]`, for example `#[undra::error] enum HttpError {{ #[error(\"failed\")] Failed }}`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0001"
            );
        }
    };
};
