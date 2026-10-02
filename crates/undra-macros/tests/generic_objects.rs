//! Generic objects and stores (ADR-058): `#[undra::api(generic)] impl<T> Cache<T> { .. }` (and
//! `#[undra::store(generic)]` with `#[undra::api(store, generic)]`) is a template that registers
//! nothing, and `#[undra::api] pub type TodoCache = Cache<Todo>;` instantiates it: the ordinary
//! object (and store) expansion on the concrete tokens, under the alias's name. Run against the
//! real runtime.
#![forbid(unsafe_code)]
#![allow(clippy::new_without_default)]

use std::any::Any;
use std::sync::{Arc, Mutex};

use undra::meta::{Schema, TypeRef, collect_schema, ids};
use undra::prelude::{Computed, Ctx, Signal, SignalValue};
use undra::runtime::{StoreObject, StoreRestorer};
use undra::signals::StoreCell;
use undra::wire::payload::ChangeOp;
use undra::wire::{Decode, Encode, Handle, KeyedPatch, PatchOp, Reader, Writer};
use undra_macros as k;

mod support;
use support::Runtime;

fn schema() -> Schema {
    collect_schema("generic-objects")
}

fn named(name: &str) -> TypeRef {
    TypeRef::Named(name.to_owned())
}

fn args(encode: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    encode(&mut w);
    w.into_vec()
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

/// A row type without the field a keyed list asks for.
#[k::api]
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tag {
    pub name: String,
}

/// What the generic code below needs of a row: plain Rust, invisible to the schema.
pub trait Row: SignalValue + Default {
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

fn construct(rt: &Runtime, object: &str, args: &[u8]) -> u64 {
    let reply = rt.call_object(object, "new", 0, args).sync_ok();
    Handle::decode_exact(&reply).unwrap().0
}

// ---------------------------------------------------------------------------------------------
// A generic object, instantiated through an alias, in its module and in another one
// ---------------------------------------------------------------------------------------------

pub mod model {
    use super::*;

    pub struct Cache<T> {
        pub(crate) items: Mutex<Vec<T>>,
    }

    /// Keeps rows by id.
    #[k::api(generic)]
    impl<T: Row> Cache<T> {
        pub fn new() -> Self {
            Cache {
                items: Mutex::new(Vec::new()),
            }
        }

        /// Stores a row, replacing the one with its id.
        pub fn put(&self, item: T) {
            let mut items = self.items.lock().unwrap();
            items.retain(|i| i.id() != item.id());
            items.push(item);
        }

        pub fn get(&self, id: u32) -> Option<T> {
            self.items
                .lock()
                .unwrap()
                .iter()
                .find(|i| i.id() == id)
                .cloned()
        }

        pub async fn all(&self) -> Vec<T> {
            self.items.lock().unwrap().clone()
        }
    }

    /// The todos the app has seen.
    #[k::api]
    pub type TodoCache = Cache<Todo>;
}

// An alias in another module than its template: `use` brings the type and the template.
pub mod notes {
    use super::Note;
    use super::model::Cache;
    use undra_macros as k;

    #[k::api]
    pub type NoteCache = Cache<Note>;
}

use model::{Cache, TodoCache};
use notes::NoteCache;

#[test]
fn a_generic_object_is_one_described_object_per_alias() {
    let schema = schema();
    let todo = schema
        .objects
        .iter()
        .find(|o| o.name == "TodoCache")
        .unwrap();
    assert_eq!(todo.type_id, ids::type_id("TodoCache"));
    assert_eq!(todo.docs, "The todos the app has seen.");
    let put = todo.methods.iter().find(|m| m.name == "put").unwrap();
    assert_eq!(put.method_id, ids::method_id("TodoCache", "put"));
    assert_eq!(put.params[0].ty, named("Todo"));
    assert_eq!(put.docs, "Stores a row, replacing the one with its id.");
    assert!(
        put.generic.is_none(),
        "an instantiation of an object has no label"
    );
    let get = todo.methods.iter().find(|m| m.name == "get").unwrap();
    assert_eq!(get.returns, TypeRef::option(named("Todo")));
    assert_eq!(todo.constructors[0].returns, named("TodoCache"));

    let note = schema
        .objects
        .iter()
        .find(|o| o.name == "NoteCache")
        .unwrap();
    assert_eq!(note.docs, "Keeps rows by id.", "the template's docs");
    let all = note.methods.iter().find(|m| m.name == "all").unwrap();
    assert!(all.is_async);
    assert_eq!(all.returns, TypeRef::vec(named("Note")));
    assert!(
        schema.objects.iter().all(|o| o.name != "Cache"),
        "the template has no identity"
    );
    schema.validate().unwrap();
}

#[test]
fn the_instantiations_are_distinct_objects_with_their_own_handles() {
    let rt = Runtime::new();
    let todos = construct(&rt, "TodoCache", &[]);
    let notes = construct(&rt, "NoteCache", &[]);

    rt.call_object(
        "TodoCache",
        "put",
        todos,
        &args(|w| Todo::with_id(4).encode(w)),
    )
    .sync_ok();
    let reply = rt
        .call_object("TodoCache", "get", todos, &args(|w| 4_u32.encode(w)))
        .sync_ok();
    assert_eq!(
        Option::<Todo>::decode_exact(&reply).unwrap(),
        Some(Todo::with_id(4))
    );

    // The Rust type behind each handle is the instantiation.
    let cache: Arc<Cache<Todo>> = rt.object::<TodoCache>(todos).unwrap();
    assert_eq!(cache.get(4), Some(Todo::with_id(4)));
    assert!(
        rt.object::<NoteCache>(todos).is_err(),
        "a handle of the other instantiation"
    );
    let reason = rt
        .call_object("NoteCache", "get", todos, &args(|w| 4_u32.encode(w)))
        .bad_request();
    assert!(reason.contains("NoteCache.get"), "{reason}");

    rt.call_object(
        "NoteCache",
        "put",
        notes,
        &args(|w| Note::with_id(1).encode(w)),
    )
    .sync_ok();
    let reply = rt
        .call_object("NoteCache", "all", notes, &[])
        .run_async()
        .unwrap();
    assert_eq!(
        Vec::<Note>::decode_exact(&reply).unwrap(),
        vec![Note::with_id(1)]
    );
}

// ---------------------------------------------------------------------------------------------
// A generic store, instantiated through an alias
// ---------------------------------------------------------------------------------------------

/// The rows the user has ticked.
#[k::store(generic, restore = "Self::assemble")]
pub struct Selection<T> {
    /// The ticked rows, keyed by a field every instantiation's row has.
    #[undra(key = "id")]
    rows: Signal<Vec<T>>,
    limit: Signal<u32>,
    count: Computed<u32>,
    label: String,
}

/// Ticking rows.
#[k::api(store, generic)]
impl<T: Row> Selection<T> {
    pub fn new(limit: u32) -> Self {
        Self::assemble(Ctx::current(), Signal::new(Vec::new()), Signal::new(limit))
    }

    fn assemble(_ctx: Ctx, rows: Signal<Vec<T>>, limit: Signal<u32>) -> Self {
        let count = Computed::new(&rows, |rows: &Vec<T>| rows.len() as u32);
        // No `__undra_cell` in the literal: the macro adds it inside this impl block.
        Selection {
            rows,
            limit,
            count,
            label: "selection".to_owned(),
        }
    }

    /// Ticks a row, or unticks it.
    pub fn toggle(&self, row: T) {
        let position = self.rows.get().iter().position(|r| r.id() == row.id());
        match position {
            Some(index) => {
                self.rows.remove(index);
            }
            None => self.rows.push(row),
        }
    }

    pub fn first(&self) -> Option<T> {
        self.rows.get().first().cloned()
    }
}

/// The ticked todos.
#[k::api]
pub type TodoSelection = Selection<Todo>;

#[k::api]
pub type NoteSelection = Selection<Note>;

#[test]
fn a_generic_store_is_one_described_store_per_alias() {
    let schema = schema();
    let store = schema
        .objects
        .iter()
        .find(|o| o.name == "TodoSelection")
        .unwrap();
    let signals = &store.store.as_ref().unwrap().signals;
    assert_eq!(signals.len(), 3);
    assert_eq!(signals[0].name, "rows");
    assert_eq!(signals[0].ty, TypeRef::vec(named("Todo")));
    assert_eq!(signals[0].key.as_deref(), Some("id"));
    assert!(signals[2].computed);
    assert_eq!(store.docs, "The ticked todos.\n\nTicking rows.");
    let toggle = store.methods.iter().find(|m| m.name == "toggle").unwrap();
    assert_eq!(toggle.params[0].ty, named("Todo"));
    assert_eq!(toggle.method_id, ids::method_id("TodoSelection", "toggle"));
    assert_eq!(toggle.docs, "Ticks a row, or unticks it.");

    let notes = schema
        .objects
        .iter()
        .find(|o| o.name == "NoteSelection")
        .unwrap();
    assert_eq!(
        notes.store.as_ref().unwrap().signals[0].ty,
        TypeRef::vec(named("Note"))
    );
    assert_eq!(
        notes.docs, "The rows the user has ticked.\n\nTicking rows.",
        "the struct's docs when the alias has none, then the block's"
    );

    // The persisted identity of each instantiation is its own (ADR-037).
    let a = schema.store_fingerprint(ids::type_id("TodoSelection"));
    let b = schema.store_fingerprint(ids::type_id("NoteSelection"));
    assert!(a.is_some() && b.is_some());
    assert_ne!(a, b);
    schema.validate().unwrap();
}

#[test]
fn a_generic_store_delivers_keyed_patches_and_restores_through_its_restorer() {
    let rt = Runtime::new();
    let handle = construct(&rt, "TodoSelection", &args(|w| 3_u32.encode(w)));
    let store: Arc<Selection<Todo>> = rt.object::<TodoSelection>(handle).unwrap();
    assert_eq!(
        StoreCell::type_id(store.cell()),
        ids::type_id("TodoSelection")
    );
    assert_eq!(store.cell().handle(), handle);
    assert_eq!(store.cell().signal_count(), 3);

    rt.real().observe(handle, 0, true);
    rt.change_sets();
    for id in [1_u32, 2] {
        rt.call_object(
            "TodoSelection",
            "toggle",
            handle,
            &args(|w| Todo::with_id(id).encode(w)),
        )
        .sync_ok();
    }
    let sets = rt.change_sets();
    let entry = &sets.last().unwrap().entries[0];
    assert_eq!(entry.op, ChangeOp::KeyedPatch);
    let patch = KeyedPatch::<Todo>::decode(&mut Reader::new(&entry.value)).unwrap();
    assert_eq!(
        patch.ops,
        vec![PatchOp::Insert {
            index: 1,
            item: Todo::with_id(2)
        }]
    );
    assert_eq!(store.count.get(), 2);

    // Restore: the registered restorer of the instantiation rebuilds `Selection<Todo>`.
    let mut record = Writer::new();
    store.cell().encode_snapshot(&mut record);
    let mut r = Reader::new(record.as_slice());
    r.read_u64().unwrap();
    r.read_u32().unwrap();
    let body = r.read_rest().to_vec();
    let restorer = undra::meta::inventory::iter::<StoreRestorer>
        .into_iter()
        .find(|r| r.type_id == ids::type_id("TodoSelection"))
        .expect("a restorer per instantiation");
    let any: Arc<dyn Any + Send + Sync> =
        (restorer.restore)(rt.ctx(), 0x0000_0002_0000_0007, &mut Reader::new(&body)).unwrap();
    let restored = any
        .downcast::<Selection<Todo>>()
        .expect("the instantiation");
    assert_eq!(restored.rows.get(), store.rows.get());
    assert_eq!(restored.limit.get(), 3);
    assert_eq!(restored.count.get(), 2, "the hook rebuilt the computed");
    assert_eq!(restored.label, "selection");

    // The other instantiation is another store type.
    let notes = construct(&rt, "NoteSelection", &args(|w| 1_u32.encode(w)));
    rt.call_object(
        "NoteSelection",
        "toggle",
        notes,
        &args(|w| Note::with_id(8).encode(w)),
    )
    .sync_ok();
    let reply = rt
        .call_object("NoteSelection", "first", notes, &[])
        .sync_ok();
    assert_eq!(
        Option::<Note>::decode_exact(&reply).unwrap(),
        Some(Note::with_id(8))
    );
}

#[test]
fn toggling_one_selection_delivers_nothing_to_the_other() {
    let rt = Runtime::new();
    let todos = construct(&rt, "TodoSelection", &args(|w| 3_u32.encode(w)));
    let notes = construct(&rt, "NoteSelection", &args(|w| 3_u32.encode(w)));
    rt.real().observe(todos, 0, true);
    rt.real().observe(notes, 0, true);
    rt.change_sets();
    rt.call_object(
        "TodoSelection",
        "toggle",
        todos,
        &args(|w| Todo::with_id(1).encode(w)),
    )
    .sync_ok();
    let sets = rt.change_sets();
    let touched: Vec<u64> = sets
        .iter()
        .flat_map(|set| set.entries.iter().map(|e| e.handle.0))
        .collect();
    assert!(touched.contains(&todos), "{touched:?}");
    assert!(!touched.contains(&notes), "{touched:?}");
}

// ---------------------------------------------------------------------------------------------
// A store with every kind of signal, a template whose struct and block name the parameter
// differently, and the persistence of each instantiation
// ---------------------------------------------------------------------------------------------

/// Rows with a derived view, a lazy page and a count.
#[k::store(generic, restore = "Self::assemble")]
pub struct Board<T> {
    #[undra(key = "id")]
    rows: Signal<Vec<T>>,
    #[undra(key = "id")]
    even: undra::prelude::DerivedList<T>,
    #[undra(key = "id")]
    page: undra::prelude::Lazy<T>,
    open: Computed<u32>,
}

/// The block names the parameter `R` where the struct says `T`: the placeholders are positional.
#[k::api(store, generic)]
impl<R: Row> Board<R> {
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(Vec::new()), undra::prelude::Lazy::new())
    }

    fn assemble(_ctx: Ctx, rows: Signal<Vec<R>>, page: undra::prelude::Lazy<R>) -> Self {
        let even = rows.derive().filter(|r: &R| r.id() % 2 == 0).build();
        let open = rows.derive().filter(|r: &R| r.id() > 0).count();
        Board {
            rows,
            even,
            page,
            open,
        }
    }

    /// Adds a row.
    pub fn add(&self, row: R) {
        self.page.push(row.clone());
        self.rows.push(row);
    }
}

