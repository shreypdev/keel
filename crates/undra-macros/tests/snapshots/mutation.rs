pub async fn add_todo(ctx: Ctx, title: String) -> Result<Todo, HttpError> {
    todo!()
}
#[doc(hidden)]
macro_rules! _undra_error_E0007_a_mutation_is_a_free_function_move_it_out_of_the_impl_block {
    () => {
        #[doc =
        "The `add_todo` mutation: its identifiers and settings, and the function `undra-query` runs."]
        pub struct AddTodoMutation; impl AddTodoMutation { #[doc =
        r" The stable id: `fnv1a32` of `query.<fn>` or `mutation.<fn>`."] pub const
        MUTATION_ID : u32 = ::undra::meta::ids::mutation_id("add_todo"); #[doc =
        r" The cache key template."] pub const KEY : & 'static str = "todos"; #[doc =
        r" The staleness window in milliseconds, if any."] pub const STALE_MS :
        ::core::option::Option < u64 > = ::core::option::Option::None; #[doc =
        r" Whether results are persisted."] pub const PERSIST : bool = false; #[doc =
        r" Retry attempts after a failure."] pub const RETRY : u32 = 2u32; #[doc =
        r" Whether the call is safe to replay."] pub const IDEMPOTENT : bool = true; }
        #[allow(non_upper_case_globals)] static __UNDRA_META_AddTodoMutation :
        ::undra::meta::QueryMeta = ::undra::meta::QueryMeta { name : "add_todo", query_id
        : ::undra::meta::ids::mutation_id("add_todo"), kind :
        ::undra::meta::QueryKind::Mutation, key : "todos", params : &
        [::undra::meta::ParamMeta { name : "title", ty :
        ::undra::meta::TypeRefMeta::String }], returns :
        ::undra::meta::TypeRefMeta::Result(& ::undra::meta::TypeRefMeta::Named("Todo"), &
        ::undra::meta::TypeRefMeta::Named("HttpError")), stale_ms :
        ::core::option::Option::None, persist : false, idempotent : true, };
        ::undra::meta::inventory::submit! { ::undra::meta::Registration::Query(&
        __UNDRA_META_AddTodoMutation) } ::undra::meta::inventory::submit! {
        ::undra::query::MutationRegistration::of:: < AddTodoMutation > () }
        ::undra::meta::inventory::submit! { ::undra::query::__private::HYDRATE }
        ::undra::meta::inventory::submit! { ::undra::query::__private::LAYER }
    };
}
_undra_error_E0007_a_mutation_is_a_free_function_move_it_out_of_the_impl_block!();
#[automatically_derived]
impl ::undra::query::MutationDef for AddTodoMutation {
    const ID: u32 = Self::MUTATION_ID;
    const KEY: &'static str = Self::KEY;
    const RETRY: u32 = Self::RETRY;
    const IDEMPOTENT: bool = Self::IDEMPOTENT;
    type Input = (String,);
    type Output = Todo;
    type Error = HttpError;
    #[allow(unused_variables)]
    fn execute(
        __ctx: ::undra::runtime::Ctx,
        __params: Self::Input,
    ) -> ::core::pin::Pin<
        ::std::boxed::Box<
            dyn ::core::future::Future<
                Output = ::core::result::Result<Todo, HttpError>,
            > + ::core::marker::Send,
        >,
    > {
        #[allow(non_snake_case)]
        fn _undra_error_E0022_the_future_of_an_async_method_must_be_Send<
            T: ::core::marker::Send,
        >(_: &T) {}
        let (__undra_a0,) = __params;
        let __fut = async move { add_todo(__ctx, __undra_a0).await };
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
const __UNDRA_CHECKS_AddTodo: () = {
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
            Result<Todo, HttpError>,
            ::core::result::Result<Todo, HttpError>,
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
        if <Todo>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0064]: `Todo` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a variant field, a signal value, a map entry or a query value\n  = help: return it as `Arc<Todo>`, take it as `&Todo` or `Arc<Todo>` (a method, constructor or function parameter or return), or use a record with the data the platform needs\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0064"
            );
        }
        let __undra_id = <Todo>::UNDRA_TYPE_ID;
        if __undra_id == 0 {
            ::core::panic!(
                "error[undra::E0061]: `Todo` is not a type declared with `#[undra::api]`\n  = note: Undra describes a type to the platforms by the name it is written with, so the name must be a record or enum declared with `#[undra::api]` or an error declared with `#[undra::error]`; anything else, such as a plain struct or an alias (`type Id = u64`), has no definition the platforms could generate\n  = help: add `#[undra::api]` to `Todo`, or, if it is an alias, write the type it stands for where it is used\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if __undra_id != ::undra::meta::ids::type_id("Todo") {
            ::core::panic!(
                "error[undra::E0061]: `Todo` here is an alias or a renamed import of an Undra type that is declared under another name\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type that name resolves to; with `type Todo = Other` or `use path::Other as Todo` the platforms would be told `Todo` and receive the layout of `Other`\n  = help: write the type under the name it is declared with (`Other` in the examples above), or declare a separate `#[undra::api] struct Todo` if you mean a distinct type\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
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
