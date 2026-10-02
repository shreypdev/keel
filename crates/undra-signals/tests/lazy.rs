//! `Lazy<T>` in a store (ADR-043 decision 3): what reaches the host for each change (one 12-byte
//! `LazyInvalidated` entry, whatever the change was), the value of `observe`, the page server, the
//! snapshot, the delivery rules (unobserved, `no_coalesce`, abandon), views over derived lists with
//! live changes against a naive model, and the page window arithmetic.

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::*;
use proptest::prelude::*;
use undra_signals::{
    ALL_SIGNALS, Lazy, LazySource, Signal, SignalsError, StoreCell, page_window, txn,
};
use undra_wire::payload::{
    ChangeEntry, ChangeOp, LazyInvalidated, LazyPage, LazyValue, StoreSnapshot,
};
use undra_wire::{Decode, Handle, Reader, Writer};

const BOOKS: u32 = 0;
const OTHER: u32 = 1;
/// The page server's handle in the object table of the runtime (what the runtime tells the cell).
const SERVER: u64 = 0x0000_0002_0000_0009;

struct Fixture {
    rig: Rig,
    books: Lazy<Todo>,
    other: Signal<u32>,
}

fn fixture(n: u32) -> Fixture {
    let rig = Rig::new();
    let books = Lazy::from_vec(todos(n));
    let other = Signal::new(0_u32);
    rig.cell.attach_lazy(&books, BOOKS).unwrap();
    rig.cell.attach(&other, OTHER).unwrap();
    rig.cell.set_lazy_handle(BOOKS, SERVER);
    Fixture { rig, books, other }
}

fn value(entry: &ChangeEntry) -> LazyValue {
    assert_eq!(entry.op, ChangeOp::Full);
    LazyValue::decode(&mut Reader::new(&entry.value)).expect("a LazyValue")
}

fn invalidated(entry: &ChangeEntry) -> LazyInvalidated {
    assert_eq!(entry.op, ChangeOp::LazyInvalidated);
    assert_eq!(entry.value.len(), 12, "an invalidation is 12 bytes");
    let mut r = Reader::new(&entry.value);
    let inv = LazyInvalidated::decode(&mut r).expect("a LazyInvalidated");
    r.finish().unwrap();
    inv
}

/// What a page call answers: the header and the decoded rows.
fn page(source: &dyn LazySource, offset: u32, limit: u32) -> (LazyPage, Vec<Todo>) {
    let mut rows = Writer::new();
    let header = source.encode_page(offset, limit, &mut rows);
    let mut r = Reader::new(rows.as_slice());
    let items: Vec<Todo> = (0..header.count)
        .map(|_| Todo::decode(&mut r).expect("a row decodes"))
        .collect();
    r.finish().expect("exactly `count` rows");
    (header, items)
}

// ---------------------------------------------------------------------------------------------
// observe: op 0 with the page server, the length and the version
// ---------------------------------------------------------------------------------------------

#[test]
fn observing_sends_the_page_server_the_length_and_the_version() {
    let f = fixture(10_000);
    let entries = f.rig.observe_all();
    assert_eq!(entries.len(), 2);
    let v = value(&entries[0]);
    assert_eq!(
        (v.handle, v.len, v.version),
        (Handle(SERVER), 10_000, 0),
        "the items themselves never cross"
    );
    assert_eq!(entries[0].value.len(), 20);
    f.books.push(todo(10_001, "new", false));
    f.rig.observe_on(BOOKS);
    let v = value(&f.rig.observe_on(BOOKS)[0]);
    assert_eq!(
        (v.len, v.version),
        (10_001, 1),
        "observing again re-sends the current stamp"
    );
}

