//! Behaviour tests for `#[undra::api]` on structs and enums and for `#[undra::error]`:
//! the generated code is compiled against the real facade and *run*.
#![forbid(unsafe_code)]

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use undra::meta::{Registration, Schema, TypeRef, collect_schema, ids};
use undra::wire::{Bytes, Decode, Encode, Timestamp, Uuid, WireError, Writer};
use undra_macros as k;

fn schema() -> Schema {
    collect_schema("wire-types-test")
}

fn record_def(name: &str) -> undra::meta::RecordDef {
    schema()
        .records
        .into_iter()
        .find(|r| r.name == name)
        .unwrap_or_else(|| panic!("record {name} not registered"))
}

fn enum_def(name: &str) -> undra::meta::EnumDef {
    schema()
        .enums
        .into_iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("enum {name} not registered"))
}

// ---------------------------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------------------------

/// A todo item.
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Todo {
    /// The id.
    pub id: Uuid,
    pub title: String,
    #[undra(default)]
    pub done: bool,
    pub tags: Vec<String>,
    pub note: Option<String>,
}

fn sample_todo() -> Todo {
    Todo {
        id: Uuid([7; 16]),
        title: "write macros".into(),
        done: true,
        tags: vec!["a".into(), "bc".into()],
        note: Some("later".into()),
    }
}

#[test]
fn record_encodes_fields_in_declaration_order() {
    let todo = sample_todo();
    let mut expected = Writer::new();
    todo.id.encode(&mut expected);
    todo.title.encode(&mut expected);
    todo.done.encode(&mut expected);
    todo.tags.encode(&mut expected);
    todo.note.encode(&mut expected);
    assert_eq!(todo.encode_to_vec(), expected.into_vec());
}

#[test]
fn record_round_trips() {
    let todo = sample_todo();
    let bytes = todo.encode_to_vec();
    assert_eq!(Todo::decode_exact(&bytes).unwrap(), todo);
}

#[test]
fn record_decode_reports_truncation_and_trailing_bytes() {
    let bytes = sample_todo().encode_to_vec();
    // Cut inside the fixed-width `Uuid`.
    assert!(matches!(
        Todo::decode_exact(&bytes[..3]),
        Err(WireError::UnexpectedEof { needed: 16, at: 0 })
    ));
    // Cut inside the last string: its length prefix now promises more than is left.
    assert!(Todo::decode_exact(&bytes[..bytes.len() - 1]).is_err());
    let mut long = bytes;
    long.push(0);
    assert!(matches!(
        Todo::decode_exact(&long),
        Err(WireError::TrailingBytes { count: 1 })
    ));
}

#[test]
fn record_meta_is_registered_with_docs_defaults_and_types() {
    let def = record_def("Todo");
    assert_eq!(def.type_id, ids::type_id("Todo"));
    assert_eq!(Todo::UNDRA_TYPE_ID, ids::type_id("Todo"));
    assert_eq!(def.docs, "A todo item.");
    let names: Vec<&str> = def.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["id", "title", "done", "tags", "note"]);
    assert_eq!(def.fields[0].ty, TypeRef::Uuid);
    assert_eq!(def.fields[0].docs, "The id.");
    assert!(def.fields[2].default);
    assert!(!def.fields[1].default);
    assert_eq!(def.fields[3].ty, TypeRef::vec(TypeRef::String));
    assert_eq!(def.fields[4].ty, TypeRef::option(TypeRef::String));
}

#[test]
fn min_encoded_len_is_the_sum_of_the_fields() {
    // Uuid 16 + String 4 + bool 1 + Vec 4 + Option 1
    assert_eq!(<Todo as Decode>::MIN_ENCODED_LEN, 16 + 4 + 1 + 4 + 1);
}

/// Every wire type in one record.
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Everything {
    pub a: bool,
    pub b: i8,
    pub c: i16,
    pub d: i32,
    pub e: i64,
    pub f: u8,
    pub g: u16,
    pub h: u32,
    pub i: u64,
    pub j: f32,
    pub k: f64,
    pub l: String,
    pub m: Bytes,
    pub n: Duration,
    pub o: Timestamp,
    pub p: Uuid,
    pub q: Option<Vec<i32>>,
    pub r: Vec<Vec<u8>>,
    pub s: HashMap<String, i32>,
    pub t: BTreeMap<u32, Vec<String>>,
    pub u: std::collections::HashMap<Uuid, bool>,
}

