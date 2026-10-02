/// Keeps rows by id.
impl<T: Row> Cache<T> {
    pub fn new() -> Self {
        Cache(Vec::new())
    }
    /// Stores a row.
    pub fn put(&self, row: T) {}
    pub fn get(&self, id: Uuid) -> Option<T> {
        None
    }
}
#[doc(hidden)]
#[allow(non_upper_case_globals, dead_code)]
const _undra_error_E0007_Cache_has_two_undra_api_impl_blocks_merge_them_into_one: () = ();
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
    fn __undra_leaf<A, K>()
    where
        A: ?::core::marker::Sized + ::undra::wire::leaf::WireLeaf<K>,
    {}
    fn __undra_identity<T>() {
        __undra_leaf::<Uuid, ::undra::wire::leaf::kinds::Uuid>();
        __undra_same::<Option<T>, ::core::option::Option<T>>();
    }
};
#[doc(hidden)]
#[macro_export]
macro_rules! __undra_template_Cache {
    ($__alias:ident, $__docs:literal, $__undra_p0:ty) => {
        ::undra::__instantiate! { #[undra_instance(kind = "object", template = "Cache",
        crate_name = "", root = "::undra", docs = "Keeps rows by id.", impl_docs = "",
        restore = "", alias_docs = $__docs)] impl $__alias { pub fn new() -> Self {}
        #[doc = r" Stores a row."] pub fn put(& self, row : $__undra_p0) {} pub fn get(&
        self, id : Uuid) -> Option < $__undra_p0 > {} } type __UndraInstanceArgs =
        ($__undra_p0,); }
    };
    ($($__rest:tt)*) => {
        ::core::compile_error!("error[undra::E0002]: `Cache` takes 1 type argument (`T`), as declared with `#[undra::api(generic)]`\n  = note: an instantiation names every type parameter of the template: the alias is what the schema and the platforms see\n  = help: write all the arguments: `#[undra::api] pub type MyCache = Cache<Todo>;`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0002");
    };
}
#[doc(hidden)]
#[allow(unused_imports, unreachable_pub)]
pub use __undra_template_Cache as Cache;
