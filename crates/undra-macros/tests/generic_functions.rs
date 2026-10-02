//! Generic functions and methods (ADR-058): `#[undra::api(generic(T = [Todo, Note]))]` lists the
//! types a function with a type parameter crosses the boundary for. Each listed type is one
//! function of the schema (`newest<Todo>`), registered, dispatched and labelled like a
//! hand-written one, against the real runtime.
#![forbid(unsafe_code)]
#![allow(clippy::new_without_default)]

use std::sync::Arc;

use undra::meta::{FunctionDef, GenericArg, GenericOf, Schema, TypeRef, collect_schema, ids};
use undra::runtime::{Ctx, Stream};
use undra::wire::{Decode, Encode, Handle, Writer};
use undra_macros as k;

mod support;
use support::Runtime;
use support::testing::stream_of;

fn schema() -> Schema {
    collect_schema("generic-functions")
}

fn named(name: &str) -> TypeRef {
    TypeRef::Named(name.to_owned())
}

fn args(encode: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    encode(&mut w);
    w.into_vec()
}

fn function(schema: &Schema, name: &str) -> FunctionDef {
    schema
        .functions
        .iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no function `{name}`"))
        .clone()
}

fn label(of: &str, ty: &str, inferred: bool) -> GenericOf {
    GenericOf {
        of: of.to_owned(),
        args: vec![GenericArg {
            param: "T".to_owned(),
            ty: named(ty),
            inferred,
        }],
    }
}

#[k::api]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Todo {
    pub id: u32,
    pub title: String,
}

#[k::api]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Note {
    pub id: u32,
    pub body: String,
}

/// Declared with `#[undra::api]` and never listed: no function is generated for it.
#[k::api]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Draft {
    pub id: u32,
}

/// A newtype is a named value type: it may be listed.
#[k::api]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tag(pub String);

/// What the generic code below needs of a row: plain Rust, invisible to the schema.
pub trait Row: Clone + Default {
    fn id(&self) -> u32;
    fn with_id(id: u32) -> Self;
}

impl Row for Todo {
    fn id(&self) -> u32 {
        self.id
    }
    fn with_id(id: u32) -> Self {
        Todo {
            id,
            title: format!("todo {id}"),
        }
    }
}

impl Row for Note {
    fn id(&self) -> u32 {
        self.id
    }
    fn with_id(id: u32) -> Self {
        Note {
            id,
            body: format!("note {id}"),
        }
    }
}

impl Row for Tag {
    fn id(&self) -> u32 {
        self.0.len() as u32
    }
    fn with_id(id: u32) -> Self {
        Tag("x".repeat(id as usize))
    }
}

// ---------------------------------------------------------------------------------------------
// Functions
// ---------------------------------------------------------------------------------------------

/// The row with the highest id.
#[k::api(generic(T = [Todo, Note, Tag]))]
pub fn newest<T: Row>(rows: Vec<T>) -> Option<T> {
    rows.into_iter().max_by_key(Row::id)
}

/// A row with a fresh id: `T` stands in the return type only.
#[k::api(generic(T = [Todo, Note]))]
pub fn draft<T: Row>(ctx: &Ctx) -> T {
    let _ = ctx;
    T::with_id(7)
}

#[k::error]
#[derive(PartialEq)]
pub enum LoadError {
    #[error("no row {0}")]
    Missing(u32),
}

/// Loads a row: `async`, with an error type, and the bound in a `where` clause.
#[k::api(generic(T = [Todo, Note]))]
pub async fn load<T>(ctx: &Ctx, id: u32) -> Result<T, LoadError>
where
    T: Row,
{
    let _ = ctx;
    if id == 0 {
        Err(LoadError::Missing(id))
    } else {
        Ok(T::with_id(id))
    }
}

/// A stream of rows.
#[k::api(generic(T = [Todo, Note]))]
pub fn rows<T: Row + Send + Unpin + 'static>(count: u32) -> impl Stream<Item = T> {
    stream_of((1..=count).map(T::with_id).collect())
}

