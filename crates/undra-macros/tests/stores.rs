//! Behaviour tests for `#[undra::store]` and `#[undra::api(store)]`: the generated signal table,
//! cell attachment, keyed lists, snapshot restore and struct-literal patching, run against
//! the real runtime and signals.
#![forbid(unsafe_code)]

use std::any::Any;
use std::sync::Arc;

use undra::meta::{TypeRef, collect_schema, ids};
use undra::prelude::{Computed, Ctx, Signal};
use undra::runtime::{StoreObject, StoreRestorer};
use undra::signals::{ALL_SIGNALS, SignalsError, StoreCell};
use undra::wire::payload::ChangeOp;
use undra::wire::{Decode, Encode, Handle, KeyedPatch, PatchOp, Reader, WireError, Writer};
use undra_macros as k;

mod support;
use support::Runtime;

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
    #[undra(key = "id")]
    rows: Signal<Vec<Row>>,
    filter: Signal<Filter>,
    #[undra(no_coalesce)]
    ticks: Signal<u32>,
    visible: Computed<Vec<Row>>,
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
            rows.iter().filter(|r| filter.matches(r)).cloned().collect()
        });
        // No `__undra_cell` in the literal: the macro adds it inside this impl block.
        Self {
            ctx,
            rows,
            filter,
            ticks,
            visible,
            label: "todos".to_owned(),
        }
    }

    pub fn add(&self, id: u32, title: String) {
        self.rows.update(|rows| rows.push(Row { id, title }));
    }

    pub fn rename(&self, id: u32, title: String) {
        self.rows.update(|rows| {
            for row in rows.iter_mut().filter(|r| r.id == id) {
                row.title.clone_from(&title);
            }
        });
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

/// A store that can be built over another store's signal, which cannot be attached twice.
#[k::store]
pub struct Twin {
    count: Signal<u32>,
}

#[k::api(store)]
#[allow(clippy::new_without_default)]
impl Twin {
    pub fn new() -> Self {
        Twin {
            count: Signal::new(0),
        }
    }

    fn sharing(other: &Twin) -> Self {
        Twin {
            count: other.count.clone(),
        }
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

/// The handle the tests pretend the snapshot re-issues.
const RESTORED_HANDLE: u64 = 0x0000_0002_0000_0007;

fn restorer(type_name: &str) -> &'static StoreRestorer {
    let type_id = ids::type_id(type_name);
    undra::meta::inventory::iter::<StoreRestorer>
        .into_iter()
        .find(|r| r.type_id == type_id)
        .unwrap_or_else(|| panic!("no restorer for {type_name}"))
}

/// The store body of a snapshot, as `StoreObject::restore` reads it: what `encode_snapshot`
/// writes minus the `handle u64, type_id u32` the runtime consumes first.
fn snapshot_body(store: &dyn Fn(&mut Writer)) -> Vec<u8> {
    let mut record = Writer::new();
    store(&mut record);
    let mut r = Reader::new(record.as_slice());
    r.read_u64().unwrap();
    r.read_u32().unwrap();
    r.read_rest().to_vec()
}

fn restore<T: Send + Sync + 'static>(
    rt: &Runtime,
    type_name: &str,
    body: &[u8],
) -> Result<Arc<T>, WireError> {
    let mut r = Reader::new(body);
    let any: Arc<dyn Any + Send + Sync> =
        (restorer(type_name).restore)(rt.ctx(), RESTORED_HANDLE, &mut r)?;
    Ok(any.downcast::<T>().expect("restored the registered type"))
}

#[test]
fn store_meta_lists_signals_with_ids_types_and_flags() {
    let schema = collect_schema("stores-test");
    let object = schema.objects.iter().find(|o| o.name == "Todos").unwrap();
    let store = object.store.as_ref().expect("Todos is a store");
    let signals: Vec<(&str, u32, bool, Option<&str>, bool)> = store
        .signals
        .iter()
        .map(|s| {
            (
                s.name.as_str(),
                s.signal_id,
                s.computed,
                s.key.as_deref(),
                s.no_coalesce,
            )
        })
        .collect();
    assert_eq!(
        signals,
        [
            ("rows", 0, false, Some("id"), false),
            ("filter", 1, false, None, false),
            // `#[undra(no_coalesce)]` reaches the schema, so the platforms can honour it (ADR-031).
            ("ticks", 2, false, None, true),
            ("visible", 3, true, None, false),
        ]
    );
    assert_eq!(store.signals[0].ty, TypeRef::vec(TypeRef::named("Row")));
    assert_eq!(store.signals[1].ty, TypeRef::named("Filter"));
    assert_eq!(store.signals[2].ty, TypeRef::U32);
    assert_eq!(store.signals[3].ty, TypeRef::vec(TypeRef::named("Row")));
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
    assert_eq!(cell.signal_count(), 4, "rows, filter, ticks, visible");

    // Observing everything yields one entry per attached signal, in declaration order.
    rt.real().observe(handle, ALL_SIGNALS, true);
    let initial = rt.change_sets();
    assert_eq!(initial.len(), 1);
    let entries: Vec<(u32, ChangeOp)> = initial[0]
        .entries
        .iter()
        .map(|e| (e.signal_id, e.op))
        .collect();
    assert_eq!(
        entries,
        [
            (0, ChangeOp::Full),
            (1, ChangeOp::Full),
            (2, ChangeOp::Full),
            (3, ChangeOp::Full)
        ]
    );
    assert!(
        initial[0]
            .entries
            .iter()
            .all(|e| e.handle == Handle(handle))
    );
}

fn add_row(rt: &Runtime, handle: u64, id: u32, title: &str) {
    let mut w = Writer::new();
    id.encode(&mut w);
    title.to_owned().encode(&mut w);
    rt.call_object("Todos", "add", handle, w.as_slice())
        .sync_ok();
}

fn patch_of(cs: &undra::wire::payload::ChangeSet, signal_id: u32) -> KeyedPatch<Row> {
    let entry = cs
        .entries
        .iter()
        .find(|e| e.signal_id == signal_id)
        .expect("the signal is in the change-set");
    assert_eq!(entry.op, ChangeOp::KeyedPatch, "{cs:?}");
    let mut r = Reader::new(&entry.value);
    let patch = KeyedPatch::<Row>::decode(&mut r).unwrap();
    r.finish().unwrap();
    patch
}

#[test]
fn keyed_lists_identify_items_by_the_hash_of_their_encoded_key_field() {
    let rt = Runtime::new();
    let (handle, _) = todos(&rt);
    rt.real().observe(handle, 0, true);
    rt.change_sets();

    // Empty to non-empty shares no key: the whole list is sent.
    add_row(&rt, handle, 1, "a");
    let cs = rt.change_sets();
    assert_eq!(cs[0].entries[0].op, ChangeOp::Full);
    // Then items are matched by key: an append is one insert...
    add_row(&rt, handle, 2, "b");
    let cs = rt.change_sets();
    assert_eq!(
        patch_of(&cs[0], 0).ops,
        [PatchOp::Insert {
            index: 1,
            item: Row {
                id: 2,
                title: "b".into()
            }
        }]
    );
    // ...and changing a field that is not the key is an update of the same item.
    rt.call_object(
        "Todos",
        "rename",
        handle,
        &[1_u32.encode_to_vec(), "z".to_owned().encode_to_vec()].concat(),
    )
    .sync_ok();
    let cs = rt.change_sets();
    assert_eq!(
        patch_of(&cs[0], 0).ops,
        [PatchOp::Update {
            index: 0,
            item: Row {
                id: 1,
                title: "z".into()
            }
        }]
    );
    // A repeated key makes matching ambiguous: the full list is sent instead of a patch.
    add_row(&rt, handle, 2, "dup");
    let cs = rt.change_sets();
    assert_eq!(cs[0].entries[0].op, ChangeOp::Full);
}

#[test]
fn no_coalesce_signals_are_delivered_even_when_unobserved() {
    let rt = Runtime::new();
    let (handle, _) = todos(&rt);
    // Nothing is observed; `ticks` is `no_coalesce`, `rows` is not.
    rt.call_object("Todos", "tick", handle, &[]).sync_ok();
    add_row(&rt, handle, 1, "a");
    let cs = rt.change_sets();
    assert_eq!(cs.len(), 1, "only the tick reached the platform: {cs:?}");
    let ids: Vec<u32> = cs[0].entries.iter().map(|e| e.signal_id).collect();
    assert_eq!(ids, [2]);
    assert_eq!(u32::decode_exact(&cs[0].entries[0].value).unwrap(), 1);
}

#[test]
fn attach_errors_are_returned_to_the_caller() {
    let first = Twin::new();
    let second = Twin::sharing(&first);
    assert_eq!(first.__undra_attach_all(), Ok(()));
    assert_eq!(
        first.__undra_attach_all(),
        Ok(()),
        "attaching twice is a no-op"
    );
    // `second` reuses the first store's signal, which belongs to `first`'s cell.
    assert_eq!(
        second.__undra_attach_all(),
        Err(SignalsError::AlreadyAttached)
    );
    // (The constructor's dispatch arm turns that error into a bad request; see `undra`'s
    // end-to-end test.)
    let rt = Runtime::new();
    assert!(rt.call_object("Twin", "new", 0, &[]).sync_ok().len() == 8);
}

#[test]
fn cell_is_created_lazily_for_stores_built_without_the_runtime() {
    let rt = Runtime::new();
    let store = Todos::new(rt.ctx());
    let first = Arc::as_ptr(store.cell());
    let second = Arc::as_ptr(store.cell());
    assert_eq!(first, second, "one cell per instance");
    assert_eq!(store.cell().signal_count(), 4);
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
fn snapshot_excludes_computed_signals() {
    let rt = Runtime::new();
    let (handle, store) = todos(&rt);
    rt.call_object("Todos", "tick", handle, &[]).sync_ok();
    let mut record = Writer::new();
    store.cell().encode_snapshot(&mut record);
    let mut r = Reader::new(record.as_slice());
    assert_eq!(
        r.read_u64().unwrap(),
        handle,
        "the record starts with the handle"
    );
    assert_eq!(r.read_u32().unwrap(), ids::type_id("Todos"));
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
fn restore_with_a_hook_rebuilds_computed_fields() {
    let rt = Runtime::new();
    let (handle, store) = todos(&rt);
    for (id, title) in [(1_u32, "a"), (2, "b"), (4, "d")] {
        store.add(id, title.to_owned());
    }
    store.set_filter(Filter::Even);
    rt.call_object("Todos", "tick", handle, &[]).sync_ok();
    rt.call_object("Todos", "tick", handle, &[]).sync_ok();

    let body = snapshot_body(&|w| store.cell().encode_snapshot(w));
    let restored = restore::<Todos>(&rt, "Todos", &body).unwrap();

    assert_eq!(restored.rows.get(), store.rows.get());
    assert_eq!(restored.filter.get(), Filter::Even);
    assert_eq!(restored.ticks.get(), 2);
    assert_eq!(restored.visible_len(), 2, "the computed value was rebuilt");
    assert_eq!(restored.label(), "todos", "state comes from the hook");
    assert!(restored.has_ctx());
    assert_eq!(
        restored.cell().signal_count(),
        4,
        "signals are attached after restore"
    );
    assert_eq!(
        restored.cell().handle(),
        RESTORED_HANDLE,
        "the re-issued handle reached the cell"
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

    let body = snapshot_body(&|w| counter.cell().encode_snapshot(w));
    let restored = restore::<Counter>(&rt, "Counter", &body).unwrap();
    assert_eq!(restored.count.get(), 2);
    assert_eq!(restored.name.get(), "renamed");
    assert_eq!(
        restored.note(),
        "",
        "non-signal state falls back to Default"
    );
    assert_eq!(restored.extra_len(), 0);
    let _ = restored.ctx.clone();
    assert_eq!(restored.cell().signal_count(), 2);
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
    let body = snapshot_body(&|w| counter.cell().encode_snapshot(w));
    for cut in 0..body.len() {
        assert!(
            restore::<Counter>(&rt, "Counter", &body[..cut]).is_err(),
            "cut at {cut} must not restore"
        );
    }
    // An absurd count fails on end of input instead of looping or allocating.
    assert!(restore::<Counter>(&rt, "Counter", &u32::MAX.to_le_bytes()).is_err());
}

#[test]
fn the_registered_cell_accessor_reaches_the_cell_through_dyn_any() {
    let rt = Runtime::new();
    let (handle, store) = todos(&rt);
    let any: Arc<dyn Any + Send + Sync> = store.clone();
    let cell = (restorer("Todos").cell)(any.as_ref()).expect("a Todos");
    assert!(Arc::ptr_eq(cell, store.cell()));
    assert_eq!(cell.handle(), handle);
    // Something that is not a `Todos` has no cell.
    let other: Arc<dyn Any + Send + Sync> = Arc::new(5_u32);
    assert!((restorer("Todos").cell)(other.as_ref()).is_none());
}

#[test]
fn stores_are_objects_too() {
    let rt = Runtime::new();
    let (handle, _) = todos(&rt);
    assert!(!rt.call_object("Todos", "has_ctx", handle, &[]).is_unknown());
    assert_eq!(<Todos as undra::runtime::UndraObject>::NAME, "Todos");
}
