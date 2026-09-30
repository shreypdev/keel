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
    let name: String = match <String as ::keel::wire::Decode>::decode(&mut __r) {
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
    __keel_out({
        let __fut = async move {
            match greet(&__ctx, name).await {
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
        ::keel::runtime::DispatchResult::Async(::std::boxed::Box::pin(__fut))
    })
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
