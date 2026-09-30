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
#[allow(non_camel_case_types, dead_code)]
trait __KeelStoreProbe_Todos {
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
    let ::core::option::Option::Some(__rt) = __rt
        .downcast_ref::<::keel::runtime::Runtime>() else {
        return __keel_unknown();
    };
    const __KEEL_ID_new: u32 = ::keel::meta::ids::method_id("Todos", "new");
    const __KEEL_ID_add: u32 = ::keel::meta::ids::method_id("Todos", "add");
    match __call.method_id {
        __KEEL_ID_new => {
            let mut __r = ::keel::wire::Reader::new(__call.args);
            if __r.finish().is_err() {
                return __keel_unknown();
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
            let title: String = match <String as ::keel::wire::Decode>::decode(
                &mut __r,
            ) {
                ::core::result::Result::Ok(__v) => __v,
                ::core::result::Result::Err(_) => return __keel_unknown(),
            };
            if __r.finish().is_err() {
                return __keel_unknown();
            }
            let __obj = match __rt.object::<Todos>(__call.handle) {
                ::core::result::Result::Ok(__o) => __o,
                ::core::result::Result::Err(_) => return __keel_unknown(),
            };
            __keel_out({
                let __out = Todos::add(&*__obj, title);
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
    docs: "",
    dispatch: __keel_dispatch_Todos,
};
::keel::meta::inventory::submit! {
    ::keel::meta::Registration::Object(& __KEEL_META_Todos)
}
