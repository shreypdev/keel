//! Derived keyed lists (ADR-039), against the reference they promise:
//!
//! ```text
//! view(source) = stable_sort_by_key( [ out(t) for t in source if passes(t) ] )
//! ```
//!
//! A model-based property test: one store with a keyed source, four attached derived lists (a
//! filter; a filter and a sort with many equal keys; a map to another record and a sort on a
//! `String`; a filter with a parameter signal) and a `count()`, driven by arbitrary interleavings
//! of every recorded operation, raw writes, transactions mixing them, parameter writes, observe and
//! unobserve per slot, Rust reads inside and outside transactions, commits abandoned by a panicking
//! encoder or sink, and a computed of the same store that panics (isolated, ADR-019 amendment). A
//! host mirror applies every change-set (patches by SPEC 3.8). After every step, for every observed
//! view: host == reference == `DerivedList::get()`; and a transaction of recorded operations only,
//! with no parameter change, reached every healthy view as a patch (no full value).
//!
//! `UNDRA_DERIVED_CASES` sets the number of cases (default 1,500).

mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use common::{EncodeBomb, HANDLE, observe};
use proptest::prelude::*;
use undra_signals::testing::CaptureSink;
use undra_signals::{
    ALL_SIGNALS, ChangeSink, Computed, DerivedList, Signal, StoreCell, txn, with_sink,
};
use undra_wire::payload::{ChangeEntry, ChangeOp, ChangeSet};
use undra_wire::{Decode, Encode, KeyedPatch, Reader, WireError, Writer};

// ---------------------------------------------------------------------------------------------
// Rows, views and the reference
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Row {
    id: u32,
    title: String,
    done: bool,
    rank: u8,
}

impl Encode for Row {
    fn encode(&self, w: &mut Writer) {
        self.id.encode(w);
        self.title.encode(w);
        self.done.encode(w);
        self.rank.encode(w);
    }
}

impl Decode for Row {
    const MIN_ENCODED_LEN: usize = 10;
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Row {
            id: u32::decode(r)?,
            title: String::decode(r)?,
            done: bool::decode(r)?,
            rank: u8::decode(r)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Label {
    id: u32,
    text: String,
}

impl Encode for Label {
    fn encode(&self, w: &mut Writer) {
        self.id.encode(w);
        self.text.encode(w);
    }
}

impl Decode for Label {
    const MIN_ENCODED_LEN: usize = 8;
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Label {
            id: u32::decode(r)?,
            text: String::decode(r)?,
        })
    }
}

fn row_key(row: &Row) -> u64 {
    u64::from(row.id)
}

fn label_key(label: &Label) -> u64 {
    u64::from(label.id)
}

fn label_of(row: &Row) -> Label {
    Label {
        id: row.id,
        text: format!("{}{}", row.title, row.id % 3),
    }
}

/// `filter + stable sort`, written the obvious way: the reference every view is checked against.
fn ref_open(rows: &[Row]) -> Vec<Row> {
    rows.iter().filter(|r| !r.done).cloned().collect()
}

fn ref_ties(rows: &[Row]) -> Vec<Row> {
    let mut v: Vec<Row> = rows.iter().filter(|r| r.rank != 0).cloned().collect();
    v.sort_by_key(|r| r.rank % 4);
    v
}

fn ref_labels(rows: &[Row]) -> Vec<Label> {
    let mut v: Vec<Label> = rows.iter().map(label_of).collect();
    v.sort_by_key(|l| l.text.clone());
    v
}

fn ref_param(rows: &[Row], show: u8) -> Vec<Row> {
    rows.iter().filter(|r| r.rank < show).cloned().collect()
}

fn ref_count(rows: &[Row]) -> u32 {
    u32::try_from(rows.iter().filter(|r| r.done).count()).unwrap()
}

// ---------------------------------------------------------------------------------------------
// The core
// ---------------------------------------------------------------------------------------------

const ROWS: u32 = 0;
const SHOW: u32 = 1;
const OPEN: u32 = 2;
const TIES: u32 = 3;
const LABELS: u32 = 4;
const PARAM: u32 = 5;
const COUNT: u32 = 6;
const BOMB: u32 = 7;
const FLAKY: u32 = 8;
const SLOTS: usize = 9;
const VIEWS: [u32; 4] = [OPEN, TIES, LABELS, PARAM];

