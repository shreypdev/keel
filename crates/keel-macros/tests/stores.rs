//! Behaviour tests for `#[keel::store]` and `#[keel::api(store)]`: the generated signal table,
//! cell attachment, keyed lists, snapshot restore and struct-literal patching, run against
//! the test facade.
#![forbid(unsafe_code)]

use std::any::Any;
use std::sync::Arc;

use keel::meta::{TypeRef, collect_schema, ids};
use keel::prelude::{Computed, Ctx, Signal};
use keel::runtime::{Runtime, StoreObject, StoreRestorer};
use keel::signals::{AttachKind, Lazy, StoreCell};
use keel::wire::{Decode, Encode, Handle, Reader, WireError, Writer};
use keel_macros as k;

#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub id: u32,
    pub title: String,
}

#[k::api]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Filter {
    All,
    Even,
}

impl Filter {
    fn matches(self, row: &Row) -> bool {
        self == Filter::All || row.id % 2 == 0
    }
}

/// A store with every kind of signal and a restore hook.
#[k::store(restore = "Self::assemble")]
pub struct Todos {
    ctx: Ctx,
    #[keel(key = "id")]
    rows: Signal<Vec<Row>>,
    filter: Signal<Filter>,
    #[keel(no_coalesce)]
    ticks: Signal<u32>,
    visible: Computed<Vec<Row>>,
    page: Lazy<Row>,
    label: String,
}

#[k::api(store)]
impl Todos {
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(
            ctx,
            Signal::new(Vec::new()),
            Signal::new(Filter::All),
            Signal::new(0),
        )
    }

    fn assemble(
        ctx: Ctx,
        rows: Signal<Vec<Row>>,
        filter: Signal<Filter>,
        ticks: Signal<u32>,
    ) -> Self {
        let visible = Computed::new((&rows, &filter), |(rows, filter)| {
            rows.into_iter().filter(|r| filter.matches(r)).collect()
        });
        // No `__keel_cell` in the literal: the macro adds it inside this impl block.
        Self {
            ctx,
            rows,
            filter,
            ticks,
            visible,
            page: Lazy::new(),
            label: "todos".to_owned(),
        }
    }

    pub fn add(&self, id: u32, title: String) {
        self.rows.update(|rows| rows.push(Row { id, title }));
    }

    pub fn set_filter(&self, filter: Filter) {
        self.filter.set(filter);
    }

    pub fn tick(&self) {
        self.ticks.update(|t| *t += 1);
    }

    pub fn visible_len(&self) -> u32 {
        self.visible.get().len() as u32
    }

    pub fn label(&self) -> String {
        self.label.clone()
    }

    pub fn has_ctx(&self) -> bool {
        let _ = self.ctx.clone();
        true
    }
}

/// A store whose restore is generated: state is `Ctx` or `Default`.
#[k::store]
pub struct Counter {
    ctx: Ctx,
    count: Signal<i64>,
    name: Signal<String>,
    note: String,
    extra: Vec<u8>,
}

#[k::api(store)]
impl Counter {
    pub fn new(ctx: &Ctx) -> Self {
        Counter {
            ctx: ctx.clone(),
            count: Signal::new(0),
            name: Signal::new("counter".to_owned()),
            note: "kept".to_owned(),
            extra: vec![1],
        }
    }

    pub fn incr(&self) -> i64 {
        self.count.update(|c| *c += 1);
        self.count.get()
    }

    pub fn note(&self) -> String {
        self.note.clone()
    }

    pub fn extra_len(&self) -> u32 {
        self.extra.len() as u32
    }
}

fn construct(rt: &Runtime, store: &str, args: &[u8]) -> u64 {
    let reply = rt.call_object(store, "new", 0, args).sync_ok();
    Handle::decode_exact(&reply).unwrap().0
}

fn todos(rt: &Runtime) -> (u64, Arc<Todos>) {
    let handle = construct(rt, "Todos", &[]);
    (handle, rt.object::<Todos>(handle).unwrap())
}

fn restorer(type_name: &str) -> &'static StoreRestorer {
    let type_id = ids::type_id(type_name);
    keel::meta::inventory::iter::<StoreRestorer>
        .into_iter()
        .find(|r| r.type_id == type_id)
        .unwrap_or_else(|| panic!("no restorer for {type_name}"))
}

fn restore<T: Send + Sync + 'static>(
    rt: &Runtime,
    type_name: &str,
    body: &[u8],
) -> Result<Arc<T>, WireError> {
    let mut r = Reader::new(body);
    let any: Arc<dyn Any + Send + Sync> = (restorer(type_name).restore)(rt.ctx(), &mut r)?;
    Ok(any.downcast::<T>().expect("restored the registered type"))
}