#[k::api]
pub type TodoBoard = Board<Todo>;

#[test]
fn every_kind_of_signal_is_described_per_instantiation() {
    let schema = schema();
    let board = schema
        .objects
        .iter()
        .find(|o| o.name == "TodoBoard")
        .unwrap();
    let signals = &board.store.as_ref().unwrap().signals;
    assert_eq!(signals.len(), 4);
    assert_eq!(signals[0].ty, TypeRef::vec(named("Todo")));
    // A derived list is a computed keyed list; a lazy list is `Lazy<Todo>`.
    assert!(signals[1].computed);
    assert_eq!(signals[1].key.as_deref(), Some("id"));
    assert_eq!(signals[2].ty, TypeRef::lazy(named("Todo")));
    assert!(signals[3].computed);
    assert_eq!(
        board
            .methods
            .iter()
            .find(|m| m.name == "add")
            .unwrap()
            .params[0]
            .ty,
        named("Todo")
    );
    schema.validate().unwrap();
}

#[test]
fn a_store_with_derived_and_lazy_lists_runs_and_restores() {
    let rt = Runtime::new();
    let handle = construct(&rt, "TodoBoard", &[]);
    for id in [1_u32, 2, 4] {
        rt.call_object(
            "TodoBoard",
            "add",
            handle,
            &args(|w| Todo::with_id(id).encode(w)),
        )
        .sync_ok();
    }
    let board: Arc<Board<Todo>> = rt.object::<TodoBoard>(handle).unwrap();
    assert_eq!(board.even.get().len(), 2);
    assert_eq!(board.open.get(), 3);

    let mut record = Writer::new();
    board.cell().encode_snapshot(&mut record);
    let mut r = Reader::new(record.as_slice());
    r.read_u64().unwrap();
    r.read_u32().unwrap();
    let body = r.read_rest().to_vec();
    let restorer = undra::meta::inventory::iter::<StoreRestorer>
        .into_iter()
        .find(|r| r.type_id == ids::type_id("TodoBoard"))
        .expect("a restorer per instantiation");
    let any: Arc<dyn Any + Send + Sync> =
        (restorer.restore)(rt.ctx(), 0x0000_0002_0000_0009, &mut Reader::new(&body)).unwrap();
    let restored = any.downcast::<Board<Todo>>().expect("the instantiation");
    assert_eq!(restored.rows.get().len(), 3);
    assert_eq!(
        restored.even.get().len(),
        2,
        "the hook rebuilt the derived list"
    );
    assert_eq!(restored.open.get(), 3, "and the computed one");
}

