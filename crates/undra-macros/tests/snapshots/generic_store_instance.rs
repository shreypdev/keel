#[doc(hidden)]
#[allow(non_upper_case_globals, dead_code)]
const _undra_error_E0070_this_instantiation_of_Selection_Todo_is_declared_twice_keep_one_alias_per_instantiation: () = ();
impl TodoSelection {
    #[doc(hidden)]
    #[allow(non_upper_case_globals)]
    pub const _undra_error_E0070_this_instantiation_of_Selection_is_declared_twice_keep_one_alias_per_instantiation: () = ();
    #[doc(hidden)]
    pub const __UNDRA_IS_STORE: bool = true;
    #[doc(hidden)]
    pub const __UNDRA_STORE_META: ::undra::meta::StoreMeta = ::undra::meta::StoreMeta {
        signals: &[
            ::undra::meta::SignalMeta {
                name: "rows",
                signal_id: 0u32,
                ty: ::undra::meta::TypeRefMeta::Vec(
                    &::undra::meta::TypeRefMeta::Named("Todo"),
                ),
                computed: false,
                key: ::core::option::Option::Some("id"),
                no_coalesce: false,
                default: false,
            },
            ::undra::meta::SignalMeta {
                name: "count",
                signal_id: 1u32,
                ty: ::undra::meta::TypeRefMeta::U32,
                computed: true,
                key: ::core::option::Option::None,
                no_coalesce: false,
                default: false,
            },
        ],
    };
    /// The struct's own documentation, which the impl block's object docs start with.
    #[doc(hidden)]
    pub const __UNDRA_DOCS: &'static str = "The rows the user has ticked.";
    /// Builds the signal cell and attaches every signal, in declaration order.
    #[doc(hidden)]
    fn __undra_build_cell(
        &self,
    ) -> ::core::result::Result<
        ::std::sync::Arc<::undra::signals::StoreCell>,
        ::undra::signals::SignalsError,
    > {
        #[allow(non_camel_case_types, dead_code)]
        fn __undra_key_rows(__item: &Todo) -> u64 {
            trait __UndraKeyed {
                const __UNDRA_FIELDS: &'static [&'static str] = &[];
            }
            impl<__T: ?::core::marker::Sized> __UndraKeyed for __T {}
            struct __UndraGate<const __OK: bool>;
            trait __UndraPass<__T: ?::core::marker::Sized> {
                type Out: ?::core::marker::Sized;
            }
            impl<__T: ?::core::marker::Sized> __UndraPass<__T> for __UndraGate<true> {
                type Out = __T;
            }
            const __UNDRA_FIELDS: &[&str] = <Todo>::__UNDRA_FIELDS;
            const __UNDRA_INDEX: usize = ::undra::meta::keys::index_of(
                __UNDRA_FIELDS,
                "id",
            );
            const __UNDRA_MESSAGE_LEN: usize = ::undra::meta::keys::message_len(
                "error[undra::E0008]: `#[undra(key = \"id\")]` on `rows` names no field of `Todo`\n  = note: `key` names the field of the list's items that identifies them, and `Todo` has ",
                __UNDRA_FIELDS,
                "\n  = help: write the name of one of those fields as the key\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0008",
            );
            const __UNDRA_MESSAGE: [u8; __UNDRA_MESSAGE_LEN] = ::undra::meta::keys::message::<
                __UNDRA_MESSAGE_LEN,
            >(
                "error[undra::E0008]: `#[undra(key = \"id\")]` on `rows` names no field of `Todo`\n  = note: `key` names the field of the list's items that identifies them, and `Todo` has ",
                __UNDRA_FIELDS,
                "\n  = help: write the name of one of those fields as the key\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0008",
            );
            const __UNDRA_MESSAGE_TEXT: &str = ::undra::meta::keys::as_str(
                &__UNDRA_MESSAGE,
            );
            const __UNDRA_KEY_IS_A_FIELD: bool = if __UNDRA_INDEX == usize::MAX {
                ::core::panic!("{}", __UNDRA_MESSAGE_TEXT)
            } else {
                true
            };
            ::std::thread_local! {
                static __UNDRA_KEY_BUF : ::core::cell::RefCell < ::undra::wire::Writer >
                = ::core::cell::RefCell::new(::undra::wire::Writer::new());
            }
            __UNDRA_KEY_BUF
                .with(|__buf| {
                    let mut __buf = __buf.borrow_mut();
                    __buf.clear();
                    let __row: &<__UndraGate<
                        { __UNDRA_KEY_IS_A_FIELD },
                    > as __UndraPass<Todo>>::Out = __item;
                    ::undra::wire::Encode::encode(&__row.id, &mut __buf);
                    ::undra::meta::ids::fnv1a64(__buf.as_slice())
                })
        }
        let __cell = ::undra::signals::StoreCell::new(
            ::undra::meta::ids::type_id("TodoSelection"),
        );
        __cell.attach_keyed(&self.rows, 0u32, __undra_key_rows)?;
        __cell.attach_computed(&self.count, 1u32)?;
        ::core::result::Result::Ok(__cell)
    }
    /// Creates the signal cell and attaches every signal (idempotent). Fails when a
    /// signal cannot be attached, for example because it already belongs to another
    /// store; the constructor's dispatch arm turns that into a bad request.
    #[doc(hidden)]
    pub fn __undra_attach_all(
        &self,
    ) -> ::core::result::Result<(), ::undra::signals::SignalsError> {
        self.__undra_cell.get_or_try_init(|| self.__undra_build_cell()).map(|_| ())
    }
    /// The store's cell.
    #[doc(hidden)]
    pub fn __undra_cell_ref(&self) -> &::std::sync::Arc<::undra::signals::StoreCell> {
        self.__undra_cell
            .get_or_init(|| {
                match self.__undra_build_cell() {
                    ::core::result::Result::Ok(__cell) => __cell,
                    ::core::result::Result::Err(_) => {
                        ::undra::signals::StoreCell::new(
                            ::undra::meta::ids::type_id("TodoSelection"),
                        )
                    }
                }
            })
    }
    /// Records the handle the object table issued.
    #[doc(hidden)]
    pub fn __undra_set_handle(&self, __handle: u64) {
        self.__undra_cell_ref().set_handle(__handle);
    }
    /// Rebuilds the store from the body of its snapshot record.
    #[doc(hidden)]
    #[allow(unused_mut, unused_variables)]
    pub fn __undra_restore(
        __ctx: ::undra::runtime::Ctx,
        __r: &mut ::undra::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::undra::wire::WireError> {
        let __count = __r.read_u32()?;
        let mut __slot_rows: ::core::option::Option<Vec<Todo>> = ::core::option::Option::None;
        for _ in 0..__count {
            let __id = __r.read_u32()?;
            let __bytes = __r.read_bytes()?;
            match __id {
                0u32 => {
                    __slot_rows = ::core::option::Option::Some(
                        <Vec<Todo> as ::undra::wire::Decode>::decode_exact(__bytes)?,
                    );
                }
                _ => {}
            }
        }
        let __value_rows = match __slot_rows {
            ::core::option::Option::Some(__v) => __v,
            ::core::option::Option::None => {
                return ::core::result::Result::Err(::undra::wire::WireError::InvalidTag {
                    tag: 0u32,
                    at: __r.position(),
                    ty: "snapshot of store TodoSelection is missing signal 0 (rows)",
                });
            }
        };
        let __value = Self::assemble(
            __ctx,
            ::undra::signals::Signal::<Vec<Todo>>::new(__value_rows),
        );
        if __value.__undra_attach_all().is_err() {
            return ::core::result::Result::Err(::undra::wire::WireError::InvalidTag {
                tag: 0,
                at: __r.position(),
                ty: "restored store TodoSelection could not attach its signals (a signal is already attached to another store)",
            });
        }
        ::core::result::Result::Ok(__value)
    }
}
#[doc(hidden)]
#[allow(non_snake_case)]
fn __undra_restore_erased_TodoSelection(
    __ctx: ::undra::runtime::Ctx,
    __handle: u64,
    __r: &mut ::undra::wire::Reader<'_>,
) -> ::core::result::Result<
    ::std::sync::Arc<dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync>,
    ::undra::wire::WireError,