#[test]
fn store_meta_lists_signals_with_ids_types_and_flags() {
    let schema = collect_schema("stores-test");
    let object = schema.objects.iter().find(|o| o.name == "Todos").unwrap();
    let store = object.store.as_ref().expect("Todos is a store");
    let signals: Vec<(&str, u32, bool, Option<&str>)> = store
        .signals
        .iter()
        .map(|s| (s.name.as_str(), s.signal_id, s.computed, s.key.as_deref()))
        .collect();
    assert_eq!(
        signals,
        [
            ("rows", 0, false, Some("id")),
            ("filter", 1, false, None),
            ("ticks", 2, false, None),
            ("visible", 3, true, None),
            ("page", 4, false, None),
        ]
    );
    assert_eq!(store.signals[0].ty, TypeRef::vec(TypeRef::named("Row")));
    assert_eq!(store.signals[1].ty, TypeRef::named("Filter"));
    assert_eq!(store.signals[2].ty, TypeRef::U32);
    assert_eq!(store.signals[3].ty, TypeRef::vec(TypeRef::named("Row")));
    assert_eq!(store.signals[4].ty, TypeRef::lazy(TypeRef::named("Row")));
    assert_eq!(object.constructors.len(), 1);
    assert_eq!(object.constructors[0].name, "new");

    let counter = schema.objects.iter().find(|o| o.name == "Counter").unwrap();
    let names: Vec<&str> = counter
        .store
        .as_ref()
        .unwrap()
        .signals
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    assert_eq!(names, ["count", "name"], "state fields are not signals");
}

#[test]
fn the_registered_schema_with_stores_validates() {
    collect_schema("stores-test")
        .validate()
        .unwrap_or_else(|errors| panic!("{errors:#?}"));
}

#[test]
fn constructors_attach_every_signal_in_order_and_record_the_handle() {
    let rt = Runtime::new();
    let (handle, store) = todos(&rt);
    let cell = store.cell();
    assert_eq!(StoreCell::type_id(cell), ids::type_id("Todos"));
    assert_eq!(
        cell.handle(),
        handle,
        "the object table's handle reached the cell"
    );

    let attached = cell.attached();
    let shape: Vec<(u32, AttachKind, bool, bool)> = attached
        .iter()
        .map(|a| (a.signal_id, a.kind, a.key.is_some(), a.no_coalesce))
        .collect();
    assert_eq!(
        shape,
        [
            (0, AttachKind::Plain, true, false),
            (1, AttachKind::Plain, false, false),
            (2, AttachKind::Plain, false, true),
            (3, AttachKind::Computed, false, false),
            (4, AttachKind::Lazy, false, false),
        ]
    );
}

#[test]
fn keyed_lists_hash_the_encoded_key_field() {
    let rt = Runtime::new();
    let (_, store) = todos(&rt);
    let attached = store.cell().attached();
    let key = attached[0].key.unwrap();
    let row = Row {
        id: 5,
        title: "x".into(),
    };
    assert_eq!(key(&row), ids::fnv1a64(&5_u32.encode_to_vec()));
    // The title is not part of the key.
    let other = Row {
        id: 5,
        title: "y".into(),
    };
    assert_eq!(key(&other), key(&row));
    // A value of another type never panics.
    assert_eq!(key(&"not a row".to_owned()), 0);
}

#[test]
fn cell_is_created_lazily_for_stores_built_without_the_runtime() {
    let rt = Runtime::new();
    let store = Todos::new(rt.ctx());
    let first = Arc::as_ptr(store.cell());
    let second = Arc::as_ptr(store.cell());
    assert_eq!(first, second, "one cell per instance");
    assert_eq!(store.cell().attached().len(), 5);
    assert_eq!(store.cell().handle(), 0);
}

#[test]
fn methods_work_through_dispatch_and_signals_update() {
    let rt = Runtime::new();
    let (handle, store) = todos(&rt);
    let add = |id: u32, title: &str| {
        let mut w = Writer::new();
        id.encode(&mut w);
        title.to_owned().encode(&mut w);
        assert!(
            rt.call_object("Todos", "add", handle, w.as_slice())
                .sync_ok()
                .is_empty()
        );
    };
    add(1, "a");
    add(2, "b");
    assert_eq!(store.rows.get().len(), 2);
    let len = rt
        .call_object("Todos", "visible_len", handle, &[])
        .sync_ok();
    assert_eq!(u32::decode_exact(&len).unwrap(), 2);
    let mut w = Writer::new();
    Filter::Even.encode(&mut w);
    rt.call_object("Todos", "set_filter", handle, w.as_slice())
        .sync_ok();
    let len = rt
        .call_object("Todos", "visible_len", handle, &[])
        .sync_ok();
    assert_eq!(u32::decode_exact(&len).unwrap(), 1);
    let label = rt.call_object("Todos", "label", handle, &[]).sync_ok();
    assert_eq!(String::decode_exact(&label).unwrap(), "todos");
}

