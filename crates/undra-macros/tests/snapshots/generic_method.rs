impl Library {
    pub fn new() -> Self {
        Library
    }
    /// The rows pinned to the top.
    pub fn pinned<T: Row>(&self) -> Vec<T> {
        Vec::new()
    }
}
#[doc(hidden)]
#[allow(non_upper_case_globals, dead_code)]
const _undra_error_E0007_Library_has_two_undra_api_impl_blocks_merge_them_into_one: () = ();
impl Library {
    /// Marks the type as an object, so a signature that uses it as a value can say so.
    #[doc(hidden)]
    pub const __UNDRA_IS_OBJECT: bool = true;
    /// The type id of the declared name: what a signature that takes or returns the
    /// object as `Arc<T>` or `&T` is checked against (E0061).
    #[doc(hidden)]
    pub const __UNDRA_OBJECT_ID: u32 = ::undra::meta::ids::type_id("Library");
}
#[doc(hidden)]
#[allow(non_camel_case_types, non_snake_case, non_upper_case_globals, dead_code)]
const _: () = {
    trait __UndraStoreProbe_Library {
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
    impl __UndraStoreProbe_Library for Library {}
    fn __undra_store_probe() -> [(); {
        ::core::assert!(
            ! < Library > ::__UNDRA_IS_STORE,
            "error[undra::E0011]: `Library` is a `#[undra::store]` but its `#[undra::api]` impl block is not marked as a store\n  = note: the impl block of a store must say so, so its constructors can attach the store's signals and its struct literals get the hidden cell field\n  = help: write `#[undra::api(store)]` on the impl block\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0011"
        );
        0
    }] {
        []
    }
    #[automatically_derived]
    impl ::undra::runtime::UndraObject for Library {
        const TYPE_ID: u32 = ::undra::meta::ids::type_id("Library");
        const NAME: &'static str = "Library";
    }
    #[allow(unused_variables, unused_mut, deprecated, clippy::all)]
    fn __undra_dispatch_Library(
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
        const __UNDRA_ID_new: u32 = ::undra::meta::ids::method_id("Library", "new");
        const __UNDRA_ID_pinned_of_Todo: u32 = ::undra::meta::ids::method_id(
            "Library",
            "pinned<Todo>",
        );
        const __UNDRA_ID_pinned_of_Note: u32 = ::undra::meta::ids::method_id(
            "Library",
            "pinned<Note>",
        );
        match __call.method_id {
            __UNDRA_ID_new => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}", "Library.new", __e
                        ),
                    );
                }
                {
                    let __value = Library::new();
                    {
                        let __handle = __rt
                            .insert_object(::std::sync::Arc::new(__value));
                        __rt.sync_ok(&__handle, ::undra::wire::Encode::encode)
                    }
                }
            }
            __UNDRA_ID_pinned_of_Todo => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}",
                            "Library.pinned<Todo>", __e
                        ),
                    );
                }
                let __obj = match __rt.object::<Library>(__call.handle) {
                    ::core::result::Result::Ok(__o) => __o,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot call `{}`: {}", "Library.pinned<Todo>", __e
                            ),
                        );
                    }
                };
                {
                    let __out = Library::pinned::<Todo>(&*__obj);
                    __rt.sync_ok(&__out, ::undra::wire::Encode::encode)
                }
            }
            __UNDRA_ID_pinned_of_Note => {
                let mut __r = ::undra::wire::Reader::new(__call.args);
                if let ::core::result::Result::Err(__e) = __r.finish() {
                    return __undra_bad_request(
                        ::std::format!(
                            "cannot decode the arguments of `{}`: {}",
                            "Library.pinned<Note>", __e
                        ),
                    );
                }
                let __obj = match __rt.object::<Library>(__call.handle) {
                    ::core::result::Result::Ok(__o) => __o,
                    ::core::result::Result::Err(__e) => {
                        return __undra_bad_request(
                            ::std::format!(
                                "cannot call `{}`: {}", "Library.pinned<Note>", __e
                            ),
                        );
                    }
                };
                {
                    let __out = Library::pinned::<Note>(&*__obj);
                    __rt.sync_ok(&__out, ::undra::wire::Encode::encode)
                }
            }
            _ => __undra_unknown(),
        }
    }
    static __UNDRA_META_Library: ::undra::meta::ObjectMeta = ::undra::meta::ObjectMeta {
        name: "Library",
        type_id: ::undra::meta::ids::type_id("Library"),
        constructors: &[
            ::undra::meta::MethodMeta {
                name: "new",
                method_id: ::undra::meta::ids::method_id("Library", "new"),
                params: &[],
                returns: ::undra::meta::TypeRefMeta::Named("Library"),
                is_async: false,
                takes_ctx: false,
                coalesce: false,
                generic: ::core::option::Option::None,
                docs: "",
            },
        ],
        methods: &[
            ::undra::meta::MethodMeta {
                name: "pinned<Todo>",
                method_id: ::undra::meta::ids::method_id("Library", "pinned<Todo>"),
                params: &[],
                returns: ::undra::meta::TypeRefMeta::Vec(
                    &::undra::meta::TypeRefMeta::Named("Todo"),
                ),
                is_async: false,
                takes_ctx: false,
                coalesce: false,
                generic: ::core::option::Option::Some(
                    &::undra::meta::GenericOfMeta {
                        of: "pinned",
                        args: &[
                            ::undra::meta::GenericArgMeta {
                                param: "T",
                                ty: ::undra::meta::TypeRefMeta::Named("Todo"),
                                inferred: false,
                            },
                        ],
                    },
                ),
                docs: "The rows pinned to the top.",
            },
            ::undra::meta::MethodMeta {
                name: "pinned<Note>",
                method_id: ::undra::meta::ids::method_id("Library", "pinned<Note>"),
                params: &[],
                returns: ::undra::meta::TypeRefMeta::Vec(
                    &::undra::meta::TypeRefMeta::Named("Note"),
                ),
                is_async: false,
                takes_ctx: false,
                coalesce: false,
                generic: ::core::option::Option::Some(
                    &::undra::meta::GenericOfMeta {
                        of: "pinned",
                        args: &[
                            ::undra::meta::GenericArgMeta {
                                param: "T",
                                ty: ::undra::meta::TypeRefMeta::Named("Note"),
                                inferred: false,
                            },
                        ],
                    },
                ),
                docs: "The rows pinned to the top.",
            },
        ],
        store: ::core::option::Option::None,
        docs: "",
        dispatch: __undra_dispatch_Library,
    };
    ::undra::meta::inventory::submit! {
        ::undra::meta::Registration::Object(& __UNDRA_META_Library)
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
    fn __undra_identity<T>() {
        __undra_same::<Vec<T>, ::std::vec::Vec<T>>();
    }
};
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code, unused, unused_braces, clippy::all)]
const _: () = {
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
                "error[undra::E0072]: `Todo` is an object and cannot be listed for `T` on `pinned`\n  = note: each instantiation is presented under the name of its type (`pinned(Todo.self)` in Swift, `pinned(Todo::class)` in Kotlin, `pinned(\"Todo\")` in TypeScript), so a listed type is a value type declared with `#[undra::api]`: a record, an enum, a newtype or a named instantiation; an object crosses by handle and has no such name\n  = help: list the record the platform needs, or name the object in the signature (`Arc<Mailbox>`) instead of making it a type parameter\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0072"
            );
        }
        let __undra_id = <Todo>::UNDRA_TYPE_ID;
        if __undra_id == 0 {
            ::core::panic!(
                "error[undra::E0061]: `Todo` is not a type declared with `#[undra::api]`\n  = note: Undra describes a type to the platforms by the name it is written with, so a listed type is a record or enum declared with `#[undra::api]` or an error declared with `#[undra::error]`; anything else, such as a plain struct or an alias (`type Id = u64`), has no definition the platforms could generate\n  = help: add `#[undra::api]` to `Todo`, or, if it is an alias, list the type it stands for\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if __undra_id != ::undra::meta::ids::type_id("Todo") {
            ::core::panic!(
                "error[undra::E0061]: `Todo` here is an alias or a renamed import of an Undra type that is declared under another name\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type that name resolves to; with `type Todo = Other` or `use path::Other as Todo` the platforms would be told `Todo` and receive the layout of `Other`\n  = help: list the type under the name it is declared with (`Other` in the examples above), or declare a separate `#[undra::api] struct Todo` if you mean a distinct type\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
    };
    const _: () = {
        if <Note>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0072]: `Note` is an object and cannot be listed for `T` on `pinned`\n  = note: each instantiation is presented under the name of its type (`pinned(Note.self)` in Swift, `pinned(Note::class)` in Kotlin, `pinned(\"Note\")` in TypeScript), so a listed type is a value type declared with `#[undra::api]`: a record, an enum, a newtype or a named instantiation; an object crosses by handle and has no such name\n  = help: list the record the platform needs, or name the object in the signature (`Arc<Mailbox>`) instead of making it a type parameter\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0072"
            );
        }
        let __undra_id = <Note>::UNDRA_TYPE_ID;
        if __undra_id == 0 {
            ::core::panic!(
                "error[undra::E0061]: `Note` is not a type declared with `#[undra::api]`\n  = note: Undra describes a type to the platforms by the name it is written with, so a listed type is a record or enum declared with `#[undra::api]` or an error declared with `#[undra::error]`; anything else, such as a plain struct or an alias (`type Id = u64`), has no definition the platforms could generate\n  = help: add `#[undra::api]` to `Note`, or, if it is an alias, list the type it stands for\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if __undra_id != ::undra::meta::ids::type_id("Note") {
            ::core::panic!(
                "error[undra::E0061]: `Note` here is an alias or a renamed import of an Undra type that is declared under another name\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type that name resolves to; with `type Note = Other` or `use path::Other as Note` the platforms would be told `Note` and receive the layout of `Other`\n  = help: list the type under the name it is declared with (`Other` in the examples above), or declare a separate `#[undra::api] struct Note` if you mean a distinct type\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
    };
};
