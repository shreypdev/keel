/// Greets.
pub async fn greet(ctx: &Ctx, name: String) -> Result<String, GreetError> {
    todo!()
}
#[doc(hidden)]
#[allow(non_snake_case, unused_variables, unused_mut, deprecated, clippy::all)]
fn __undra_dispatch_fn_greet(
    __rt: &dyn ::core::any::Any,
    __call: ::undra::meta::DispatchCall<'_>,
) -> ::undra::meta::DispatchOutcome {
    fn __undra_out(
        __result: ::undra::runtime::DispatchResult,
    ) -> ::undra::meta::DispatchOutcome {
        ::undra::meta::DispatchOutcome::new(__result)
    }
    fn __undra_unknown() -> ::undra::meta::DispatchOutcome {
        __undra_out(::undra::runtime::DispatchResult::Unknown)
    }
    #[allow(dead_code)]
    fn __undra_bad_request(
        __reason: ::std::string::String,
    ) -> ::undra::meta::DispatchOutcome {
        __undra_out(::undra::runtime::DispatchResult::BadRequest(__reason))
    }
    fn _undra_error_E0022_the_future_of_an_async_method_must_be_Send<
        T: ::core::marker::Send,
    >(_: &T) {}
    let ::core::option::Option::Some(__rt) = __rt
        .downcast_ref::<::undra::runtime::Runtime>() else {
        return __undra_unknown();
    };
    if __call.method_id != ::undra::meta::ids::function_id("greet") {
        return __undra_unknown();
    }
    let mut __r = ::undra::wire::Reader::new(__call.args);
    let __undra_a0: String = match <String as ::undra::wire::Decode>::decode(&mut __r) {
        ::core::result::Result::Ok(__v) => __v,
        ::core::result::Result::Err(__e) => {
            return __undra_bad_request(
                ::std::format!(
                    "cannot decode argument `{}` of `{}`: {}", "name", "greet", __e
                ),
            );
        }
    };
    if let ::core::result::Result::Err(__e) = __r.finish() {
        return __undra_bad_request(
            ::std::format!("cannot decode the arguments of `{}`: {}", "greet", __e),
        );
    }
    let __ctx = __rt.ctx();
    {
        let __fut = async move {
            match greet(&__ctx, __undra_a0).await {
                ::core::result::Result::Ok(__v) => {
                    ::core::result::Result::<
                        _,
                        ::std::vec::Vec<u8>,
                    >::Ok(::undra::wire::Encode::encode_to_vec(&__v))
                }
                ::core::result::Result::Err(__e) => {
                    ::core::result::Result::<
                        ::std::vec::Vec<u8>,
                        _,
                    >::Err(::undra::wire::Encode::encode_to_vec(&__e))
                }
            }
        };
        _undra_error_E0022_the_future_of_an_async_method_must_be_Send(&__fut);
        __undra_out(
            ::undra::runtime::DispatchResult::Async(::std::boxed::Box::pin(__fut)),
        )
    }
}
#[allow(non_upper_case_globals)]
static __UNDRA_META_fn_greet: ::undra::meta::FunctionMeta = ::undra::meta::FunctionMeta {
    name: "greet",
    method_id: ::undra::meta::ids::function_id("greet"),
    params: &[
        ::undra::meta::ParamMeta {
            name: "name",
            ty: ::undra::meta::TypeRefMeta::String,
        },
    ],
    returns: ::undra::meta::TypeRefMeta::Result(
        &::undra::meta::TypeRefMeta::String,
        &::undra::meta::TypeRefMeta::Named("GreetError"),
    ),
    is_async: true,
    takes_ctx: true,
    docs: "Greets.",
    dispatch: __undra_dispatch_fn_greet,
};
::undra::meta::inventory::submit! {
    ::undra::meta::Registration::Function(& __UNDRA_META_fn_greet)
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
    fn __undra_identity() {
        __undra_same::<String, ::std::string::String>();
        __undra_same::<
            Result<String, GreetError>,
            ::core::result::Result<String, GreetError>,
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
        if <GreetError>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0064]: `GreetError` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a variant field, a signal value, a map entry or a query value\n  = help: return it as `Arc<GreetError>`, take it as `&GreetError` or `Arc<GreetError>` (a method, constructor or function parameter or return), or use a record with the data the platform needs\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0064"
            );
        }
        let __undra_id = <GreetError>::UNDRA_TYPE_ID;
        if __undra_id == 0 {
            ::core::panic!(
                "error[undra::E0061]: `GreetError` is not a type declared with `#[undra::api]`\n  = note: Undra describes a type to the platforms by the name it is written with, so the name must be a record or enum declared with `#[undra::api]` or an error declared with `#[undra::error]`; anything else, such as a plain struct or an alias (`type Id = u64`), has no definition the platforms could generate\n  = help: add `#[undra::api]` to `GreetError`, or, if it is an alias, write the type it stands for where it is used\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if __undra_id != ::undra::meta::ids::type_id("GreetError") {
            ::core::panic!(
                "error[undra::E0061]: `GreetError` here is an alias or a renamed import of an Undra type that is declared under another name\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type that name resolves to; with `type GreetError = Other` or `use path::Other as GreetError` the platforms would be told `GreetError` and receive the layout of `Other`\n  = help: write the type under the name it is declared with (`Other` in the examples above), or declare a separate `#[undra::api] struct GreetError` if you mean a distinct type\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if !<GreetError>::UNDRA_IS_ERROR {
            ::core::panic!(
                "error[undra::E0001]: `GreetError` is used as the error type of a `Result`, but it is not a `#[undra::error]` enum\n  = note: the platforms throw the error type by name, and only `#[undra::error]` enums carry the messages they show\n  = help: declare it with `#[undra::error]`, for example `#[undra::error] enum GreetError {{ #[error(\"failed\")] Failed }}`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0001"
            );
        }
    };
};