/// `T` in a map value and in an `Option` parameter.
#[k::api(generic(T = [Todo, Note]))]
pub fn index<T: Row>(rows: Vec<T>, pinned: Option<T>) -> std::collections::HashMap<u32, T> {
    let mut map: std::collections::HashMap<u32, T> =
        rows.into_iter().map(|r| (r.id(), r)).collect();
    if let Some(row) = pinned {
        map.insert(0, row);
    }
    map
}

/// A plain object, passed to a generic function next to the type parameter.
pub struct Mailbox {
    pub label: String,
}

#[k::api]
impl Mailbox {
    pub fn new() -> Self {
        Mailbox {
            label: "inbox".to_owned(),
        }
    }
}

/// An object parameter beside `T`.
#[k::api(generic(T = [Todo, Note]))]
pub fn archive<T: Row>(into: Arc<Mailbox>, rows: Vec<T>) -> String {
    format!("{}: {}", into.label, rows.len())
}

#[test]
fn a_generic_function_registers_one_function_per_listed_type() {
    let schema = schema();
    let names: Vec<&str> = schema.functions.iter().map(|f| f.name.as_str()).collect();
    for name in [
        "newest<Todo>",
        "newest<Note>",
        "newest<Tag>",
        "draft<Todo>",
        "draft<Note>",
        "load<Todo>",
        "load<Note>",
        "rows<Todo>",
        "rows<Note>",
        "index<Todo>",
        "archive<Note>",
    ] {
        assert!(names.contains(&name), "{names:?}");
    }
    assert!(
        !names.iter().any(|n| n.contains("Draft")),
        "a type that is not listed has no function: {names:?}"
    );
    // The generic function itself has no identity.
    assert!(!names.contains(&"newest"), "{names:?}");

    let todo = function(&schema, "newest<Todo>");
    assert_eq!(todo.method_id, ids::function_id("newest<Todo>"));
    assert_eq!(todo.params[0].ty, TypeRef::vec(named("Todo")));
    assert_eq!(todo.returns, TypeRef::option(named("Todo")));
    assert_eq!(todo.docs, "The row with the highest id.");
    // The newtype is listed under its own name, with its own signature.
    assert_eq!(
        function(&schema, "newest<Tag>").params[0].ty,
        TypeRef::vec(named("Tag"))
    );
    assert_eq!(
        function(&schema, "index<Note>").returns,
        TypeRef::map(TypeRef::U32, named("Note"))
    );
    assert_eq!(
        function(&schema, "archive<Todo>").params[0].ty,
        TypeRef::Object("Mailbox".to_owned())
    );
    schema.validate().unwrap();
}

#[test]
fn the_label_says_which_function_an_instantiation_is_and_whether_the_arguments_fix_the_type() {
    let schema = schema();
    // `T` stands in the type of a parameter: the arguments fix it.
    assert_eq!(
        function(&schema, "newest<Todo>").generic,
        Some(label("newest", "Todo", true))
    );
    assert_eq!(
        function(&schema, "archive<Note>").generic,
        Some(label("archive", "Note", true))
    );
    // `T` stands in the return type only, and `ctx` is not a parameter of the schema.
    assert_eq!(
        function(&schema, "draft<Note>").generic,
        Some(label("draft", "Note", false))
    );
    assert_eq!(
        function(&schema, "load<Todo>").generic,
        Some(label("load", "Todo", false))
    );
    assert_eq!(
        function(&schema, "rows<Note>").generic,
        Some(label("rows", "Note", false))
    );
    // A function that is not generic has no label.
    assert!(
        schema
            .functions
            .iter()
            .filter(|f| !f.name.contains('<'))
            .all(|f| f.generic.is_none())
    );
}

