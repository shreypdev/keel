//! One fixture per diagnostic code: every rejection must carry the code, a note explaining why,
//! a help line with the fix and the docs link (constitution rule R8), and must still emit the
//! original item so the user sees no follow-up errors.

use proc_macro2::TokenStream;
use quote::quote;

use crate::impl_;
use crate::impl_::query::Flavor;

/// The messages of the `compile_error!` invocations in `tokens`.
fn messages(tokens: &TokenStream) -> Vec<String> {
    let file: syn::File = syn::parse2(tokens.clone()).expect("output parses");
    file.items
        .iter()
        .filter_map(|item| match item {
            syn::Item::Macro(m)
                if m.mac
                    .path
                    .segments
                    .last()
                    .is_some_and(|seg| seg.ident == "compile_error") =>
            {
                let lit: syn::LitStr = syn::parse2(m.mac.tokens.clone()).expect("string literal");
                Some(lit.value())
            }
            _ => None,
        })
        .collect()
}

/// Asserts that `tokens` reports `code` with the documented shape and still contains `keeps`.
fn expect(code: &str, tokens: TokenStream, keeps: &str) {
    let rendered = tokens.to_string();
    let all = messages(&tokens);
    let message = all
        .iter()
        .find(|m| m.starts_with(&format!("error[keel::{code}]: ")))
        .unwrap_or_else(|| panic!("no {code} in {all:?}"));
    let lines: Vec<&str> = message.lines().collect();
    assert_eq!(
        lines.len(),
        4,
        "{code} must be what/note/help/docs: {message}"
    );
    assert!(lines[1].starts_with("  = note: "), "{message}");
    assert!(lines[2].starts_with("  = help: "), "{message}");
    assert_eq!(
        lines[3],
        format!("  = docs: https://keel.dev/errors/{code}"),
        "{message}"
    );
    assert!(
        rendered.contains(keeps),
        "the original item `{keeps}` must survive the error: {rendered}"
    );
    assert!(
        !rendered.contains("# [keel (") && !rendered.contains("# [error ("),
        "helper attributes must be stripped from the fallback item: {rendered}"
    );
}

fn api(item: TokenStream) -> TokenStream {
    impl_::expand_api(quote!(), item)
}

#[test]
fn e0001_unsupported_type() {
    expect(
        "E0001",
        api(quote!(
            pub struct S {
                #[keel(default)]
                a: &str,
            }
        )),
        "struct S",
    );
    expect(
        "E0001",
        api(quote!(
            pub struct S {
                n: usize,
            }
        )),
        "struct S",
    );
    expect(
        "E0001",
        api(quote!(impl C { pub fn f(&self, ctx: &Ctx) {} })),
        "impl C",
    );
}

#[test]
fn e0002_generics() {
    expect(
        "E0002",
        api(quote!(
            pub struct S<T> {
                a: T,
            }
        )),
        "struct S",
    );
    expect(
        "E0002",
        api(quote!(
            impl<T> C<T> {
                pub fn f(&self) {}
            }
        )),
        "impl",
    );
    expect(
        "E0002",
        api(quote!(
            pub fn f<T>(x: T) {}
        )),
        "fn f",
    );
}

#[test]
fn e0003_lifetimes() {
    expect(
        "E0003",
        api(quote!(
            pub struct S<'a> {
                a: u8,
            }
        )),
        "struct S",
    );
    expect(
        "E0003",
        api(quote!(
            pub struct S {
                a: &'a str,
            }
        )),
        "struct S",
    );
}

#[test]
fn e0004_trait_objects_and_callbacks() {
    expect(
        "E0004",
        api(quote!(impl C { pub fn f(&self, cb: Box<dyn Fn()>) {} })),
        "impl C",
    );
    expect(
        "E0004",
        api(quote!(
            pub fn f(x: impl Display) {}
        )),
        "fn f",
    );
}

#[test]
fn e0005_result_and_stream_outside_return() {
    expect(
        "E0005",
        api(quote!(
            pub struct S {
                r: Result<u8, E>,
            }
        )),
        "struct S",
    );
    expect(
        "E0005",
        api(quote!(
            pub fn f(r: Result<u8, E>) {}
        )),
        "fn f",
    );
}

#[test]
fn e0006_map_keys() {
    expect(
        "E0006",
        api(quote!(
            pub struct S {
                m: HashMap<f64, u8>,
            }
        )),
        "struct S",
    );
}