#[test]
fn a_lazy_list_cell_knows_its_page_server_and_its_handle() {
    let f = fixture(3);
    let sources = f.rig.cell.lazy_sources();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].0, BOOKS);
    assert_eq!(sources[0].1.stamp(), (3, 0));
    assert_eq!(f.rig.cell.lazy_handle(BOOKS), SERVER);
    assert_eq!(f.rig.cell.lazy_handle(OTHER), 0, "not a lazy signal");
    assert_eq!(f.rig.cell.lazy_handle(99), 0, "no such signal");
    f.rig.cell.set_lazy_handle(OTHER, 5);
    f.rig.cell.set_lazy_handle(99, 5);
    assert_eq!(f.rig.cell.lazy_handle(OTHER), 0);
    // Without a handle yet the value carries the null handle.
    let bare = StoreCell::new(1);
    let list = Lazy::from_vec(vec![1_u32]);
    bare.attach_lazy(&list, 0).unwrap();
    bare.set_handle(HANDLE);
    let entries = observe(&bare, ALL_SIGNALS, true);
    assert!(value(&entries[0]).handle.is_null());
}

// ---------------------------------------------------------------------------------------------
// a change: one entry of 12 bytes, whatever it was
// ---------------------------------------------------------------------------------------------

#[test]
fn every_kind_of_change_sends_exactly_one_invalidation_of_twelve_bytes() {
    let f = fixture(10_000);
    f.rig.observe_all();
    let books = &f.books;
    type Change<'a> = Box<dyn Fn() + 'a>;
    let changes: Vec<(&str, Change)> = vec![
        ("push", Box::new(|| books.push(todo(20_000, "p", false)))),
        (
            "insert",
            Box::new(|| books.insert(5, todo(20_001, "i", false))),
        ),
        ("remove", Box::new(|| drop(books.remove(7)))),
        (
            "update_at",
            Box::new(|| books.update_at(3, |t| t.done = true)),
        ),
        ("move_item", Box::new(|| books.move_item(0, 9_000))),
        ("replace", Box::new(|| books.replace(todos(500)))),
        ("clear", Box::new(|| books.clear())),
    ];
    let mut version = 0;
    for (name, change) in changes {
        f.rig.run(change);
        let set = f.rig.one_set();
        assert_eq!(set.entries.len(), 1, "{name}: only the list changed");
        let inv = invalidated(&set.entries[0]);
        version += 1;
        assert_eq!(
            (inv.len as usize, inv.version),
            (books.len(), version),
            "{name}"
        );
    }
    assert_eq!(books.len(), 0);
}

#[test]
fn a_transaction_of_many_writes_is_one_invalidation_with_the_last_stamp() {
    let f = fixture(100);
    f.rig.observe_all();
    f.rig.run(|| {
        txn(|| {
            for i in 0..50 {
                f.books.push(todo(1_000 + i, "x", false));
            }
            f.books.remove(0);
            f.other.set(1);
        });
    });
    let set = f.rig.one_set();
    assert_eq!(ids(&set), [BOOKS, OTHER]);
    let inv = invalidated(entry(&set, BOOKS));
    assert_eq!((inv.len, inv.version), (149, 51));
}

#[test]
fn an_unobserved_list_costs_nothing_and_observing_sends_the_current_stamp() {
    let f = fixture(10);
    f.rig.run(|| {
        f.books.push(todo(11, "a", false));
        f.books.push(todo(12, "b", false));
    });
    assert!(f.rig.sets().is_empty(), "nobody observes it");
    let v = value(&f.rig.observe_on(BOOKS)[0]);
    assert_eq!((v.len, v.version), (12, 2));
    f.rig.observe_off(BOOKS);
    f.rig.run(|| f.books.push(todo(13, "c", false)));
    assert!(f.rig.sets().is_empty(), "observation stopped");
}

#[test]
fn a_write_that_changes_nothing_sends_nothing() {
    let f = fixture(3);
    f.rig.observe_all();
    f.rig.run(|| {
        f.books.clear();
    });
    assert_eq!(f.rig.one_set().entries.len(), 1);
    f.rig.run(|| f.books.clear());
    assert!(
        f.rig.sets().is_empty(),
        "clearing an empty list is not a change"
    );
    f.rig.run(|| f.books.push(todo(1, "a", false)));
    f.rig.sets();
    f.rig.run(|| f.books.move_item(0, 0));
    assert!(
        f.rig.sets().is_empty(),
        "moving an item onto itself is not a change"
    );
}

// ---------------------------------------------------------------------------------------------
// delivery rules
// ---------------------------------------------------------------------------------------------