#[test]
fn each_instantiation_dispatches_to_the_generic_function() {
    let rt = Runtime::new();
    let reply = rt
        .call_function(
            "newest<Todo>",
            &args(|w| vec![Todo::with_id(2), Todo::with_id(7), Todo::with_id(3)].encode(w)),
        )
        .sync_ok();
    assert_eq!(
        Option::<Todo>::decode_exact(&reply).unwrap(),
        Some(Todo::with_id(7))
    );
    let reply = rt
        .call_function("newest<Note>", &args(|w| vec![Note::with_id(5)].encode(w)))
        .sync_ok();
    assert_eq!(
        Option::<Note>::decode_exact(&reply).unwrap(),
        Some(Note::with_id(5))
    );
    let reply = rt
        .call_function("newest<Tag>", &args(|w| Vec::<Tag>::new().encode(w)))
        .sync_ok();
    assert_eq!(Option::<Tag>::decode_exact(&reply).unwrap(), None);

    // `T` only in the return type: the context is injected.
    let reply = rt.call_function("draft<Note>", &[]).sync_ok();
    assert_eq!(Note::decode_exact(&reply).unwrap(), Note::with_id(7));
}

#[test]
fn an_async_instantiation_keeps_its_typed_error() {
    let rt = Runtime::new();
    let reply = rt
        .call_function("load<Note>", &args(|w| 9_u32.encode(w)))
        .run_async()
        .unwrap();
    assert_eq!(Note::decode_exact(&reply).unwrap(), Note::with_id(9));
    let error = rt
        .call_function("load<Todo>", &args(|w| 0_u32.encode(w)))
        .run_async()
        .unwrap_err();
    assert_eq!(
        LoadError::decode_exact(&error).unwrap(),
        LoadError::Missing(0)
    );
}

#[test]
fn a_stream_instantiation_streams_the_listed_type() {
    let rt = Runtime::new();
    let items = rt
        .call_function("rows<Todo>", &args(|w| 3_u32.encode(w)))
        .run_stream();
    let decoded: Vec<Todo> = items
        .into_iter()
        .map(|item| Todo::decode_exact(&item.unwrap()).unwrap())
        .collect();
    assert_eq!(decoded, (1..=3).map(Todo::with_id).collect::<Vec<_>>());
}

#[test]
fn an_object_parameter_stands_beside_the_type_parameter() {
    let rt = Runtime::new();
    let reply = rt.call_object("Mailbox", "new", 0, &[]).sync_ok();
    let handle = Handle::decode_exact(&reply).unwrap();
    let reply = rt
        .call_function(
            "archive<Todo>",
            &args(|w| {
                handle.encode(w);
                vec![Todo::with_id(1), Todo::with_id(2)].encode(w);
            }),
        )
        .sync_ok();
    assert_eq!(String::decode_exact(&reply).unwrap(), "inbox: 2");
}

#[test]
fn a_map_and_an_option_of_the_listed_type_cross_like_hand_written_ones() {
    let rt = Runtime::new();
    let reply = rt
        .call_function(
            "index<Todo>",
            &args(|w| {
                vec![Todo::with_id(4)].encode(w);
                Some(Todo::with_id(9)).encode(w);
            }),
        )
        .sync_ok();
    let map = std::collections::HashMap::<u32, Todo>::decode_exact(&reply).unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(map[&0], Todo::with_id(9));
    assert_eq!(map[&4], Todo::with_id(4));
}

#[test]
fn an_id_that_names_no_instantiation_is_unknown() {
    let rt = Runtime::new();
    // `Draft` was never listed: the call carries an id the dispatcher does not have.
    let outcome = rt.call_function_raw(
        "newest<Todo>",
        ids::function_id("newest<Draft>"),
        &args(|w| Vec::<Todo>::new().encode(w)),
    );
    assert!(outcome.is_unknown());
    // Another instantiation's id is not this one's either.
    let outcome = rt.call_function_raw(
        "newest<Todo>",
        ids::function_id("newest<Note>"),
        &args(|w| Vec::<Todo>::new().encode(w)),
    );
    assert!(outcome.is_unknown());
}

