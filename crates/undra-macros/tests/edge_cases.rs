//! Awkward but legal inputs: types and impls in different modules, names that could collide with
//! the generated code's locals, raw identifiers, `mut` parameters, default trait bodies and
//! documentation with quotes and braces. The macros must handle all of them without
//! diagnostics or warnings (clippy runs over this file with `-D warnings`).
#![forbid(unsafe_code)]

use std::sync::Arc;

use undra::meta::{TypeRef, collect_schema, ids};
use undra::prelude::Ctx;
use undra::runtime::Port;
use undra::wire::{Decode, Encode, Handle};
use undra_macros as k;

mod support;
use support::Runtime;
use support::testing::block_on;

mod model {
    use undra_macros as k;

    #[k::api]
    #[derive(Clone, Debug, PartialEq)]
    pub struct Point {
        pub x: i32,
        pub y: i32,
    }

    /// An object defined here and implemented in a sibling module.
    #[derive(Default)]
    pub struct Canvas {
        pub(super) points: std::sync::Mutex<Vec<Point>>,
    }
}

mod api {
    use undra_macros as k;

    use super::model::{Canvas, Point};

    #[k::api]
    impl Canvas {
        pub fn new() -> Self {
            Canvas::default()
        }

        pub fn plot(&self, point: Point) -> u32 {
            let mut points = self.points.lock().unwrap();
            points.push(point);
            points.len() as u32
        }
    }

    // A free function next to the impl.
    #[k::api]
    pub fn origin() -> Point {
        Point { x: 0, y: 0 }
    }
}

#[test]
fn an_impl_in_another_module_than_the_type_works() {
    let rt = Runtime::new();
    let handle = Handle::decode_exact(&rt.call_object("Canvas", "new", 0, &[]).sync_ok())
        .unwrap()
        .0;
    let reply = rt
        .call_object(
            "Canvas",
            "plot",
            handle,
            &model::Point { x: 1, y: 2 }.encode_to_vec(),
        )
        .sync_ok();
    assert_eq!(u32::decode_exact(&reply).unwrap(), 1);
    let origin = rt.call_function("origin", &[]).sync_ok();
    assert_eq!(
        model::Point::decode_exact(&origin).unwrap(),
        model::Point { x: 0, y: 0 }
    );
}

/// Parameter names that look like the dispatcher's own locals.
#[derive(Default)]
pub struct Shadow;

#[k::api]
impl Shadow {
    pub fn new() -> Self {
        Shadow
    }

    #[allow(clippy::too_many_arguments)]
    pub fn echo(
        &self,
        call: u32,
        rt: u32,
        obj: u32,
        r: u32,
        ctx: u32,
        handle: u32,
        args: u32,
        method_id: u32,
    ) -> u32 {
        call + rt + obj + r + ctx + handle + args + method_id
    }

    pub fn takes_mut(&self, mut value: Vec<u8>) -> u32 {
        value.push(1);
        value.len() as u32
    }

    pub fn r#match(&self, r#type: String) -> String {
        r#type
    }
}

#[test]
fn locals_of_the_dispatcher_cannot_be_shadowed_by_parameters() {
    let rt = Runtime::new();
    let handle = Handle::decode_exact(&rt.call_object("Shadow", "new", 0, &[]).sync_ok())
        .unwrap()
        .0;
    let mut args = Vec::new();
    for value in 1_u32..=8 {
        value.encode(&mut undra::wire::Writer::new());
        args.extend(value.to_le_bytes());
    }
    let reply = rt.call_object("Shadow", "echo", handle, &args).sync_ok();
    assert_eq!(u32::decode_exact(&reply).unwrap(), 36);
    let reply = rt
        .call_object(
            "Shadow",
            "takes_mut",
            handle,
            &vec![1_u8, 2].encode_to_vec(),
        )
        .sync_ok();
    assert_eq!(u32::decode_exact(&reply).unwrap(), 3);
    let reply = rt
        .call_object("Shadow", "match", handle, &"t".to_owned().encode_to_vec())
        .sync_ok();
    assert_eq!(String::decode_exact(&reply).unwrap(), "t");

    let schema = collect_schema("edge");
    let shadow = schema.objects.iter().find(|o| o.name == "Shadow").unwrap();
    let names: Vec<&str> = shadow.methods.iter().map(|m| m.name.as_str()).collect();
    assert!(
        names.contains(&"match"),
        "raw identifiers lose `r#`: {names:?}"
    );
    let m = shadow.methods.iter().find(|m| m.name == "match").unwrap();
    assert_eq!(m.params[0].name, "type");
    assert_eq!(m.method_id, ids::method_id("Shadow", "match"));
}

