impl Todos {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            rows: Signal::new(vec![]),
            __keel_cell: ::core::default::Default::default(),
        }
    }
    pub fn add(&self, title: String) {}
}
#[doc(hidden)]
#[allow(non_upper_case_globals, dead_code)]
const _keel_error_E0007_a_type_takes_one_keel_api_impl_block_Todos: () = ();
impl Todos {
    /// Marks the type as an object, so a signature that uses it as a value can say so.
    #[doc(hidden)]
    pub const __KEEL_IS_OBJECT: bool = true;
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code)]
trait __KeelStoreProbe_Todos {
    const __KEEL_IS_STORE: bool = false;
    const __KEEL_DOCS: &'static str = "";
    fn __keel_cell_ref(&self) -> &::std::sync::Arc<::keel::signals::StoreCell> {
        ::core::unreachable!("not a `#[keel::store]`: E0011 stops the build first")
    }
    fn __keel_restore(
        _ctx: ::keel::runtime::Ctx,
        _r: &mut ::keel::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::keel::wire::WireError>
    where
        Self: ::core::marker::Sized,
    {
        ::core::unreachable!("not a `#[keel::store]`: E0011 stops the build first")
    }
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
impl __KeelStoreProbe_Todos for Todos {}
const _: () = {
    ::core::assert!(
        < Todos > ::__KEEL_IS_STORE,
        "error[keel::E0011]: `Todos` is implemented with `#[keel::api(store)]` but the struct has no `#[keel::store]`\n  = note: the `store` marker wires the constructors to the struct's signals, which only `#[keel::store]` sets up\n  = help: add `#[keel::store]` to `struct Todos`, or remove `store` from the impl attribute\n  = docs: https://keel.dev/errors/E0011"
    );
};
#[automatically_derived]
impl ::keel::runtime::KeelObject for Todos {
    const TYPE_ID: u32 = ::keel::meta::ids::type_id("Todos");
    const NAME: &'static str = "Todos";
}
#[automatically_derived]
impl ::keel::runtime::StoreObject for Todos {
    fn cell(&self) -> &::std::sync::Arc<::keel::signals::StoreCell> {
        self.__keel_cell_ref()
    }
    fn restore(
        __ctx: ::keel::runtime::Ctx,
        __r: &mut ::keel::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::keel::wire::WireError> {
        Self::__keel_restore(__ctx, __r)
    }
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
fn __keel_dispatch_Todos(
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
    let ::core::option::Option::Some(__rt) = __rt
        .downcast_ref::<::keel::runtime::Runtime>() else {
        return __keel_unknown();
    };
    const __KEEL_ID_new: u32 = ::keel::meta::ids::method_id("Todos", "new");
    const __KEEL_ID_add: u32 = ::keel::meta::ids::method_id("Todos", "add");
    match __call.method_id {
        __KEEL_ID_new => {
            let mut __r = ::keel::wire::Reader::new(__call.args);
            if let ::core::result::Result::Err(__e) = __r.finish() {
                return __keel_bad_request(
                    ::std::format!(
                        "cannot decode the arguments of `{}`: {}", "Todos.new", __e
                    ),
                );
            }
            let __ctx = __rt.ctx();
            __keel_out({
                let __value = Todos::new(__ctx);
                match __value.__keel_attach_all() {
                    ::core::result::Result::Ok(()) => {
                        let __arc = ::std::sync::Arc::new(__value);
                        let __handle = __rt
                            .insert_object(::std::sync::Arc::clone(&__arc));
                        (*__arc).__keel_set_handle(__handle.0);
                        ::keel::runtime::DispatchResult::Sync(
                            ::core::result::Result::Ok(
                                ::keel::wire::Encode::encode_to_vec(&__handle),
                            ),
                        )
                    }
                    ::core::result::Result::Err(__why) => {
                        ::keel::runtime::DispatchResult::BadRequest(
                            ::std::format!(
                                "store `{}` could not attach its signals: {}", "Todos",
                                __why
                            ),
                        )
                    }
                }
            })
        }
        __KEEL_ID_add => {
            let mut __r = ::keel::wire::Reader::new(__call.args);
            let __keel_a0: String = match <String as ::keel::wire::Decode>::decode(
                &mut __r,
            ) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!(
                            "cannot decode argument `{}` of `{}`: {}", "title",
                            "Todos.add", __e
                        ),
                    );
                }
            };
            if let ::core::result::Result::Err(__e) = __r.finish() {
                return __keel_bad_request(
                    ::std::format!(
                        "cannot decode the arguments of `{}`: {}", "Todos.add", __e
                    ),
                );
            }
            let __obj = match __rt.object::<Todos>(__call.handle) {
                ::core::result::Result::Ok(__o) => __o,
                ::core::result::Result::Err(__e) => {
                    return __keel_bad_request(
                        ::std::format!("cannot call `{}`: {}", "Todos.add", __e),
                    );
                }
            };
            __keel_out({
                let __out = Todos::add(&*__obj, __keel_a0);
                ::keel::runtime::DispatchResult::Sync(
                    ::core::result::Result::Ok(
                        ::keel::wire::Encode::encode_to_vec(&__out),
                    ),
                )
            })
        }
        _ => __keel_unknown(),
    }
}
#[allow(non_upper_case_globals)]
static __KEEL_META_Todos: ::keel::meta::ObjectMeta = ::keel::meta::ObjectMeta {
    name: "Todos",
    type_id: ::keel::meta::ids::type_id("Todos"),
    constructors: &[
        ::keel::meta::MethodMeta {
            name: "new",
            method_id: ::keel::meta::ids::method_id("Todos", "new"),
            params: &[],
            returns: ::keel::meta::TypeRefMeta::Named("Todos"),
            is_async: false,
            takes_ctx: true,
            docs: "",
        },
    ],
    methods: &[
        ::keel::meta::MethodMeta {
            name: "add",
            method_id: ::keel::meta::ids::method_id("Todos", "add"),
            params: &[
                ::keel::meta::ParamMeta {
                    name: "title",
                    ty: ::keel::meta::TypeRefMeta::String,
                },
            ],
            returns: ::keel::meta::TypeRefMeta::Unit,
            is_async: false,
            takes_ctx: false,
            docs: "",
        },
    ],
    store: ::core::option::Option::Some(<Todos>::__KEEL_STORE_META),
    docs: {
        const __KEEL_A: &str = <Todos>::__KEEL_DOCS;
        const __KEEL_B: &str = "";
        const __KEEL_SEP: usize = if __KEEL_A.is_empty() || __KEEL_B.is_empty() {
            0
        } else {
            2
        };
        const __KEEL_N: usize = __KEEL_A.len() + __KEEL_SEP + __KEEL_B.len();
        const __KEEL_BYTES: [u8; __KEEL_N] = {
            let (__a, __b) = (__KEEL_A.as_bytes(), __KEEL_B.as_bytes());
            let mut __out = [0u8; __KEEL_N];
            let mut __i = 0;
            while __i < __a.len() {
                __out[__i] = __a[__i];
                __i += 1;
            }
            if __KEEL_SEP == 2 {
                __out[__a.len()] = b'\n';
                __out[__a.len() + 1] = b'\n';
            }
            let mut __j = 0;
            while __j < __b.len() {
                __out[__a.len() + __KEEL_SEP + __j] = __b[__j];
                __j += 1;
            }
            __out
        };
        match ::core::str::from_utf8(&__KEEL_BYTES) {
            ::core::result::Result::Ok(__s) => __s,
            ::core::result::Result::Err(_) => "",
        }
    },
    dispatch: __keel_dispatch_Todos,
};
::keel::meta::inventory::submit! {
    ::keel::meta::Registration::Object(& __KEEL_META_Todos)
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
    }
};