/// A sink that panics on demand (an abandoned delivery, ADR-019 H1), else captures.
struct Sink {
    capture: Arc<CaptureSink>,
    panic: AtomicBool,
}

impl ChangeSink for Sink {
    fn deliver(&self, change_set: &[u8]) {
        assert!(!self.panic.load(Ordering::SeqCst), "sink failure");
        self.capture.deliver(change_set);
    }
}

struct Core {
    cell: Arc<StoreCell>,
    rows: Signal<Vec<Row>>,
    show: Signal<u8>,
    open: DerivedList<Row>,
    ties: DerivedList<Row>,
    labels: DerivedList<Label>,
    param: DerivedList<Row>,
    count: Computed<u32>,
    bomb_armed: Arc<AtomicBool>,
    flaky_armed: Arc<AtomicBool>,
    sink: Arc<Sink>,
    next_id: std::cell::Cell<u32>,
}

fn core() -> Core {
    let cell = StoreCell::new(0xD0E5);
    cell.set_handle(HANDLE);
    let initial: Vec<Row> = (1..=6)
        .map(|id| Row {
            id,
            title: ["a", "b", "c"][id as usize % 3].into(),
            done: id % 2 == 0,
            rank: (id % 5) as u8,
        })
        .collect();
    let rows = Signal::new(initial);
    let show = Signal::new(2_u8);
    let open = rows.derive().filter(|r: &Row| !r.done).build();
    let ties = rows
        .derive()
        .filter(|r: &Row| r.rank != 0)
        .sort_by_key(|r: &Row| r.rank % 4)
        .build();
    let labels = rows
        .derive()
        .map(label_of)
        .sort_by_key(|l: &Label| l.text.clone())
        .build();
    let param = rows
        .derive()
        .filter_with(&show, |show, r: &Row| r.rank < *show)
        .build();
    let count = rows.derive().filter(|r: &Row| r.done).count();
    let bomb_armed = Arc::new(AtomicBool::new(false));
    let bomb = Signal::new(EncodeBomb::new(&bomb_armed, 0));
    let flaky_armed = Arc::new(AtomicBool::new(false));
    let flaky = {
        let armed = Arc::clone(&flaky_armed);
        Computed::new(&rows, move |l: &Vec<Row>| {
            assert!(!armed.load(Ordering::SeqCst), "computed failure");
            u32::try_from(l.len()).unwrap()
        })
    };
    cell.attach_keyed(&rows, ROWS, row_key).unwrap();
    cell.attach(&show, SHOW).unwrap();
    cell.attach_derived(&open, OPEN, row_key).unwrap();
    cell.attach_derived(&ties, TIES, row_key).unwrap();
    cell.attach_derived(&labels, LABELS, label_key).unwrap();
    cell.attach_derived(&param, PARAM, row_key).unwrap();
    cell.attach_computed(&count, COUNT).unwrap();
    // Touched by every step that aborts, so the abort covers the views it shares a commit with.
    cell.attach(&bomb, BOMB).unwrap();
    cell.attach_computed(&flaky, FLAKY).unwrap();
    // Keep the bomb reachable for `Core::touch_bomb`.
    BOMB_SIGNAL.with(|slot| *slot.borrow_mut() = Some(bomb));
    Core {
        cell,
        rows,
        show,
        open,
        ties,
        labels,
        param,
        count,
        bomb_armed,
        flaky_armed,
        sink: Arc::new(Sink {
            capture: CaptureSink::new(),
            panic: AtomicBool::new(false),
        }),
        next_id: std::cell::Cell::new(100),
    }
}

thread_local! {
    static BOMB_SIGNAL: std::cell::RefCell<Option<Signal<EncodeBomb>>> = const { std::cell::RefCell::new(None) };
}

fn touch_bomb() {
    BOMB_SIGNAL.with(|slot| {
        if let Some(bomb) = slot.borrow().as_ref() {
            bomb.update(|b| b.value = b.value.wrapping_add(1));
        }
    });
}