#[test]
fn e0007_unsupported_shapes() {
    expect(
        "E0007",
        api(quote!(
            pub struct S(u8);
        )),
        "struct S",
    );
    expect(
        "E0007",
        api(quote!(
            pub trait T {}
        )),
        "trait T",
    );
    expect(
        "E0007",
        api(quote!(
            pub enum E {}
        )),
        "enum E",
    );
    expect(
        "E0007",
        api(quote!(impl C { pub fn helper() {} })),
        "impl C",
    );
}

#[test]
fn e0008_unknown_attributes_and_arguments() {
    expect(
        "E0008",
        api(quote!(
            pub struct S {
                #[keel(bogus)]
                a: u8,
            }
        )),
        "struct S",
    );
    expect(
        "E0008",
        impl_::expand_api(
            quote!(nope),
            quote!(
                pub struct S {
                    a: u8,
                }
            ),
        ),
        "struct S",
    );
}

#[test]
fn e0010_error_variants() {
    expect(
        "E0010",
        impl_::expand_error(
            quote!(),
            quote!(
                pub enum E {
                    A,
                }
            ),
        ),
        "enum E",
    );
    expect(
        "E0010",
        impl_::expand_error(
            quote!(),
            quote!(
                pub enum E {
                    #[error("x {9}")]
                    A(u8),
                }
            ),
        ),
        "enum E",
    );
}

#[test]
fn e0011_store_without_constructor() {
    expect(
        "E0011",
        impl_::expand_api(quote!(store), quote!(impl S { pub fn get(&self) {} })),
        "impl S",
    );
}

#[test]
fn e0012_trait_object_in_a_record_field() {
    expect(
        "E0012",
        api(quote!(
            pub struct Cart {
                pub items: Vec<Box<dyn Any>>,
            }
        )),
        "struct Cart",
    );
}

#[test]
fn e0013_store_restore() {
    expect(
        "E0013",
        impl_::expand_store(
            quote!(),
            quote!(
                pub struct S {
                    a: Signal<u8>,
                    b: Computed<u8>,
                }
            ),
        ),
        "struct S",
    );
}

#[test]
fn e0020_and_e0021_receivers() {
    expect(
        "E0020",
        api(quote!(impl C { pub fn f(&mut self) {} })),
        "impl C",
    );
    expect("E0021", api(quote!(impl C { pub fn f(self) {} })), "impl C");
}

#[test]
fn e0030_e0031_e0032_ports() {
    expect(
        "E0030",
        impl_::expand_port(
            quote!(),
            quote!(
                pub trait P {
                    fn f(&self, s: &str);
                }
            ),
        ),
        "trait P",
    );
    expect(
        "E0031",
        impl_::expand_port(
            quote!(event),
            quote!(
                pub trait P {
                    fn f(&self) -> u8;
                }
            ),
        ),
        "trait P",
    );
    expect(
        "E0032",
        impl_::expand_port(
            quote!(sync),
            quote!(
                pub trait P {
                    async fn f(&self);
                }
            ),
        ),
        "trait P",
    );
}

#[test]
fn e0040_query_arguments() {
    let query = quote!(
        pub async fn q(ctx: &Ctx) -> Result<u8, E> {
            todo!()
        }
    );
    expect(
        "E0040",
        impl_::expand_query(Flavor::Query, quote!(), query.clone()),
        "fn q",
    );
    expect(
        "E0040",
        impl_::expand_query(Flavor::Mutation, quote!(stale = "1s"), query),
        "fn q",
    );
}

#[test]
fn e0041_query_signature() {
    expect(
        "E0041",
        impl_::expand_query(
            Flavor::Query,
            quote!(key = "k"),
            quote!(
                pub fn q(ctx: &Ctx) -> Result<u8, E> {
                    todo!()
                }
            ),
        ),
        "fn q",
    );
}

#[test]
fn several_problems_are_all_reported_in_one_expansion() {
    let tokens = api(quote!(
        pub struct S {
            a: &str,
            b: usize,
            c: HashMap<f64, u8>,
        }
    ));
    let all = messages(&tokens);
    assert_eq!(all.len(), 3, "{all:?}");
    assert!(all[0].contains("E0001") && all[1].contains("E0001") && all[2].contains("E0006"));
}

#[test]
fn the_wrong_kind_of_item_is_e0007_with_the_item_kept() {
    expect(
        "E0007",
        impl_::expand_store(
            quote!(),
            quote!(
                pub enum E {
                    A,
                }
            ),
        ),
        "enum E",
    );
    expect(
        "E0007",
        impl_::expand_error(
            quote!(),
            quote!(
                pub struct S {
                    a: u8,
                }
            ),
        ),
        "struct S",
    );
    expect(
        "E0007",
        impl_::expand_query(
            Flavor::Query,
            quote!(key = "k"),
            quote!(
                pub struct S;
            ),
        ),
        "struct S",
    );
}
