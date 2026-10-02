pub struct Todos {
    ctx: Ctx,
    rows: Signal<Vec<Row>>,
    ticks: Signal<u32>,
    visible: Computed<Vec<Row>>,
    label: String,
    #[doc(hidden)]
    pub __undra_cell: ::undra::signals::CellSlot,
}
impl Todos {
    #[doc(hidden)]
    pub const __UNDRA_IS_STORE: bool = true;
    #[doc(hidden)]
    pub const __UNDRA_STORE_META: ::undra::meta::StoreMeta = ::undra::meta::StoreMeta {
        signals: &[
            ::undra::meta::SignalMeta {
                name: "rows",
                signal_id: 0u32,
                ty: ::undra::meta::TypeRefMeta::Vec(
                    &::undra::meta::TypeRefMeta::Named("Row"),
                ),
                computed: false,
                key: ::core::option::Option::Some("id"),
                no_coalesce: false,
                default: false,
            },
            ::undra::meta::SignalMeta {
                name: "ticks",
                signal_id: 1u32,
                ty: ::undra::meta::TypeRefMeta::U32,
                computed: false,
                key: ::core::option::Option::None,
                no_coalesce: true,
                default: false,
            },
            ::undra::meta::SignalMeta {
                name: "visible",
                signal_id: 2u32,
                ty: ::undra::meta::TypeRefMeta::Vec(
                    &::undra::meta::TypeRefMeta::Named("Row"),
                ),
                computed: true,
                key: ::core::option::Option::None,
                no_coalesce: false,
                default: false,
            },
        ],
    };
    /// The struct's own documentation, which the impl block's object docs start with.
    #[doc(hidden)]
    pub const __UNDRA_DOCS: &'static str = "";
    /// Builds the signal cell and attaches every signal, in declaration order.
    #[doc(hidden)]
    fn __undra_build_cell(
        &self,
    ) -> ::core::result::Result<
        ::std::sync::Arc<::undra::signals::StoreCell>,
        ::undra::signals::SignalsError,
    > {
        #[allow(non_camel_case_types, dead_code)]
        fn __undra_key_rows(__item: &Row) -> u64 {
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
            const __UNDRA_FIELDS: &[&str] = <Row>::__UNDRA_FIELDS;
            const __UNDRA_INDEX: usize = ::undra::meta::keys::index_of(
                __UNDRA_FIELDS,
                "id",
            );
            const __UNDRA_MESSAGE_LEN: usize = ::undra::meta::keys::message_len(
                "error[undra::E0008]: `#[undra(key = \"id\")]` on `rows` names no field of `Row`\n  = note: `key` names the field of the list's items that identifies them, and `Row` has ",
                __UNDRA_FIELDS,
                "\n  = help: write the name of one of those fields as the key\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0008",
            );
            const __UNDRA_MESSAGE: [u8; __UNDRA_MESSAGE_LEN] = ::undra::meta::keys::message::<
                __UNDRA_MESSAGE_LEN,
            >(
                "error[undra::E0008]: `#[undra(key = \"id\")]` on `rows` names no field of `Row`\n  = note: `key` names the field of the list's items that identifies them, and `Row` has ",
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
                    > as __UndraPass<Row>>::Out = __item;
                    ::undra::wire::Encode::encode(&__row.id, &mut __buf);
                    ::undra::meta::ids::fnv1a64(__buf.as_slice())
                })
        }
        let __cell = ::undra::signals::StoreCell::new(
            ::undra::meta::ids::type_id("Todos"),
        );
        __cell.attach_keyed(&self.rows, 0u32, __undra_key_rows)?;
        __cell.attach(&self.ticks, 1u32)?;
        __cell.set_no_coalesce(1u32)?;
        __cell.attach_computed(&self.visible, 2u32)?;
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
                            ::undra::meta::ids::type_id("Todos"),
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
        let mut __slot_rows: ::core::option::Option<Vec<Row>> = ::core::option::Option::None;
        let mut __slot_ticks: ::core::option::Option<u32> = ::core::option::Option::None;
        for _ in 0..__count {
            let __id = __r.read_u32()?;
            let __bytes = __r.read_bytes()?;
            match __id {
                0u32 => {
                    __slot_rows = ::core::option::Option::Some(
                        <Vec<Row> as ::undra::wire::Decode>::decode_exact(__bytes)?,
                    );
                }
                1u32 => {
                    __slot_ticks = ::core::option::Option::Some(
                        <u32 as ::undra::wire::Decode>::decode_exact(__bytes)?,
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
                    ty: "snapshot of store Todos is missing signal 0 (rows)",
                });
            }
        };
        let __value_ticks = match __slot_ticks {
            ::core::option::Option::Some(__v) => __v,
            ::core::option::Option::None => {
                return ::core::result::Result::Err(::undra::wire::WireError::InvalidTag {
                    tag: 1u32,
                    at: __r.position(),
                    ty: "snapshot of store Todos is missing signal 1 (ticks)",
                });
            }
        };
        let __value = Self::assemble(
            __ctx,
            ::undra::signals::Signal::<Vec<Row>>::new(__value_rows),
            ::undra::signals::Signal::<u32>::new(__value_ticks),
        );
        if __value.__undra_attach_all().is_err() {
            return ::core::result::Result::Err(::undra::wire::WireError::InvalidTag {
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
fn __undra_restore_erased_Todos(
    __ctx: ::undra::runtime::Ctx,
    __handle: u64,
    __r: &mut ::undra::wire::Reader<'_>,
) -> ::core::result::Result<
    ::std::sync::Arc<dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync>,
    ::undra::wire::WireError,
> {
    let __value = <Todos>::__undra_restore(__ctx, __r)?;
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
fn __undra_cell_erased_Todos(
    __any: &(dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync),
) -> ::core::option::Option<&::std::sync::Arc<::undra::signals::StoreCell>> {
    __any.downcast_ref::<Todos>().map(<Todos>::__undra_cell_ref)
}
::undra::meta::inventory::submit! {
    ::undra::runtime::StoreRestorer { type_id : ::undra::meta::ids::type_id("Todos"),
    restore : __undra_restore_erased_Todos, cell : __undra_cell_erased_Todos, }
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code, unused)]
const _: () = {
    #[diagnostic::on_unimplemented(
        message = "error[undra::E0011]: store `Todos` has no `#[undra::api(store)]` impl block\n  = note: the constructors of the impl block create the store and wire its signals to the platforms; without the block the store can never be instantiated\n  = help: add an `impl Todos` block marked `#[undra::api(store)]` with a constructor such as `pub fn new(ctx: Ctx) -> Self`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0011",
        label = "this store has no `#[undra::api(store)]` impl block"
    )]
    trait __UndraStoreNeedsImpl {}
    impl<__UndraT: ::undra::runtime::UndraObject> __UndraStoreNeedsImpl for __UndraT {}
    fn __undra_need_impl<__UndraT: __UndraStoreNeedsImpl>() {}
    fn __undra_check_impl() {
        __undra_need_impl::<Todos>();
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
        __undra_same::<Vec<Row>, ::std::vec::Vec<Row>>();
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
        if <Row>::__UNDRA_IS_OBJECT {
            ::core::panic!(
                "error[undra::E0064]: `Row` is an object and cannot be used as a value\n  = note: an object lives in the core and crosses the boundary as a handle; its contents have no wire representation, so it cannot be a field, a variant field, a signal value, a map entry or a query value\n  = help: return it as `Arc<Row>`, take it as `&Row` or `Arc<Row>` (a method, constructor or function parameter or return), or use a record with the data the platform needs\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0064"
            );
        }
        let __undra_id = <Row>::UNDRA_TYPE_ID;
        if __undra_id == 0 {
            ::core::panic!(
                "error[undra::E0061]: `Row` is not a type declared with `#[undra::api]`\n  = note: Undra describes a type to the platforms by the name it is written with, so the name must be a record or enum declared with `#[undra::api]` or an error declared with `#[undra::error]`; anything else, such as a plain struct or an alias (`type Id = u64`), has no definition the platforms could generate\n  = help: add `#[undra::api]` to `Row`, or, if it is an alias, write the type it stands for where it is used\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
        if __undra_id != ::undra::meta::ids::type_id("Row") {
            ::core::panic!(
                "error[undra::E0061]: `Row` here is an alias or a renamed import of an Undra type that is declared under another name\n  = note: Undra describes a type to the platforms by the name it is written with, while the generated code encodes the type that name resolves to; with `type Row = Other` or `use path::Other as Row` the platforms would be told `Row` and receive the layout of `Other`\n  = help: write the type under the name it is declared with (`Other` in the examples above), or declare a separate `#[undra::api] struct Row` if you mean a distinct type\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0061"
            );
        }
    };
};