// ---------------------------------------------------------------------------------------------
// Keys, objects as arguments, callbacks and singletons
// ---------------------------------------------------------------------------------------------

/// A newtype key.
#[k::api]
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct UserId(pub String);

/// A plain object with two parameters, a map in its signatures and a streamed method.
pub struct Directory<K, V> {
    entries: Mutex<std::collections::HashMap<K, V>>,
}

/// Keys and values.
#[k::api(generic)]
impl<
    K: Clone + Eq + std::hash::Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + Unpin + 'static,
> Directory<K, V>
{
    pub fn new() -> Self {
        Directory {
            entries: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Stores a value under a key.
    pub fn put(&self, key: K, value: V) {
        self.entries.lock().unwrap().insert(key, value);
    }

    /// Everything, by key.
    pub fn all(&self) -> std::collections::HashMap<K, V> {
        self.entries.lock().unwrap().clone()
    }

    /// The values, one at a time.
    pub fn values(&self) -> impl undra::runtime::Stream<Item = V> + Send + 'static {
        support::testing::stream_of(self.entries.lock().unwrap().values().cloned().collect())
    }
}

#[k::api]
pub type UserDirectory = Directory<UserId, Todo>;

#[k::api]
pub type NameDirectory = Directory<String, Note>;

#[test]
fn a_map_in_a_signature_takes_whatever_key_its_position_accepts() {
    let schema = schema();
    let users = schema
        .objects
        .iter()
        .find(|o| o.name == "UserDirectory")
        .unwrap();
    let all = users.methods.iter().find(|m| m.name == "all").unwrap();
    assert_eq!(all.returns, TypeRef::map(named("UserId"), named("Todo")));
    let names = schema
        .objects
        .iter()
        .find(|o| o.name == "NameDirectory")
        .unwrap();
    assert_eq!(
        names
            .methods
            .iter()
            .find(|m| m.name == "all")
            .unwrap()
            .returns,
        TypeRef::map(TypeRef::String, named("Note"))
    );
    assert_eq!(
        names
            .methods
            .iter()
            .find(|m| m.name == "values")
            .unwrap()
            .returns,
        TypeRef::stream(named("Note"))
    );
    schema.validate().unwrap();

    let rt = Runtime::new();
    let handle = construct(&rt, "UserDirectory", &[]);
    rt.call_object(
        "UserDirectory",
        "put",
        handle,
        &args(|w| {
            UserId("ada".to_owned()).encode(w);
            Todo::with_id(1).encode(w);
        }),
    )
    .sync_ok();
    let reply = rt
        .call_object("UserDirectory", "all", handle, &[])
        .sync_ok();
    let all = std::collections::HashMap::<UserId, Todo>::decode_exact(&reply).unwrap();
    assert_eq!(all[&UserId("ada".to_owned())], Todo::with_id(1));
    let streamed = rt
        .call_object("UserDirectory", "values", handle, &[])
        .run_stream();
    assert_eq!(streamed.len(), 1);
}

/// An object kept by another: `T` is an object, spelled `Arc<T>` in the template.
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

    pub fn label(&self) -> String {
        self.label.clone()
    }
}