#[test]
fn every_type_maps_and_round_trips() {
    let value = Everything {
        a: true,
        b: -1,
        c: -2,
        d: -3,
        e: -4,
        f: 5,
        g: 6,
        h: 7,
        i: 8,
        j: 1.5,
        k: -2.25,
        l: "héllo \u{1F30A}".into(),
        m: Bytes(vec![1, 2, 3]),
        n: Duration::new(3, 500),
        o: Timestamp(1_700_000_000_123),
        p: Uuid([9; 16]),
        q: Some(vec![]),
        r: vec![vec![1], vec![]],
        s: HashMap::from([("x".to_owned(), 1), ("y".to_owned(), 2)]),
        t: BTreeMap::from([(2, vec!["b".to_owned()]), (1, vec![])]),
        u: HashMap::from([(Uuid([1; 16]), true)]),
    };
    let bytes = value.encode_to_vec();
    assert_eq!(Everything::decode_exact(&bytes).unwrap(), value);

    let def = record_def("Everything");
    let types: Vec<TypeRef> = def.fields.iter().map(|f| f.ty.clone()).collect();
    assert_eq!(
        types,
        [
            TypeRef::Bool,
            TypeRef::I8,
            TypeRef::I16,
            TypeRef::I32,
            TypeRef::I64,
            TypeRef::U8,
            TypeRef::U16,
            TypeRef::U32,
            TypeRef::U64,
            TypeRef::F32,
            TypeRef::F64,
            TypeRef::String,
            TypeRef::Bytes,
            TypeRef::Duration,
            TypeRef::Timestamp,
            TypeRef::Uuid,
            TypeRef::option(TypeRef::vec(TypeRef::I32)),
            TypeRef::vec(TypeRef::vec(TypeRef::U8)),
            TypeRef::map(TypeRef::String, TypeRef::I32),
            TypeRef::map(TypeRef::U32, TypeRef::vec(TypeRef::String)),
            TypeRef::map(TypeRef::Uuid, TypeRef::Bool),
        ]
    );
}

/// A recursive record.
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub label: String,
    pub children: Vec<Node>,
    pub next: Option<Box<Node>>,
}

#[test]
fn recursive_records_round_trip_and_are_depth_limited() {
    let tree = Node {
        label: "root".into(),
        children: vec![Node {
            label: "leaf".into(),
            children: vec![],
            next: Some(Box::new(Node {
                label: "tail".into(),
                children: vec![],
                next: None,
            })),
        }],
        next: None,
    };
    let bytes = tree.encode_to_vec();
    assert_eq!(Node::decode_exact(&bytes).unwrap(), tree);

    // A hostile chain of `Some(Box<Node>)` must fail cleanly instead of overflowing the stack.
    let mut hostile = Vec::new();
    for _ in 0..10_000 {
        hostile.extend_from_slice(&[0, 0, 0, 0]); // label ""
        hostile.extend_from_slice(&[0, 0, 0, 0]); // no children
        hostile.push(1); // next: Some
    }
    assert!(matches!(
        Node::decode_exact(&hostile),
        Err(WireError::NestingTooDeep { .. })
    ));
}

/// Recursive types written with `Self`.
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Outline {
    pub title: String,
    pub sections: Vec<Self>,
    pub parent: Option<Box<Self>>,
}

/// A recursive enum written with `Self`.
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Num(i64),
    Add(Box<Self>, Box<Self>),
    Neg { inner: Box<Self> },
}

#[test]
fn self_in_fields_means_the_type_itself() {
    let outline = Outline {
        title: "a".into(),
        sections: vec![Outline {
            title: "b".into(),
            sections: vec![],
            parent: None,
        }],
        parent: Some(Box::new(Outline {
            title: "p".into(),
            sections: vec![],
            parent: None,
        })),
    };
    assert_eq!(
        Outline::decode_exact(&outline.encode_to_vec()).unwrap(),
        outline
    );
    let expr = Expr::Add(
        Box::new(Expr::Num(1)),
        Box::new(Expr::Neg {
            inner: Box::new(Expr::Num(2)),
        }),
    );
    assert_eq!(Expr::decode_exact(&expr.encode_to_vec()).unwrap(), expr);

    let def = record_def("Outline");
    assert_eq!(def.fields[1].ty, TypeRef::vec(TypeRef::named("Outline")));
    assert_eq!(def.fields[2].ty, TypeRef::option(TypeRef::named("Outline")));
    let def = enum_def("Expr");
    assert_eq!(def.variants[1].fields[0].ty, TypeRef::named("Expr"));
    assert_eq!(def.variants[2].fields[0].ty, TypeRef::named("Expr"));
}

/// A singly linked list: `Option<Box<Self>>` is the idiomatic spelling of a record that holds
/// itself, and every platform generator must take it (Swift boxes the field).
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct ListNode {
    pub value: i32,
    pub next: Option<Box<Self>>,
}