/// A doc comment with "quotes", {braces}, `code`, a backslash \ and unicode: \u{1F30A}.
///
/// Second paragraph.
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Documented {
    /// Field doc with "quotes" and {braces}.
    pub value: u8,
}

#[test]
fn docs_reach_the_schema_verbatim() {
    let schema = collect_schema("edge");
    let record = schema
        .records
        .iter()
        .find(|r| r.name == "Documented")
        .unwrap();
    assert_eq!(
        record.docs,
        "A doc comment with \"quotes\", {braces}, `code`, a backslash \\ and unicode: \\u{1F30A}.\n\nSecond paragraph."
    );
    assert_eq!(
        record.fields[0].docs,
        "Field doc with \"quotes\" and {braces}."
    );
}

// A port with default method bodies, sync and async.
#[k::port]
pub trait Greeter {
    fn name(&self) -> String;

    fn greeting(&self) -> String {
        format!("hello {}", self.name())
    }

    async fn greet_later(&self) -> String {
        format!("later {}", self.name())
    }

    async fn required(&self, extra: u8) -> u8;
}

struct English;

#[k::port]
impl Greeter for English {
    fn name(&self) -> String {
        "english".to_owned()
    }

    async fn required(&self, extra: u8) -> u8 {
        extra + 1
    }
}

#[test]
fn default_bodies_in_port_traits_work_with_the_desugaring() {
    let english: Arc<dyn Greeter> = Arc::new(English);
    assert_eq!(english.greeting(), "hello english");
    assert_eq!(block_on(english.greet_later()), "later english");
    assert_eq!(block_on(english.required(1)), 2);
    // The proxy overrides every method, defaults included: they go to the host.
    let rt = Runtime::new();
    rt.bind_foreign(<dyn Greeter as Port>::PORT_ID, |method_id, _| {
        if method_id == ids::port_method_id("Greeter", "greeting") {
            Ok("from host".to_owned().encode_to_vec())
        } else {
            Ok(String::new().encode_to_vec())
        }
    });
    assert_eq!(greeter(&rt.ctx()).greeting(), "from host");
}

/// An enum with many variants and a `repr` attribute.
#[k::api]
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wide {
    V0,
    V1,
    V2,
    V3,
    V4,
    V5,
    V6,
    V7,
    V8,
    V9,
    V10,
    V11,
    V12,
    V13,
    V14,
    V15,
}

#[test]
fn enums_with_repr_and_many_variants() {
    assert_eq!(Wide::V15.encode_to_vec(), [15, 0]);
    assert_eq!(Wide::decode_exact(&[7, 0]).unwrap(), Wide::V7);
    assert!(Wide::decode_exact(&[16, 0]).is_err());
}

/// Nested generics of every wire collection.
#[allow(clippy::type_complexity)]
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Nested {
    pub deep:
        std::collections::HashMap<String, Vec<Option<Vec<std::collections::BTreeMap<u8, String>>>>>,
}

#[test]
fn deeply_nested_collections_map_and_round_trip() {
    let value = Nested {
        deep: [(
            "k".to_owned(),
            vec![
                Some(vec![[(1_u8, "v".to_owned())].into_iter().collect()]),
                None,
            ],
        )]
        .into_iter()
        .collect(),
    };
    assert_eq!(Nested::decode_exact(&value.encode_to_vec()).unwrap(), value);
    let schema = collect_schema("edge");
    let record = schema.records.iter().find(|r| r.name == "Nested").unwrap();
    assert_eq!(
        record.fields[0].ty,
        TypeRef::map(
            TypeRef::String,
            TypeRef::vec(TypeRef::option(TypeRef::vec(TypeRef::map(
                TypeRef::U8,
                TypeRef::String
            ))))
        )
    );
}

#[test]
fn ctx_is_a_plain_shared_handle() {
    // The `Ctx` given to free functions is the runtime's; cloning is cheap and allowed.
    let rt = Runtime::new();
    let ctx: Ctx = rt.ctx();
    let _copy = ctx.clone();
}
