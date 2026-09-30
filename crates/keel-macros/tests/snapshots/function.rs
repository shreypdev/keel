/// Greets.
pub async fn greet(ctx: &Ctx, name: String) -> Result<String, GreetError> {
    todo!()
}
#[doc(hidden)]
#[allow(non_snake_case, unused_variables, unused_mut, deprecated, clippy::all)]
fn __keel_dispatch_fn_greet(
    __rt: &dyn ::core::any::Any,
    __call: ::keel::meta::DispatchCall<'_>,
) -> ::keel::meta::DispatchOutcome {
    fn __keel_out(
        __result: ::keel::runtime::DispatchResult,
    ) -> ::keel::meta::DispatchOutcome {
        ::keel::meta::DispatchOutcome::new(__result)
    }
    fn __keel_unknown() -> ::keel::meta::DispatchOutcome {
        __keel_out(::keel::runtime::DispatchResult::Unknown)
    }
    #[allow(dead_code)]
    fn __keel_bad_request(
        __reason: ::std::string::String,
    ) -> ::keel::meta::DispatchOutcome {
        __keel_out(::keel::runtime::DispatchResult::BadRequest(__reason))
    }
    fn __keel_assert_send<T: ::core::marker::Send>(_: &T) {}
    let ::core::option::Option::Some(__rt) = __rt
        .downcast_ref::<::keel::runtime::Runtime>() else {
        return __keel_unknown();
    };
    if __call.method_id != ::keel::meta::ids::function_id("greet") {
        return __keel_unknown();
    }
    let mut __r = ::keel::wire::Reader::new(__call.args);
    let __keel_a0: String = match <String as ::keel::wire::Decode>::decode(&mut __r) {
        ::core::result::Result::Ok(__v) => __v,
        ::core::result::Result::Err(__e) => {
            return __keel_bad_request(
                ::std::format!(
                    "cannot decode argument `{}` of `{}`: {}", "name", "greet", __e
                ),
            );
        }
    };
    if let ::core::result::Result::Err(__e) = __r.finish() {
        return __keel_bad_request(
            ::std::format!("cannot decode the arguments of `{}`: {}", "greet", __e),
        );
    }
    let __ctx = __rt.ctx();
    {
        let __fut = async move {
            match greet(&__ctx, __keel_a0).await {
                ::core::result::Result::Ok(__v) => {
                    ::core::result::Result::<
                        _,
                        ::std::vec::Vec<u8>,
                    >::Ok(::keel::wire::Encode::encode_to_vec(&__v))
                }
                ::core::result::Result::Err(__e) => {
                    ::core::result::Result::<
                        ::std::vec::Vec<u8>,
                        _,
                    >::Err(::keel::wire::Encode::encode_to_vec(&__e))
                }
            }
        };
        __keel_assert_send(&__fut);
        __keel_out(::keel::runtime::DispatchResult::Async(::std::boxed::Box::pin(__fut)))
    }
}
#[allow(non_upper_case_globals)]
static __KEEL_META_fn_greet: ::keel::meta::FunctionMeta = ::keel::meta::FunctionMeta {
    name: "greet",
    method_id: ::keel::meta::ids::function_id("greet"),
    params: &[
        ::keel::meta::ParamMeta {
            name: "name",
            ty: ::keel::meta::TypeRefMeta::String,
        },
    ],
    returns: ::keel::meta::TypeRefMeta::Result(
        &::keel::meta::TypeRefMeta::String,
        &::keel::meta::TypeRefMeta::Named("GreetError"),
    ),
    is_async: true,
    takes_ctx: true,
    docs: "Greets.",
    dispatch: __keel_dispatch_fn_greet,
};
::keel::meta::inventory::submit! {
    ::keel::meta::Registration::Function(& __KEEL_META_fn_greet)
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
        __keel_same::<String, ::std::string::String>();
        __keel_same::<
            Result<String, GreetError>,
            ::core::result::Result<String, GreetError>,
        >();
    }
    trait __KeelFallback {
        const KEEL_TYPE_ID: u32 = 0;
        const KEEL_IS_ERROR: bool = false;
        const __KEEL_IS_OBJECT: bool = false;
    }
    impl<T: ?::core::marker::Sized> __KeelFallback for T {}
    const _: () = {
        if <GreetError>::__KEEL_IS_OBJECT {
            ::core::panic!(
                "error[keel::E0064]: `GreetError` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a parameter or a return value\n  = help: return a record with the data the platform needs, or construct the object from the platform with one of its constructors\n  = docs: https://keel.dev/errors/E0064"
            );
        }
        if <GreetError>::KEEL_TYPE_ID != ::keel::meta::ids::type_id("GreetError") {
            ::core::panic!(
                "error[keel::E0061]: the schema records this type as `GreetError`, but the type written here is not that type\n  = note: Keel describes a type to the platforms by the name it is written with, while the generated code encodes the type the name resolves to; an alias (`type GreetError = Other`), a renamed import (`use path::Other as GreetError`) or a type that is not declared with `#[keel::api]` makes the two differ, so the platforms would read the wrong layout\n  = help: write the type under its declared name (for an alias or a renamed import, use `Other` here), or declare it with `#[keel::api]` (`#[keel::error]` for errors)\n  = docs: https://keel.dev/errors/E0061"
            );
        }
        if !<GreetError>::KEEL_IS_ERROR {
            ::core::panic!(
                "error[keel::E0001]: `GreetError` is used as the error type of a `Result`, but it is not a `#[keel::error]` enum\n  = note: the platforms throw the error type by name, and only `#[keel::error]` enums carry the messages they show\n  = help: declare it with `#[keel::error]`, for example `#[keel::error] enum GreetError {{ #[error(\"failed\")] Failed }}`\n  = docs: https://keel.dev/errors/E0001"
            );
        }
    };
};
