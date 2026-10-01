//! `DerivedList<T>` store fields (ADR-039 section 9): the schema describes them as read-only keyed
//! lists (`computed: true` with a `key`), the generated attach makes them ship keyed patches, and a
//! restore hook rebuilds them. Run against the real runtime and signals.
#![forbid(unsafe_code)]

use std::any::Any;
use std::sync::Arc;

use undra::meta::{TypeRef, collect_schema, ids};
use undra::prelude::{Computed, Ctx, DerivedList, Signal};
use undra::runtime::{StoreObject, StoreRestorer};
use undra::wire::payload::{ChangeOp, ChangeSet};
use undra::wire::{Decode, Encode, Handle, KeyedPatch, PatchOp, Reader, Writer};
use undra_macros as k;

mod support;
use support::Runtime;

#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Task {
    pub id: u32,
    pub title: String,
    pub done: bool,
}

#[k::api]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Show {
    All,
    Open,
}

/// The founder's example, reduced: a keyed source, a parameter, a derived view and a count.
#[k::store(restore = "Self::assemble")]
pub struct Board {
    #[undra(key = "id")]
    tasks: Signal<Vec<Task>>,
    show: Signal<Show>,
    #[undra(key = "id")]
    visible: DerivedList<Task>,
    open: Computed<u32>,
}

#[k::api(store)]
impl Board {
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(Vec::new()), Signal::new(Show::All))
    }

    fn assemble(_ctx: Ctx, tasks: Signal<Vec<Task>>, show: Signal<Show>) -> Self {
        let visible = tasks
            .derive()
            .filter_with(&show, |show, t: &Task| *show == Show::All || !t.done)
            .build();
        let open = tasks.derive().filter(|t: &Task| !t.done).count();
        Self {
            tasks,
            show,
            visible,
            open,
        }
    }

    pub fn add(&self, id: u32, title: String) {
        self.tasks.push(Task {
            id,
            title,
            done: false,
        });
    }

    pub fn toggle(&self, at: u32) {
        self.tasks.update_at(at as usize, |t| t.done = !t.done);
    }

    pub fn set_show(&self, show: Show) {
        self.show.set(show);
    }

    pub fn visible_len(&self) -> u32 {
        self.visible.len() as u32
    }
}

const TASKS: u32 = 0;
const SHOW: u32 = 1;
const VISIBLE: u32 = 2;
const OPEN: u32 = 3;

fn board(rt: &Runtime) -> (u64, Arc<Board>) {
    let reply = rt.call_object("Board", "new", 0, &[]).sync_ok();
    let handle = Handle::decode_exact(&reply).unwrap().0;
    (handle, rt.object::<Board>(handle).unwrap())
}

fn add(rt: &Runtime, handle: u64, id: u32, title: &str) {
    let args = [id.encode_to_vec(), title.to_owned().encode_to_vec()].concat();
    rt.call_object("Board", "add", handle, &args).sync_ok();
}

fn entry(cs: &ChangeSet, id: u32) -> &undra::wire::payload::ChangeEntry {
    cs.entries
        .iter()
        .find(|e| e.signal_id == id)
        .unwrap_or_else(|| panic!("no entry for {id}: {cs:?}"))
}

fn patch(cs: &ChangeSet, id: u32) -> Vec<PatchOp<Task>> {
    let e = entry(cs, id);
    assert_eq!(e.op, ChangeOp::KeyedPatch, "{cs:?}");
    let mut r = Reader::new(&e.value);
    let patch = KeyedPatch::<Task>::decode(&mut r).unwrap();
    r.finish().unwrap();
    patch.ops
}

fn task(id: u32, title: &str, done: bool) -> Task {
    Task {
        id,
        title: title.into(),
        done,
    }
}