/// Two records that hold each other through boxed options.
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Parent {
    pub name: String,
    pub child: Option<Box<Child>>,
}

/// The other half of `Parent`.
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Child {
    pub name: String,
    pub parent: Option<Box<Parent>>,
}

/// A record and an enum that hold each other.
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Block {
    pub label: String,
    pub last: Option<Box<Stmt>>,
}

/// The enum half of `Block`.
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub enum Stmt {
    Nop,
    Nested(Block),
}

fn list(values: &[i32]) -> Option<Box<ListNode>> {
    values.iter().rev().fold(None, |next, &value| {
        Some(Box::new(ListNode { value, next }))
    })
}

#[test]
fn a_linked_record_is_an_option_of_itself_on_the_wire_and_in_the_schema() {
    let node = ListNode {
        value: 1,
        next: list(&[2, 3]),
    };
    let bytes = node.encode_to_vec();
    // value, then `Some` (1) and the next node, down to `None` (0).
    assert_eq!(
        bytes,
        [
            1, 0, 0, 0, 1, //
            2, 0, 0, 0, 1, //
            3, 0, 0, 0, 0
        ]
    );
    assert_eq!(ListNode::decode_exact(&bytes).unwrap(), node);
    let end = ListNode {
        value: 7,
        next: None,
    };
    assert_eq!(end.encode_to_vec(), [7, 0, 0, 0, 0]);

    let def = record_def("ListNode");
    assert_eq!(def.fields[0].ty, TypeRef::I32);
    assert_eq!(
        def.fields[1].ty,
        TypeRef::option(TypeRef::named("ListNode"))
    );
}

#[test]
fn mutually_recursive_records_round_trip_and_name_each_other() {
    let family = Parent {
        name: "p".into(),
        child: Some(Box::new(Child {
            name: "c".into(),
            parent: Some(Box::new(Parent {
                name: "pp".into(),
                child: None,
            })),
        })),
    };
    assert_eq!(
        Parent::decode_exact(&family.encode_to_vec()).unwrap(),
        family
    );
    assert_eq!(
        record_def("Parent").fields[1].ty,
        TypeRef::option(TypeRef::named("Child"))
    );
    assert_eq!(
        record_def("Child").fields[1].ty,
        TypeRef::option(TypeRef::named("Parent"))
    );
}

#[test]
fn a_record_and_an_enum_can_hold_each_other() {
    let block = Block {
        label: "outer".into(),
        last: Some(Box::new(Stmt::Nested(Block {
            label: "inner".into(),
            last: Some(Box::new(Stmt::Nop)),
        }))),
    };
    assert_eq!(Block::decode_exact(&block.encode_to_vec()).unwrap(), block);
    assert_eq!(
        record_def("Block").fields[1].ty,
        TypeRef::option(TypeRef::named("Stmt"))
    );
    assert_eq!(
        enum_def("Stmt").variants[1].fields[0].ty,
        TypeRef::named("Block")
    );
}

/// A second path to the facade, to exercise `crate = ".."`.
mod rooted {
    pub use undra::{meta, wire};
}

/// Uses the second path to the facade.
#[k::api(crate = "crate::rooted")]
#[derive(Clone, Debug, PartialEq)]
pub struct Rooted {
    pub x: i32,
}

/// Uses the item-level override.
#[k::api]
#[undra(crate = "crate::rooted")]
#[derive(Clone, Debug, PartialEq)]
pub struct RootedByAttribute {
    pub x: i32,
}

#[test]
fn crate_override_works_from_arguments_and_attributes() {
    assert_eq!(Rooted { x: 5 }.encode_to_vec(), 5_i32.encode_to_vec());
    assert_eq!(
        RootedByAttribute::decode_exact(&5_i32.encode_to_vec()).unwrap(),
        RootedByAttribute { x: 5 }
    );
    assert!(schema().records.iter().any(|r| r.name == "Rooted"));
    assert!(
        schema()
            .records
            .iter()
            .any(|r| r.name == "RootedByAttribute")
    );
}

/// Raw identifiers are unwrapped in the schema.
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Keywords {
    pub r#type: String,
    pub r#match: u8,
}

#[test]
fn raw_identifiers_lose_their_prefix_in_the_schema() {
    let def = record_def("Keywords");
    assert_eq!(def.fields[0].name, "type");
    assert_eq!(def.fields[1].name, "match");
    let value = Keywords {
        r#type: "t".into(),
        r#match: 1,
    };
    assert_eq!(
        Keywords::decode_exact(&value.encode_to_vec()).unwrap(),
        value
    );
}