#[test]
fn no_coalesce_delivers_an_unobserved_list_as_its_value() {
    let f = fixture(4);
    f.rig.cell.set_no_coalesce(BOOKS).unwrap();
    f.rig.run(|| f.books.push(todo(5, "e", false)));
    let set = f.rig.one_set();
    let v = value(entry(&set, BOOKS));
    assert_eq!((v.handle, v.len, v.version), (Handle(SERVER), 5, 1));
    // Observed: an invalidation, as for any observed slot.
    f.rig.observe_on(BOOKS);
    f.rig.run(|| f.books.push(todo(6, "f", false)));
    assert_eq!(invalidated(entry(&f.rig.one_set(), BOOKS)).len, 6);
}

#[test]
fn an_abandoned_delivery_announces_the_list_again() {
    let f = fixture(3);
    let armed = Arc::new(AtomicBool::new(false));
    let bomb = Signal::new(EncodeBomb::new(&armed, 0));
    f.rig.cell.attach(&bomb, 2).unwrap();
    f.rig.observe_all();

    armed.store(true, Ordering::SeqCst);
    let aborted = catch_unwind(AssertUnwindSafe(|| {
        f.rig.run(|| {
            txn(|| {
                f.books.push(todo(4, "d", false));
                bomb.update(|_| {});
            });
        });
    }));
    assert!(aborted.is_err());
    armed.store(false, Ordering::SeqCst);
    assert!(
        f.rig.sets().is_empty(),
        "the change-set was abandoned whole"
    );

    // The next commit of the store sends the list again, even though this write is to another slot.
    f.rig.run(|| f.other.set(9));
    let set = f.rig.one_set();
    let inv = invalidated(entry(&set, BOOKS));
    assert_eq!((inv.len, inv.version), (4, 1));
}

#[test]
fn an_observe_that_unwinds_leaves_the_list_unannounced() {
    let f = fixture(3);
    let armed = Arc::new(AtomicBool::new(true));
    let bomb = Signal::new(EncodeBomb::new(&armed, 0));
    f.rig.cell.attach(&bomb, 2).unwrap();
    let result = catch_unwind(AssertUnwindSafe(|| f.rig.observe_all()));
    assert!(result.is_err());
    assert!(!f.rig.cell.is_observed(BOOKS));
    armed.store(false, Ordering::SeqCst);
    // A clean observe afterwards works and carries the stamp.
    assert_eq!(value(&f.rig.observe_on(BOOKS)[0]).len, 3);
}

#[test]
fn the_list_attaches_to_one_store_slot_for_life() {
    let f = fixture(1);
    assert_eq!(
        f.rig.cell.attach_lazy(&f.books, 2),
        Err(SignalsError::AlreadyAttached)
    );
    assert!(f.books.is_attached());
    let other = Lazy::<u32>::new();
    assert!(matches!(
        StoreCell::new(1).attach_lazy(&other, 4),
        Err(SignalsError::OutOfOrder { .. })
    ));
    assert!(!other.is_attached());
}

// ---------------------------------------------------------------------------------------------
// the page server
// ---------------------------------------------------------------------------------------------

#[test]
fn a_page_is_read_at_one_version_with_the_total_and_matches_the_invalidation() {
    let f = fixture(10_000);
    f.rig.observe_all();
    let source = f.rig.cell.lazy_sources().remove(0).1;

    let (header, rows) = page(&*source, 9_990, 50);
    assert_eq!(
        (header.version, header.total, header.count),
        (0, 10_000, 10)
    );
    assert_eq!(rows[0], todo(9_991, "t9991", false));

    f.rig
        .run(|| f.books.update_at(9_995, |t| t.title = "changed".into()));
    let inv = invalidated(&f.rig.one_set().entries[0]);
    let (header, rows) = page(&*source, 9_990, 50);
    assert_eq!(
        header.version, inv.version,
        "the page says which change it includes"
    );
    assert_eq!(rows[5].title, "changed");
    assert_eq!(header.total, inv.len);

    // Past the end: the total, no rows.
    let (header, rows) = page(&*source, 10_000, 5);
    assert_eq!((header.total, header.count), (10_000, 0));
    assert!(rows.is_empty());
}