// ---------------------------------------------------------------------------------------------
// What the host knows
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct Mirror {
    observed: [bool; SLOTS],
    open: Vec<Row>,
    ties: Vec<Row>,
    labels: Vec<Label>,
    param: Vec<Row>,
    count: u32,
    last_txn: u64,
}

fn apply_list<T: Decode + Clone>(list: &mut Vec<T>, e: &ChangeEntry) {
    match e.op {
        ChangeOp::Full => *list = Vec::<T>::decode_exact(&e.value).expect("a full value decodes"),
        ChangeOp::KeyedPatch => {
            let mut r = Reader::new(&e.value);
            let patch = KeyedPatch::<T>::decode(&mut r).expect("a patch decodes");
            r.finish().expect("no trailing bytes");
            patch
                .apply(list)
                .expect("a delivered patch always applies to what the host has");
        }
        other => panic!("unexpected op {other:?} for signal {}", e.signal_id),
    }
}

impl Mirror {
    fn apply_entry(&mut self, e: &ChangeEntry) {
        match e.signal_id {
            OPEN => apply_list(&mut self.open, e),
            TIES => apply_list(&mut self.ties, e),
            LABELS => apply_list(&mut self.labels, e),
            PARAM => apply_list(&mut self.param, e),
            COUNT => self.count = u32::decode_exact(&e.value).unwrap(),
            ROWS | SHOW | BOMB | FLAKY => {}
            other => panic!("unexpected signal {other}"),
        }
    }