// ---------------------------------------------------------------------------------------------
// Enums
// ---------------------------------------------------------------------------------------------

/// A shape.
#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// Nothing.
    Empty,
    Circle {
        radius: f64,
    },
    Rect(f64, f64),
    Label(String),
    Many {
        #[undra(default)]
        parts: Vec<Shape>,
        name: Option<String>,
    },
}

#[test]
fn enum_variants_are_a_u16_index_plus_fields() {
    assert_eq!(Shape::Empty.encode_to_vec(), [0, 0]);
    let mut expected = vec![1, 0];
    expected.extend(2.5_f64.to_le_bytes());
    assert_eq!(Shape::Circle { radius: 2.5 }.encode_to_vec(), expected);
    let mut expected = vec![2, 0];
    expected.extend(1.0_f64.to_le_bytes());
    expected.extend(2.0_f64.to_le_bytes());
    assert_eq!(Shape::Rect(1.0, 2.0).encode_to_vec(), expected);
}

#[test]
fn enums_round_trip() {
    let value = Shape::Many {
        parts: vec![
            Shape::Empty,
            Shape::Circle { radius: 1.0 },
            Shape::Rect(2.0, 3.0),
            Shape::Label("x".into()),
        ],
        name: Some("group".into()),
    };
    assert_eq!(Shape::decode_exact(&value.encode_to_vec()).unwrap(), value);
}

#[test]
fn unknown_variant_index_is_an_invalid_tag_with_position() {
    let bytes = [9, 0];
    assert_eq!(
        Shape::decode_exact(&bytes),
        Err(WireError::InvalidTag {
            tag: 9,
            at: 0,
            ty: "Shape"
        })
    );
    let mut r = undra::wire::Reader::new(&[1, 0, 0, 0, 5, 0]);
    r.read_u32().unwrap();
    assert_eq!(
        Shape::decode(&mut r),
        Err(WireError::InvalidTag {
            tag: 5,
            at: 4,
            ty: "Shape"
        })
    );
}

#[test]
fn enum_meta_lists_variants_with_shapes() {
    let def = enum_def("Shape");
    assert!(!def.is_error);
    assert_eq!(def.docs, "A shape.");
    let variants: Vec<(&str, u16, bool, usize)> = def
        .variants
        .iter()
        .map(|v| (v.name.as_str(), v.index, v.tuple, v.fields.len()))
        .collect();
    assert_eq!(
        variants,
        [
            ("Empty", 0, false, 0),
            ("Circle", 1, false, 1),
            ("Rect", 2, true, 2),
            ("Label", 3, true, 1),
            ("Many", 4, false, 2)
        ]
    );
    assert_eq!(def.variants[0].docs, "Nothing.");
    assert_eq!(def.variants[2].fields[0].name, "0");
    assert_eq!(def.variants[2].fields[1].name, "1");
    assert_eq!(def.variants[4].fields[0].name, "parts");
    assert!(def.variants[4].fields[0].default);
    assert_eq!(def.variants[4].message, None);
    assert_eq!(Shape::UNDRA_TYPE_ID, ids::type_id("Shape"));
}

/// A unit-only enum with a keyword-ish variant.
#[k::api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    All,
    Active,
    Done,
}

#[test]
fn unit_enums_are_two_bytes() {
    for (i, filter) in [Filter::All, Filter::Active, Filter::Done]
        .into_iter()
        .enumerate()
    {
        let bytes = filter.encode_to_vec();
        assert_eq!(bytes, [i as u8, 0]);
        assert_eq!(Filter::decode_exact(&bytes).unwrap(), filter);
    }
    assert_eq!(<Filter as Decode>::MIN_ENCODED_LEN, 2);
}

// ---------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------

/// Transport failures.
#[k::error]
#[derive(Clone, PartialEq)]
pub enum NetError {
    #[error("network error: {0}")]
    Network(String),
    #[error("request timed out")]
    Timeout,
}