#[test]
fn hostile_page_arguments_are_clamped_never_a_panic() {
    let list = Lazy::from_vec(todos(5));
    for (offset, limit) in [
        (0, 0),
        (5, 1),
        (6, 1),
        (u32::MAX, u32::MAX),
        (u32::MAX, 0),
        (0, u32::MAX),
        (4, u32::MAX),
        (1, 3),
    ] {
        let (header, rows) = page(&list, offset, limit);
        let window = page_window(5, offset, limit);
        assert_eq!(header.count as usize, window.len(), "({offset}, {limit})");
        assert_eq!(rows, todos(5)[window]);
        assert_eq!(header.total, 5);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, ..ProptestConfig::default() })]

    #[test]
    fn the_window_is_the_slice_the_arguments_ask_for(len in 0_usize..300, offset in any::<u32>(), limit in any::<u32>()) {
        let window = page_window(len, offset, limit);
        prop_assert!(window.start <= window.end && window.end <= len);
        let (o, l) = (offset as usize, limit as usize);
        // The model: what a naive loop would hand over.
        let naive: Vec<usize> = (0..len).filter(|i| *i >= o && (*i - o) < l).collect();
        prop_assert_eq!(window.clone().collect::<Vec<_>>(), naive);
        if o >= len {
            prop_assert_eq!(window, len..len);
        }
    }

    #[test]
    fn a_page_of_an_owned_list_is_the_window_of_its_items(
        n in 0_u32..120, offset in 0_u32..150, limit in 0_u32..150,
    ) {
        let list = Lazy::from_vec(todos(n));
        let (header, rows) = page(&list, offset, limit);
        let window = page_window(n as usize, offset, limit);
        prop_assert_eq!(header.total, n);
        prop_assert_eq!(header.count as usize, window.len());
        prop_assert_eq!(rows, todos(n)[window].to_vec());
    }
}

// ---------------------------------------------------------------------------------------------
// snapshots
// ---------------------------------------------------------------------------------------------

fn snapshot_of(cell: &StoreCell) -> StoreSnapshot {
    let mut out = Writer::new();
    cell.encode_snapshot(&mut out);
    StoreSnapshot::decode(&mut Reader::new(out.as_slice())).unwrap()
}

#[test]
fn a_snapshot_carries_the_items_as_the_vec_of_them() {
    let f = fixture(4);
    f.books.update_at(1, |t| t.done = true);
    let snapshot = snapshot_of(&f.rig.cell);
    let ids: Vec<u32> = snapshot.signals.iter().map(|(id, _)| *id).collect();
    assert_eq!(
        ids,
        [BOOKS, OTHER],
        "a Lazy is store state, unlike a computed or a view"
    );
    let items: Vec<Todo> = Decode::decode_exact(&snapshot.signals[0].1).unwrap();
    assert_eq!(items, f.books.to_vec());
    assert!(items[1].done);
    // And back: the same bytes rebuild the list.
    let restored = Lazy::from_vec(items);
    assert_eq!(restored.to_vec(), f.books.to_vec());
}

#[test]
fn a_view_is_derived_data_and_snapshots_as_an_empty_list() {
    let rig = Rig::new();
    let source = Signal::new(todos(10));
    let view = Lazy::over(&source.derive().filter(|t: &Todo| t.id % 2 == 0).build());
    rig.cell.attach_lazy(&view, 0).unwrap();
    assert_eq!(view.len(), 5);
    let snapshot = snapshot_of(&rig.cell);
    assert_eq!(snapshot.signals.len(), 1);
    assert_eq!(snapshot.signals[0].1, [0, 0, 0, 0]);
}

#[test]
fn encode_signal_is_the_vec_of_the_items() {
    let f = fixture(3);
    let mut out = Writer::new();
    assert!(f.rig.cell.encode_signal(BOOKS, &mut out));
    let items: Vec<Todo> = Decode::decode_exact(out.as_slice()).unwrap();
    assert_eq!(items, todos(3));
}

// ---------------------------------------------------------------------------------------------
// Lazy::over: a view of a derived list
// ---------------------------------------------------------------------------------------------

struct ViewFixture {
    rig: Rig,
    source: Signal<Vec<Todo>>,
}

/// `source` of `n` todos; the view shows the not-done ones, sorted by `title`.
fn view_fixture(n: u32) -> ViewFixture {
    view_fixture_keyed(n, true)
}