#[test]
fn the_schema_describes_a_derived_list_as_a_read_only_keyed_list() {
    let schema = collect_schema("derived-stores-test");
    let object = schema.objects.iter().find(|o| o.name == "Board").unwrap();
    let store = object.store.as_ref().unwrap();
    let signals: Vec<(&str, bool, Option<&str>)> = store
        .signals
        .iter()
        .map(|s| (s.name.as_str(), s.computed, s.key.as_deref()))
        .collect();
    assert_eq!(
        signals,
        [
            ("tasks", false, Some("id")),
            ("show", false, None),
            ("visible", true, Some("id")),
            ("open", true, None),
        ]
    );
    assert_eq!(store.signals[2].ty, TypeRef::vec(TypeRef::named("Task")));
    schema
        .validate()
        .unwrap_or_else(|errors| panic!("{errors:#?}"));
}

#[test]
fn a_derived_field_ships_keyed_patches_and_its_count_follows() {
    let rt = Runtime::new();
    let (handle, store) = board(&rt);
    rt.real().observe(handle, u32::MAX, true);
    let initial = rt.change_sets();
    assert_eq!(entry(&initial[0], TASKS).op, ChangeOp::Full);
    assert_eq!(entry(&initial[0], VISIBLE).op, ChangeOp::Full);

    for (id, title) in [(1, "a"), (2, "b"), (3, "c")] {
        add(&rt, handle, id, title);
        let cs = rt.change_sets();
        assert_eq!(
            patch(&cs[0], VISIBLE),
            [PatchOp::Insert {
                index: id - 1,
                item: task(id, title, false)
            }]
        );
        assert_eq!(u32::decode_exact(&entry(&cs[0], OPEN).value).unwrap(), id);
    }
    // Toggling a visible task under `All` is an Update.
    rt.call_object("Board", "toggle", handle, &1_u32.encode_to_vec())
        .sync_ok();
    let cs = rt.change_sets();
    assert_eq!(
        patch(&cs[0], VISIBLE),
        [PatchOp::Update {
            index: 1,
            item: task(2, "b", true)
        }]
    );
    // The parameter: `Open` hides it, as one Remove.
    rt.call_object("Board", "set_show", handle, &Show::Open.encode_to_vec())
        .sync_ok();
    let cs = rt.change_sets();
    let ids: Vec<u32> = cs[0].entries.iter().map(|e| e.signal_id).collect();
    assert_eq!(ids, [SHOW, VISIBLE], "the source did not change");
    assert_eq!(patch(&cs[0], VISIBLE), [PatchOp::Remove { index: 1 }]);
    assert_eq!(store.visible_len(), 2);
}

#[test]
fn restore_rebuilds_the_derived_field_from_its_source() {
    let rt = Runtime::new();
    let (_, store) = board(&rt);
    store.add(1, "a".into());
    store.add(2, "b".into());
    store.toggle(0);
    store.set_show(Show::Open);

    let mut record = Writer::new();
    store.cell().encode_snapshot(&mut record);
    let mut r = Reader::new(record.as_slice());
    r.read_u64().unwrap();
    r.read_u32().unwrap();
    let body = r.read_rest().to_vec();
    let mut r = Reader::new(&body);
    assert_eq!(
        r.read_u32().unwrap(),
        2,
        "tasks and show; the view and the count are derived"
    );

    let restorer: &StoreRestorer = undra::meta::inventory::iter::<StoreRestorer>
        .into_iter()
        .find(|r| r.type_id == ids::type_id("Board"))
        .unwrap();
    let mut r = Reader::new(&body);
    let any: Arc<dyn Any + Send + Sync> =
        (restorer.restore)(rt.ctx(), 0x0000_0002_0000_0009, &mut r).unwrap();
    let restored = any.downcast::<Board>().unwrap();
    assert_eq!(restored.visible.get(), [task(2, "b", false)]);
    assert_eq!(restored.open.get(), 1);
    // Observing after the restore sends the rebuilt view in full.
    let mut out = Writer::new();
    assert_eq!(restored.cell().observe(VISIBLE, true, &mut out), 1);
}