#[test]
fn snapshot_excludes_computed_and_lazy_signals() {
    let rt = Runtime::new();
    let (handle, store) = todos(&rt);
    rt.call_object("Todos", "tick", handle, &[]).sync_ok();
    let mut body = Writer::new();
    store.cell().encode_snapshot(&mut body);
    let mut r = Reader::new(body.as_slice());
    assert_eq!(r.read_u32().unwrap(), 3, "rows, filter, ticks");
    let mut ids_seen = Vec::new();
    for _ in 0..3 {
        ids_seen.push(r.read_u32().unwrap());
        r.read_bytes().unwrap();
    }
    assert_eq!(ids_seen, [0, 1, 2]);
    r.finish().unwrap();
}

#[test]
fn restore_with_a_hook_rebuilds_computed_and_lazy_fields() {
    let rt = Runtime::new();
    let (handle, store) = todos(&rt);
    for (id, title) in [(1_u32, "a"), (2, "b"), (4, "d")] {
        store.add(id, title.to_owned());
    }
    store.set_filter(Filter::Even);
    rt.call_object("Todos", "tick", handle, &[]).sync_ok();
    rt.call_object("Todos", "tick", handle, &[]).sync_ok();

    let mut body = Writer::new();
    store.cell().encode_snapshot(&mut body);
    let restored = restore::<Todos>(&rt, "Todos", body.as_slice()).unwrap();

    assert_eq!(restored.rows.get(), store.rows.get());
    assert_eq!(restored.filter.get(), Filter::Even);
    assert_eq!(restored.ticks.get(), 2);
    assert_eq!(restored.visible_len(), 2, "the computed value was rebuilt");
    assert_eq!(restored.label(), "todos", "state comes from the hook");
    assert!(restored.has_ctx());
    assert_eq!(
        restored.cell().attached().len(),
        5,
        "signals are attached after restore"
    );
    // The restored store is independent of the original.
    restored.add(9, "z".to_owned());
    assert_eq!(store.rows.get().len(), 3);
}

#[test]
fn restore_without_a_hook_uses_ctx_and_default() {
    let rt = Runtime::new();
    let handle = construct(&rt, "Counter", &{
        // `Counter::new(ctx: &Ctx)` takes no wire arguments.
        Vec::new()
    });
    let counter = rt.object::<Counter>(handle).unwrap();
    counter.incr();
    counter.incr();
    counter.name.set("renamed".to_owned());

    let mut body = Writer::new();
    counter.cell().encode_snapshot(&mut body);
    let restored = restore::<Counter>(&rt, "Counter", body.as_slice()).unwrap();
    assert_eq!(restored.count.get(), 2);
    assert_eq!(restored.name.get(), "renamed");
    assert_eq!(
        restored.note(),
        "",
        "non-signal state falls back to Default"
    );
    assert_eq!(restored.extra_len(), 0);
    let _ = restored.ctx.clone();
    assert_eq!(restored.cell().attached().len(), 2);
}

#[test]
fn restore_ignores_unknown_signals_and_rejects_missing_ones() {
    let rt = Runtime::new();
    let handle = construct(&rt, "Counter", &[]);
    let counter = rt.object::<Counter>(handle).unwrap();

    // An extra signal id 9 from a newer snapshot is skipped.
    let mut w = Writer::new();
    w.write_u32(3);
    w.write_u32(0);
    w.write_bytes(&7_i64.encode_to_vec());
    w.write_u32(9);
    w.write_bytes(&[1, 2, 3]);
    w.write_u32(1);
    w.write_bytes(&"n".to_owned().encode_to_vec());
    let restored = restore::<Counter>(&rt, "Counter", w.as_slice()).unwrap();
    assert_eq!(
        (restored.count.get(), restored.name.get()),
        (7, "n".to_owned())
    );

    // Signal 1 is missing.
    let mut w = Writer::new();
    w.write_u32(1);
    w.write_u32(0);
    w.write_bytes(&7_i64.encode_to_vec());
    let error = restore::<Counter>(&rt, "Counter", w.as_slice())
        .err()
        .unwrap();
    assert!(
        matches!(error, WireError::InvalidTag { tag: 1, ty, .. } if ty.contains("missing signal 1 (name)")),
        "{error:?}"
    );

    // A value that does not decode, and truncated input.
    let mut w = Writer::new();
    w.write_u32(1);
    w.write_u32(0);
    w.write_bytes(&[1, 2]);
    assert!(restore::<Counter>(&rt, "Counter", w.as_slice()).is_err());
    let mut body = Writer::new();
    counter.cell().encode_snapshot(&mut body);
    for cut in 0..body.as_slice().len() {
        assert!(
            restore::<Counter>(&rt, "Counter", &body.as_slice()[..cut]).is_err(),
            "cut at {cut} must not restore"
        );
    }
    // An absurd count fails on end of input instead of looping or allocating.
    assert!(restore::<Counter>(&rt, "Counter", &u32::MAX.to_le_bytes()).is_err());
}

#[test]
fn stores_are_objects_too() {
    let rt = Runtime::new();
    let (handle, _) = todos(&rt);
    assert!(!rt.call_object("Todos", "has_ctx", handle, &[]).is_unknown());
    assert_eq!(<Todos as keel::runtime::KeelObject>::NAME, "Todos");
}