    fn apply_set(&mut self, set: &ChangeSet) {
        assert!(set.txn_id > self.last_txn, "commit order");
        self.last_txn = set.txn_id;
        assert!(
            !set.entries.is_empty(),
            "a change-set with no entry is not delivered"
        );
        let mut seen = [false; SLOTS];
        let mut last = None;
        for e in &set.entries {
            let id = e.signal_id as usize;
            assert!(!seen[id], "one entry per signal");
            seen[id] = true;
            assert!(last < Some(e.signal_id), "entries are ordered by signal id");
            last = Some(e.signal_id);
            assert!(self.observed[id], "an unobserved signal was delivered");
            self.apply_entry(e);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Steps
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Change {
    Toggle,
    Rank(u8),
    Title(u8),
    All(u8),
}

#[derive(Clone, Debug)]
enum Edit {
    // Recorded.
    Push(u8),
    Insert(usize, u8),
    Remove(usize),
    UpdateAt(usize, Change),
    Move(usize, usize),
    Clear,
    // Raw.
    RawToggle(usize),
    Set(Vec<u8>),
    Replace(Vec<u8>),
    // A parameter.
    Show(u8),
    // A Rust read inside the transaction.
    Read(u8),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Recorded,
    Raw,
    Param,
    Nothing,
}

impl Core {
    fn row(&self, seed: u8) -> Row {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        Row {
            id,
            title: ["a", "b", "c", "d"][seed as usize % 4].into(),
            done: seed % 3 == 0,
            rank: seed % 5,
        }
    }

    fn edit(&self, e: &Edit) -> Kind {
        let len = self.rows.with(Vec::len);
        match e {
            Edit::Push(seed) => {
                self.rows.push(self.row(*seed));
                Kind::Recorded
            }
            Edit::Insert(at, seed) => {
                self.rows.insert(at % (len + 1), self.row(*seed));
                Kind::Recorded
            }
            Edit::Remove(at) if len > 0 => {
                self.rows.remove(at % len);
                Kind::Recorded
            }
            Edit::UpdateAt(at, change) if len > 0 => {
                self.rows.update_at(at % len, |r| match *change {
                    Change::Toggle => r.done = !r.done,
                    Change::Rank(k) => r.rank = k % 5,
                    Change::Title(t) => r.title = ["a", "b", "c", "d"][t as usize % 4].into(),
                    Change::All(s) => {
                        r.done = !r.done;
                        r.rank = s % 5;
                        r.title = ["d", "c", "b", "a"][s as usize % 4].into();
                    }
                });
                Kind::Recorded
            }
            Edit::Move(a, b) if len > 0 => {
                self.rows.move_item(a % len, b % len);
                Kind::Recorded
            }
            Edit::Clear => {
                self.rows.clear();
                Kind::Recorded
            }
            Edit::Remove(_) | Edit::UpdateAt(..) | Edit::Move(..) => Kind::Nothing,
            Edit::RawToggle(at) => {
                self.rows.update(|l| {
                    if !l.is_empty() {
                        let at = at % l.len();
                        l[at].done = !l[at].done;
                    }
                });
                Kind::Raw
            }
            Edit::Set(seeds) => {
                let rows = seeds.iter().map(|s| self.row(*s)).collect();
                self.rows.set(rows);
                Kind::Raw
            }
            Edit::Replace(seeds) => {
                let rows = seeds.iter().map(|s| self.row(*s)).collect();
                self.rows.replace(rows);
                Kind::Raw
            }
            Edit::Show(v) => {
                let before = self.show.get();
                self.show.set(*v % 6);
                if before == *v % 6 {
                    Kind::Nothing
                } else {
                    Kind::Param
                }
            }
            Edit::Read(which) => {
                self.check_reads(*which);
                Kind::Nothing
            }
        }
    }

    /// Every Rust read agrees with the reference, inside a transaction or not.
    fn check_reads(&self, which: u8) {
        let rows = self.rows.get();
        let show = self.show.get();
        match which % 5 {
            0 => {
                assert_eq!(self.open.get(), ref_open(&rows));
                assert_eq!(self.open.len(), ref_open(&rows).len());
            }
            1 => self.ties.with(|v| assert_eq!(v, &ref_ties(&rows))),
            2 => {
                assert_eq!(self.labels.get(), ref_labels(&rows));
                assert_eq!(self.labels.is_empty(), rows.is_empty());
            }
            3 => assert_eq!(self.param.get(), ref_param(&rows, show)),
            _ => assert_eq!(self.count.get(), ref_count(&rows)),
        }
    }

    fn total_full_values(&self) -> [u64; 4] {
        [
            self.open.stats().full_values,
            self.ties.stats().full_values,
            self.labels.stats().full_values,
            self.param.stats().full_values,
        ]
    }
}

#[derive(Clone, Debug)]
enum Step {
    Edit(Edit),
    Txn(Vec<Edit>),
    /// A transaction whose commit an encoder abandons.
    Abort(Vec<Edit>),
    /// A transaction whose delivery the sink abandons.
    SinkPanic(Vec<Edit>),
    /// A transaction during which a computed of the store panics (isolated).
    Flaky(Vec<Edit>),
    ObserveOn(u32),
    ObserveOff(u32),
    ObserveAllOn,
    ObserveAllOff,
    Read(u8),
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![
        3 => Just(Change::Toggle),
        2 => any::<u8>().prop_map(Change::Rank),
        2 => any::<u8>().prop_map(Change::Title),
        1 => any::<u8>().prop_map(Change::All),
    ]
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        4 => any::<u8>().prop_map(Edit::Push),
        3 => (0_usize..40, any::<u8>()).prop_map(|(a, s)| Edit::Insert(a, s)),
        3 => (0_usize..40).prop_map(Edit::Remove),
        6 => (0_usize..40, change()).prop_map(|(a, c)| Edit::UpdateAt(a, c)),
        3 => (0_usize..40, 0_usize..40).prop_map(|(a, b)| Edit::Move(a, b)),
        1 => Just(Edit::Clear),
        1 => (0_usize..40).prop_map(Edit::RawToggle),
        1 => proptest::collection::vec(any::<u8>(), 0..8).prop_map(Edit::Set),
        1 => proptest::collection::vec(any::<u8>(), 0..8).prop_map(Edit::Replace),
        2 => any::<u8>().prop_map(Edit::Show),
        1 => any::<u8>().prop_map(Edit::Read),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        8 => edit().prop_map(Step::Edit),
        6 => proptest::collection::vec(edit(), 1..7).prop_map(Step::Txn),
        1 => proptest::collection::vec(edit(), 1..4).prop_map(Step::Abort),
        1 => proptest::collection::vec(edit(), 1..4).prop_map(Step::SinkPanic),
        1 => proptest::collection::vec(edit(), 1..4).prop_map(Step::Flaky),
        2 => (0_u32..SLOTS as u32).prop_map(Step::ObserveOn),
        2 => (0_u32..SLOTS as u32).prop_map(Step::ObserveOff),
        1 => Just(Step::ObserveAllOn),
        1 => Just(Step::ObserveAllOff),
        1 => any::<u8>().prop_map(Step::Read),
    ]
}

/// Drives a script against the core and the mirror.
fn run_script(steps: &[Step]) {
    let core = core();
    let mut mirror = Mirror::default();
    // Per view: the host holds a copy the core's patches apply to (observed, and no delivery of it
    // was abandoned since).
    let mut healthy = [false; SLOTS];

    let observe_step = |mirror: &mut Mirror, healthy: &mut [bool; SLOTS], id: u32, on: bool| {
        let entries = observe(&core.cell, id, on);
        let ids: Vec<u32> = if id == ALL_SIGNALS {
            (0..SLOTS as u32).collect()
        } else {
            vec![id]
        };
        for i in &ids {
            mirror.observed[*i as usize] = on;
            healthy[*i as usize] = on;
        }
        for e in &entries {
            assert_eq!(e.op, ChangeOp::Full, "observe sends full values");
            mirror.apply_entry(e);
        }
    };
    observe_step(&mut mirror, &mut healthy, ALL_SIGNALS, true);

    for step in steps {
        let (edits, abort, sink_panic, flaky): (&[Edit], bool, bool, bool) = match step {
            Step::Edit(e) => (std::slice::from_ref(e), false, false, false),
            Step::Txn(es) => (es, false, false, false),
            Step::Abort(es) => (es, true, false, false),
            Step::SinkPanic(es) => (es, false, true, false),
            Step::Flaky(es) => (es, false, false, true),
            Step::ObserveOn(id) => {
                observe_step(&mut mirror, &mut healthy, *id, true);
                (&[], false, false, false)
            }
            Step::ObserveOff(id) => {
                observe_step(&mut mirror, &mut healthy, *id, false);
                (&[], false, false, false)
            }
            Step::ObserveAllOn => {
                observe_step(&mut mirror, &mut healthy, ALL_SIGNALS, true);
                (&[], false, false, false)
            }
            Step::ObserveAllOff => {
                observe_step(&mut mirror, &mut healthy, ALL_SIGNALS, false);
                (&[], false, false, false)
            }
            Step::Read(which) => {
                core.check_reads(*which);
                (&[], false, false, false)
            }
        };

        if !edits.is_empty() {
            let full_before = core.total_full_values();
            let mut kinds = Vec::new();
            core.bomb_armed.store(abort, Ordering::SeqCst);
            core.sink.panic.store(sink_panic, Ordering::SeqCst);
            core.flaky_armed.store(flaky, Ordering::SeqCst);
            let result = catch_unwind(AssertUnwindSafe(|| {
                with_sink(core.sink.clone(), || {
                    txn(|| {
                        for e in edits {
                            kinds.push(core.edit(e));
                        }
                        if abort {
                            touch_bomb();
                        }
                    });
                });
            }));
            core.bomb_armed.store(false, Ordering::SeqCst);
            core.sink.panic.store(false, Ordering::SeqCst);
            core.flaky_armed.store(false, Ordering::SeqCst);
            let sets = core.sink.capture.take_decoded();
            let aborted = result.is_err();
            if aborted {
                // Only an observed bomb or an observed slot reaching the panicking sink aborts.
                assert!(
                    abort || sink_panic,
                    "only the bomb or the sink abandon a commit"
                );
                assert!(sets.is_empty(), "an abandoned commit sends nothing");
                healthy = [false; SLOTS];
            } else {
                // The claim: views that were healthy and saw only recorded operations (no raw
                // write, no parameter change) were sent as patches, never as full values.
                let recorded_only = kinds
                    .iter()
                    .all(|k| matches!(k, Kind::Recorded | Kind::Nothing));
                let full_after = core.total_full_values();
                for (i, view) in VIEWS.iter().enumerate() {
                    if recorded_only && healthy[*view as usize] {
                        assert_eq!(
                            full_after[i], full_before[i],
                            "view {view} was sent in full after recorded operations only: {step:?}"
                        );
                    }
                }
                for set in &sets {
                    mirror.apply_set(set);
                }
                if flaky {
                    // The panicking computed is held back on its own; the views were delivered.
                    let failed = core.cell.failed_signals();
                    assert!(failed.iter().all(|(id, _)| *id == FLAKY), "{failed:?}");
                }
            }

            if aborted {
                // The next commit that touches the store sends what the abort held back, in full.
                with_sink(core.sink.clone(), || core.rows.push(core.row(1)));
                for set in core.sink.capture.take_decoded() {
                    mirror.apply_set(&set);
                }
                for view in VIEWS {
                    healthy[view as usize] = mirror.observed[view as usize];
                }
            }
        }

        check_mirror(&core, &mirror, step);
    }

    // A final resynchronisation always brings the host up to date.
    observe_step(&mut mirror, &mut healthy, ALL_SIGNALS, true);
    check_mirror(&core, &mirror, &Step::ObserveAllOn);
    for view in [&core.open, &core.ties, &core.param] {
        assert!(view.is_attached());
    }
}

/// Every observed view: host == reference == `get()`.
fn check_mirror(core: &Core, mirror: &Mirror, step: &Step) {
    let rows = core.rows.get();
    let show = core.show.get();
    if mirror.observed[OPEN as usize] {
        assert_eq!(mirror.open, ref_open(&rows), "open after {step:?}");
    }
    if mirror.observed[TIES as usize] {
        assert_eq!(mirror.ties, ref_ties(&rows), "ties after {step:?}");
    }
    if mirror.observed[LABELS as usize] {
        assert_eq!(mirror.labels, ref_labels(&rows), "labels after {step:?}");
    }
    if mirror.observed[PARAM as usize] {
        assert_eq!(mirror.param, ref_param(&rows, show), "param after {step:?}");
    }
    if mirror.observed[COUNT as usize] {
        assert_eq!(mirror.count, ref_count(&rows), "count after {step:?}");
    }
    assert_eq!(core.open.get(), ref_open(&rows));
    assert_eq!(core.ties.get(), ref_ties(&rows));
    assert_eq!(core.labels.get(), ref_labels(&rows));
    assert_eq!(core.param.get(), ref_param(&rows, show));
    assert_eq!(core.count.get(), ref_count(&rows));
}

fn cases() -> u32 {
    std::env::var("UNDRA_DERIVED_CASES")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(1_500)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), ..ProptestConfig::default() })]

    #[test]
    fn every_observed_view_equals_filter_and_stable_sort_of_the_source(
        steps in proptest::collection::vec(step(), 1..40)
    ) {
        run_script(&steps);
    }
}