pub struct Shelf<T> {
    items: Mutex<Vec<Arc<T>>>,
}

#[k::api(generic)]
impl<T: Send + Sync + 'static> Shelf<T> {
    pub fn new() -> Self {
        Shelf {
            items: Mutex::new(Vec::new()),
        }
    }

    /// Puts an object on the shelf.
    pub fn put(&self, item: Arc<T>) {
        self.items.lock().unwrap().push(item);
    }

    /// The first object on the shelf, if there is one.
    pub fn first(&self) -> Option<Arc<T>> {
        self.items.lock().unwrap().first().cloned()
    }

    /// Every object on the shelf.
    pub fn all(&self) -> Vec<Arc<T>> {
        self.items.lock().unwrap().clone()
    }
}

#[k::api]
pub type MailboxShelf = Shelf<Mailbox>;

#[test]
fn an_object_is_an_argument_of_a_template_that_spells_arc_of_the_parameter() {
    let schema = schema();
    let shelf = schema
        .objects
        .iter()
        .find(|o| o.name == "MailboxShelf")
        .unwrap();
    let put = shelf.methods.iter().find(|m| m.name == "put").unwrap();
    assert_eq!(put.params[0].ty, TypeRef::Object("Mailbox".to_owned()));
    let first = shelf.methods.iter().find(|m| m.name == "first").unwrap();
    assert_eq!(
        first.returns,
        TypeRef::option(TypeRef::Object("Mailbox".to_owned()))
    );
    schema.validate().unwrap();

    let rt = Runtime::new();
    let shelf = construct(&rt, "MailboxShelf", &[]);
    let mailbox = construct(&rt, "Mailbox", &[]);
    rt.call_object(
        "MailboxShelf",
        "put",
        shelf,
        &args(|w| Handle(mailbox).encode(w)),
    )
    .sync_ok();
    let reply = rt
        .call_object("MailboxShelf", "first", shelf, &[])
        .sync_ok();
    let again = Option::<Handle>::decode_exact(&reply).unwrap().unwrap();
    // One object, one wrapper per handle (ADR-040): the shelf hands out the same instance.
    assert_eq!(again.0, mailbox);
}

