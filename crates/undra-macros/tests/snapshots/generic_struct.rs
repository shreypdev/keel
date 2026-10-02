/// One page of a list.
pub struct Page<T> {
    /// The rows.
    pub items: Vec<T>,
    pub next: Option<String>,
    pub by_name: HashMap<String, Vec<Option<T>>>,
}
#[automatically_derived]
impl<T: ::undra::wire::Encode> ::undra::wire::Encode for Page<T> {
    #[allow(unused_variables)]
    fn encode(&self, __w: &mut ::undra::wire::Writer) {
        ::undra::wire::Encode::encode(&self.items, __w);
        ::undra::wire::Encode::encode(&self.next, __w);
        ::undra::wire::Encode::encode(&self.by_name, __w);
    }
}
#[automatically_derived]
impl<T: ::undra::wire::Decode> ::undra::wire::Decode for Page<T> {
    const MIN_ENCODED_LEN: usize = 0usize
        + <Vec<T> as ::undra::wire::Decode>::MIN_ENCODED_LEN
        + <Option<String> as ::undra::wire::Decode>::MIN_ENCODED_LEN
        + <HashMap<String, Vec<Option<T>>> as ::undra::wire::Decode>::MIN_ENCODED_LEN;
    #[allow(unused_variables)]
    fn decode(
        __r: &mut ::undra::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::undra::wire::WireError> {
        ::core::result::Result::Ok(Self {
            items: <Vec<T> as ::undra::wire::Decode>::decode(__r)?,
            next: <Option<String> as ::undra::wire::Decode>::decode(__r)?,
            by_name: <HashMap<
                String,
                Vec<Option<T>>,
            > as ::undra::wire::Decode>::decode(__r)?,
        })
    }
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
    fn __undra_map_key<K>()
    where
        K: ?::core::marker::Sized + ::undra::wire::leaf::MapKey,
    {}
    fn __undra_identity<T>() {
        __undra_same::<Vec<T>, ::std::vec::Vec<T>>();
        __undra_same::<Option<String>, ::core::option::Option<String>>();
        __undra_same::<String, ::std::string::String>();
        __undra_same::<
            HashMap<String, Vec<Option<T>>>,
            ::std::collections::HashMap<String, Vec<Option<T>>>,
        >();
        __undra_same::<Vec<Option<T>>, ::std::vec::Vec<Option<T>>>();
        __undra_same::<Option<T>, ::core::option::Option<T>>();
        __undra_map_key::<String>();
    }
};
#[doc(hidden)]
#[macro_export]
macro_rules! __undra_template_Page {
    ($__alias:ident, $__docs:literal, $T:ty) => {
        ::undra::__instantiate! { #[undra_instance(template = "Page", crate_name = "",
        root = "::undra", docs = "One page of a list.", alias_docs = $__docs)] pub struct
        $__alias { #[doc = r" The rows."] pub items : Vec < $T >, #[undra(default)] pub
        next : Option < String >, pub by_name : HashMap < String, Vec < Option < $T > >
        >, } }
    };
    ($($__rest:tt)*) => {
        ::core::compile_error!("error[undra::E0002]: `Page` takes 1 type argument (`T`), as declared with `#[undra::api(generic)]`\n  = note: an instantiation names every type parameter of the template: the alias is what the schema and the platforms see\n  = help: write all the arguments: `#[undra::api] pub type TodoPage = Page<Todo>;`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0002");
    };
}
#[doc(hidden)]
#[allow(unused_imports, unreachable_pub)]
pub use __undra_template_Page as Page;