/// Failures of the todo core.
#[k::error]
#[derive(Clone, PartialEq)]
pub enum TodoError {
    /// The title was blank.
    #[error("title cannot be empty")]
    EmptyTitle,
    #[error("todo {0} not found")]
    NotFound(Uuid),
    #[error("code {code}: {reason}")]
    Rejected { code: u16, reason: String },
    #[error("bad value {0:?} ({1:>4})")]
    Bad(String, u8),
    #[error(transparent)]
    Http(#[from] NetError),
    #[error("storage failed")]
    Storage(#[source] NetError),
    #[error("{{literal}} {0}")]
    Braces(u8),
    #[error("only the second: {1}")]
    Second(String, u8),
}

#[test]
fn error_display_follows_the_messages() {
    assert_eq!(TodoError::EmptyTitle.to_string(), "title cannot be empty");
    assert_eq!(
        TodoError::NotFound(Uuid([0; 16])).to_string(),
        "todo 00000000-0000-0000-0000-000000000000 not found"
    );
    assert_eq!(
        TodoError::Rejected {
            code: 409,
            reason: "conflict".into()
        }
        .to_string(),
        "code 409: conflict"
    );
    assert_eq!(
        TodoError::Bad("x".into(), 7).to_string(),
        "bad value \"x\" (   7)"
    );
    assert_eq!(
        TodoError::Http(NetError::Timeout).to_string(),
        "request timed out"
    );
    assert_eq!(
        TodoError::Storage(NetError::Timeout).to_string(),
        "storage failed"
    );
    assert_eq!(TodoError::Braces(3).to_string(), "{literal} 3");
    assert_eq!(
        TodoError::Second("a".into(), 9).to_string(),
        "only the second: 9"
    );
}

#[test]
fn error_source_and_from() {
    use std::error::Error;

    assert!(TodoError::EmptyTitle.source().is_none());
    let storage = TodoError::Storage(NetError::Network("down".into()));
    assert_eq!(storage.source().unwrap().to_string(), "network error: down");
    // Transparent forwards `source()` to the inner error, which has none.
    assert!(TodoError::Http(NetError::Timeout).source().is_none());

    let converted: TodoError = NetError::Timeout.into();
    assert_eq!(converted, TodoError::Http(NetError::Timeout));

    fn fails() -> Result<(), TodoError> {
        Err(NetError::Network("boom".into()))?;
        Ok(())
    }
    assert_eq!(
        fails().unwrap_err(),
        TodoError::Http(NetError::Network("boom".into()))
    );

    let boxed: Box<dyn Error> = Box::new(TodoError::EmptyTitle);
    assert_eq!(boxed.to_string(), "title cannot be empty");
    // `Debug` was derived by the macro.
    assert_eq!(format!("{:?}", TodoError::EmptyTitle), "EmptyTitle");
}

#[test]
fn errors_encode_like_enums() {
    for value in [
        TodoError::EmptyTitle,
        TodoError::NotFound(Uuid([3; 16])),
        TodoError::Rejected {
            code: 500,
            reason: "no".into(),
        },
        TodoError::Http(NetError::Network("x".into())),
        TodoError::Storage(NetError::Timeout),
    ] {
        assert_eq!(
            TodoError::decode_exact(&value.encode_to_vec()).unwrap(),
            value
        );
    }
    // Variant index then the inner error (index 0 = Network).
    let mut expected = vec![4, 0, 0, 0];
    expected.extend(1_u32.to_le_bytes());
    expected.push(b'x');
    assert_eq!(
        TodoError::Http(NetError::Network("x".into())).encode_to_vec(),
        expected
    );
}

#[test]
fn error_meta_carries_is_error_and_messages() {
    let def = enum_def("TodoError");
    assert!(def.is_error);
    assert_eq!(def.docs, "Failures of the todo core.");
    assert_eq!(
        def.variants[0].message.as_deref(),
        Some("title cannot be empty")
    );
    assert_eq!(def.variants[0].docs, "The title was blank.");
    assert_eq!(
        def.variants[1].message.as_deref(),
        Some("todo {0} not found")
    );
    assert_eq!(
        def.variants[2].message.as_deref(),
        Some("code {code}: {reason}")
    );
    assert!(!def.variants[2].tuple);
    // Transparent variants have no message of their own.
    assert_eq!(def.variants[4].message, None);
    assert_eq!(def.variants[4].fields[0].ty, TypeRef::named("NetError"));
}

#[test]
fn every_registration_is_of_the_expected_kind() {
    let records = undra::meta::registrations()
        .filter(|r| matches!(r, Registration::Record(_)))
        .count();
    let enums = undra::meta::registrations()
        .filter(|r| matches!(r, Registration::Enum(_)))
        .count();
    assert!(records >= 9, "{records}");
    assert!(enums >= 4, "{enums}");
}

#[test]
fn the_registered_schema_validates() {
    // Everything registered by this test binary refers only to types it also registers.
    let schema = schema();
    if let Err(errors) = schema.validate() {
        let unrelated: Vec<_> = errors
            .iter()
            .filter(|e| !e.to_string().contains("`Ctx`"))
            .collect();
        assert!(unrelated.is_empty(), "{unrelated:?}");
    }
}