/// As [`view_fixture`]; `keyed: false` attaches the source as a plain signal (a test that writes
/// duplicate ids).
fn view_fixture_keyed(n: u32, keyed: bool) -> ViewFixture {
    let rig = Rig::new();
    let source = Signal::new(todos(n));
    let view = Lazy::over(
        &source
            .derive()
            .filter(|t: &Todo| !t.done)
            .sort_by_key(|t: &Todo| t.title.clone())
            .build(),
    );
    if keyed {
        rig.cell.attach_keyed(&source, 0, todo_key).unwrap();
    } else {
        rig.cell.attach(&source, 0).unwrap();
    }
    rig.cell.attach_lazy(&view, 1).unwrap();
    rig.cell.set_lazy_handle(1, SERVER);
    ViewFixture { rig, source }
}

/// The view as the definition says: `stable_sort_by_key(filter(source))`.
fn model(source: &[Todo]) -> Vec<Todo> {
    let mut rows: Vec<Todo> = source.iter().filter(|t| !t.done).cloned().collect();
    rows.sort_by(|a, b| a.title.cmp(&b.title));
    rows
}

#[test]
fn a_view_is_announced_when_the_view_changes_and_only_then() {
    let f = view_fixture(100);
    f.rig.observe_all();
    let v = value(&f.rig.observe_on(1)[0]);
    assert_eq!((v.handle, v.len), (Handle(SERVER), 100));

    // A row enters the view: one invalidation, and the source's own patch.
    f.rig.run(|| f.source.push(todo(101, "zzz", false)));
    let set = f.rig.one_set();
    assert_eq!(ids(&set), [0, 1]);
    let inv = invalidated(entry(&set, 1));
    assert_eq!(inv.len, 101);

    // A row the filter ignores: the source patches, the view is not announced.
    f.rig.run(|| f.source.push(todo(102, "aaa", true)));
    assert_eq!(ids(&f.rig.one_set()), [0]);

    // A row leaves the view.
    f.rig.run(|| f.source.update_at(0, |t| t.done = true));
    let set = f.rig.one_set();
    let inv2 = invalidated(entry(&set, 1));
    assert_eq!(inv2.len, 100);
    assert!(inv2.version > inv.version);
}

#[test]
fn a_view_pages_through_the_index_and_reads_the_committed_state() {
    let f = view_fixture(1_000);
    f.rig.observe_all();
    let source = f.rig.cell.lazy_sources().remove(0).1;
    let (header, rows) = page(&*source, 100, 50);
    assert_eq!((header.total, header.count), (1_000, 50));
    assert_eq!(rows, model(&f.source.get())[100..150]);

    // Writes the index has not replayed yet (nobody read since): a page is still right, because
    // asking replays the source's recorded operations first.
    f.source.update_at(0, |t| t.done = true);
    f.source.push(todo(1_001, "t0", false));
    f.source.remove(500);
    f.source.move_item(3, 900);
    let (header, rows) = page(&*source, 0, 2_000);
    let expected = model(&f.source.get());
    assert_eq!(header.total as usize, expected.len());
    assert_eq!(rows, expected);
    // A raw write cannot be replayed: the index rebuilds, the page is still right.
    f.source.set(todos(40));
    let (header, rows) = page(&*source, 30, 100);
    assert_eq!(header.total, 40);
    assert_eq!(rows, model(&todos(40))[30..]);
}

#[test]
fn a_view_over_a_mapped_derived_list_pages_the_mapped_rows() {
    let source = Signal::new((0..30_u32).collect::<Vec<_>>());
    let squares = source
        .derive()
        .filter(|n| n % 2 == 1)
        .map(|n| n * n)
        .build();
    let view = Lazy::over(&squares);
    assert_eq!(view.with_range(2..5, |rows| rows.to_vec()), [25, 49, 81]);
    source.push(31);
    assert_eq!(view.get(view.len() - 1), Some(961));
    assert_eq!(view.get(view.len()), None);
}

