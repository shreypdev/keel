#[doc(hidden)]
#[allow(non_upper_case_globals, dead_code)]
const _undra_error_E0070_this_instantiation_of_Cache_Todo_is_declared_twice_keep_one_alias_per_instantiation: () = ();
#[doc(hidden)]
#[allow(non_upper_case_globals, dead_code)]
const _undra_error_E0007_TodoCache_has_two_undra_api_impl_blocks_merge_them_into_one: () = ();
impl TodoCache {
    #[doc(hidden)]
    #[allow(non_upper_case_globals)]
    pub const _undra_error_E0070_this_instantiation_of_Cache_is_declared_twice_keep_one_alias_per_instantiation: () = ();
    /// Marks the type as an object, so a signature that uses it as a value can say so.
    #[doc(hidden)]
    pub const __UNDRA_IS_OBJECT: bool = true;
    /// The type id of the declared name: what a signature that takes or returns the
    /// object as `Arc<T>` or `&T` is checked against (E0061).
    #[doc(hidden)]
    pub const __UNDRA_OBJECT_ID: u32 = ::undra::meta::ids::type_id("TodoCache");
    /// The declared name of the instantiation, for a signature that names it through a
    /// generic application (`Arc<Selection<T>>`).
    #[doc(hidden)]
    pub const __UNDRA_OBJECT_NAME: &'static str = "TodoCache";
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code)]
const _: () = {
    trait __UndraGenericStoreProbe {
        const __UNDRA_IS_GENERIC_STORE: bool = false;
    }
    impl<__UndraT: ?::core::marker::Sized> __UndraGenericStoreProbe for __UndraT {}
    ::core::assert!(
        ! < TodoCache > ::__UNDRA_IS_GENERIC_STORE,
        "error[undra::E0011]: `Cache` is a `#[undra::store(generic)]` but its impl block is not marked as a store\n  = note: the impl block of a store must say so, so its constructors can attach the store's signals and its struct literals get the hidden cell field\n  = help: write `#[undra::api(store, generic)]` on the impl block\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0011"
    );
};
#[doc(hidden)]
#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code)]
const _: () = {
    trait __UndraStoreProbe_TodoCache {
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
    impl __UndraStoreProbe_TodoCache for TodoCache {}
    fn __undra_store_probe() -> [(); {
        ::core::assert!(
            ! < TodoCache > ::__UNDRA_IS_STORE,
            "error[undra::E0011]: `TodoCache` is a `#[undra::store]` but its `#[undra::api]` impl block is not marked as a store\n  = note: the impl block of a store must say so, so its constructors can attach the store's signals and its struct literals get the hidden cell field\n  = help: write `#[undra::api(store)]` on the impl block\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0011"
        );
        0
    }] {
        []
    }
    #[automatically_derived]
    impl ::undra::runtime::UndraObject for TodoCache {
        const TYPE_ID: u32 = ::undra::meta::ids::type_id("TodoCache");
        const NAME: &'static str = "TodoCache";
    }
    #[allow(unused_variables, unused_mut, deprecated, clippy::all)]
    fn __undra_dispatch_TodoCache(
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
        let ::core::option::Option::Some(__rt) = __rt
            .downcast_ref::<::undra::runtime::Runtime>() else {
            return __undra_unknown();
        };
        const __UNDRA_ID_new: u32 = ::undra::meta::ids::method_id("TodoCache", "new");
        const __UNDRA_ID_put: u32 = ::undra::meta::ids::method_id("TodoCache", "put");
        const __UNDRA_ID_get: u32 = ::undra::meta::ids::method_id("TodoCache", "get");
        match __call.method_id {
            __UNDRA_ID_new => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}", "TodoCache.new",
                            __e
                        ),
                    );
                }
                {
                    let __value = TodoCache::new();
                    {
                        let __handle = __rt
                            .insert_object(::std::sync::Arc::new(__value));
                        __rt.sync_ok(&__handle, ::undra::wire::Encode::encode)
                    }
                }
            }
            __UNDRA_ID_put => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                let __undra_a0: Todo = match <Todo as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot decode argument `{}` of `{}`: {}", "row",
                                "TodoCache.put", __e
                            ),
                        );
                    }
                };
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}", "TodoCache.put",
                            __e
                        ),
                    );
                }
                let __obj = match __rt.object::<TodoCache>(__call.handle) {
                    ::core::result::Result::Ok(__o) => __o,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!("cannot call `{}`: {}", "TodoCache.put", __e),
                        );
                    }
                };
                {
                    let __out = TodoCache::put(&*__obj, __undra_a0);
                    __rt.sync_ok(&__out, ::undra::wire::Encode::encode)
                }
            }
            __UNDRA_ID_get => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                let __undra_a0: Uuid = match <Uuid as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot decode argument `{}` of `{}`: {}", "id",
                                "TodoCache.get", __e
                            ),
                        );
                    }
                };
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}", "TodoCache.get",
                            __e
                        ),
                    );
                }
                let __obj = match __rt.object::<TodoCache>(__call.handle) {
                    ::core::result::Result::Ok(__o) => __o,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!("cannot call `{}`: {}", "TodoCache.get", __e),
                        );
                    }
                };
                {
                    let __out = TodoCache::get(&*__obj, __undra_a0);
                    __rt.sync_ok(&__out, ::undra::wire::Encode::encode)
                }
            }
            _ => __undra_unknown(),
        }
    }
    static __UNDRA_META_TodoCache: ::undra::meta::ObjectMeta = ::undra::meta::ObjectMeta {
        name: "TodoCache",
        type_id: ::undra::meta::ids::type_id("TodoCache"),
        constructors: &[
            ::undra::meta::MethodMeta {
                name: "new",
                method_id: ::undra::meta::ids::method_id("TodoCache", "new"),
                params: &[],
                returns: ::undra::meta::TypeRefMeta::Named("TodoCache"),
                is_async: false,
                takes_ctx: false,
                coalesce: false,
                generic: ::core::option::Option::None,
                docs: "",
            },
        ],
        methods: &[
            ::undra::meta::MethodMeta {
                name: "put",
                method_id: ::undra::meta::ids::method_id("TodoCache", "put"),
                params: &[
                    ::undra::meta::ParamMeta {
                        name: "row",
                        ty: ::undra::meta::TypeRefMeta::Named("Todo"),
                    },
                ],
                returns: ::undra::meta::TypeRefMeta::Unit,
                is_async: false,
                takes_ctx: false,
                coalesce: false,
                generic: ::core::option::Option::None,
                docs: "Stores a row.",
            },
            ::undra::meta::MethodMeta {
                name: "get",
                method_id: ::undra::meta::ids::method_id("TodoCache", "get"),
                params: &[
                    ::undra::meta::ParamMeta {
                        name: "id",
                        ty: ::undra::meta::TypeRefMeta::Uuid,
                    },
                ],
                returns: ::undra::meta::TypeRefMeta::Option(
                    &::undra::meta::TypeRefMeta::Named("Todo"),
                ),
                is_async: false,
                takes_ctx: false,
                coalesce: false,
                generic: ::core::option::Option::None,
                docs: "",
            },
        ],
        store: ::core::option::Option::None,
        docs: "The todos the app has seen.",
        dispatch: __undra_dispatch_TodoCache,
    };
    ::undra::meta::inventory::submit! {
        ::undra::meta::Registration::Object(& __UNDRA_META_TodoCache)
    }
};
