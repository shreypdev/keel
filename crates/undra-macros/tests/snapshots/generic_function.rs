/// The row that changed last, if there is one.
pub fn newest<T: Row>(rows: Vec<T>) -> Option<T> {
    rows.into_iter().max_by_key(Row::changed)
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
    fn __undra_identity<T>() {
        __undra_same::<Vec<T>, ::std::vec::Vec<T>>();
        __undra_same::<Option<T>, ::core::option::Option<T>>();
    }
};
#[doc(hidden)]
#[allow(non_snake_case, unused_variables, unused_mut, deprecated, clippy::all)]
fn __undra_dispatch_fn_newest_of_Todo(
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
    if __call.method_id != ::undra::meta::ids::function_id("newest<Todo>") {
        return __undra_unknown();
    }
    let mut __r = ::undra::wire::Reader::new(__call.args);
    let __undra_a0: Vec<Todo> = match <Vec<
        Todo,
    > as ::undra::wire::Decode>::decode(&mut __r) {
        ::core::result::Result::Ok(__v) => __v,
        ::core::result::Result::Err(__e) => {
            return __undra_bad_request(
                ::std::format!(
                    "cannot decode argument `{}` of `{}`: {}", "rows", "newest<Todo>",
                    __e
                ),
            );
        }
    };
    if let ::core::result::Result::Err(__e) = __r.finish() {
        return __undra_bad_request(
            ::std::format!(
                "cannot decode the arguments of `{}`: {}", "newest<Todo>", __e
            ),
        );
    }
    {
        let __out = newest::<Todo>(__undra_a0);
        __rt.sync_ok(&__out, ::undra::wire::Encode::encode)
    }
}
#[allow(non_upper_case_globals)]
static __UNDRA_META_fn_newest_of_Todo: ::undra::meta::FunctionMeta = ::undra::meta::FunctionMeta {
    name: "newest<Todo>",
    method_id: ::undra::meta::ids::function_id("newest<Todo>"),
    params: &[
        ::undra::meta::ParamMeta {
            name: "rows",
            ty: ::undra::meta::TypeRefMeta::Vec(
                &::undra::meta::TypeRefMeta::Named("Todo"),
            ),
        },
    ],
    returns: ::undra::meta::TypeRefMeta::Option(
        &::undra::meta::TypeRefMeta::Named("Todo"),
    ),
    is_async: false,
    takes_ctx: false,
    generic: ::core::option::Option::Some(
        &::undra::meta::GenericOfMeta {
            of: "newest",
            args: &[
                ::undra::meta::GenericArgMeta {
                    param: "T",
                    ty: ::undra::meta::TypeRefMeta::Named("Todo"),
                    inferred: true,
                },
            ],
        },
    ),
    docs: "The row that changed last, if there is one.",
    dispatch: __undra_dispatch_fn_newest_of_Todo,
};
::undra::meta::inventory::submit! {
    ::undra::meta::Registration::Function(& __UNDRA_META_fn_newest_of_Todo)
}
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
                "error[undra::E0072]: `Todo` is an object and cannot be listed for `T` on `newest`\n  = note: each instantiation is presented under the name of its type (`newest(Todo.self)` in Swift, `newest(Todo::class)` in Kotlin, `newest(\"Todo\")` in TypeScript), so a listed type is a value type declared with `#[undra::api]`: a record, an enum, a newtype or a named instantiation; an object crosses by handle and has no such name\n  = help: list the record the platform needs, or name the object in the signature (`Arc<Mailbox>`) instead of making it a type parameter\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0072"
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
};
#[doc(hidden)]
#[allow(non_snake_case, unused_variables, unused_mut, deprecated, clippy::all)]
fn __undra_dispatch_fn_newest_of_Note(
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
    if __call.method_id != ::undra::meta::ids::function_id("newest<Note>") {
        return __undra_unknown();
    }
    let mut __r = ::undra::wire::Reader::new(__call.args);
    let __undra_a0: Vec<Note> = match <Vec<
        Note,
    > as ::undra::wire::Decode>::decode(&mut __r) {
        ::core::result::Result::Ok(__v) => __v,
        ::core::result::Result::Err(__e) => {
            return __undra_bad_request(
                ::std::format!(
                    "cannot decode argument `{}` of `{}`: {}", "rows", "newest<Note>",
                    __e
                ),
            );
        }
    };
    if let ::core::result::Result::Err(__e) = __r.finish() {
        return __undra_bad_request(
            ::std::format!(
                "cannot decode the arguments of `{}`: {}", "newest<Note>", __e
            ),
        );
    }
    {
        let __out = newest::<Note>(__undra_a0);
        __rt.sync_ok(&__out, ::undra::wire::Encode::encode)
    }
}
#[allow(non_upper_case_globals)]
static __UNDRA_META_fn_newest_of_Note: ::undra::meta::FunctionMeta = ::undra::meta::FunctionMeta {
    name: "newest<Note>",
    method_id: ::undra::meta::ids::function_id("newest<Note>"),
    params: &[
        ::undra::meta::ParamMeta {
            name: "rows",
            ty: ::undra::meta::TypeRefMeta::Vec(
                &::undra::meta::TypeRefMeta::Named("Note"),
            ),
        },
    ],
    returns: ::undra::meta::TypeRefMeta::Option(
        &::undra::meta::TypeRefMeta::Named("Note"),
    ),
    is_async: false,
    takes_ctx: false,
    generic: ::core::option::Option::Some(
        &::undra::meta::GenericOfMeta {
            of: "newest",
            args: &[
                ::undra::meta::GenericArgMeta {
                    param: "T",
                    ty: ::undra::meta::TypeRefMeta::Named("Note"),
                    inferred: true,
                },
            ],
        },
    ),
    docs: "The row that changed last, if there is one.",
    dispatch: __undra_dispatch_fn_newest_of_Note,
};
::undra::meta::inventory::submit! {
    ::undra::meta::Registration::Function(& __UNDRA_META_fn_newest_of_Note)
}
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
        if <Note>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0072]: `Note` is an object and cannot be listed for `T` on `newest`\n  = note: each instantiation is presented under the name of its type (`newest(Note.self)` in Swift, `newest(Note::class)` in Kotlin, `newest(\"Note\")` in TypeScript), so a listed type is a value type declared with `#[undra::api]`: a record, an enum, a newtype or a named instantiation; an object crosses by handle and has no such name\n  = help: list the record the platform needs, or name the object in the signature (`Arc<Mailbox>`) instead of making it a type parameter\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0072"
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
