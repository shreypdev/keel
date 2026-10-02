//! Snapshot tests: the expansion of each macro on a fixture, pretty-printed with
//! `prettyplease` and compared with `tests/snapshots/<name>.rs`.
//!
//! The snapshots are the reviewable form of "what the macros emit"; `undra-bindgen` and the
//! runtime depend on these shapes. After an intended change, regenerate them with
//!
//! ```text
//! UPDATE_SNAPSHOTS=1 cargo test -p undra-macros
//! ```
//!
//! and review the diff. There is no `insta` dependency on purpose: the comparison is a plain
//! string equality on checked-in files.

use std::path::PathBuf;

use proc_macro2::TokenStream;
use quote::quote;

use crate::impl_;

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("snapshots")
        .join(format!("{name}.rs"))
}

/// Pretty-prints generated code. Fails the test if it is not valid Rust syntax.
fn pretty(tokens: TokenStream) -> String {
    let file: syn::File = syn::parse2(tokens.clone())
        .unwrap_or_else(|error| panic!("generated code does not parse: {error}\n{tokens}"));
    prettyplease::unparse(&file)
}

/// The line-by-line difference between two texts, for a readable failure.
fn first_difference(expected: &str, actual: &str) -> String {
    for (index, (e, a)) in expected.lines().zip(actual.lines()).enumerate() {
        if e != a {
            return format!("line {}:\n  expected: {e}\n  actual:   {a}", index + 1);
        }
    }
    format!(
        "expected {} lines, got {}",
        expected.lines().count(),
        actual.lines().count()
    )
}

fn check(name: &str, tokens: TokenStream) {
    let actual = pretty(tokens);
    assert!(
        !actual.contains("compile_error"),
        "fixture `{name}` produced a diagnostic:\n{actual}"
    );
    let path = snapshot_path(name);
    if std::env::var_os("UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().expect("snapshot dir")).expect("create snapshot dir");
        std::fs::write(&path, &actual).expect("write snapshot");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing snapshot {}; create it with UPDATE_SNAPSHOTS=1 cargo test -p undra-macros",
            path.display()
        )
    });
    assert!(
        expected == actual,
        "snapshot `{name}` changed ({}); review and run UPDATE_SNAPSHOTS=1 cargo test -p undra-macros",
        first_difference(&expected, &actual)
    );
}

fn api(attr: TokenStream, item: TokenStream) -> TokenStream {
    impl_::expand_api(attr, item)
}

#[test]
fn record() {
    check(
        "record",
        api(
            quote!(),
            quote! {
                /// A todo item.
                #[derive(Clone, Debug, PartialEq)]
                pub struct Todo {
                    /// Stable id.
                    pub id: Uuid,
                    pub title: String,
                    #[undra(default)]
                    pub done: bool,
                    pub tags: Vec<String>,
                    pub due: Option<Timestamp>,
                    pub scores: HashMap<String, i32>,
                }
            },
        ),
    );
}

#[test]
fn record_with_crate_override() {
    check(
        "record_crate_override",
        api(
            quote!(crate = "::undra_runtime"),
            quote! {
                pub struct Point { pub x: f64, pub y: f64 }
            },
        ),
    );
}

#[test]
fn enum_with_every_variant_shape() {
    check(
        "enum",
        api(
            quote!(),
            quote! {
                /// A shape.
                pub enum Shape {
                    /// Nothing.
                    Empty,
                    Circle { radius: f64 },
                    Rect(f64, f64),
                }
            },
        ),
    );
}

