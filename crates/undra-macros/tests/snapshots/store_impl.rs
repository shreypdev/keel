impl Todos {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            rows: Signal::new(vec![]),
            __undra_cell: ::core::default::Default::default(),
        }
    }
    pub fn add(&self, title: String) {}
}
#[doc(hidden)]
#[allow(non_upper_case_globals, dead_code)]
const _undra_error_E0007_Todos_has_two_undra_api_impl_blocks_merge_them_into_one: () = ();
impl Todos {
    /// Marks the type as an object, so a signature that uses it as a value can say so.
    #[doc(hidden)]
    pub const __UNDRA_IS_OBJECT: bool = true;
}
#[automatically_derived]
impl ::undra::runtime::UndraObject for Todos {
    const TYPE_ID: u32 = ::undra::meta::ids::type_id("Todos");
    const NAME: &'static str = "Todos";
}
#[doc(hidden)]
#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code)]
const _: () = {
    trait __UndraStoreProbe_Todos {
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
    impl __UndraStoreProbe_Todos for Todos {}
    const _: () = {
        ::core::assert!(
            < Todos > ::__UNDRA_IS_STORE,
            "error[undra::E0011]: `Todos` is implemented with `#[undra::api(store)]` but the struct has no `#[undra::store]`\n  = note: the `store` marker wires the constructors to the struct's signals, which only `#[undra::store]` sets up\n  = help: add `#[undra::store]` to `struct Todos`, or remove `store` from the impl attribute\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0011"
        );
    };
    #[automatically_derived]
    impl ::undra::runtime::StoreObject for Todos {
        fn cell(&self) -> &::std::sync::Arc<::undra::signals::StoreCell> {
            self.__undra_cell_ref()
        }
        fn restore(
            __ctx: ::undra::runtime::Ctx,
            __r: &mut ::undra::wire::Reader<'_>,
        ) -> ::core::result::Result<Self, ::undra::wire::WireError> {
            Self::__undra_restore(__ctx, __r)
        }
    }
    #[allow(unused_variables, unused_mut, deprecated, clippy::all)]
    fn __undra_dispatch_Todos(
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
        const __UNDRA_ID_new: u32 = ::undra::meta::ids::method_id("Todos", "new");
        const __UNDRA_ID_add: u32 = ::undra::meta::ids::method_id("Todos", "add");
        match __call.method_id {
            __UNDRA_ID_new => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}", "Todos.new", __e
                        ),
                    );
                }
                let __ctx = __rt.ctx();
                {
                    let __value = Todos::new(__ctx);
                    match __value.__undra_attach_all() {
                        ::core::result::Result::Ok(()) => {
                            let __arc = ::std::sync::Arc::new(__value);
                            let __handle = __rt
                                .insert_object(::std::sync::Arc::clone(&__arc));
                            (*__arc).__undra_set_handle(__handle.0);
                            __rt.sync_ok(&__handle, ::undra::wire::Encode::encode)
                        }
                        ::core::result::Result::Err(__why) => {
                            __undra_bad_request(
                                ::std::format!(
                                    "store `{}` could not attach its signals: {}", "Todos",
                                    __why
                                ),
                            )
                        }
                    }
                }
            }
            __UNDRA_ID_add => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                let __undra_a0: String = match <String as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot decode argument `{}` of `{}`: {}", "title",
                                "Todos.add", __e
                            ),
                        );
                    }
                };
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}", "Todos.add", __e
                        ),
                    );
                }
                let __obj = match __rt.object::<Todos>(__call.handle) {
                    ::core::result::Result::Ok(__o) => __o,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!("cannot call `{}`: {}", "Todos.add", __e),
                        );
                    }
                };
                {
                    let __out = Todos::add(&*__obj, __undra_a0);
                    __rt.sync_ok(&__out, ::undra::wire::Encode::encode)
                }
            }
            _ => __undra_unknown(),
        }
    }
    static __UNDRA_META_Todos: ::undra::meta::ObjectMeta = ::undra::meta::ObjectMeta {
        name: "Todos",
        type_id: ::undra::meta::ids::type_id("Todos"),
        constructors: &[
            ::undra::meta::MethodMeta {
                name: "new",
                method_id: ::undra::meta::ids::method_id("Todos", "new"),
                params: &[],
                returns: ::undra::meta::TypeRefMeta::Named("Todos"),
                is_async: false,
                takes_ctx: true,
                docs: "",
            },
        ],
        methods: &[
            ::undra::meta::MethodMeta {
                name: "add",
                method_id: ::undra::meta::ids::method_id("Todos", "add"),
                params: &[
                    ::undra::meta::ParamMeta {
                        name: "title",
                        ty: ::undra::meta::TypeRefMeta::String,
                    },
                ],
                returns: ::undra::meta::TypeRefMeta::Unit,
                is_async: false,
                takes_ctx: false,
                docs: "",
            },
        ],
        store: ::core::option::Option::Some(<Todos>::__UNDRA_STORE_META),
        docs: {
            const __UNDRA_A: &str = <Todos>::__UNDRA_DOCS;
            const __UNDRA_B: &str = "";
            const __UNDRA_SEP: usize = if __UNDRA_A.is_empty() || __UNDRA_B.is_empty() {
                0
            } else {
                2
            };
            const __UNDRA_N: usize = __UNDRA_A.len() + __UNDRA_SEP + __UNDRA_B.len();
            const __UNDRA_BYTES: [u8; __UNDRA_N] = {
                let (__a, __b) = (__UNDRA_A.as_bytes(), __UNDRA_B.as_bytes());
                let mut __out = [0u8; __UNDRA_N];
                let mut __i = 0;
                while __i < __a.len() {
                    __out[__i] = __a[__i];
                    __i += 1;
                }
                if __UNDRA_SEP == 2 {
                    __out[__a.len()] = b'\n';
                    __out[__a.len() + 1] = b'\n';
                }
                let mut __j = 0;
                while __j < __b.len() {
                    __out[__a.len() + __UNDRA_SEP + __j] = __b[__j];
                    __j += 1;
                }
                __out
            };
            match ::core::str::from_utf8(&__UNDRA_BYTES) {
                ::core::result::Result::Ok(__s) => __s,
                ::core::result::Result::Err(_) => "",
            }
        },
        dispatch: __undra_dispatch_Todos,
    };
    ::undra::meta::inventory::submit! {
        ::undra::meta::Registration::Object(& __UNDRA_META_Todos)
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
        __undra_same::<String, ::std::string::String>();
    }
};