#[test]
fn a_view_whose_pipeline_panics_is_held_back_on_its_own() {
    let rig = Rig::new();
    let source = Signal::new(todos(3));
    let view = Lazy::over(
        &source
            .derive()
            .filter(|t: &Todo| {
                assert!(t.title != "boom", "filter failure");
                true
            })
            .build(),
    );
    let tag = Signal::new(0_u32);
    rig.cell.attach_keyed(&source, 0, todo_key).unwrap();
    rig.cell.attach_lazy(&view, 1).unwrap();
    rig.cell.attach(&tag, 2).unwrap();
    rig.observe_all();

    let result = catch_unwind(AssertUnwindSafe(|| {
        rig.run(|| {
            txn(|| {
                source.update_at(1, |t| t.title = "boom".into());
                tag.set(1);
            })
        })
    }));
    assert!(
        result.is_ok(),
        "the write that triggered the commit succeeds"
    );
    assert_eq!(
        ids(&rig.one_set()),
        [0, 2],
        "the rest of the store is delivered"
    );
    assert!(rig.cell.is_failed(1));
    assert!(rig.cell.failed_signals()[0].1.contains("filter failure"));

    // The input changes so that the pipeline succeeds: the view is announced again.
    rig.run(|| source.update_at(1, |t| t.title = "fine".into()));
    let set = rig.one_set();
    assert_eq!(invalidated(entry(&set, 1)).len, 3);
    assert!(!rig.cell.is_failed(1));
}

/// Review (types-paging): a transaction of more recorded operations than the derived index keeps
/// for one commit (4,096) rebuilds the index. A page read before it (one "in flight" on the host)
/// carries the older version, the commit announces a newer one with the new length, and a page
/// read after it is at that version and agrees with the model.
#[test]
fn a_view_whose_derived_index_rebuilds_announces_a_newer_version_than_a_page_in_flight() {
    let f = view_fixture(200);
    f.rig.observe_all();
    let source = f.rig.cell.lazy_sources().remove(0).1;
    let announced = value(&f.rig.observe_on(1)[0]);
    let (before, _) = page(&*source, 0, 50);
    assert_eq!(before.version, announced.version);

    f.rig.run(|| {
        txn(|| {
            for i in 0..5_000_u32 {
                f.source
                    .push(todo(10_000 + i, &format!("r{i:05}"), i % 3 == 0));
            }
        });
    });
    let expected = model(&f.source.get());
    let set = f.rig.one_set();
    let inv = invalidated(entry(&set, 1));
    assert_eq!(inv.len as usize, expected.len());
    assert!(
        inv.version > before.version,
        "the page read before the rebuild is stale to the host"
    );
    for (offset, limit) in [(0, 50), (1_000, 50), (expected.len() as u32 - 10, 50)] {
        let (header, rows) = page(&*source, offset, limit);
        assert_eq!(header.version, inv.version);
        assert_eq!(header.total as usize, expected.len());
        assert_eq!(
            rows,
            expected[page_window(expected.len(), offset, limit)].to_vec()
        );
    }
}

#[derive(Clone, Debug)]
enum Write {
    Push(u32, bool),
    Insert(usize, u32, bool),
    Remove(usize),
    Update(usize, bool),
    Rename(usize, u8),
    Move(usize, usize),
    Clear,
    Set(u8),
}

fn write() -> impl Strategy<Value = Write> {
    prop_oneof![
        4 => (0_u32..1_000, any::<bool>()).prop_map(|(id, d)| Write::Push(id, d)),
        3 => (0_usize..80, 0_u32..1_000, any::<bool>()).prop_map(|(a, id, d)| Write::Insert(a, id, d)),
        3 => (0_usize..80).prop_map(Write::Remove),
        3 => (0_usize..80, any::<bool>()).prop_map(|(a, d)| Write::Update(a, d)),
        3 => (0_usize..80, 0_u8..6).prop_map(|(a, t)| Write::Rename(a, t)),
        2 => (0_usize..80, 0_usize..80).prop_map(|(a, b)| Write::Move(a, b)),
        1 => Just(Write::Clear),
        1 => (0_u8..40).prop_map(Write::Set),
    ]
}

