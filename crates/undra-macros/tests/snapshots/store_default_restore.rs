pub struct Counter {
    ctx: Ctx,
    count: Signal<i64>,
    note: String,
    #[doc(hidden)]
    pub __undra_cell: ::undra::signals::CellSlot,
}
impl Counter {
    #[doc(hidden)]
    pub const __UNDRA_IS_STORE: bool = true;
    #[doc(hidden)]
    pub const __UNDRA_STORE_META: ::undra::meta::StoreMeta = ::undra::meta::StoreMeta {
        signals: &[
            ::undra::meta::SignalMeta {
                name: "count",
                signal_id: 0u32,
                ty: ::undra::meta::TypeRefMeta::I64,
                computed: false,
                key: ::core::option::Option::None,
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
        let __cell = ::undra::signals::StoreCell::new(
            ::undra::meta::ids::type_id("Counter"),
        );
        __cell.attach(&self.count, 0u32)?;
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
                            ::undra::meta::ids::type_id("Counter"),
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
        let mut __slot_count: ::core::option::Option<i64> = ::core::option::Option::None;
        for _ in 0..__count {
            let __id = __r.read_u32()?;
            let __bytes = __r.read_bytes()?;
            match __id {
                0u32 => {
                    __slot_count = ::core::option::Option::Some(
                        <i64 as ::undra::wire::Decode>::decode_exact(__bytes)?,
                    );
                }
                _ => {}
            }
        }
        let __value_count = match __slot_count {
            ::core::option::Option::Some(__v) => __v,
            ::core::option::Option::None => {
                return ::core::result::Result::Err(::undra::wire::WireError::InvalidTag {
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
                note: <String as __UndraRestoreDefault_Counter>::__undra_default(),
                count: ::undra::signals::Signal::<i64>::new(__value_count),
                __undra_cell: ::core::default::Default::default(),
            }
        };
        if __value.__undra_attach_all().is_err() {
            return ::core::result::Result::Err(::undra::wire::WireError::InvalidTag {
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
fn __undra_restore_erased_Counter(
    __ctx: ::undra::runtime::Ctx,
    __handle: u64,
    __r: &mut ::undra::wire::Reader<'_>,
) -> ::core::result::Result<
    ::std::sync::Arc<dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync>,
    ::undra::wire::WireError,
> {
    let __value = <Counter>::__undra_restore(__ctx, __r)?;
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
fn __undra_cell_erased_Counter(
    __any: &(dyn ::core::any::Any + ::core::marker::Send + ::core::marker::Sync),
) -> ::core::option::Option<&::std::sync::Arc<::undra::signals::StoreCell>> {
    __any.downcast_ref::<Counter>().map(<Counter>::__undra_cell_ref)
}
::undra::meta::inventory::submit! {
    ::undra::runtime::StoreRestorer { type_id : ::undra::meta::ids::type_id("Counter"),
    restore : __undra_restore_erased_Counter, cell : __undra_cell_erased_Counter, }
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code)]
#[diagnostic::on_unimplemented(
    message = "error[undra::E0013]: store `Counter` cannot be restored automatically: its field of type `{Self}` has no `Default`\n  = note: restoring a snapshot rebuilds the store from its plain signals and fills every other field with `Default::default()` (a `Ctx` is cloned from the argument)\n  = help: implement `Default` for the type, or add `#[undra::store(restore = \"Self::rebuild\")]` with `fn rebuild(ctx: Ctx, <one Signal<T> per plain signal, in order>) -> Self`, the same code `new` uses to build the store\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0013",
    label = "this field type has no `Default`"
)]
trait __UndraRestoreDefault_Counter: ::core::marker::Sized {
    fn __undra_default() -> Self;
}
impl<__UndraT: ::core::default::Default> __UndraRestoreDefault_Counter for __UndraT {
    fn __undra_default() -> Self {
        <__UndraT as ::core::default::Default>::default()
    }
}
#[doc(hidden)]
#[allow(non_camel_case_types, dead_code, unused)]
const _: () = {
    #[diagnostic::on_unimplemented(
        message = "error[undra::E0011]: store `Counter` has no `#[undra::api(store)]` impl block\n  = note: the constructors of the impl block create the store and wire its signals to the platforms; without the block the store can never be instantiated\n  = help: add an `impl Counter` block marked `#[undra::api(store)]` with a constructor such as `pub fn new(ctx: Ctx) -> Self`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0011",
        label = "this store has no `#[undra::api(store)]` impl block"
    )]
    trait __UndraStoreNeedsImpl {}
    impl<__UndraT: ::undra::runtime::UndraObject> __UndraStoreNeedsImpl for __UndraT {}
    fn __undra_need_impl<__UndraT: __UndraStoreNeedsImpl>() {}
    fn __undra_check_impl() {
        __undra_need_impl::<Counter>();
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
        __undra_same::<i64, ::core::primitive::i64>();
    }
};