// ---------------------------------------------------------------------------------------------
// Singletons intern per instantiation
// ---------------------------------------------------------------------------------------------

/// A shared default of a row type.
pub struct Defaults<T> {
    value: T,
}

#[k::api(generic)]
impl<T: Row> Defaults<T> {
    /// The one instance.
    pub fn shared(_ctx: &Ctx) -> Arc<Self> {
        static ONES: Mutex<Vec<(std::any::TypeId, Arc<dyn Any + Send + Sync>)>> =
            Mutex::new(Vec::new());
        let mut ones = ONES.lock().unwrap();
        let id = std::any::TypeId::of::<T>();
        if let Some((_, one)) = ones.iter().find(|(i, _)| *i == id) {
            return one.clone().downcast::<Defaults<T>>().expect("one per type");
        }
        let one = Arc::new(Defaults {
            value: T::default(),
        });
        ones.push((id, one.clone()));
        one
    }

    pub fn value(&self) -> T {
        self.value.clone()
    }
}

#[k::api]
pub type TodoDefaults = Defaults<Todo>;

#[k::api]
pub type NoteDefaults = Defaults<Note>;

#[test]
fn a_singleton_is_interned_per_instantiation() {
    let rt = Runtime::new();
    let a = construct_shared(&rt, "TodoDefaults");
    let b = construct_shared(&rt, "TodoDefaults");
    let c = construct_shared(&rt, "NoteDefaults");
    assert_eq!(
        a, b,
        "the same Arc is the same handle while the host holds it"
    );
    assert_ne!(a, c);
    let reply = rt.call_object("NoteDefaults", "value", c, &[]).sync_ok();
    assert_eq!(Note::decode_exact(&reply).unwrap(), Note::default());
}