> {
    let __value = <TodoSelection>::__undra_restore(__ctx, __r)?;
    __value.__undra_set_handle(__handle);
    ::core::result::Result::Ok(
        ::std::sync::Arc::new(__value)
            as ::std::sync::Arc<
                dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync,
            >,
    )
}
#[doc(hidden)]
#[allow(non_snake_case)]
fn __undra_cell_erased_TodoSelection(
    __any: &(dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync),
) -> ::core::option::Option<&::std::sync::Arc<::undra::signals::StoreCell>> {
    __any.downcast_ref::<TodoSelection>().map(<TodoSelection>::__undra_cell_ref)
}
::undra::meta::inventory::submit! {
    ::undra::runtime::StoreRestorer { type_id :
    ::undra::meta::ids::type_id("TodoSelection"), restore :
    __undra_restore_erased_TodoSelection, cell : __undra_cell_erased_TodoSelection, }
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code, unused)]
const _: () = {
    #[diagnostic::on_unimplemented(
        message = "error[undra::E0011]: store `TodoSelection` has no `#[undra::api(store)]` impl block\n  = note: the constructors of the impl block create the store and wire its signals to the platforms; without the block the store can never be instantiated\n  = help: add an `impl TodoSelection` block marked `#[undra::api(store)]` with a constructor such as `pub fn new(ctx: Ctx) -> Self`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0011",
        label = "this store has no `#[undra::api(store)]` impl block"
    )]
    trait __UndraStoreNeedsImpl {}
    impl<__UndraT: ::undra::runtime::UndraObject> __UndraStoreNeedsImpl for __UndraT {}
    fn __undra_need_impl<__UndraT: __UndraStoreNeedsImpl>() {}
    fn __undra_check_impl() {
        __undra_need_impl::<TodoSelection>();
    }
};
#[doc(hidden)]
#[allow(non_upper_case_globals, dead_code)]
const _undra_error_E0007_TodoSelection_has_two_undra_api_impl_blocks_merge_them_into_one: () = ();
impl TodoSelection {
    /// Marks the type as an object, so a signature that uses it as a value can say so.
    #[doc(hidden)]
    pub const __UNDRA_IS_OBJECT: bool = true;
    /// The type id of the declared name: what a signature that takes or returns the
    /// object as `Arc<T>` or `&T` is checked against (E0061).
    #[doc(hidden)]
    pub const __UNDRA_OBJECT_ID: u32 = ::undra::meta::ids::type_id("TodoSelection");
    /// The declared name of the instantiation, for a signature that names it through a
    /// generic application (`Arc<Selection<T>>`).
    #[doc(hidden)]
    pub const __UNDRA_OBJECT_NAME: &'static str = "TodoSelection";
}
#[doc(hidden)]
#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code)]
const _: () = {
    trait __UndraStoreProbe_TodoSelection {
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
    impl __UndraStoreProbe_TodoSelection for TodoSelection {}
    fn __undra_store_probe() -> [(); {
        ::core::assert!(
            < TodoSelection > ::__UNDRA_IS_STORE,
            "error[undra::E0011]: `TodoSelection` is implemented with `#[undra::api(store)]` but the struct has no `#[undra::store]`\n  = note: the `store` marker wires the constructors to the struct's signals, which only `#[undra::store]` sets up\n  = help: add `#[undra::store]` to `struct TodoSelection`, or remove `store` from the impl attribute\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0011"
        );
        0
    }] {
        []
    }
    #[automatically_derived]
    impl ::undra::runtime::StoreObject for TodoSelection {
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
    #[automatically_derived]
    impl ::undra::runtime::UndraObject for TodoSelection {
        const TYPE_ID: u32 = ::undra::meta::ids::type_id("TodoSelection");
        const NAME: &'static str = "TodoSelection";
        fn __undra_store_cell(
            &self,
        ) -> ::core::option::Option<&::std::sync::Arc<::undra::signals::StoreCell>> {
            ::core::option::Option::Some(self.__undra_cell_ref())
        }
        fn __undra_attach(
            &self,
        ) -> ::core::result::Result<(), ::undra::signals::SignalsError> {
            self.__undra_attach_all()
        }
    }
    #[allow(unused_variables, unused_mut, deprecated, clippy::all)]
    fn __undra_dispatch_TodoSelection(
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
        const __UNDRA_ID_new: u32 = ::undra::meta::ids::method_id(
            "TodoSelection",
            "new",
        );
        const __UNDRA_ID_toggle: u32 = ::undra::meta::ids::method_id(
            "TodoSelection",
            "toggle",
        );
        match __call.method_id {
            __UNDRA_ID_new => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}",
                            "TodoSelection.new", __e
                        ),
                    );
                }
                let __ctx = __rt.ctx();
                {
                    let __value = TodoSelection::new(__ctx);
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
                                    "store `{}` could not attach its signals: {}",
                                    "TodoSelection", __why
                                ),
                            )
                        }
                    }
                }
            }
            __UNDRA_ID_toggle => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                let __undra_a0: Todo = match <Todo as ::undra::wire::Decode>::decode(
                    &mut __r,
                ) {
                    ::core::result::Result::Ok(__v) => __v,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot decode argument `{}` of `{}`: {}", "row",
                                "TodoSelection.toggle", __e
                            ),
                        );
                    }
                };
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}",
                            "TodoSelection.toggle", __e
                        ),
                    );
                }
                let __obj = match __rt.object::<TodoSelection>(__call.handle) {
                    ::core::result::Result::Ok(__o) => __o,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot call `{}`: {}", "TodoSelection.toggle", __e
                            ),
                        );
                    }
                };
                {
                    let __out = TodoSelection::toggle(&*__obj, __undra_a0);
                    __rt.sync_ok(&__out, ::undra::wire::Encode::encode)
                }
            }
            _ => __undra_unknown(),
        }
    }
    static __UNDRA_META_TodoSelection: ::undra::meta::ObjectMeta = ::undra::meta::ObjectMeta {
        name: "TodoSelection",
        type_id: ::undra::meta::ids::type_id("TodoSelection"),
        constructors: &[
            ::undra::meta::MethodMeta {
                name: "new",
                method_id: ::undra::meta::ids::method_id("TodoSelection", "new"),
                params: &[],
                returns: ::undra::meta::TypeRefMeta::Named("TodoSelection"),
                is_async: false,
                takes_ctx: true,
                coalesce: false,
                generic: ::core::option::Option::None,
                docs: "",
            },
        ],
        methods: &[
            ::undra::meta::MethodMeta {
                name: "toggle",
                method_id: ::undra::meta::ids::method_id("TodoSelection", "toggle"),
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
                docs: "Ticks a row.",
            },
        ],
        store: ::core::option::Option::Some(<TodoSelection>::__UNDRA_STORE_META),
        docs: {
            const __UNDRA_A: &str = <TodoSelection>::__UNDRA_DOCS;
            const __UNDRA_B: &str = "Ticking rows.";
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
        dispatch: __undra_dispatch_TodoSelection,
    };
    ::undra::meta::inventory::submit! {
        ::undra::meta::Registration::Object(& __UNDRA_META_TodoSelection)
    }
};