#[test]
fn bad_arguments_name_the_instantiation() {
    let rt = Runtime::new();
    let reason = rt.call_function("newest<Note>", &[1, 2]).bad_request();
    assert!(reason.contains("newest<Note>"), "{reason}");
}

// ---------------------------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------------------------

/// A shelf that keeps rows of several types.
pub struct Library {
    pinned: u32,
}

#[k::api]
impl Library {
    pub fn new() -> Self {
        Library { pinned: 3 }
    }

    /// The pinned rows of a type.
    #[undra(generic(T = [Todo, Note]))]
    pub fn pinned<T: Row>(&self) -> Vec<T> {
        (1..=self.pinned).map(T::with_id).collect()
    }

    /// Takes one row and answers its id: `T` is fixed by the argument.
    #[undra(generic(T = [Todo, Note]))]
    pub async fn remember<T: Row>(&self, row: T) -> u32 {
        row.id() + self.pinned
    }

    /// An ordinary method beside the generic ones.
    pub fn count(&self) -> u32 {
        self.pinned
    }
}

#[test]
fn a_generic_method_is_one_method_per_listed_type() {
    let schema = schema();
    let library = schema.objects.iter().find(|o| o.name == "Library").unwrap();
    let names: Vec<&str> = library.methods.iter().map(|m| m.name.as_str()).collect();
    for name in [
        "pinned<Todo>",
        "pinned<Note>",
        "remember<Todo>",
        "remember<Note>",
        "count",
    ] {
        assert!(names.contains(&name), "{names:?}");
    }
    let pinned = library
        .methods
        .iter()
        .find(|m| m.name == "pinned<Note>")
        .unwrap();
    assert_eq!(pinned.method_id, ids::method_id("Library", "pinned<Note>"));
    assert_eq!(pinned.returns, TypeRef::vec(named("Note")));
    assert_eq!(pinned.docs, "The pinned rows of a type.");
    assert_eq!(pinned.generic, Some(label("pinned", "Note", false)));
    let remember = library
        .methods
        .iter()
        .find(|m| m.name == "remember<Todo>")
        .unwrap();
    assert!(remember.is_async);
    assert_eq!(remember.params[0].ty, named("Todo"));
    assert_eq!(remember.generic, Some(label("remember", "Todo", true)));
    assert!(
        library
            .methods
            .iter()
            .find(|m| m.name == "count")
            .unwrap()
            .generic
            .is_none()
    );
    schema.validate().unwrap();
}

#[test]
fn a_generic_method_dispatches_to_the_generic_body() {
    let rt = Runtime::new();
    let reply = rt.call_object("Library", "new", 0, &[]).sync_ok();
    let handle = Handle::decode_exact(&reply).unwrap().0;
    let reply = rt
        .call_object("Library", "pinned<Todo>", handle, &[])
        .sync_ok();
    assert_eq!(
        Vec::<Todo>::decode_exact(&reply).unwrap(),
        (1..=3).map(Todo::with_id).collect::<Vec<_>>()
    );
    let reply = rt
        .call_object(
            "Library",
            "remember<Note>",
            handle,
            &args(|w| Note::with_id(10).encode(w)),
        )
        .run_async()
        .unwrap();
    assert_eq!(u32::decode_exact(&reply).unwrap(), 13);
    let reply = rt.call_object("Library", "count", handle, &[]).sync_ok();
    assert_eq!(u32::decode_exact(&reply).unwrap(), 3);
    // An id the object does not have.
    assert!(
        rt.call_object_raw(
            "Library",
            ids::method_id("Library", "pinned<Draft>"),
            handle,
            &[]
        )
        .is_unknown()
    );
    // A bad request names the instantiation.
    let reason = rt
        .call_object("Library", "remember<Todo>", handle, &[1])
        .bad_request();
    assert!(reason.contains("Library.remember<Todo>"), "{reason}");
}
