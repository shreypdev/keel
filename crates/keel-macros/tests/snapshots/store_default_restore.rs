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
        let __cell = ::keel::signals::StoreCell::new(
            ::keel::meta::ids::type_id("Counter"),
        );
        __cell.attach(&self.count, 0u32)?;
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
                            ::keel::meta::ids::type_id("Counter"),
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
                note: <String as __KeelRestoreDefault_Counter>::__keel_default(),
                count: ::keel::signals::Signal::<i64>::new(__value_count),
                __keel_cell: ::core::default::Default::default(),
            }
        };
        if __value.__keel_attach_all().is_err() {
            return ::core::result::Result::Err(::keel::wire::WireError::InvalidTag {
                tag: 0,
                at: __r.position(),
                ty: "restored store Counter could not attach its signals (a signal is already attached to another store)",
            });
        }
        ::core::result::Result::Ok(__value)
    }
}
#[doc(hidden)]
#[allow(non_snake_case)]
fn __keel_restore_erased_Counter(
    __ctx: ::keel::runtime::Ctx,
    __handle: u64,
    __r: &mut ::keel::wire::Reader<'_>,
) -> ::core::result::Result<
    ::std::sync::Arc<dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync>,
    ::keel::wire::WireError,
> {
    let __value = <Counter>::__keel_restore(__ctx, __r)?;
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
fn __keel_cell_erased_Counter(
    __any: &(dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync),
) -> ::core::option::Option<&::std::sync::Arc<::keel::signals::StoreCell>> {
    __any.downcast_ref::<Counter>().map(<Counter>::__keel_cell_ref)
}
::keel::meta::inventory::submit! {
    ::keel::runtime::StoreRestorer { type_id : ::keel::meta::ids::type_id("Counter"),
    restore : __keel_restore_erased_Counter, cell : __keel_cell_erased_Counter, }
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code)]
#[diagnostic::on_unimplemented(
    message = "error[keel::E0013]: store `Counter` cannot be restored automatically: its field of type `{Self}` has no `Default`\n  = note: restoring a snapshot rebuilds the store from its plain signals and fills every other field with `Default::default()` (a `Ctx` is cloned from the argument)\n  = help: implement `Default` for the type, or add `#[keel::store(restore = \"Self::rebuild\")]` with `fn rebuild(ctx: Ctx, <one Signal<T> per plain signal, in order>) -> Self`, the same code `new` uses to build the store\n  = docs: https://keel.dev/errors/E0013",
    label = "this field type has no `Default`"
)]
trait __KeelRestoreDefault_Counter: ::core::marker::Sized {
    fn __keel_default() -> Self;
}
impl<__KeelT: ::core::default::Default> __KeelRestoreDefault_Counter for __KeelT {
    fn __keel_default() -> Self {
        <__KeelT as ::core::default::Default>::default()
    }
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code, unused)]
const _: () = {
    #[diagnostic::on_unimplemented(
        message = "error[keel::E0011]: store `Counter` has no `#[keel::api(store)]` impl block\n  = note: the constructors of the impl block create the store and wire its signals to the platforms; without the block the store can never be instantiated\n  = help: add an `impl Counter` block marked `#[keel::api(store)]` with a constructor such as `pub fn new(ctx: Ctx) -> Self`\n  = docs: https://keel.dev/errors/E0011",
        label = "this store has no `#[keel::api(store)]` impl block"
    )]
    trait __KeelStoreNeedsImpl {}
    impl<__KeelT: ::keel::runtime::KeelObject> __KeelStoreNeedsImpl for __KeelT {}
    fn __keel_need_impl<__KeelT: __KeelStoreNeedsImpl>() {}
    fn __keel_check_impl() {
        __keel_need_impl::<Counter>();
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
        __keel_same::<i64, ::core::primitive::i64>();
    }
};
