/// A calculator.
impl Calculator {
    /// Creates one.
    pub fn new(ctx: &Ctx, base: i64) -> Self {
        todo!()
    }
    pub fn open(path: String) -> Result<Self, CalcError> {
        todo!()
    }
    /// Adds.
    pub fn add(&self, a: i64, b: i64) -> i64 {
        todo!()
    }
    pub fn divide(&self, a: i64, b: i64) -> Result<i64, CalcError> {
        todo!()
    }
    pub async fn slow_add(&self, a: i64) -> i64 {
        todo!()
    }
    pub async fn slow_divide(&self, a: i64, b: i64) -> Result<i64, CalcError> {
        todo!()
    }
    pub fn counts(&self, up_to: u32) -> impl Stream<Item = u32> + 'static {
        todo!()
    }
    fn helper(&self) {}
}
#[doc(hidden)]
#[allow(non_upper_case_globals, dead_code)]
const _undra_error_E0007_Calculator_has_two_undra_api_impl_blocks_merge_them_into_one: () = ();
impl Calculator {
    /// Marks the type as an object, so a signature that uses it as a value can say so.
    #[doc(hidden)]
    pub const __UNDRA_IS_OBJECT: bool = true;
    /// The type id of the declared name: what a signature that takes or returns the
    /// object as `Arc<T>` or `&T` is checked against (E0061).
    #[doc(hidden)]
    pub const __UNDRA_OBJECT_ID: u32 = ::undra::meta::ids::type_id("Calculator");
}
#[doc(hidden)]
#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code)]
const _: () = {
    trait __UndraStoreProbe_Calculator {
        const __UNDRA_IS_STORE: bool = false;
        const __UNDRA_DOCS: &'static str = "";
        fn __undra_cell_ref(&self) -> &::std::sync::Arc<::undra::signals::StoreCell> {
            ::core::unreachable!("not a `#[undra::store]`: E0011 stops the build first")
        }
        fn __undra_restore(
            _ctx: ::undra::runtime::Ctx,
            _r: &mut ::undra::wire::Reader<'_>,
        ) -> ::core::result::Result<Self, ::undra::wire::WireError>
        where
            Self: ::core::marker::Sized,
        {
            ::core::unreachable!("not a `#[undra::store]`: E0011 stops the build first")
        }
        const __UNDRA_STORE_META: ::undra::meta::StoreMeta = ::undra::meta::StoreMeta {
            signals: &[],
        };
        fn __undra_attach_all(
            &self,
        ) -> ::core::result::Result<(), ::undra::signals::SignalsError> {
            ::core::result::Result::Ok(())
        }
        fn __undra_set_handle(&self, _handle: u64) {}
    }
    impl __UndraStoreProbe_Calculator for Calculator {}
    fn __undra_store_probe() -> [(); {
        ::core::assert!(
            ! < Calculator > ::__UNDRA_IS_STORE,
            "error[undra::E0011]: `Calculator` is a `#[undra::store]` but its `#[undra::api]` impl block is not marked as a store\n  = note: the impl block of a store must say so, so its constructors can attach the store's signals and its struct literals get the hidden cell field\n  = help: write `#[undra::api(store)]` on the impl block\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0011"
        );
        0
    }] {
        []
    }
    #[automatically_derived]
    impl ::undra::runtime::UndraObject for Calculator {
        const TYPE_ID: u32 = ::undra::meta::ids::type_id("Calculator");
        const NAME: &'static str = "Calculator";
    }
    #[allow(unused_variables, unused_mut, deprecated, clippy::all)]
    fn __undra_dispatch_Calculator(
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
        struct __UndraMap<S>(::core::pin::Pin<::std::boxed::Box<S>>);
        impl<S> ::undra::runtime::Stream for __UndraMap<S>
        where
            S: ::undra::runtime::Stream,
            S::Item: ::undra::wire::Encode,
        {
            type Item = ::core::result::Result<::std::vec::Vec<u8>, ::std::vec::Vec<u8>>;
            fn poll_next(
                self: ::core::pin::Pin<&mut Self>,
                __cx: &mut ::core::task::Context<'_>,
            ) -> ::core::task::Poll<::core::option::Option<Self::Item>> {
                let __this = self.get_mut();
                match ::undra::runtime::Stream::poll_next(__this.0.as_mut(), __cx) {
                    ::core::task::Poll::Ready(::core::option::Option::Some(__item)) => {
                        ::core::task::Poll::Ready(
                            ::core::option::Option::Some(
                                ::core::result::Result::Ok(
                                    ::undra::wire::Encode::encode_to_vec(&__item),
                                ),
                            ),
                        )
                    }
                    ::core::task::Poll::Ready(::core::option::Option::None) => {
                        ::core::task::Poll::Ready(::core::option::Option::None)
                    }
                    ::core::task::Poll::Pending => ::core::task::Poll::Pending,
                }
            }
        }
        let ::core::option::Option::Some(__rt) = __rt
            .downcast_ref::<::undra::runtime::Runtime>() else {
            return __undra_unknown();
        };
        const __UNDRA_ID_new: u32 = ::undra::meta::ids::method_id("Calculator", "new");
        const __UNDRA_ID_open: u32 = ::undra::meta::ids::method_id("Calculator", "open");
        const __UNDRA_ID_add: u32 = ::undra::meta::ids::method_id("Calculator", "add");
        const __UNDRA_ID_divide: u32 = ::undra::meta::ids::method_id(
            "Calculator",
            "divide",
        );
        const __UNDRA_ID_slow_add: u32 = ::undra::meta::ids::method_id(
            "Calculator",
            "slow_add",
        );
        const __UNDRA_ID_slow_divide: u32 = ::undra::meta::ids::method_id(
            "Calculator",
            "slow_divide",
        );
        const __UNDRA_ID_counts: u32 = ::undra::meta::ids::method_id(
            "Calculator",
            "counts",
        );
        match __call.method_id {
            __UNDRA_ID_new => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                let __undra_a0: i64 = match <i64 as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot decode argument `{}` of `{}`: {}", "base",
                                "Calculator.new", __e
                            ),
                        );
                    }
                };
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}", "Calculator.new",
                            __e
                        ),
                    );
                }
                let __ctx = __rt.ctx();
                {
                    let __value = Calculator::new(&__ctx, __undra_a0);
                    {
                        let __handle = __rt
                            .insert_object(::std::sync::Arc::new(__value));
                        __rt.sync_ok(&__handle, ::undra::wire::Encode::encode)
                    }
                }
            }
            __UNDRA_ID_open => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                let __undra_a0: String = match <String as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot decode argument `{}` of `{}`: {}", "path",
                                "Calculator.open", __e
                            ),
                        );
                    }
                };
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}", "Calculator.open",
                            __e
                        ),
                    );
                }
                match Calculator::open(__undra_a0) {
                    ::core::result::Result::Ok(__value) => {
                        let __handle = __rt
                            .insert_object(::std::sync::Arc::new(__value));
                        __rt.sync_ok(&__handle, ::undra::wire::Encode::encode)
                    }
                    ::core::result::Result::Err(__e) => {
                        __rt.sync_err(&__e, ::undra::wire::Encode::encode)
                    }
                }
            }
            __UNDRA_ID_add => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                let __undra_a0: i64 = match <i64 as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot decode argument `{}` of `{}`: {}", "a",
                                "Calculator.add", __e
                            ),
                        );
                    }
                };
                let __undra_a1: i64 = match <i64 as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot decode argument `{}` of `{}`: {}", "b",
                                "Calculator.add", __e
                            ),
                        );
                    }
                };
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}", "Calculator.add",
                            __e
                        ),
                    );
                }
                let __obj = match __rt.object::<Calculator>(__call.handle) {
                    ::core::result::Result::Ok(__o) => __o,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!("cannot call `{}`: {}", "Calculator.add", __e),
                        );
                    }
                };
                {
                    let __out = Calculator::add(&*__obj, __undra_a0, __undra_a1);
                    __rt.sync_ok(&__out, ::undra::wire::Encode::encode)
                }
            }
            __UNDRA_ID_divide => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                let __undra_a0: i64 = match <i64 as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot decode argument `{}` of `{}`: {}", "a",
                                "Calculator.divide", __e
                            ),
                        );
                    }
                };
                let __undra_a1: i64 = match <i64 as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot decode argument `{}` of `{}`: {}", "b",
                                "Calculator.divide", __e
                            ),
                        );
                    }
                };
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}",
                            "Calculator.divide", __e
                        ),
                    );
                }
                let __obj = match __rt.object::<Calculator>(__call.handle) {
                    ::core::result::Result::Ok(__o) => __o,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot call `{}`: {}", "Calculator.divide", __e
                            ),
                        );
                    }
                };
                match Calculator::divide(&*__obj, __undra_a0, __undra_a1) {
                    ::core::result::Result::Ok(__v) => {
                        __rt.sync_ok(&__v, ::undra::wire::Encode::encode)
                    }
                    ::core::result::Result::Err(__e) => {
                        __rt.sync_err(&__e, ::undra::wire::Encode::encode)
                    }
                }
            }
            __UNDRA_ID_slow_add => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                let __undra_a0: i64 = match <i64 as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot decode argument `{}` of `{}`: {}", "a",
                                "Calculator.slow_add", __e
                            ),
                        );
                    }
                };
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}",
                            "Calculator.slow_add", __e
                        ),
                    );
                }
                let __obj = match __rt.object::<Calculator>(__call.handle) {
                    ::core::result::Result::Ok(__o) => __o,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot call `{}`: {}", "Calculator.slow_add", __e
                            ),
                        );
                    }
                };
                {
                    let __fut = async move {
                        let __out = Calculator::slow_add(&*__obj, __undra_a0).await;
                        ::core::result::Result::<
                            _,
                            ::std::vec::Vec<u8>,
                        >::Ok(::undra::wire::Encode::encode_to_vec(&__out))
                    };
                    _undra_error_E0022_the_future_of_an_async_method_must_be_Send(
                        &__fut,
                    );
                    __undra_out(
                        ::undra::runtime::DispatchResult::Async(
                            ::std::boxed::Box::pin(__fut),
                        ),
                    )
                }
            }
            __UNDRA_ID_slow_divide => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                let __undra_a0: i64 = match <i64 as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot decode argument `{}` of `{}`: {}", "a",
                                "Calculator.slow_divide", __e
                            ),
                        );
                    }
                };
                let __undra_a1: i64 = match <i64 as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot decode argument `{}` of `{}`: {}", "b",
                                "Calculator.slow_divide", __e
                            ),
                        );
                    }
                };
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}",
                            "Calculator.slow_divide", __e
                        ),
                    );
                }
                let __obj = match __rt.object::<Calculator>(__call.handle) {
                    ::core::result::Result::Ok(__o) => __o,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot call `{}`: {}", "Calculator.slow_divide", __e
                            ),
                        );
                    }
                };
                {
                    let __fut = async move {
                        match Calculator::slow_divide(&*__obj, __undra_a0, __undra_a1)
                            .await
                        {
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
                    _undra_error_E0022_the_future_of_an_async_method_must_be_Send(
                        &__fut,
                    );
                    __undra_out(
                        ::undra::runtime::DispatchResult::Async(
                            ::std::boxed::Box::pin(__fut),
                        ),
                    )
                }
            }
            __UNDRA_ID_counts => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                let __undra_a0: u32 = match <u32 as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot decode argument `{}` of `{}`: {}", "up_to",
                                "Calculator.counts", __e
                            ),
                        );
                    }
                };
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}",
                            "Calculator.counts", __e
                        ),
                    );
                }
                let __obj = match __rt.object::<Calculator>(__call.handle) {
                    ::core::result::Result::Ok(__o) => __o,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot call `{}`: {}", "Calculator.counts", __e
                            ),
                        );
                    }
                };
                {
                    let __stream = __UndraMap(
                        ::std::boxed::Box::pin(Calculator::counts(&*__obj, __undra_a0)),
                    );
                    _undra_error_E0022_the_future_of_an_async_method_must_be_Send(
                        &__stream,
                    );
                    __undra_out(
                        ::undra::runtime::DispatchResult::Stream(
                            ::std::boxed::Box::pin(__stream),
                        ),
                    )
                }
            }
            _ => __undra_unknown(),
        }
    }
    static __UNDRA_META_Calculator: ::undra::meta::ObjectMeta = ::undra::meta::ObjectMeta {
        name: "Calculator",
        type_id: ::undra::meta::ids::type_id("Calculator"),
        constructors: &[
            ::undra::meta::MethodMeta {
                name: "new",
                method_id: ::undra::meta::ids::method_id("Calculator", "new"),
                params: &[
                    ::undra::meta::ParamMeta {
                        name: "base",
                        ty: ::undra::meta::TypeRefMeta::I64,
                    },
                ],
                returns: ::undra::meta::TypeRefMeta::Named("Calculator"),
                is_async: false,
                takes_ctx: true,
                coalesce: false,
                docs: "Creates one.",
            },
            ::undra::meta::MethodMeta {
                name: "open",
                method_id: ::undra::meta::ids::method_id("Calculator", "open"),
                params: &[
                    ::undra::meta::ParamMeta {
                        name: "path",
                        ty: ::undra::meta::TypeRefMeta::String,
                    },
                ],
                returns: ::undra::meta::TypeRefMeta::Result(
                    &::undra::meta::TypeRefMeta::Named("Calculator"),
                    &::undra::meta::TypeRefMeta::Named("CalcError"),
                ),
                is_async: false,
                takes_ctx: false,
                coalesce: false,
                docs: "",
            },
        ],
        methods: &[
            ::undra::meta::MethodMeta {
                name: "add",
                method_id: ::undra::meta::ids::method_id("Calculator", "add"),
                params: &[
                    ::undra::meta::ParamMeta {
                        name: "a",
                        ty: ::undra::meta::TypeRefMeta::I64,
                    },
                    ::undra::meta::ParamMeta {
                        name: "b",
                        ty: ::undra::meta::TypeRefMeta::I64,
                    },
                ],
                returns: ::undra::meta::TypeRefMeta::I64,
                is_async: false,
                takes_ctx: false,
                coalesce: false,
                docs: "Adds.",
            },
            ::undra::meta::MethodMeta {
                name: "divide",
                method_id: ::undra::meta::ids::method_id("Calculator", "divide"),
                params: &[
                    ::undra::meta::ParamMeta {
                        name: "a",
                        ty: ::undra::meta::TypeRefMeta::I64,
                    },
                    ::undra::meta::ParamMeta {
                        name: "b",
                        ty: ::undra::meta::TypeRefMeta::I64,
                    },
                ],
                returns: ::undra::meta::TypeRefMeta::Result(
                    &::undra::meta::TypeRefMeta::I64,
                    &::undra::meta::TypeRefMeta::Named("CalcError"),
                ),
                is_async: false,
                takes_ctx: false,
                coalesce: false,
                docs: "",
            },
            ::undra::meta::MethodMeta {
                name: "slow_add",
                method_id: ::undra::meta::ids::method_id("Calculator", "slow_add"),
                params: &[
                    ::undra::meta::ParamMeta {
                        name: "a",
                        ty: ::undra::meta::TypeRefMeta::I64,
                    },
                ],
                returns: ::undra::meta::TypeRefMeta::I64,
                is_async: true,
                takes_ctx: false,
                coalesce: false,
                docs: "",
            },
            ::undra::meta::MethodMeta {
                name: "slow_divide",
                method_id: ::undra::meta::ids::method_id("Calculator", "slow_divide"),
                params: &[
                    ::undra::meta::ParamMeta {
                        name: "a",
                        ty: ::undra::meta::TypeRefMeta::I64,
                    },
                    ::undra::meta::ParamMeta {
                        name: "b",
                        ty: ::undra::meta::TypeRefMeta::I64,
                    },
                ],
                returns: ::undra::meta::TypeRefMeta::Result(
                    &::undra::meta::TypeRefMeta::I64,
                    &::undra::meta::TypeRefMeta::Named("CalcError"),
                ),
                is_async: true,
                takes_ctx: false,
                coalesce: false,
                docs: "",
            },
            ::undra::meta::MethodMeta {
                name: "counts",
                method_id: ::undra::meta::ids::method_id("Calculator", "counts"),
                params: &[
                    ::undra::meta::ParamMeta {
                        name: "up_to",
                        ty: ::undra::meta::TypeRefMeta::U32,
                    },
                ],
                returns: ::undra::meta::TypeRefMeta::Stream(
                    &::undra::meta::TypeRefMeta::U32,
                ),
                is_async: false,
                takes_ctx: false,
                coalesce: false,
                docs: "",
            },
        ],
        store: ::core::option::Option::None,
        docs: "A calculator.",
        dispatch: __undra_dispatch_Calculator,
    };
    ::undra::meta::inventory::submit! {
        ::undra::meta::Registration::Object(& __UNDRA_META_Calculator)
    }
};
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
        __undra_same::<i64, ::core::primitive::i64>();
        __undra_same::<String, ::std::string::String>();
        __undra_same::<Result<i64, CalcError>, ::core::result::Result<i64, CalcError>>();
        __undra_same::<u32, ::core::primitive::u32>();
    }
    trait __UndraFallback {
        const UNDRA_TYPE_ID: u32 = 0;
        const UNDRA_IS_ERROR: bool = false;
        const __UNDRA_IS_OBJECT: bool = false;
        const __UNDRA_OBJECT_ID: u32 = 0;
    }
    impl<T: ?::core::marker::Sized> __UndraFallback for T {}
    const _: () = {
        if <CalcError>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0064]: `CalcError` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a variant field, a signal value, a map entry or a query value\n  = help: return it as `Arc<CalcError>`, take it as `&CalcError` or `Arc<CalcError>` (a method, constructor or function parameter or return), or use a record with the data the platform needs\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0064"
            );
        }
        let __undra_id = <CalcError>::UNDRA_TYPE_ID;
        if __undra_id == 0 {
            ::core::panic!(
                "error[undra::E0061]: `CalcError` is not a type declared with `#[undra::api]`\n  = note: Undra describes a type to the platforms by the name it is written with, so the name must be a record or enum declared with `#[undra::api]` or an error declared with `#[undra::error]`; anything else, such as a plain struct or an alias (`type Id = u64`), has no definition the platforms could generate\n  = help: add `#[undra::api]` to `CalcError`, or, if it is an alias, write the type it stands for where it is used\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if __undra_id != ::undra::meta::ids::type_id("CalcError") {
            ::core::panic!(
                "error[undra::E0061]: `CalcError` here is an alias or a renamed import of an Undra type that is declared under another name\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type that name resolves to; with `type CalcError = Other` or `use path::Other as CalcError` the platforms would be told `CalcError` and receive the layout of `Other`\n  = help: write the type under the name it is declared with (`Other` in the examples above), or declare a separate `#[undra::api] struct CalcError` if you mean a distinct type\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if !<CalcError>::UNDRA_IS_ERROR {
            ::core::panic!(
                "error[undra::E0001]: `CalcError` is used as the error type of a `Result`, but it is not a `#[undra::error]` enum\n  = note: the platforms throw the error type by name, and only `#[undra::error]` enums carry the messages they show\n  = help: declare it with `#[undra::error]`, for example `#[undra::error] enum CalcError {{ #[error(\"failed\")] Failed }}`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0001"
            );
        }
    };
};