fn titled(id: u32, title: u8, done: bool) -> Todo {
    todo(id, &format!("k{title}"), done)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, ..ProptestConfig::default() })]

    /// Pages of a view agree with `stable_sort(filter(source))` after every write, however the
    /// writes are batched into transactions and whenever the index is read in between; every
    /// change that moves the view is announced with the right length, and the version a page
    /// reports is the version the last announcement carried.
    #[test]
    fn pages_of_a_view_follow_a_naive_model_through_live_changes(
        writes in proptest::collection::vec((write(), 0_u8..3), 1..60),
        windows in proptest::collection::vec((0_u32..70, 0_u32..40), 1..4),
    ) {
        let f = view_fixture_keyed(12, false);
        f.rig.observe_all();
        let source = f.rig.cell.lazy_sources().remove(0).1;
        let mut announced = value(&f.rig.observe_on(1)[0]);
        let next_id = std::cell::Cell::new(10_000_u32);
        for (w, batch) in writes {
            let apply = |w: &Write| {
                let len = f.source.with(Vec::len);
                match *w {
                    Write::Push(id, d) => f.source.push(titled(id, (id % 6) as u8, d)),
                    Write::Insert(at, id, d) => f.source.insert(at % (len + 1), titled(id, (id % 6) as u8, d)),
                    Write::Remove(at) if len > 0 => { f.source.remove(at % len); }
                    Write::Update(at, d) if len > 0 => f.source.update_at(at % len, |t| t.done = d),
                    Write::Rename(at, t) if len > 0 => f.source.update_at(at % len, |x| x.title = format!("k{t}")),
                    Write::Move(a, b) if len > 0 => f.source.move_item(a % len, b % len),
                    Write::Clear => f.source.clear(),
                    Write::Set(n) => f.source.set((0..u32::from(n)).map(|i| {
                        next_id.set(next_id.get() + 1);
                        titled(next_id.get(), (i % 6) as u8, i % 4 == 0)
                    }).collect()),
                    _ => {}
                }
            };
            // `batch` 0: one write per transaction, 1: the write twice in one transaction, 2: read
            // the index (a page) between the writes of one transaction.
            f.rig.run(|| match batch {
                0 => apply(&w),
                1 => txn(|| { apply(&w); apply(&Write::Push(next_id.get(), false)); }),
                _ => txn(|| { apply(&w); let _ = page(&*source, 0, 5); apply(&Write::Clear); apply(&w); }),
            });
            let expected = model(&f.source.get());
            for set in f.rig.sets() {
                for e in set.entries.iter().filter(|e| e.signal_id == 1) {
                    let inv = invalidated(e);
                    prop_assert_eq!(inv.len as usize, expected.len());
                    prop_assert!(inv.version >= announced.version);
                    announced.version = inv.version;
                    announced.len = inv.len;
                }
            }
            prop_assert_eq!(announced.len as usize, expected.len(), "the host's length is current");
            for &(offset, limit) in &windows {
                let (header, rows) = page(&*source, offset, limit);
                let window = page_window(expected.len(), offset, limit);
                prop_assert_eq!(header.total as usize, expected.len());
                prop_assert_eq!(header.version, announced.version, "the page is at the announced version");
                prop_assert_eq!(rows, expected[window].to_vec());
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Several lazy lists, restore-shaped stores
// ---------------------------------------------------------------------------------------------

#[test]
fn a_store_may_hold_several_lazy_lists_each_with_its_own_page_server() {
    let rig = Rig::new();
    let a = Lazy::from_vec(vec![1_u32, 2]);
    let n = Signal::new(0_u8);
    let b = Lazy::from_vec(vec![String::from("x")]);
    rig.cell.attach_lazy(&a, 0).unwrap();
    rig.cell.attach(&n, 1).unwrap();
    rig.cell.attach_lazy(&b, 2).unwrap();
    let sources = rig.cell.lazy_sources();
    assert_eq!(
        sources.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        [0, 2]
    );
    rig.cell.set_lazy_handle(0, 100);
    rig.cell.set_lazy_handle(2, 200);
    rig.observe_all();
    rig.run(|| {
        txn(|| {
            a.push(3);
            b.push(String::from("y"));
        });
    });
    let set = rig.one_set();
    assert_eq!(ids(&set), [0, 2]);
    assert_eq!(invalidated(entry(&set, 0)).len, 3);
    assert_eq!(invalidated(entry(&set, 2)).len, 2);
}