#[test]
fn error_enum() {
    check(
        "error",
        impl_::expand_error(
            quote!(),
            quote! {
                #[derive(Clone)]
                pub enum TodoError {
                    #[error("title cannot be empty")]
                    EmptyTitle,
                    #[error("todo {0} not found")]
                    NotFound(Uuid),
                    #[error("rejected with {code}: {reason}")]
                    Rejected { code: u16, reason: String },
                    #[error(transparent)]
                    Http(#[from] HttpError),
                    #[error("storage failed")]
                    Storage(#[source] StorageError),
                }
            },
        ),
    );
}

#[test]
fn object() {
    check(
        "object",
        api(
            quote!(),
            quote! {
                /// A calculator.
                impl Calculator {
                    /// Creates one.
                    pub fn new(ctx: &Ctx, base: i64) -> Self { todo!() }

                    pub fn open(path: String) -> Result<Self, CalcError> { todo!() }

                    /// Adds.
                    pub fn add(&self, a: i64, b: i64) -> i64 { todo!() }

                    pub fn divide(&self, a: i64, b: i64) -> Result<i64, CalcError> { todo!() }

                    pub async fn slow_add(&self, a: i64) -> i64 { todo!() }

                    pub async fn slow_divide(&self, a: i64, b: i64) -> Result<i64, CalcError> { todo!() }

                    pub fn counts(&self, up_to: u32) -> impl Stream<Item = u32> { todo!() }

                    fn helper(&self) {}
                }
            },
        ),
    );
}

#[test]
fn free_function() {
    check(
        "function",
        api(
            quote!(),
            quote! {
                /// Greets.
                pub async fn greet(ctx: &Ctx, name: String) -> Result<String, GreetError> { todo!() }
            },
        ),
    );
}

#[test]
fn store_struct() {
    check(
        "store",
        impl_::expand_store(
            quote!(restore = "Self::assemble"),
            quote! {
                pub struct Todos {
                    ctx: Ctx,
                    #[undra(key = "id")]
                    rows: Signal<Vec<Row>>,
                    #[undra(no_coalesce)]
                    ticks: Signal<u32>,
                    visible: Computed<Vec<Row>>,
                    label: String,
                }
            },
        ),
    );
}

#[test]
fn store_with_generated_restore() {
    check(
        "store_default_restore",
        impl_::expand_store(
            quote!(),
            quote! {
                pub struct Counter {
                    ctx: Ctx,
                    count: Signal<i64>,
                    note: String,
                    #[undra(default)]
                    visits: Signal<u32>,
                }
            },
        ),
    );
}

#[test]
fn migration_hooks() {
    let ty = impl_::expand_migrate(
        quote!(ty = "Todo", from = "0x00000000000000ff"),
        quote! {
            fn todo_v1(old: &DynValue) -> Result<Todo, MigrateError> {
                unimplemented!()
            }
        },
    );
    let signal = impl_::expand_migrate(
        quote!(store = "Profile", signal = "age"),
        quote! {
            fn age(old: Option<&DynValue>) -> Result<f32, MigrateError> {
                Ok(18.0)
            }
        },
    );
    let mutation = impl_::expand_migrate(
        quote!(mutation = "add_todo"),
        quote! {
            fn add_todo(old: &DynRecord) -> Result<DynRecord, MigrateError> {
                Ok(old.clone())
            }
        },
    );
    check("migrate", quote!(#ty #signal #mutation));
}

#[test]
fn store_impl_block() {
    check(
        "store_impl",
        api(
            quote!(store),
            quote! {
                impl Todos {
                    pub fn new(ctx: Ctx) -> Self {
                        Self { ctx, rows: Signal::new(vec![]) }
                    }
                    pub fn add(&self, title: String) {}
                }
            },
        ),
    );
}

#[test]
fn async_port() {
    check(
        "port_async",
        impl_::expand_port(
            quote!(),
            quote! {
                /// Talks HTTP.
                pub trait Http {
                    /// Sends a request.
                    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError>;
                    async fn ping(&self, url: String) -> bool;
                }
            },
        ),
    );
}

#[test]
fn sync_port() {
    check(
        "port_sync",
        impl_::expand_port(
            quote!(sync),
            quote! {
                pub trait Clock {
                    fn now_ms(&self) -> i64;
                    fn log(&self, level: u8, message: String);
                }
            },
        ),
    );
}

#[test]
fn event_port() {
    check(
        "port_event",
        impl_::expand_port(
            quote!(event),
            quote! {
                pub trait Connectivity {
                    fn changed(&self, online: bool, kind: NetKind);
                }
            },
        ),
    );
}

#[test]
fn port_impl_block() {
    check(
        "port_impl",
        impl_::expand_port(
            quote!(),
            quote! {
                impl Http for FakeHttp {
                    async fn request(&self, req: HttpRequest) -> Result<HttpResponse, HttpError> {
                        todo!()
                    }
                    fn other(&self) {}
                }
            },
        ),
    );
}

#[test]
fn query() {
    check(
        "query",
        impl_::expand_query(
            impl_::query::Flavor::Query,
            quote!(key = "todos:{page}", stale = "30s", persist, retry = 5),
            quote! {
                pub async fn todos(ctx: &Ctx, page: u32) -> Result<Vec<Todo>, HttpError> { todo!() }
            },
        ),
    );
}

#[test]
fn mutation() {
    check(
        "mutation",
        impl_::expand_query(
            impl_::query::Flavor::Mutation,
            quote!(idempotent, retry = 2, key = "todos"),
            quote! {
                pub async fn add_todo(ctx: Ctx, title: String) -> Result<Todo, HttpError> { todo!() }
            },
        ),
    );
}

#[test]
fn every_snapshot_file_has_a_test() {
    // A snapshot nobody checks is dead weight: list the directory and require a fixture for
    // each entry. (Adding a fixture means adding its name here and a `check(..)` above.)
    let known = [
        "record",
        "record_crate_override",
        "enum",
        "error",
        "object",
        "function",
        "store",
        "store_default_restore",
        "store_impl",
        "port_async",
        "port_sync",
        "port_event",
        "port_impl",
        "query",
        "mutation",
        "migrate",
    ];
    let dir = snapshot_path("x");
    let dir = dir.parent().expect("snapshot dir");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return; // nothing generated yet (first run with UPDATE_SNAPSHOTS)
    };
    for entry in entries {
        let name = entry.expect("dir entry").file_name();
        let name = name.to_string_lossy();
        let stem = name.trim_end_matches(".rs");
        assert!(known.contains(&stem), "unexpected snapshot file {name}");
    }
}