fn construct_shared(rt: &Runtime, object: &str) -> u64 {
    let reply = rt.call_object(object, "shared", 0, &[]).sync_ok();
    Handle::decode_exact(&reply).unwrap().0
}

// ---------------------------------------------------------------------------------------------
// A generic type applied to a type parameter is named through its alias (section 4)
// ---------------------------------------------------------------------------------------------

/// A page of rows.
#[k::api(generic)]
#[derive(Clone, Debug, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<String>,
}

#[k::api]
pub type TodoPage = Page<Todo>;

#[k::api]
pub type NotePage = Page<Note>;

/// `Page<T>` is spelled in the signature and named by its alias for each listed type.
#[k::api(generic(T = [Todo, Note]))]
pub fn page_of<T: Row>(rows: Vec<T>) -> Page<T> {
    Page {
        items: rows,
        next: None,
    }
}

/// A store handed out and taken by a generic function: `Arc<Selection<T>>` and `&Selection<T>`.
#[k::api(generic(T = [Todo, Note]))]
pub fn open_selection<T: Row>(limit: u32) -> Arc<Selection<T>> {
    Arc::new(Selection::<T>::new(limit))
}

#[k::api(generic(T = [Todo, Note]))]
pub fn ticked<T: Row>(selection: &Selection<T>) -> u32 {
    selection.rows.get().len() as u32
}

