pub struct Counter {
    ctx: Ctx,
    count: Signal<i64>,
    note: String,
    #[doc(hidden)]
    pub __keel_cell: ::keel::signals::CellSlot,
}
impl Counter {
    #[doc(hidden)]
    pub const __KEEL_IS_STORE: bool = true;
    #[doc(hidden)]
    pub const __KEEL_STORE_META: ::keel::meta::StoreMeta = ::keel::meta::StoreMeta {
        signals: &[
            ::keel::meta::SignalMeta {
                name: "count",
                signal_id: 0u32,
                ty: ::keel::meta::TypeRefMeta::I64,
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
impl ::keel::runtime::StoreObject for Counter {
    fn cell(&self) -> &::std::sync::Arc<::keel::signals::StoreCell> {
        self.__keel_cell
            .get_or_init(|| {
                let __cell = ::keel::signals::StoreCell::new(
                    ::keel::meta::ids::type_id("Counter"),
                );
                __cell.attach(&self.count, 0u32, ::core::option::Option::None);
                __cell
            })
    }
    #[allow(unused_mut, unused_variables)]
    fn restore(
        __ctx: ::keel::runtime::Ctx,
        __r: &mut ::keel::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::keel::wire::WireError> {
        let __count = __r.read_u32()?;
        let mut __slot_count: ::core::option::Option<i64> = ::core::option::Option::None;
        for _ in 0..__count {
            let __id = __r.read_u32()?;
            let __bytes = __r.read_bytes()?;
            match __id {
                0u32 => {
                    __slot_count = ::core::option::Option::Some(
                        <i64 as ::keel::wire::Decode>::decode_exact(__bytes)?,
                    );
                }
                _ => {}
            }
        }
        let __value_count = match __slot_count {
            ::core::option::Option::Some(__v) => __v,
            ::core::option::Option::None => {
                return ::core::result::Result::Err(::keel::wire::WireError::InvalidTag {
                    tag: 0u32,
                    at: __r.position(),
                    ty: "snapshot of store Counter is missing signal 0 (count)",
                });
            }
        };
        let __value = {
            let _ = &__ctx;
            Self {
                ctx: ::core::clone::Clone::clone(&__ctx),
                note: ::core::default::Default::default(),
                count: ::keel::signals::Signal::<i64>::new(__value_count),
                __keel_cell: ::core::default::Default::default(),
            }
        };
        __value.__keel_attach_all();
        ::core::result::Result::Ok(__value)
    }
}
#[doc(hidden)]
#[allow(non_snake_case)]
fn __keel_restore_erased_Counter(
    __ctx: ::keel::runtime::Ctx,
    __r: &mut ::keel::wire::Reader<'_>,
) -> ::core::result::Result<
    ::std::sync::Arc<dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync>,
    ::keel::wire::WireError,
> {
    <Counter as ::keel::runtime::StoreObject>::restore(__ctx, __r)
        .map(|__value| {
            ::std::sync::Arc::new(__value)
                as ::std::sync::Arc<
                    dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync,
                >
        })
}
::keel::meta::inventory::submit! {
    ::keel::runtime::StoreRestorer { type_id : ::keel::meta::ids::type_id("Counter"),
    restore : __keel_restore_erased_Counter, }
}
