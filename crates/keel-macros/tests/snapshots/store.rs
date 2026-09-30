pub struct Todos {
    ctx: Ctx,
    rows: Signal<Vec<Row>>,
    ticks: Signal<u32>,
    visible: Computed<Vec<Row>>,
    page: Lazy<Row>,
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
            ::keel::meta::SignalMeta {
                name: "page",
                signal_id: 3u32,
                ty: ::keel::meta::TypeRefMeta::Lazy(
                    &::keel::meta::TypeRefMeta::Named("Row"),
                ),
                computed: false,
                key: ::core::option::Option::None,
            },
        ],
    };
    /// Creates the signal cell and attaches every signal (idempotent).
    #[doc(hidden)]
    pub fn __keel_attach_all(&self) {
        let _ = <Self as ::keel::runtime::StoreObject>::cell(self);
    }
    /// Records the handle the object table issued.
    #[doc(hidden)]
    pub fn __keel_set_handle(&self, __handle: u64) {
        <Self as ::keel::runtime::StoreObject>::cell(self).set_handle(__handle);
    }
}
#[automatically_derived]
impl ::keel::runtime::StoreObject for Todos {
    fn cell(&self) -> &::std::sync::Arc<::keel::signals::StoreCell> {
        fn __keel_key_rows(__item: &dyn ::core::any::Any) -> u64 {
            match __item.downcast_ref::<Row>() {
                ::core::option::Option::Some(__item) => {
                    ::keel::meta::ids::fnv1a64(
                        &::keel::wire::Encode::encode_to_vec(&__item.id),
                    )
                }
                ::core::option::Option::None => 0,
            }
        }
        self.__keel_cell
            .get_or_init(|| {
                let __cell = ::keel::signals::StoreCell::new(
                    ::keel::meta::ids::type_id("Todos"),
                );
                __cell
                    .attach(
                        &self.rows,
                        0u32,
                        ::core::option::Option::Some(__keel_key_rows),
                    );
                __cell.attach(&self.ticks, 1u32, ::core::option::Option::None);
                __cell.set_no_coalesce(1u32);
                __cell.attach_computed(&self.visible, 2u32);
                __cell.attach_lazy(&self.page, 3u32);
                __cell
            })
    }
    #[allow(unused_mut, unused_variables)]
    fn restore(
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
        __value.__keel_attach_all();
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
    let __value = <Todos as ::keel::runtime::StoreObject>::restore(__ctx, __r)?;
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
    __any.downcast_ref::<Todos>().map(<Todos as ::keel::runtime::StoreObject>::cell)
}
::keel::meta::inventory::submit! {
    ::keel::runtime::StoreRestorer { type_id : ::keel::meta::ids::type_id("Todos"),
    restore : __keel_restore_erased_Todos, cell : __keel_cell_erased_Todos, }
}
