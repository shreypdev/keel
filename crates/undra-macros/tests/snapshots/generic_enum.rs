pub enum Loadable<T, E> {
    Loading,
    Loaded(T),
    Failed { error: E, retry_after: Option<Duration> },
}
#[automatically_derived]
impl<T: ::undra::wire::Encode, E: ::undra::wire::Encode> ::undra::wire::Encode
for Loadable<T, E> {
    fn encode(&self, __w: &mut ::undra::wire::Writer) {
        match self {
            Self::Loading => {
                __w.write_u16(0u16);
            }
            Self::Loaded(__f0) => {
                __w.write_u16(1u16);
                ::undra::wire::Encode::encode(__f0, __w);
            }
            Self::Failed { error: __f0, retry_after: __f1 } => {
                __w.write_u16(2u16);
                ::undra::wire::Encode::encode(__f0, __w);
                ::undra::wire::Encode::encode(__f1, __w);
            }
        }
    }
}
#[automatically_derived]
impl<T: ::undra::wire::Decode, E: ::undra::wire::Decode> ::undra::wire::Decode
for Loadable<T, E> {
    const MIN_ENCODED_LEN: usize = 2;
    fn decode(
        __r: &mut ::undra::wire::Reader<'_>,
    ) -> ::core::result::Result<Self, ::undra::wire::WireError> {
        let __at = __r.position();
        match __r.read_u16()? {
            0u16 => ::core::result::Result::Ok(Self::Loading),
            1u16 => {
                ::core::result::Result::Ok(
                    Self::Loaded(<T as ::undra::wire::Decode>::decode(__r)?),
                )
            }
            2u16 => {
                ::core::result::Result::Ok(Self::Failed {
                    error: <E as ::undra::wire::Decode>::decode(__r)?,
                    retry_after: <Option<
                        Duration,
                    > as ::undra::wire::Decode>::decode(__r)?,
                })
            }
            __tag => {
                ::core::result::Result::Err(::undra::wire::WireError::InvalidTag {
                    tag: ::core::primitive::u32::from(__tag),
                    at: __at,
                    ty: "Loadable",
                })
            }
        }
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
    fn __undra_leaf<A, K>()
    where
        A: ?::core::marker::Sized + ::undra::wire::leaf::WireLeaf<K>,
    {}
    fn __undra_identity<T, E>() {
        __undra_same::<Option<Duration>, ::core::option::Option<Duration>>();
        __undra_leaf::<Duration, ::undra::wire::leaf::kinds::Duration>();
    }
};
#[doc(hidden)]
#[macro_export]
macro_rules! __undra_template_Loadable {
    ($__alias:ident, $__docs:literal, $T:ty, $E:ty) => {
        ::undra::__instantiate! { #[undra_instance(template = "Loadable", crate_name =
        "", root = "::undra", docs = "", alias_docs = $__docs)] pub enum $__alias {
        Loading, Loaded($T), Failed { error : $E, retry_after : Option < Duration > }, }
        }
    };
    ($($__rest:tt)*) => {
        ::core::compile_error!("error[undra::E0002]: `Loadable` takes 2 type arguments (`T`, `E`), as declared with `#[undra::api(generic)]`\n  = note: an instantiation names every type parameter of the template: the alias is what the schema and the platforms see\n  = help: write all the arguments: `#[undra::api] pub type TodoPage = Loadable<Todo, Todo>;`\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0002");
    };
}
#[doc(hidden)]
#[allow(unused_imports, unreachable_pub)]
pub use __undra_template_Loadable as Loadable;
