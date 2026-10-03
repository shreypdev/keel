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
        .find(|m| m.starts_with(&format!("error[undra::{code}]: ")))
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
        format!("  = docs: https://shreypdev.github.io/undra/docs/errors.html#{code}"),
        "{message}"
    );
    assert!(
        rendered.contains(keeps),
        "the original item `{keeps}` must survive the error: {rendered}"
    );
    assert!(
        !rendered.contains("# [undra (") && !rendered.contains("# [error ("),
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
                #[undra(default)]
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
            pub struct S(u8, u8);
        )),
        "struct S",
    );
    expect(
        "E0007",
        api(quote!(
            pub struct Marker;
        )),
        "struct Marker",
    );
    expect(
        "E0007",
        api(quote!(
            pub type Id = u64;
        )),
        "type Id",
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
                #[undra(bogus)]
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

#[test]
fn e0007_records_need_fields() {
    for item in [
        quote!(
            pub struct Marker;
        ),
        quote!(
            pub struct Braces {}
        ),
    ] {
        expect("E0007", api(item), "struct");
    }
}

#[test]
fn e0007_misplaced_undra_attributes_on_methods_are_reported_once() {
    // A method exposed one by one, and a query inside an impl block: the impl block's macro sees
    // the attributes unexpanded and reports them; the fallback drops them, so the method's own
    // expansion does not report them again.
    let tokens = api(quote!(
        impl C {
            #[undra::api]
            pub fn f(&self) {}
            #[undra::query(key = "k")]
            pub async fn q(ctx: &Ctx) -> Result<u8, E> {}
        }
    ));
    let all = messages(&tokens);
    assert_eq!(all.len(), 2, "{all:?}");
    assert!(all[0].contains("`#[undra::api]` on a method"), "{}", all[0]);
    assert!(all[1].contains("`#[undra::query]` inside an `impl` block"));
    assert!(!tokens.to_string().contains("undra :: query"), "{tokens}");
    expect(
        "E0007",
        api(quote!(impl C { #[undra::mutation] pub async fn m(ctx: &Ctx) {} })),
        "impl C",
    );
    // A receiver on a function the attribute was put on.
    expect(
        "E0007",
        api(quote!(
            pub fn f(&self) {}
        )),
        "fn f",
    );
}

#[test]
fn e0001_result_error_type_must_be_a_name() {
    expect(
        "E0001",
        api(quote!(
            pub fn f() -> Result<u8, String> {}
        )),
        "fn f",
    );
    expect(
        "E0001",
        api(quote!(impl C { pub fn new() -> Result<Self, String> {} })),
        "impl C",
    );
    expect(
        "E0001",
        api(quote!(
            pub fn f(h: Handle) {}
        )),
        "fn f",
    );
}

#[test]
fn e0042_a_query_cannot_cache_unit_or_option() {
    for ret in [quote!(Result<(), E>), quote!(Result<Option<Todo>, E>)] {
        expect(
            "E0042",
            impl_::expand_query(
                Flavor::Query,
                quote!(key = "k"),
                quote!(
                    pub async fn q(ctx: &Ctx) -> #ret {}
                ),
            ),
            "fn q",
        );
    }
    // A mutation may return `()`.
    let ok = impl_::expand_query(
        Flavor::Mutation,
        quote!(),
        quote!(
            pub async fn m(ctx: &Ctx) -> Result<(), E> {}
        ),
    );
    assert!(messages(&ok).is_empty());
}

#[test]
fn e0063_nested_options() {
    expect(
        "E0063",
        api(quote!(
            pub struct S {
                a: Option<Option<u8>>,
            }
        )),
        "struct S",
    );
}

#[test]
fn e0010_error_enum_helpers_on_a_plain_enum_and_implicit_placeholders() {
    expect(
        "E0010",
        api(quote!(
            pub enum E {
                #[error("x")]
                A,
            }
        )),
        "enum E",
    );
    expect(
        "E0010",
        impl_::expand_error(
            quote!(),
            quote!(
                pub enum E {
                    #[error("{} and {}")]
                    A(u8, u8),
                }
            ),
        ),
        "enum E",
    );
}

#[test]
fn e0008_values_of_the_wrong_kind_are_undra_diagnostics() {
    let cases = [
        (
            impl_::expand_api(
                quote!(crate),
                quote!(
                    pub fn f() {}
                ),
            ),
            "`crate` needs a value",
        ),
        (
            impl_::expand_api(
                quote!(crate = 5),
                quote!(
                    pub fn f() {}
                ),
            ),
            "`crate` must be a string literal",
        ),
        (
            impl_::expand_port(
                quote!(sync = true),
                quote!(
                    pub trait P {
                        fn f(&self);
                    }
                ),
            ),
            "`sync` takes no value",
        ),
        (
            impl_::expand_store(
                quote!(restore = 5),
                quote!(
                    pub struct S {
                        a: Signal<u8>,
                    }
                ),
            ),
            "`restore` must name a function",
        ),
        (
            api(quote!(
                pub struct S {
                    #[undra(default = true)]
                    a: u8,
                }
            )),
            "`default` takes no value",
        ),
    ];
    for (tokens, expected) in cases {
        let all = messages(&tokens);
        assert!(
            all.iter()
                .any(|m| m.starts_with("error[undra::E0008]") && m.contains(expected)),
            "{expected}: {all:?}"
        );
    }
    let query = impl_::expand_query(
        Flavor::Query,
        quote!(key = "k", retry = "3"),
        quote!(
            pub async fn q(ctx: &Ctx) -> Result<u8, E> {}
        ),
    );
    let all = messages(&query);
    assert!(all[0].starts_with("error[undra::E0040]") && all[0].contains("`retry` must be"));
}

#[test]
fn a_failed_store_still_provides_what_its_impl_block_uses() {
    // M1: the error is the only error because the struct keeps the hidden field and the members
    // the `#[undra::api(store)]` impl block refers to.
    let tokens = impl_::expand_store(
        quote!(crate = "::k"),
        quote!(
            pub struct S {
                #[undra(key = "id")]
                a: Signal<Vec<Row>>,
                b: Computed<u8>,
            }
        ),
    );
    let rendered = tokens.to_string();
    let all = messages(&tokens);
    assert_eq!(all.len(), 1, "{all:?}");
    for needle in [
        "pub __undra_cell : :: k :: signals :: CellSlot",
        "pub const __UNDRA_IS_STORE : bool = true",
        "pub const __UNDRA_STORE_META : :: k :: meta :: StoreMeta",
        "pub fn __undra_attach_all",
        "pub fn __undra_set_handle",
        "pub fn __undra_cell_ref",
        "pub fn __undra_restore",
    ] {
        assert!(
            rendered.contains(needle),
            "missing `{needle}` in {rendered}"
        );
    }
    assert!(!rendered.contains("# [undra ("), "{rendered}");
}

#[test]
fn error_helpers_are_left_to_another_derive_that_owns_them() {
    // `#[derive(thiserror::Error)]` brings its own `#[error]`, `#[from]` and `#[source]`: the
    // E0010 for a plain enum must not fire, and the fallback must keep them.
    let tokens = api(quote!(
        #[derive(Debug, thiserror::Error)]
        pub enum E {
            #[error("one {0}")]
            A(u8),
            #[error("two")]
            B(#[source] String),
        }
    ));
    let rendered = tokens.to_string();
    assert!(messages(&tokens).is_empty(), "{rendered}");
    assert!(rendered.contains("# [error (\"one {0}\")]"), "{rendered}");
    assert!(rendered.contains("# [source]"), "{rendered}");
    // A failing item that keeps the helpers keeps them in the fallback too.
    let tokens = api(quote!(
        #[derive(Debug, thiserror::Error)]
        pub enum E {
            #[error("one")]
            A(&str),
        }
    ));
    assert!(
        tokens.to_string().contains("# [error (\"one\")]"),
        "{tokens}"
    );
}

/// Whether a token stream's text contains `needle`, ignoring whitespace.
fn squashed(tokens: &TokenStream) -> String {
    tokens.to_string().split_whitespace().collect()
}

#[test]
fn a_query_is_declared_through_a_guard_that_names_the_rule() {
    // A macro cannot see the block it sits in. The struct, statics and registrations a query adds
    // next to the function are module-level items, so a plain `impl` block (not `#[undra::api]`,
    // which is reported above) would get an error for each of them. Declared and invoked through
    // a `macro_rules!` whose name is the rule, `rustc` reports one parse error and one "cannot
    // find macro" error that names the rule and the fix.
    let query = squashed(&impl_::expand_query(
        Flavor::Query,
        quote!(key = "todos"),
        quote!(
            pub async fn todos(ctx: &Ctx, page: u32) -> Result<Vec<u32>, Failure> {
                Ok(vec![])
            }
        ),
    ));
    let guard = "_undra_error_E0007_a_query_is_a_free_function_move_it_out_of_the_impl_block";
    assert!(query.contains(&format!("macro_rules!{guard}")), "{query}");
    let invoked = query
        .find(&format!("{guard}!();"))
        .expect("the guard is invoked");
    // The struct and its registrations are inside; the `QueryDef` impl, which names the user's
    // parameter types, and the checks are not, so an error `rustc` reports on a type the user
    // wrote does not claim to come from the guard.
    let defined = query.find("pubstructTodosQuery;").expect("the struct");
    assert!(defined < invoked, "{query}");
    let query_def = query
        .find("implQueryDef")
        .or_else(|| query.find("::query::QueryDeffor"))
        .expect("the impl");
    assert!(query_def > invoked, "{query}");
    let checks = query
        .find("const__UNDRA_CHECKS_Todos:()=")
        .expect("named checks");
    assert!(checks > invoked, "{query}");
    let mutation = squashed(&impl_::expand_query(
        Flavor::Mutation,
        quote!(),
        quote!(
            pub async fn clear(ctx: &Ctx) -> Result<(), Failure> {
                Ok(())
            }
        ),
    ));
    assert!(
        mutation.contains(
            "_undra_error_E0007_a_mutation_is_a_free_function_move_it_out_of_the_impl_block!();"
        ),
        "{mutation}"
    );
}

#[test]
fn a_function_that_names_self_is_an_associated_function() {
    // `Self` only exists in an `impl` block, so a signature that uses it is the placement a macro
    // on a function cannot otherwise see.
    expect(
        "E0007",
        api(quote!(
            pub fn new() -> Self {
                Calc
            }
        )),
        "fn new",
    );
    let message = messages(&api(quote!(
        pub fn open(path: String) -> Result<Self, Failure> {
            todo!()
        }
    )))
    .remove(0);
    assert!(
        message.contains(
            "`#[undra::api]` on `open`, which is an associated function: its signature uses `Self`"
        ),
        "{message}"
    );
    assert!(
        message.contains("write `#[undra::api]` above the `impl` block"),
        "{message}"
    );
    let message = messages(&impl_::expand_query(
        Flavor::Query,
        quote!(key = "k"),
        quote!(
            pub async fn first(ctx: &Ctx) -> Result<Vec<Self>, Failure> {
                todo!()
            }
        ),
    ))
    .remove(0);
    assert!(
        message.contains("`#[undra::query]` on `first`, which is inside an `impl` block: its signature uses `Self`"),
        "{message}"
    );
    // A free function that does not mention `Self` is fine.
    assert!(
        messages(&api(quote!(
            pub fn plain(a: u32) -> u32 {
                a
            }
        )))
        .is_empty()
    );
}

#[test]
fn two_api_impl_blocks_get_one_constant_that_reads_as_the_rule() {
    let tokens = squashed(&api(quote!(
        impl Calc {
            pub fn new() -> Self {
                Calc
            }
        }
    )));
    assert!(
        tokens.contains(
            "const_undra_error_E0007_Calc_has_two_undra_api_impl_blocks_merge_them_into_one:()=();"
        ),
        "{tokens}"
    );
    // Everything else the block names lives in an anonymous constant, so a second block only
    // conflicts where Rust makes it (`UndraObject` implemented twice).
    for private in ["fn__undra_dispatch_Calc", "static__UNDRA_META_Calc"] {
        let at = tokens.find(private).unwrap_or_else(|| panic!("{private}"));
        let scope = tokens[..at]
            .rfind("const_:()={")
            .expect("inside an anonymous constant");
        assert!(scope < at, "{private}");
        assert!(
            !tokens[..scope].contains(private),
            "{private} is defined once"
        );
    }
}

#[test]
fn a_store_block_without_a_constructor_says_the_block_is_the_one_that_counts() {
    let tokens = impl_::expand_api(
        quote!(store),
        quote!(
            impl Counter {
                pub fn bump(&self) {}
            }
        ),
    );
    let all = messages(&tokens);
    assert_eq!(all.len(), 1, "{all:?}");
    assert!(
        all[0].contains("store `Counter` has no constructor in this `#[undra::api(store)]` block"),
        "{}",
        all[0]
    );
    assert!(
        all[0].contains("a constructor in another block is not seen"),
        "{}",
        all[0]
    );
    assert!(
        all[0].contains("move this block's methods into that one"),
        "{}",
        all[0]
    );
}

#[test]
fn unknown_options_and_arguments_suggest_the_nearest_name() {
    let message = |tokens: &TokenStream| messages(tokens).remove(0);
    let tokens = api(quote!(
        pub struct S {
            #[undra(defualt)]
            a: u8,
        }
    ));
    assert!(
        message(&tokens).contains("did you mean `default`?"),
        "{tokens}"
    );
    let tokens = impl_::expand_store(
        quote!(restor = "Self::rebuild"),
        quote!(
            pub struct S {
                a: u8,
            }
        ),
    );
    assert!(
        message(&tokens).contains("did you mean `restore`?"),
        "{tokens}"
    );
    // Nothing near it: the list instead.
    let tokens = api(quote!(
        pub struct S {
            #[undra(frobnicate)]
            a: u8,
        }
    ));
    let text = message(&tokens);
    assert!(
        text.contains(
            "remove `frobnicate`, or use one of: `crate`, `default`, `key`, `no_coalesce`"
        ),
        "{text}"
    );
}

#[test]
fn the_wrong_item_names_what_it_is_and_where_the_attribute_goes() {
    let tokens = impl_::expand_store(
        quote!(),
        quote!(
            pub enum E {
                A,
            }
        ),
    );
    let text = messages(&tokens).remove(0);
    assert!(
        text.contains("`#[undra::store]` cannot be applied to an `enum`"),
        "{text}"
    );
    assert!(
        text.contains("move `#[undra::store]` onto a struct, or remove it"),
        "{text}"
    );
    let tokens = impl_::expand_query(
        Flavor::Query,
        quote!(key = "k"),
        quote!(
            pub struct S;
        ),
    );
    let text = messages(&tokens).remove(0);
    assert!(
        text.contains("`#[undra::query]` cannot be applied to a `struct`"),
        "{text}"
    );
    assert!(
        text.contains("move `#[undra::query]` onto an `async fn`, or remove it"),
        "{text}"
    );
}

#[test]
fn e0007_a_query_on_a_port_method_is_reported_where_it_is() {
    let tokens = impl_::expand_port(
        quote!(),
        quote!(
            pub trait Weather {
                #[undra::query(key = "forecast")]
                async fn forecast(&self) -> String;
            }
        ),
    );
    let all = messages(&tokens);
    assert_eq!(all.len(), 1, "{all:?}");
    assert!(
        all[0].contains("`#[undra::query]` on the method `forecast` of a port trait"),
        "{}",
        all[0]
    );
    assert!(all[0].contains("weather(ctx).forecast(..)"), "{}", all[0]);
    assert!(
        all[0].contains("so write it outside the trait"),
        "{}",
        all[0]
    );
    expect("E0007", tokens, "trait Weather");
}

#[test]
fn a_keyed_list_looks_its_key_up_in_the_users_crate() {
    // The macro cannot see the fields of the item type: the lookup is a constant in the user's
    // crate, and the E0008 for a name that is not a field is raised there (see `store.rs`).
    let tokens = impl_::expand_store(
        quote!(),
        quote!(
            pub struct Rows {
                ctx: Ctx,
                #[undra(key = "idd")]
                rows: Signal<Vec<Row>>,
            }
        ),
    );
    let text = squashed(&tokens);
    assert!(
        text.contains("::undra::meta::keys::index_of(__UNDRA_FIELDS,\"idd\")"),
        "{text}"
    );
    assert!(text.contains("namesnofieldof`Row`"), "{text}");
    assert!(
        !text.contains("__item.idd"),
        "no field access of the user's spelling: {text}"
    );
}

#[test]
fn a_reference_field_is_reported_once_and_not_again_as_a_missing_lifetime() {
    // `&str` is E0001 ("use an owned `String`"). The item is still emitted, and without a lifetime
    // it would add rustc's E0106, whose advice (introduce a lifetime) says the opposite.
    let tokens = api(quote!(
        pub struct Profile {
            pub name: &str,
            pub tags: Vec<&str>,
        }
    ));
    expect("E0001", tokens.clone(), "struct Profile");
    let text = squashed(&tokens);
    assert!(text.contains("pubname:&'staticstr"), "{text}");
    assert!(text.contains("Vec<&'staticstr>"), "{text}");
}

/// Where the messages that no ui test can show are locked (see `tests/catalogue.rs`).
fn golden_path(code: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden/diagnostics")
        .join(format!("{code}.txt"))
}

/// E0070 for an alias in another crate than its template is raised while the alias expands, in a
/// crate that is not the template's: a ui test is one crate, so the message is locked here
/// (`UPDATE_GOLDEN=1 cargo test -p undra-macros --lib` rewrites it). The other E0070, a second alias
/// of one instantiation, is `rustc`'s duplicate-definition error and has the ui test
/// `e0070_second_alias`.
#[test]
fn e0070_an_alias_outside_the_crate_of_its_template() {
    let alias: syn::Ident = syn::parse_quote!(TodoPage);
    let diag = impl_::generic::foreign_alias_diag(&alias, "Page", "model", "app");
    let text = format!("{}\n", diag.message());
    let path = golden_path("E0070");
    if std::env::var("UPDATE_GOLDEN").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &text).unwrap();
    }
    let golden = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; run with UPDATE_GOLDEN=1", path.display()));
    assert_eq!(
        golden, text,
        "the message of E0070 changed: UPDATE_GOLDEN=1"
    );
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "{text}");
    assert!(lines[0].starts_with("error[undra::E0070]: "));
    assert_eq!(
        lines[3],
        "  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0070"
    );
}

// ---------------------------------------------------------------------------------------------
// Generic functions, methods, objects and stores (ADR-058)
// ---------------------------------------------------------------------------------------------

#[test]
fn e0072_a_function_that_cannot_be_listed_keeps_its_item() {
    expect(
        "E0072",
        impl_::expand_api(
            quote!(generic(T = [Vec<Todo>])),
            quote!(
                pub fn first<T>(rows: Vec<T>) -> Option<T> {
                    None
                }
            ),
        ),
        "pub fn first < T >",
    );
}

#[test]
fn e0002_a_generic_function_without_a_list_keeps_its_item() {
    expect(
        "E0002",
        impl_::expand_api(
            quote!(),
            quote!(
                pub fn first<T>(rows: Vec<T>) -> u8 {
                    0
                }
            ),
        ),
        "pub fn first < T >",
    );
}

#[test]
fn e0074_a_generic_block_with_the_wrong_header_keeps_its_block_and_leaves_a_stub_template() {
    let out = impl_::expand_api(
        quote!(generic),
        quote!(
            impl<T> Cache<Vec<T>> {
                pub fn len(&self) -> u32 {
                    0
                }
            }
        ),
    );
    expect("E0074", out.clone(), "impl < T > Cache < Vec < T > >");
    // The aliases written for the template do not add "cannot find macro" to the one error.
    assert!(
        crate::tests::has(&out.to_string(), "macro_rules! __undra_template_Cache"),
        "{out}"
    );
}

#[test]
fn e0074_a_method_with_its_own_parameters_in_a_generic_block() {
    expect(
        "E0074",
        impl_::expand_api(
            quote!(generic),
            quote!(
                impl<T: Row> Cache<T> {
                    pub fn map<U: Default>(&self) -> U {
                        U::default()
                    }
                }
            ),
        ),
        "pub fn map < U : Default >",
    );
}

#[test]
fn e0008_generic_on_a_store_without_type_parameters() {
    expect(
        "E0008",
        impl_::expand_store(
            quote!(generic),
            quote!(
                pub struct Counter {
                    count: Signal<u32>,
                }
            ),
        ),
        "pub struct Counter",
    );
}

#[test]
fn a_failed_generic_store_still_defines_what_its_block_and_its_aliases_call() {
    let out = impl_::expand_store(
        quote!(generic),
        quote!(
            pub struct Selection<T> {
                rows: Signal<&str>,
                marker: Option<T>,
            }
        ),
    );
    expect("E0001", out.clone(), "pub struct Selection < T >");
    let shown = out.to_string();
    assert!(
        crate::tests::has(
            &shown,
            "macro_rules! _undra_error_E0011_Selection_is_not_a_generic_store"
        ),
        "{shown}"
    );
    assert!(
        crate::tests::has(&shown, "macro_rules! __undra_template_Selection"),
        "{shown}"
    );
}

#[test]
fn e0070_an_alias_of_a_generic_object_in_another_crate() {
    let item = |kind: &str| -> TokenStream {
        let impl_docs = "";
        quote! {
            #[undra_instance(kind = #kind, template = "Cache", crate_name = "model_crate", root = "::undra", docs = "", impl_docs = #impl_docs, restore = "", alias_docs = "")]
            impl TodoCache {
                pub fn new() -> Self { }
            }
            type __UndraInstanceArgs = (Todo,);
        }
    };
    let items: Vec<syn::Item> = syn::parse2::<syn::File>(item("object")).unwrap().items;
    let error = impl_::generic::instantiate_object_in_for_tests(items, "app_crate")
        .unwrap_err()
        .to_string();
    assert!(
        error.starts_with(
            "error[undra::E0070]: `TodoCache` instantiates `Cache`, which is declared in the crate `model_crate`, not in `app_crate`"
        ),
        "{error}"
    );
    // The other way out is an object of this crate's own, not a record.
    assert!(
        error.contains(
            "or write the object out in this crate, with an `#[undra::api]` impl block of its own"
        ),
        "{error}"
    );
    assert!(!error.contains("struct or enum"), "{error}");
}

/// The module-level E0070 constant of an instantiation of `Cache` with the type arguments `args`.
fn e0070_rule_of(args: TokenStream) -> String {
    let object = impl_::expand_instantiate(quote! {
        #[undra_instance(kind = "object", template = "Cache", crate_name = "", root = "::undra", docs = "", impl_docs = "", restore = "", alias_docs = "")]
        impl SomeCache {
            pub fn new() -> Self { }
        }
        type __UndraInstanceArgs = #args;
    })
    .to_string();
    let start = object
        .find("_undra_error_E0070_this_instantiation_of_Cache_")
        .unwrap_or_else(|| panic!("no rule constant: {object}"));
    object[start..]
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .next()
        .unwrap()
        .to_owned()
}

#[test]
fn the_rule_of_e0070_names_each_instantiation_unambiguously() {
    // Two different instantiations in one module must not define one constant: that would be a
    // false "declared twice". Dropping punctuation made these pairs meet.
    for (a, b) in [
        (quote!((A_B, C,)), quote!((A, B_C,))),
        (quote!((Vec<Todo>,)), quote!((VecTodo,))),
        (quote!((crate::model::Todo,)), quote!((cratemodelTodo,))),
        (quote!((A, BC,)), quote!((AB, C,))),
        (quote!((A_B,)), quote!((A, B,))),
    ] {
        let (left, right) = (e0070_rule_of(a.clone()), e0070_rule_of(b.clone()));
        assert_ne!(left, right, "{a} and {b}");
    }
    // The same instantiation spelled the same way is one constant, and a plain name reads as itself.
    assert_eq!(
        e0070_rule_of(quote!((Todo,))),
        e0070_rule_of(quote!((Todo,)))
    );
    assert_eq!(
        e0070_rule_of(quote!((Todo,))),
        "_undra_error_E0070_this_instantiation_of_Cache_Todo_is_declared_twice_keep_one_alias_per_instantiation"
    );
}

#[test]
fn an_instantiation_carries_the_rule_of_e0070_in_one_place_per_part() {
    let object = impl_::expand_instantiate(quote! {
        #[undra_instance(kind = "object", template = "Cache", crate_name = "", root = "::undra", docs = "", impl_docs = "", restore = "", alias_docs = "")]
        impl TodoCache {
            pub fn new() -> Self { }
        }
        type __UndraInstanceArgs = (crate::model::Todo,);
    })
    .to_string();
    // The module-level constant names the type arguments, so two aliases in one module collide.
    assert!(
        object.contains("_undra_error_E0070_this_instantiation_of_Cache_crate_C_Cmodel_C_CTodo_is_declared_twice_keep_one_alias_per_instantiation"),
        "{object}"
    );
    // A plain object's alias also carries the inherent constant and the generic-store check.
    assert!(
        object.matches("pub const _undra_error_E0070_this_instantiation_of_Cache_is_declared_twice_keep_one_alias_per_instantiation").count() == 1,
        "{object}"
    );
    assert!(object.contains("__UNDRA_IS_GENERIC_STORE"), "{object}");
    let store = impl_::expand_instantiate(quote! {
        #[undra_instance(kind = "store", template = "Selection", crate_name = "", root = "::undra", docs = "", impl_docs = "", restore = "", alias_docs = "")]
        pub struct TodoSelection { rows: Signal<Vec<Todo>> }
        impl TodoSelection {
            pub fn new(ctx: Ctx) -> Self { }
        }
        type __UndraInstanceArgs = (Todo,);
    })
    .to_string();
    // A store carries the inherent constant once (in the store's own part), and no check that it
    // is not a store.
    assert_eq!(
        store
            .matches("pub const _undra_error_E0070_this_instantiation_of_Selection_is_declared_twice_keep_one_alias_per_instantiation")
            .count(),
        1,
        "{store}"
    );
    assert!(!store.contains("__UNDRA_IS_GENERIC_STORE"), "{store}");
}
