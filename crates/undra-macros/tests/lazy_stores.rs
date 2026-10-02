//! `Lazy<T>` store fields (ADR-043 decision 3), end to end with the real runtime and signals: the
//! schema, the generated attach and restore, the page server the runtime registers for each lazy
//! signal (observe, page, mutate, invalidate, release), snapshot/restore (the items come back, the
//! handle is new, op 0 is re-sent), `#[undra(default)]`, and a `Lazy::over` view with a restore hook.
#![forbid(unsafe_code)]

use std::any::Any;
use std::sync::Arc;

use undra::meta::{TypeRef, collect_schema, ids};
use undra::prelude::{Ctx, Lazy, Signal};
use undra::runtime::testing::{ReplyRecord, call_payload, decode_reply};
use undra::runtime::{StoreObject, StoreRestorer};
use undra::wire::payload::{
    CallTarget, ChangeEntry, ChangeOp, ChangeSet, LazyInvalidated, LazyPage, LazyValue, ReplyStatus,
};
use undra::wire::{Decode, Encode, Handle, Reader, Writer};
use undra_macros as k;

mod support;
use support::Runtime;

#[k::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Book {
    pub id: u32,
    pub title: String,
}

/// An owned lazy list, a defaulted one and a plain signal.
#[k::store]
pub struct Library {
    #[undra(key = "id")]
    books: Lazy<Book>,
    #[undra(default)]
    tags: Lazy<String>,
    loaded: Signal<u32>,
}

#[k::api(store)]
impl Library {
    pub fn new(_ctx: Ctx) -> Self {
        Self {
            books: Lazy::new(),
            tags: Lazy::new(),
            loaded: Signal::new(0),
        }
    }

    pub fn load(&self, n: u32) {
        self.books.replace((1..=n).map(book).collect());
        self.loaded.set(n);
    }

    pub fn add(&self, id: u32, title: String) {
        self.books.push(Book { id, title });
    }

    pub fn rename(&self, at: u32, title: String) {
        self.books.update_at(at as usize, |b| b.title = title);
    }

    pub fn tag(&self, tag: String) {
        self.tags.push(tag);
    }
}

fn book(id: u32) -> Book {
    Book {
        id,
        title: format!("book {id}"),
    }
}

/// A view over a derived list: the books of one decade, newest id first, behind a restore hook.
#[k::store(restore = "Self::assemble")]
pub struct Catalog {
    all: Signal<Vec<Book>>,
    #[undra(key = "id")]
    recent: Lazy<Book>,
}

#[k::api(store)]
impl Catalog {
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(Vec::new()), Lazy::new())
    }

    /// The restore hook (and the constructor's body): the view is rebuilt from the source, so the
    /// empty list a snapshot holds for it is ignored.
    fn assemble(_ctx: Ctx, all: Signal<Vec<Book>>, _persisted: Lazy<Book>) -> Self {
        let recent = Lazy::over(
            &all.derive()
                .filter(|b: &Book| b.id % 2 == 0)
                .sort_by_key(|b: &Book| std::cmp::Reverse(b.id))
                .build(),
        );
        Catalog { all, recent }
    }

    pub fn add(&self, id: u32) {
        self.all.push(book(id));
    }
}

const BOOKS: u32 = 0;
const TAGS: u32 = 1;
const LOADED: u32 = 2;

fn handle_of(reply: &[u8]) -> u64 {
    Handle::decode_exact(reply).unwrap().0
}

fn library(rt: &Runtime) -> (u64, Arc<Library>) {
    let handle = handle_of(&rt.call_object("Library", "new", 0, &[]).sync_ok());
    (handle, rt.object::<Library>(handle).unwrap())
}

fn call(rt: &Runtime, handle: u64, method: &str, args: &[u8]) {
    rt.call_object("Library", method, handle, args).sync_ok();
}

fn entry(cs: &ChangeSet, id: u32) -> &ChangeEntry {
    cs.entries
        .iter()
        .find(|e| e.signal_id == id)
        .unwrap_or_else(|| panic!("no entry for signal {id}: {cs:?}"))
}

fn value(e: &ChangeEntry) -> LazyValue {
    assert_eq!(e.op, ChangeOp::Full);
    LazyValue::decode(&mut Reader::new(&e.value)).unwrap()
}

fn invalidated(e: &ChangeEntry) -> LazyInvalidated {
    assert_eq!(e.op, ChangeOp::LazyInvalidated);
    assert_eq!(e.value.len(), 12);
    LazyInvalidated::decode(&mut Reader::new(&e.value)).unwrap()
}

