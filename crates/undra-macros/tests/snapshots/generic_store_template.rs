/// The rows the user has ticked.
pub struct Selection<T> {
    /// The ticked rows.
    rows: Signal<Vec<T>>,
    count: Computed<u32>,
    #[doc(hidden)]
    pub __undra_cell: ::undra::signals::CellSlot,
}
impl<T> Selection<T> {
    /// Marks the struct as a generic store, so the impl block of a plain object
    /// that is not marked as a store can be told (E0011).
    #[doc(hidden)]
    pub const __UNDRA_IS_GENERIC_STORE: bool = true;
}
#[doc(hidden)]
#[allow(unused_macros)]
macro_rules! _undra_error_E0011_Selection_is_not_a_generic_store_declared_above_this_impl_block_in_the_same_module {
    ($__impl_docs:literal { $($__block:tt)* }) => {
        ::undra::__compose_store! { "Selection" "::undra" "The rows the user has ticked."
        "Self::assemble"[T] { pub struct __UNDRA_ALIAS { #[doc = r" The ticked rows."]
        #[undra(key = "id")] rows : Signal < Vec < __UNDRA_PARAM_0 > >, count : Computed
        < u32 >, } } $__impl_docs { $($__block)* } }
    };
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
        __undra_same::<u32, ::core::primitive::u32>();
    }
};
