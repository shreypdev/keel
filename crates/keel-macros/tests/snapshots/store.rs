pub struct Todos {
    ctx: Ctx,
    rows: Signal<Vec<Row>>,
    ticks: Signal<u32>,
    visible: Computed<Vec<Row>>,
    label: String,
    #[doc(hidden)]
    pub __keel_cell: ::keel::signals::CellSlot,
}
impl Todos {
    #[doc(hidden)]
    pub const __KEEL_IS_STORE: bool = true;
    #[doc(hidden)]
    pub const __KEEL_STORE_META: ::keel::meta::StoreMeta = ::keel::meta::StoreMeta {
        signals: &[
            ::keel::meta::SignalMeta {
                name: "rows",
                signal_id: 0u32,
                ty: ::keel::meta::TypeRefMeta::Vec(
                    &::keel::meta::TypeRefMeta::Named("Row"),
                ),
                computed: false,
                key: ::core::option::Option::Some("id"),
            },
            ::keel::meta::SignalMeta {
                name: "ticks",
                signal_id: 1u32,
                ty: ::keel::meta::TypeRefMeta::U32,
                computed: false,
                key: ::core::option::Option::None,
            },
            ::keel::meta::SignalMeta {
                name: "visible",
                signal_id: 2u32,
                ty: ::keel::meta::TypeRefMeta::Vec(
                    &::keel::meta::TypeRefMeta::Named("Row"),
                ),
                computed: true,
                key: ::core::option::Option::None,
            },
        ],
    };
    /// The struct's own documentation, which the impl block's object docs start with.
    #[doc(hidden)]
    pub const __KEEL_DOCS: &'static str = "";
    /// Builds the signal cell and attaches every signal, in declaration order.
    #[doc(hidden)]
    fn __keel_build_cell(
        &self,
    ) -> ::core::result::Result<
        ::std::sync::Arc<::keel::signals::StoreCell>,
        ::keel::signals::SignalsError,
    > {
        fn __keel_key_rows(__item: &Row) -> u64 {
            ::std::thread_local! {
                static __KEEL_KEY_BUF : ::core::cell::RefCell < ::keel::wire::Writer > =
                ::core::cell::RefCell::new(::keel::wire::Writer::new());
            }
            __KEEL_KEY_BUF
                .with(|__buf| {
                    let mut __buf = __buf.borrow_mut();
                    __buf.clear();
                    ::keel::wire::Encode::encode(&__item.id, &mut __buf);
                    ::keel::meta::ids::fnv1a64(__buf.as_slice())
                })
        }
        let __cell = ::keel::signals::StoreCell::new(
            ::keel::meta::ids::type_id("Todos"),
        );
        __cell.attach_keyed(&self.rows, 0u32, __keel_key_rows)?;
        __cell.attach(&self.ticks, 1u32)?;
        __cell.set_no_coalesce(1u32)?;
        __cell.attach_computed(&self.visible, 2u32)?;
        ::core::result::Result::Ok(__cell)
    }
    /// Creates the signal cell and attaches every signal (idempotent). Fails when a
    /// signal cannot be attached, for example because it already belongs to another
    /// store; the constructor's dispatch arm turns that into a bad request.
    #[doc(hidden)]
    pub fn __keel_attach_all(
        &self,
    ) -> ::core::result::Result<(), ::keel::signals::SignalsError> {
        self.__keel_cell.get_or_try_init(|| self.__keel_build_cell()).map(|_| ())
    }
    /// The store's cell.
    #[doc(hidden)]
    pub fn __keel_cell_ref(&self) -> &::std::sync::Arc<::keel::signals::StoreCell> {
        self.__keel_cell
            .get_or_init(|| {
                match self.__keel_build_cell() {
                    ::core::result::Result::Ok(__cell) => __cell,
                    ::core::result::Result::Err(_) => {
                        ::keel::signals::StoreCell::new(
                            ::keel::meta::ids::type_id("Todos"),
                        )
                    }
                }
            })
    }
    /// Records the handle the object table issued.
    #[doc(hidden)]
    pub fn __keel_set_handle(&self, __handle: u64) {
        self.__keel_cell_ref().set_handle(__handle);
    }
    /// Rebuilds the store from the body of its snapshot record.
    #[doc(hidden)]
    #[allow(unused_mut, unused_variables)]
    pub fn __keel_restore(
        __ctx: ::keel::runtime::Ctx,
        __r: &mut ::keel::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::keel::wire::WireError> {
        let __count = __r.read_u32()?;
        let mut __slot_rows: ::core::option::Option<Vec<Row>> = ::core::option::Option::None;
        let mut __slot_ticks: ::core::option::Option<u32> = ::core::option::Option::None;
        for _ in 0..__count {
            let __id = __r.read_u32()?;
            let __bytes = __r.read_bytes()?;
            match __id {
                0u32 => {
                    __slot_rows = ::core::option::Option::Some(
                        <Vec<Row> as ::keel::wire::Decode>::decode_exact(__bytes)?,
                    );
                }
                1u32 => {
                    __slot_ticks = ::core::option::Option::Some(
                        <u32 as ::keel::wire::Decode>::decode_exact(__bytes)?,
                    );
                }
                _ => {}
            }
        }
        let __value_rows = match __slot_rows {
            ::core::option::Option::Some(__v) => __v,
            ::core::option::Option::None => {
                return ::core::result::Result::Err(::keel::wire::WireError::InvalidTag {
                    tag: 0u32,
                    at: __r.position(),
                    ty: "snapshot of store Todos is missing signal 0 (rows)",
                });
            }
        };
        let __value_ticks = match __slot_ticks {
            ::core::option::Option::Some(__v) => __v,
            ::core::option::Option::None => {
                return ::core::result::Result::Err(::keel::wire::WireError::InvalidTag {
                    tag: 1u32,
                    at: __r.position(),
                    ty: "snapshot of store Todos is missing signal 1 (ticks)",
                });
            }
        };
        let __value = Self::assemble(
            __ctx,
            ::keel::signals::Signal::<Vec<Row>>::new(__value_rows),
            ::keel::signals::Signal::<u32>::new(__value_ticks),
        );
        if __value.__keel_attach_all().is_err() {
            return ::core::result::Result::Err(::keel::wire::WireError::InvalidTag {
                tag: 0,
                at: __r.position(),
                ty: "restored store Todos could not attach its signals (a signal is already attached to another store)",
            });
        }
        ::core::result::Result::Ok(__value)
    }
}
#[doc(hidden)]
#[allow(non_snake_case)]
fn __keel_restore_erased_Todos(
    __ctx: ::keel::runtime::Ctx,
    __handle: u64,
    __r: &mut ::keel::wire::Reader<'_>,
) -> ::core::result::Result<
    ::std::sync::Arc<dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync>,
    ::keel::wire::WireError,
> {
    let __value = <Todos>::__keel_restore(__ctx, __r)?;
    __value.__keel_set_handle(__handle);
    ::core::result::Result::Ok(
        ::std::sync::Arc::new(__value)
            as ::std::sync::Arc<
                dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync,
            >,
    )
}
#[doc(hidden)]
#[allow(non_snake_case)]
fn __keel_cell_erased_Todos(
    __any: &(dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync),
) -> ::core::option::Option<&::std::sync::Arc<::keel::signals::StoreCell>> {
    __any.downcast_ref::<Todos>().map(<Todos>::__keel_cell_ref)
}
::keel::meta::inventory::submit! {
    ::keel::runtime::StoreRestorer { type_id : ::keel::meta::ids::type_id("Todos"),
    restore : __keel_restore_erased_Todos, cell : __keel_cell_erased_Todos, }
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code, unused)]
const _: () = {
    #[diagnostic::on_unimplemented(
        message = "error[keel::E0011]: store `Todos` has no `#[keel::api(store)]` impl block\n  = note: the constructors of the impl block create the store and wire its signals to the platforms; without the block the store can never be instantiated\n  = help: add an `impl Todos` block marked `#[keel::api(store)]` with a constructor such as `pub fn new(ctx: Ctx) -> Self`\n  = docs: https://keel.dev/errors/E0011",
        label = "this store has no `#[keel::api(store)]` impl block"
    )]
    trait __KeelStoreNeedsImpl {}
    impl<__KeelT: ::keel::runtime::KeelObject> __KeelStoreNeedsImpl for __KeelT {}
    fn __keel_need_impl<__KeelT: __KeelStoreNeedsImpl>() {}
    fn __keel_check_impl() {
        __keel_need_impl::<Todos>();
    }
};
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
        __keel_same::<Vec<Row>, ::std::vec::Vec<Row>>();
        __keel_same::<u32, ::core::primitive::u32>();
    }
    trait __KeelFallback {
        const KEEL_TYPE_ID: u32 = 0;
        const KEEL_IS_ERROR: bool = false;
        const __KEEL_IS_OBJECT: bool = false;
    }
    impl<T: ?::core::marker::Sized> __KeelFallback for T {}
    const _: () = {
        if <Row>::__KEEL_IS_OBJECT {
            ::core::panic!(
                "error[keel::E0064]: `Row` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a parameter or a return value\n  = help: return a record with the data the platform needs, or construct the object from the platform with one of its constructors\n  = docs: https://keel.dev/errors/E0064"
            );
        }
        if <Row>::KEEL_TYPE_ID != ::keel::meta::ids::type_id("Row") {
            ::core::panic!(
                "error[keel::E0061]: the schema records this type as `Row`, but the type written here is not that type\n  = note: Keel describes a type to the platforms by the name it is written with, while the generated code encodes the type the name resolves to; an alias (`type Row = Other`), a renamed import (`use path::Other as Row`) or a type that is not declared with `#[keel::api]` makes the two differ, so the platforms would read the wrong layout\n  = help: write the type under its declared name (for an alias or a renamed import, use `Other` here), or declare it with `#[keel::api]` (`#[keel::error]` for errors)\n  = docs: https://keel.dev/errors/E0061"
            );
        }
    };
};