/// What a page call answers: the reply, as the host decodes it.
fn page_reply(rt: &Runtime, server: Handle, offset: u32, limit: u32) -> ReplyRecord {
    let payload = call_payload(
        CallTarget::LazyPage {
            handle: server,
            offset,
            limit,
        },
        7,
        &[],
    );
    rt.real().call_sync_with(&payload, decode_reply)
}

fn page<T: Decode>(rt: &Runtime, server: Handle, offset: u32, limit: u32) -> (LazyPage, Vec<T>) {
    let reply = page_reply(rt, server, offset, limit);
    assert_eq!(reply.status, ReplyStatus::Ok, "{reply:?}");
    let mut r = Reader::new(&reply.body);
    let header = LazyPage::decode(&mut r).unwrap();
    let rows = (0..header.count)
        .map(|_| T::decode(&mut r).unwrap())
        .collect();
    r.finish().unwrap();
    (header, rows)
}

/// A number of the runtime's `stats_json`.
fn stat(rt: &Runtime, key: &str) -> u64 {
    let json = rt.real().stats_json();
    let at = json
        .find(&format!("\"{key}\":"))
        .expect("a stat of that name")
        + key.len()
        + 3;
    json[at..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap()
}

#[test]
fn the_schema_describes_a_lazy_list_as_lazy_of_its_row_type() {
    let schema = collect_schema("lazy-stores-test");
    let object = schema.objects.iter().find(|o| o.name == "Library").unwrap();
    let store = object.store.as_ref().unwrap();
    let signals: Vec<(&str, &TypeRef, bool, Option<&str>, bool)> = store
        .signals
        .iter()
        .map(|s| {
            (
                s.name.as_str(),
                &s.ty,
                s.computed,
                s.key.as_deref(),
                s.default,
            )
        })
        .collect();
    assert_eq!(
        signals,
        [
            (
                "books",
                &TypeRef::lazy(TypeRef::named("Book")),
                false,
                Some("id"),
                false
            ),
            ("tags", &TypeRef::lazy(TypeRef::String), false, None, true),
            ("loaded", &TypeRef::U32, false, None, false),
        ]
    );
    schema
        .validate()
        .unwrap_or_else(|errors| panic!("{errors:#?}"));
    // The type closure and fingerprint of the store reach `Book` through `Lazy(Book)`.
    let type_id = ids::type_id("Library");
    let closure = schema.store_closure(type_id).unwrap();
    assert!(closure.record("Book").is_some(), "{closure:?}");
    assert_ne!(schema.store_fingerprint(type_id).unwrap(), 0);
}

#[test]
fn observing_sends_the_page_server_and_paging_serves_the_rows() {
    let rt = Runtime::new();
    let (handle, store) = library(&rt);
    call(&rt, handle, "load", &10_000_u32.encode_to_vec());
    rt.change_sets();
    rt.real().observe(handle, u32::MAX, true);
    let cs = rt.change_sets();
    assert_eq!(cs.len(), 1);
    let v = value(entry(&cs[0], BOOKS));
    assert_eq!((v.len, v.version), (10_000, 1));
    assert!(!v.handle.is_null());
    assert_eq!(
        v.handle.0,
        store.cell().lazy_handle(BOOKS),
        "the runtime told the cell"
    );
    // The tags list is empty but is a lazy list too, with a page server of its own.
    let tags = value(entry(&cs[0], TAGS));
    assert_eq!((tags.len, tags.version), (0, 0));
    assert_ne!(tags.handle, v.handle);

    // A window in the middle of the list.
    let (header, rows) = page::<Book>(&rt, v.handle, 4_990, 50);
    assert_eq!(
        (header.version, header.total, header.count),
        (1, 10_000, 50)
    );
    assert_eq!(rows[0], book(4_991));
    assert_eq!(rows[49], book(5_040));
    // An offset past the end is the empty window; the cap holds.
    let (header, rows) = page::<Book>(&rt, v.handle, 10_000, 5);
    assert_eq!((header.total, header.count, rows.len()), (10_000, 0, 0));
    let (header, _) = page::<Book>(&rt, v.handle, 0, u32::MAX);
    assert_eq!(header.count, undra::runtime::MAX_PAGE_ITEMS);
}

#[test]
fn a_change_is_one_invalidation_of_twelve_bytes_and_the_next_page_is_at_its_version() {
    let rt = Runtime::new();
    let (handle, _store) = library(&rt);
    call(&rt, handle, "load", &1_000_u32.encode_to_vec());
    rt.real().observe(handle, u32::MAX, true);
    let v = value(entry(&rt.change_sets()[0], BOOKS));

    call(
        &rt,
        handle,
        "rename",
        &[
            500_u32.encode_to_vec(),
            "renamed".to_owned().encode_to_vec(),
        ]
        .concat(),
    );
    let cs = rt.change_sets();
    assert_eq!(cs.len(), 1);
    assert_eq!(cs[0].entries.len(), 1, "only the list changed");
    let inv = invalidated(&cs[0].entries[0]);
    assert_eq!((inv.len, inv.version), (1_000, v.version + 1));

    let (header, rows) = page::<Book>(&rt, v.handle, 498, 5);
    assert_eq!(header.version, inv.version);
    assert_eq!(rows[2].title, "renamed");

    // Many writes of one call are still one entry, and an append moves the length.
    call(
        &rt,
        handle,
        "add",
        &[1_001_u32.encode_to_vec(), "last".to_owned().encode_to_vec()].concat(),
    );
    let inv = invalidated(&rt.change_sets()[0].entries[0]);
    assert_eq!(inv.len, 1_001);
    let (header, rows) = page::<Book>(&rt, v.handle, 1_000, 10);
    assert_eq!(
        (header.total, rows),
        (1_001, vec![book(1_001).with_title("last")])
    );
}

impl Book {
    fn with_title(mut self, title: &str) -> Book {
        self.title = title.to_owned();
        self
    }
}

#[test]
fn a_released_store_takes_its_page_servers_with_it() {
    let rt = Runtime::new();
    let (handle, _store) = library(&rt);
    call(&rt, handle, "load", &5_u32.encode_to_vec());
    rt.real().observe(handle, u32::MAX, true);
    let v = value(entry(&rt.change_sets()[0], BOOKS));
    assert_eq!(page_reply(&rt, v.handle, 0, 1).status, ReplyStatus::Ok);

    // The host cannot release a page server itself: it is not its reference to give back.
    rt.real().release(v.handle.0);
    assert_eq!(page_reply(&rt, v.handle, 0, 1).status, ReplyStatus::Ok);

    // The table holds the store and one page server per lazy signal, but the host owns one
    // reference: the store's.
    assert_eq!(stat(&rt, "live_handles"), 3);
    assert_eq!(stat(&rt, "host_refs"), 1);
    rt.real().release(handle);
    let reply = page_reply(&rt, v.handle, 0, 1);
    assert_eq!(reply.status, ReplyStatus::BadRequest, "{reply:?}");
    assert_eq!(
        stat(&rt, "live_handles"),
        0,
        "nothing of the store is left in the table"
    );
}

#[test]
fn a_page_server_is_never_in_a_snapshot_and_a_restore_gives_the_list_back_with_a_new_handle() {
    let rt = Runtime::new();
    let (handle, _store) = library(&rt);
    call(&rt, handle, "load", &300_u32.encode_to_vec());
    call(&rt, handle, "tag", &"fiction".to_owned().encode_to_vec());
    rt.real().observe(handle, u32::MAX, true);
    let before = rt.change_sets();
    let old = value(entry(&before[0], BOOKS));
    let snapshot = rt.real().snapshot();

    // The objects the table holds besides the store are page servers: none is in the snapshot.
    let decoded = undra::wire::payload::Snapshot::decode(&mut Reader::new(&snapshot)).unwrap();
    assert_eq!(decoded.stores.len(), 1);

    // Changes after the snapshot are taken back by the restore.
    call(&rt, handle, "load", &5_u32.encode_to_vec());
    rt.change_sets();
    rt.real().restore(&snapshot).unwrap();
    let cs = rt.change_sets();
    assert_eq!(cs.len(), 1, "one change-set per re-observed store");
    let now = value(entry(&cs[0], BOOKS));
    assert_eq!(now.len, 300, "the items came back");
    assert_ne!(now.handle, old.handle, "the page server is new");
    assert_eq!(
        page_reply(&rt, old.handle, 0, 1).status,
        ReplyStatus::BadRequest,
        "the old one is stale"
    );
    let (header, rows) = page::<Book>(&rt, now.handle, 298, 10);
    assert_eq!((header.total, rows), (300, vec![book(299), book(300)]));
    let tags = value(entry(&cs[0], TAGS));
    let (_, tags) = page::<String>(&rt, tags.handle, 0, 10);
    assert_eq!(tags, ["fiction"]);

    // The restored store delivers again: one invalidation per change.
    call(&rt, handle, "tag", &"poetry".to_owned().encode_to_vec());
    let cs = rt.change_sets();
    assert_eq!(cs[0].entries.len(), 1);
    assert_eq!(invalidated(&cs[0].entries[0]).len, 2);
}

#[test]
fn a_defaulted_lazy_list_is_empty_when_the_snapshot_lacks_it() {
    let rt = Runtime::new();
    let restorer: &StoreRestorer = undra::meta::inventory::iter::<StoreRestorer>
        .into_iter()
        .find(|r| r.type_id == ids::type_id("Library"))
        .unwrap();
    // A body with `books` (1) and `loaded` (3) only.
    let mut body = Writer::new();
    body.write_u32(2);
    for (id, value) in [
        (BOOKS, vec![book(1), book(2)].encode_to_vec()),
        (LOADED, 2_u32.encode_to_vec()),
    ] {
        body.write_u32(id);
        body.write_bytes(&value);
    }
    let mut r = Reader::new(body.as_slice());
    let any: Arc<dyn Any + Send + Sync> =
        (restorer.restore)(rt.ctx(), 0x0000_0002_0000_0009, &mut r).unwrap();
    let restored = any.downcast::<Library>().unwrap();
    assert_eq!(restored.books.to_vec(), [book(1), book(2)]);
    assert!(restored.tags.is_empty(), "#[undra(default)] fills it empty");
    assert_eq!(restored.loaded.get(), 2);

    // Without `#[undra(default)]` a missing lazy list is an error, as for any signal.
    let mut body = Writer::new();
    body.write_u32(0);
    let mut r = Reader::new(body.as_slice());
    let error = (restorer.restore)(rt.ctx(), 0x0000_0002_0000_0009, &mut r)
        .err()
        .expect("`books` is missing");
    assert!(error.to_string().contains("missing signal"), "{error}");
}

#[test]
fn a_view_is_paged_through_the_derived_index_announced_once_and_rebuilt_by_the_restore_hook() {
    let rt = Runtime::new();
    let handle = handle_of(&rt.call_object("Catalog", "new", 0, &[]).sync_ok());
    let catalog = rt.object::<Catalog>(handle).unwrap();
    for id in 1..=20 {
        rt.call_object("Catalog", "add", handle, &id.encode_to_vec())
            .sync_ok();
    }
    rt.real().observe(handle, u32::MAX, true);
    let cs = rt.change_sets();
    let v = value(entry(&cs[0], 1));
    assert_eq!(v.len, 10);
    let (header, rows) = page::<Book>(&rt, v.handle, 0, 3);
    assert_eq!(header.total, 10);
    assert_eq!(
        rows,
        [book(20), book(18), book(16)],
        "even ids, newest first"
    );

    // An even id enters the view: one invalidation; an odd one is not announced at all.
    rt.call_object("Catalog", "add", handle, &22_u32.encode_to_vec())
        .sync_ok();
    let cs = rt.change_sets();
    assert_eq!(invalidated(entry(&cs[0], 1)).len, 11);
    rt.call_object("Catalog", "add", handle, &23_u32.encode_to_vec())
        .sync_ok();
    let cs = rt.change_sets();
    assert!(cs[0].entries.iter().all(|e| e.signal_id != 1), "{cs:?}");

    // A snapshot holds an empty list for the view; the hook rebuilds it from `all`.
    let snapshot = rt.real().snapshot();
    rt.call_object("Catalog", "add", handle, &24_u32.encode_to_vec())
        .sync_ok();
    rt.change_sets();
    rt.real().restore(&snapshot).unwrap();
    let cs = rt.change_sets();
    let now = value(entry(&cs[0], 1));
    assert_eq!(
        now.len, 11,
        "the view is the restored source's, not the empty snapshot value"
    );
    let (_, rows) = page::<Book>(&rt, now.handle, 0, 2);
    assert_eq!(rows, [book(22), book(20)]);
    assert_eq!(catalog.recent.len(), 12, "the old store object is detached");
}

#[test]
fn encode_and_decode_agree_with_the_vec_the_snapshot_holds() {
    // The snapshot value of a lazy list is the `Vec<T>` of its items.
    let rt = Runtime::new();
    let (handle, store) = library(&rt);
    call(&rt, handle, "load", &3_u32.encode_to_vec());
    let mut record = Writer::new();
    store.cell().encode_snapshot(&mut record);
    let snapshot =
        undra::wire::payload::StoreSnapshot::decode(&mut Reader::new(record.as_slice())).unwrap();
    let books = snapshot
        .signals
        .iter()
        .find(|(id, _)| *id == BOOKS)
        .unwrap();
    assert_eq!(
        Vec::<Book>::decode_exact(&books.1).unwrap(),
        [book(1), book(2), book(3)]
    );
}