#[test]
fn a_generic_application_in_a_generic_signature_is_named_by_its_alias() {
    let schema = schema();
    let page = schema
        .functions
        .iter()
        .find(|f| f.name == "page_of<Note>")
        .unwrap();
    assert_eq!(page.returns, named("NotePage"));
    let open = schema
        .functions
        .iter()
        .find(|f| f.name == "open_selection<Todo>")
        .unwrap();
    assert_eq!(open.returns, TypeRef::Object("TodoSelection".to_owned()));
    let ticked = schema
        .functions
        .iter()
        .find(|f| f.name == "ticked<Note>")
        .unwrap();
    assert_eq!(
        ticked.params[0].ty,
        TypeRef::Object("NoteSelection".to_owned())
    );
    schema.validate().unwrap();

    let rt = Runtime::new();
    let reply = rt
        .call_function("page_of<Todo>", &args(|w| vec![Todo::with_id(1)].encode(w)))
        .sync_ok();
    assert_eq!(
        TodoPage::decode_exact(&reply).unwrap(),
        Page {
            items: vec![Todo::with_id(1)],
            next: None
        }
    );
    let reply = rt
        .call_function("open_selection<Note>", &args(|w| 2_u32.encode(w)))
        .sync_ok();
    let selection = Handle::decode_exact(&reply).unwrap();
    rt.call_object(
        "NoteSelection",
        "toggle",
        selection.0,
        &args(|w| Note::with_id(3).encode(w)),
    )
    .sync_ok();
    let reply = rt
        .call_function("ticked<Note>", &args(|w| selection.encode(w)))
        .sync_ok();
    assert_eq!(u32::decode_exact(&reply).unwrap(), 1);
}

// ---------------------------------------------------------------------------------------------
// Callbacks, bounds in a `where` clause and the author's own bounds on the struct
// ---------------------------------------------------------------------------------------------

/// Tells the host a row arrived.
#[k::callback]
pub trait Arrivals {
    fn arrived(&self, id: u32);
}

pub struct Inbox<T>
where
    T: Row,
{
    rows: Mutex<Vec<T>>,
    listeners: Mutex<Vec<Arc<dyn Arrivals>>>,
}

/// An inbox that tells its listeners.
#[k::api(generic)]
impl<T> Inbox<T>
where
    T: Row,
{
    pub fn new() -> Self {
        Inbox {
            rows: Mutex::new(Vec::new()),
            listeners: Mutex::new(Vec::new()),
        }
    }

    /// Asks to be told about every row that arrives.
    pub fn listen(&self, listener: Arc<dyn Arrivals>) {
        self.listeners.lock().unwrap().push(listener);
    }

    /// A row arrives.
    pub fn deliver(&self, row: T) {
        for listener in self.listeners.lock().unwrap().iter() {
            listener.arrived(row.id());
        }
        self.rows.lock().unwrap().push(row);
    }
}

#[k::api]
pub type TodoInbox = Inbox<Todo>;

#[test]
fn a_callback_parameter_and_a_where_clause_are_the_authors_to_write() {
    let schema = schema();
    let inbox = schema
        .objects
        .iter()
        .find(|o| o.name == "TodoInbox")
        .unwrap();
    let listen = inbox.methods.iter().find(|m| m.name == "listen").unwrap();
    assert_eq!(
        listen.params[0].ty,
        TypeRef::Callback("Arrivals".to_owned())
    );
    assert_eq!(
        inbox
            .methods
            .iter()
            .find(|m| m.name == "deliver")
            .unwrap()
            .params[0]
            .ty,
        named("Todo")
    );
    schema.validate().unwrap();
}
