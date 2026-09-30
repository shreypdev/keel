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
#[allow(non_camel_case_types, dead_code)]
trait __KeelStoreProbe_Calculator {
    const __KEEL_IS_STORE: bool = false;
    const __KEEL_STORE_META: ::keel::meta::StoreMeta = ::keel::meta::StoreMeta {
        signals: &[],
    };
    fn __keel_attach_all(
        &self,
    ) -> ::core::result::Result<(), ::keel::signals::SignalsError> {
        ::core::result::Result::Ok(())
    }
    fn __keel_set_handle(&self, _handle: u64) {}
}
impl __KeelStoreProbe_Calculator for Calculator {}
const _: () = {
    ::core::assert!(
        ! < Calculator > ::__KEEL_IS_STORE,
        "error[keel::E0011]: `Calculator` is a `#[keel::store]` but its `#[keel::api]` impl block is not marked as a store\n  = note: the impl block of a store must say so, so its constructors can attach the store's signals and its struct literals get the hidden cell field\n  = help: write `#[keel::api(store)]` on the impl block\n  = docs: https://keel.dev/errors/E0011"
    );
};
#[automatically_derived]
impl ::keel::runtime::KeelObject for Calculator {
    const TYPE_ID: u32 = ::keel::meta::ids::type_id("Calculator");
    const NAME: &'static str = "Calculator";
}
#[doc(hidden)]
#[allow(
    non_snake_case,
    non_upper_case_globals,
    unused_variables,
    unused_mut,
    deprecated,
    clippy::all
)]
fn __keel_dispatch_Calculator(
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
    struct __KeelMap<S>(::core::pin::Pin<::std::boxed::Box<S>>);
    impl<S> ::keel::runtime::Stream for __KeelMap<S>
    where
        S: ::keel::runtime::Stream,
        S::Item: ::keel::wire::Encode,
    {
        type Item = ::core::result::Result<::std::vec::Vec<u8>, ::std::vec::Vec<u8>>;
        fn poll_next(
            self: ::core::pin::Pin<&mut Self>,
            __cx: &mut ::core::task::Context<'_>,
        ) -> ::core::task::Poll<::core::option::Option<Self::Item>> {
            let __this = self.get_mut();
            match ::keel::runtime::Stream::poll_next(__this.0.as_mut(), __cx) {
                ::core::task::Poll::Ready(::core::option::Option::Some(__item)) => {
                    ::core::task::Poll::Ready(
                        ::core::option::Option::Some(
                            ::core::result::Result::Ok(
                                ::keel::wire::Encode::encode_to_vec(&__item),
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
        .downcast_ref::<::keel::runtime::Runtime>() else {
        return __keel_unknown();
    };
    const __KEEL_ID_new: u32 = ::keel::meta::ids::method_id("Calculator", "new");
    const __KEEL_ID_open: u32 = ::keel::meta::ids::method_id("Calculator", "open");
    const __KEEL_ID_add: u32 = ::keel::meta::ids::method_id("Calculator", "add");
    const __KEEL_ID_divide: u32 = ::keel::meta::ids::method_id("Calculator", "divide");
    const __KEEL_ID_slow_add: u32 = ::keel::meta::ids::method_id(
        "Calculator",
        "slow_add",
    );
    const __KEEL_ID_slow_divide: u32 = ::keel::meta::ids::method_id(
        "Calculator",
        "slow_divide",
    );
    const __KEEL_ID_counts: u32 = ::keel::meta::ids::method_id("Calculator", "counts");
    match __call.method_id {
        __KEEL_ID_new => {
            let mut __r = ::keel::wire::Reader::new(__call.args);
            let base: i64 = match <i64 as ::keel::wire::Decode>::decode(&mut __r) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!(
                            "cannot decode argument `{}` of `{}`: {}", "base",
                            "Calculator.new", __e
                        ),
                    );
                }
            };
            if let ::core::result::Result::Err(__e) = __r.finish() {
                return __keel_bad_request(
                    ::std::format!(
                        "cannot decode the arguments of `{}`: {}", "Calculator.new", __e
                    ),
                );
            }
            let __ctx = __rt.ctx();
            __keel_out({
                let __value = Calculator::new(&__ctx, base);
                {
                    let __handle = __rt.insert_object(::std::sync::Arc::new(__value));
                    ::keel::runtime::DispatchResult::Sync(
                        ::core::result::Result::Ok(
                            ::keel::wire::Encode::encode_to_vec(&__handle),
                        ),
                    )
                }
            })
        }
        __KEEL_ID_open => {
            let mut __r = ::keel::wire::Reader::new(__call.args);
            let path: String = match <String as ::keel::wire::Decode>::decode(&mut __r) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!(
                            "cannot decode argument `{}` of `{}`: {}", "path",
                            "Calculator.open", __e
                        ),
                    );
                }
            };
            if let ::core::result::Result::Err(__e) = __r.finish() {
                return __keel_bad_request(
                    ::std::format!(
                        "cannot decode the arguments of `{}`: {}", "Calculator.open", __e
                    ),
                );
            }
            __keel_out(
                match Calculator::open(path) {
                    ::core::result::Result::Ok(__value) => {
                        let __handle = __rt
                            .insert_object(::std::sync::Arc::new(__value));
                        ::keel::runtime::DispatchResult::Sync(
                            ::core::result::Result::Ok(
                                ::keel::wire::Encode::encode_to_vec(&__handle),
                            ),
                        )
                    }
                    ::core::result::Result::Err(__e) => {
                        ::keel::runtime::DispatchResult::Sync(
                            ::core::result::Result::Err(
                                ::keel::wire::Encode::encode_to_vec(&__e),
                            ),
                        )
                    }
                },
            )
        }
        __KEEL_ID_add => {
            let mut __r = ::keel::wire::Reader::new(__call.args);
            let a: i64 = match <i64 as ::keel::wire::Decode>::decode(&mut __r) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!(
                            "cannot decode argument `{}` of `{}`: {}", "a",
                            "Calculator.add", __e
                        ),
                    );
                }
            };
            let b: i64 = match <i64 as ::keel::wire::Decode>::decode(&mut __r) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!(
                            "cannot decode argument `{}` of `{}`: {}", "b",
                            "Calculator.add", __e
                        ),
                    );
                }
            };
            if let ::core::result::Result::Err(__e) = __r.finish() {
                return __keel_bad_request(
                    ::std::format!(
                        "cannot decode the arguments of `{}`: {}", "Calculator.add", __e
                    ),
                );
            }
            let __obj = match __rt.object::<Calculator>(__call.handle) {
                ::core::result::Result::Ok(__o) => __o,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!("cannot call `{}`: {}", "Calculator.add", __e),
                    );
                }
            };
            __keel_out({
                let __out = Calculator::add(&*__obj, a, b);
                ::keel::runtime::DispatchResult::Sync(
                    ::core::result::Result::Ok(
                        ::keel::wire::Encode::encode_to_vec(&__out),
                    ),
                )
            })
        }
        __KEEL_ID_divide => {
            let mut __r = ::keel::wire::Reader::new(__call.args);
            let a: i64 = match <i64 as ::keel::wire::Decode>::decode(&mut __r) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!(
                            "cannot decode argument `{}` of `{}`: {}", "a",
                            "Calculator.divide", __e
                        ),
                    );
                }
            };
            let b: i64 = match <i64 as ::keel::wire::Decode>::decode(&mut __r) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!(
                            "cannot decode argument `{}` of `{}`: {}", "b",
                            "Calculator.divide", __e
                        ),
                    );
                }
            };
            if let ::core::result::Result::Err(__e) = __r.finish() {
                return __keel_bad_request(
                    ::std::format!(
                        "cannot decode the arguments of `{}`: {}", "Calculator.divide",
                        __e
                    ),
                );
            }
            let __obj = match __rt.object::<Calculator>(__call.handle) {
                ::core::result::Result::Ok(__o) => __o,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!("cannot call `{}`: {}", "Calculator.divide", __e),
                    );
                }
            };
            __keel_out(
                match Calculator::divide(&*__obj, a, b) {
                    ::core::result::Result::Ok(__v) => {
                        ::keel::runtime::DispatchResult::Sync(
                            ::core::result::Result::Ok(
                                ::keel::wire::Encode::encode_to_vec(&__v),
                            ),
                        )
                    }
                    ::core::result::Result::Err(__e) => {
                        ::keel::runtime::DispatchResult::Sync(
                            ::core::result::Result::Err(
                                ::keel::wire::Encode::encode_to_vec(&__e),
                            ),
                        )
                    }
                },
            )
        }
        __KEEL_ID_slow_add => {
            let mut __r = ::keel::wire::Reader::new(__call.args);
            let a: i64 = match <i64 as ::keel::wire::Decode>::decode(&mut __r) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!(
                            "cannot decode argument `{}` of `{}`: {}", "a",
                            "Calculator.slow_add", __e
                        ),
                    );
                }
            };
            if let ::core::result::Result::Err(__e) = __r.finish() {
                return __keel_bad_request(
                    ::std::format!(
                        "cannot decode the arguments of `{}`: {}", "Calculator.slow_add",
                        __e
                    ),
                );
            }
            let __obj = match __rt.object::<Calculator>(__call.handle) {
                ::core::result::Result::Ok(__o) => __o,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!(
                            "cannot call `{}`: {}", "Calculator.slow_add", __e
                        ),
                    );
                }
            };
            __keel_out({
                let __fut = async move {
                    let __out = Calculator::slow_add(&*__obj, a).await;
                    ::core::result::Result::<
                        _,
                        ::std::vec::Vec<u8>,
                    >::Ok(::keel::wire::Encode::encode_to_vec(&__out))
                };
                __keel_assert_send(&__fut);
                ::keel::runtime::DispatchResult::Async(::std::boxed::Box::pin(__fut))
            })
        }
        __KEEL_ID_slow_divide => {
            let mut __r = ::keel::wire::Reader::new(__call.args);
            let a: i64 = match <i64 as ::keel::wire::Decode>::decode(&mut __r) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!(
                            "cannot decode argument `{}` of `{}`: {}", "a",
                            "Calculator.slow_divide", __e
                        ),
                    );
                }
            };
            let b: i64 = match <i64 as ::keel::wire::Decode>::decode(&mut __r) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!(
                            "cannot decode argument `{}` of `{}`: {}", "b",
                            "Calculator.slow_divide", __e
                        ),
                    );
                }
            };
            if let ::core::result::Result::Err(__e) = __r.finish() {
                return __keel_bad_request(
                    ::std::format!(
                        "cannot decode the arguments of `{}`: {}",
                        "Calculator.slow_divide", __e
                    ),
                );
            }
            let __obj = match __rt.object::<Calculator>(__call.handle) {
                ::core::result::Result::Ok(__o) => __o,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!(
                            "cannot call `{}`: {}", "Calculator.slow_divide", __e
                        ),
                    );
                }
            };
            __keel_out({
                let __fut = async move {
                    match Calculator::slow_divide(&*__obj, a, b).await {
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
        __KEEL_ID_counts => {
            let mut __r = ::keel::wire::Reader::new(__call.args);
            let up_to: u32 = match <u32 as ::keel::wire::Decode>::decode(&mut __r) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!(
                            "cannot decode argument `{}` of `{}`: {}", "up_to",
                            "Calculator.counts", __e
                        ),
                    );
                }
            };
            if let ::core::result::Result::Err(__e) = __r.finish() {
                return __keel_bad_request(
                    ::std::format!(
                        "cannot decode the arguments of `{}`: {}", "Calculator.counts",
                        __e
                    ),
                );
            }
            let __obj = match __rt.object::<Calculator>(__call.handle) {
                ::core::result::Result::Ok(__o) => __o,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!("cannot call `{}`: {}", "Calculator.counts", __e),
                    );
                }
            };
            __keel_out({
                let __stream = __KeelMap(
                    ::std::boxed::Box::pin(Calculator::counts(&*__obj, up_to)),
                );
                __keel_assert_send(&__stream);
                ::keel::runtime::DispatchResult::Stream(::std::boxed::Box::pin(__stream))
            })
        }
        _ => __keel_unknown(),
    }
}
#[allow(non_upper_case_globals)]
static __KEEL_META_Calculator: ::keel::meta::ObjectMeta = ::keel::meta::ObjectMeta {
    name: "Calculator",
    type_id: ::keel::meta::ids::type_id("Calculator"),
    constructors: &[
        ::keel::meta::MethodMeta {
            name: "new",
            method_id: ::keel::meta::ids::method_id("Calculator", "new"),
            params: &[
                ::keel::meta::ParamMeta {
                    name: "base",
                    ty: ::keel::meta::TypeRefMeta::I64,
                },
            ],
            returns: ::keel::meta::TypeRefMeta::Named("Calculator"),
            is_async: false,
            takes_ctx: true,
            docs: "Creates one.",
        },
        ::keel::meta::MethodMeta {
            name: "open",
            method_id: ::keel::meta::ids::method_id("Calculator", "open"),
            params: &[
                ::keel::meta::ParamMeta {
                    name: "path",
                    ty: ::keel::meta::TypeRefMeta::String,
                },
            ],
            returns: ::keel::meta::TypeRefMeta::Result(
                &::keel::meta::TypeRefMeta::Named("Calculator"),
                &::keel::meta::TypeRefMeta::Named("CalcError"),
            ),
            is_async: false,
            takes_ctx: false,
            docs: "",
        },
    ],
    methods: &[
        ::keel::meta::MethodMeta {
            name: "add",
            method_id: ::keel::meta::ids::method_id("Calculator", "add"),
            params: &[
                ::keel::meta::ParamMeta {
                    name: "a",
                    ty: ::keel::meta::TypeRefMeta::I64,
                },
                ::keel::meta::ParamMeta {
                    name: "b",
                    ty: ::keel::meta::TypeRefMeta::I64,
                },
            ],
            returns: ::keel::meta::TypeRefMeta::I64,
            is_async: false,
            takes_ctx: false,
            docs: "Adds.",
        },
        ::keel::meta::MethodMeta {
            name: "divide",
            method_id: ::keel::meta::ids::method_id("Calculator", "divide"),
            params: &[
                ::keel::meta::ParamMeta {
                    name: "a",
                    ty: ::keel::meta::TypeRefMeta::I64,
                },
                ::keel::meta::ParamMeta {
                    name: "b",
                    ty: ::keel::meta::TypeRefMeta::I64,
                },
            ],
            returns: ::keel::meta::TypeRefMeta::Result(
                &::keel::meta::TypeRefMeta::I64,
                &::keel::meta::TypeRefMeta::Named("CalcError"),
            ),
            is_async: false,
            takes_ctx: false,
            docs: "",
        },
        ::keel::meta::MethodMeta {
            name: "slow_add",
            method_id: ::keel::meta::ids::method_id("Calculator", "slow_add"),
            params: &[
                ::keel::meta::ParamMeta {
                    name: "a",
                    ty: ::keel::meta::TypeRefMeta::I64,
                },
            ],
            returns: ::keel::meta::TypeRefMeta::I64,
            is_async: true,
            takes_ctx: false,
            docs: "",
        },
        ::keel::meta::MethodMeta {
            name: "slow_divide",
            method_id: ::keel::meta::ids::method_id("Calculator", "slow_divide"),
            params: &[
                ::keel::meta::ParamMeta {
                    name: "a",
                    ty: ::keel::meta::TypeRefMeta::I64,
                },
                ::keel::meta::ParamMeta {
                    name: "b",
                    ty: ::keel::meta::TypeRefMeta::I64,
                },
            ],
            returns: ::keel::meta::TypeRefMeta::Result(
                &::keel::meta::TypeRefMeta::I64,
                &::keel::meta::TypeRefMeta::Named("CalcError"),
            ),
            is_async: true,
            takes_ctx: false,
            docs: "",
        },
        ::keel::meta::MethodMeta {
            name: "counts",
            method_id: ::keel::meta::ids::method_id("Calculator", "counts"),
            params: &[
                ::keel::meta::ParamMeta {
                    name: "up_to",
                    ty: ::keel::meta::TypeRefMeta::U32,
                },
            ],
            returns: ::keel::meta::TypeRefMeta::Stream(&::keel::meta::TypeRefMeta::U32),
            is_async: false,
            takes_ctx: false,
            docs: "",
        },
    ],
    store: ::core::option::Option::None,
    docs: "A calculator.",
    dispatch: __keel_dispatch_Calculator,
};
::keel::meta::inventory::submit! {
    ::keel::meta::Registration::Object(& __KEEL_META_Calculator)
}