#[test]
fn a_fixed_script_through_every_path() {
    use Edit::*;
    run_script(&[
        Step::Edit(Push(7)),
        Step::Txn(vec![
            Insert(1, 8),
            UpdateAt(0, Change::Toggle),
            Move(0, 2),
            Remove(1),
            Show(4),
            Read(0),
        ]),
        Step::Txn(vec![Clear, Push(1), RawToggle(0), Push(2)]),
        Step::Abort(vec![Push(3), Remove(0)]),
        Step::SinkPanic(vec![UpdateAt(0, Change::All(9))]),
        Step::Flaky(vec![Push(4), UpdateAt(1, Change::Rank(3))]),
        Step::Edit(Push(4)),
        Step::ObserveOff(TIES),
        Step::Edit(Push(5)),
        Step::Edit(Clear),
        Step::ObserveOn(TIES),
        Step::Edit(Push(6)),
        Step::Txn(vec![Replace(vec![6, 7, 8]), Push(8), Show(1)]),
        Step::Txn(vec![
            Push(9),
            Move(0, 2),
            UpdateAt(1, Change::Title(2)),
            Read(2),
        ]),
        Step::ObserveAllOff,
        Step::Edit(Push(10)),
        Step::Read(3),
        Step::ObserveAllOn,
        Step::Edit(Move(0, 1)),
    ]);
}
