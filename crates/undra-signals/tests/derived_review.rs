//! Derived keyed lists (ADR-039) under the adversarial review of 2026-10-02: the shapes the first
//! model test (`tests/derived.rs`) does not reach.
//!
//! * A second model, with its own views: four views and a `Computed` over one source, one of them
//!   parameterised by a `count()` of the same source (the parameter changes whenever a row's
//!   membership in the count does), one with both a filter parameter and a sort parameter, and
//!   transactions of thousands of operations, read part-way or not, so that a tap overflows and
//!   operations keep arriving after it did, inside one transaction (the rebuild path in the drain
//!   that meets them), and so that the derived ops overflow while the tap does not.
//! * Exact boundaries the model cannot aim at: a parameter walk of 256 and 257 ops on top of
//!   replayed source ops in the same drain, for a filter (inserts and removes) and a sort key
//!   (moves); a row that moves, changes and is removed in one transaction; duplicate keys.
//! * Read-your-writes: what a Rust read inside a transaction sees.
//! * A drain that is part-way through its walk while another thread writes the source and drops
//!   every other handle (the store, the view's clones): writers never wait for a drain.
//!
//! `UNDRA_DERIVED_REVIEW_CASES` sets the model's number of cases (default 200).

mod common;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};

use common::{HANDLE, Rig, entry, observe, patch_of, value_of};
use proptest::prelude::*;
use undra_signals::testing::CaptureSink;
use undra_signals::{ALL_SIGNALS, Computed, DerivedList, Signal, StoreCell, txn, with_sink};
use undra_wire::payload::{ChangeEntry, ChangeOp, ChangeSet};
use undra_wire::{Decode, Encode, KeyedPatch, PatchOp, Reader, WireError, Writer};

const TAP_LIMIT: usize = 4096;
const OUT_LIMIT: usize = 4096;
const PARAM_WALK_LIMIT: usize = 256;

// ---------------------------------------------------------------------------------------------
// Rows and references
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
struct Item {
    id: u32,
    grp: u8,
    done: bool,
    score: u16,
}

impl Encode for Item {
    fn encode(&self, w: &mut Writer) {
        self.id.encode(w);
        self.grp.encode(w);
        self.done.encode(w);
        self.score.encode(w);
    }
}

impl Decode for Item {
    const MIN_ENCODED_LEN: usize = 8;
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Item {
            id: u32::decode(r)?,
            grp: u8::decode(r)?,
            done: bool::decode(r)?,
            score: u16::decode(r)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Tag {
    id: u32,
    text: String,
}

impl Encode for Tag {
    fn encode(&self, w: &mut Writer) {
        self.id.encode(w);
        self.text.encode(w);
    }
}

impl Decode for Tag {
    const MIN_ENCODED_LEN: usize = 8;
    fn decode(r: &mut Reader<'_>) -> Result<Self, WireError> {
        Ok(Tag {
            id: u32::decode(r)?,
            text: String::decode(r)?,
        })
    }
}

fn item_key(item: &Item) -> u64 {
    u64::from(item.id)
}

fn tag_of(item: &Item) -> Tag {
    Tag {
        id: item.id,
        text: format!("{}:{}", item.grp, item.score % 5),
    }
}

fn sort_key(dir: bool, item: &Item) -> u16 {
    if dir {
        item.score
    } else {
        u16::MAX - item.score
    }
}

fn ref_done_count(rows: &[Item]) -> u32 {
    u32::try_from(rows.iter().filter(|r| r.done).count()).unwrap()
}

fn ref_ties(rows: &[Item]) -> Vec<Item> {
    let mut v: Vec<Item> = rows.iter().filter(|r| !r.done).cloned().collect();
    v.sort_by_key(|r| r.score % 3);
    v
}

fn ref_quota(rows: &[Item]) -> Vec<Item> {
    let count = ref_done_count(rows);
    let mut v: Vec<Item> = rows
        .iter()
        .filter(|r| u32::from(r.grp) <= count)
        .cloned()
        .collect();
    v.sort_by_key(|r| r.grp);
    v
}

fn ref_sortp(rows: &[Item], cut: u16, dir: bool) -> Vec<Item> {
    let mut v: Vec<Item> = rows.iter().filter(|r| r.score < cut).cloned().collect();
    v.sort_by_key(|r| sort_key(dir, r));
    v
}

fn ref_tags(rows: &[Item]) -> Vec<Tag> {
    rows.iter().map(tag_of).collect()
}

// ---------------------------------------------------------------------------------------------
// The second model
// ---------------------------------------------------------------------------------------------

const ROWS: u32 = 0;
const CUT: u32 = 1;
const DIR: u32 = 2;
const DONE: u32 = 3;
const TIES: u32 = 4;
const QUOTA: u32 = 5;
const SORTP: u32 = 6;
const TAGS: u32 = 7;
const TIES_LEN: u32 = 8;
const SLOTS: usize = 9;
const VIEWS: [u32; 4] = [TIES, QUOTA, SORTP, TAGS];

struct Core {
    cell: Arc<StoreCell>,
    rows: Signal<Vec<Item>>,
    cut: Signal<u16>,
    dir: Signal<bool>,
    done: Computed<u32>,
    ties: DerivedList<Item>,
    quota: DerivedList<Item>,
    sortp: DerivedList<Item>,
    tags: DerivedList<Tag>,
    ties_len: Computed<u32>,
    sink: Arc<CaptureSink>,
    next_id: std::cell::Cell<u32>,
}

fn core() -> Core {
    let cell = StoreCell::new(0xBEEF);
    cell.set_handle(HANDLE);
    let initial: Vec<Item> = (1..=8)
        .map(|id| Item {
            id,
            grp: (id % 4) as u8,
            done: id % 3 == 0,
            score: (id * 7 % 20) as u16,
        })
        .collect();
    let rows = Signal::new(initial);
    let cut = Signal::new(12_u16);
    let dir = Signal::new(true);
    let done = rows.derive().filter(|r: &Item| r.done).count();
    let ties = rows
        .derive()
        .filter(|r: &Item| !r.done)
        .sort_by_key(|r: &Item| r.score % 3)
        .build();
    // Parameterised by a count of its own source.
    let quota = rows
        .derive()
        .filter_with(&done, |count: &u32, r: &Item| u32::from(r.grp) <= *count)
        .sort_by_key(|r: &Item| r.grp)
        .build();
    let sortp = rows
        .derive()
        .filter_with(&cut, |cut: &u16, r: &Item| r.score < *cut)
        .sort_by_key_with(&dir, |dir: &bool, r: &Item| sort_key(*dir, r))
        .build();
    let tags = rows.derive().map(tag_of).build();
    let ties_len = Computed::new(&ties, |v: &Vec<Item>| u32::try_from(v.len()).unwrap());
    cell.attach_keyed(&rows, ROWS, item_key).unwrap();
    cell.attach(&cut, CUT).unwrap();
    cell.attach(&dir, DIR).unwrap();
    cell.attach_computed(&done, DONE).unwrap();
    cell.attach_derived(&ties, TIES, item_key).unwrap();
    cell.attach_derived(&quota, QUOTA, item_key).unwrap();
    cell.attach_derived(&sortp, SORTP, item_key).unwrap();
    cell.attach_derived(&tags, TAGS, |t: &Tag| u64::from(t.id))
        .unwrap();
    cell.attach_computed(&ties_len, TIES_LEN).unwrap();
    Core {
        cell,
        rows,
        cut,
        dir,
        done,
        ties,
        quota,
        sortp,
        tags,
        ties_len,
        sink: CaptureSink::new(),
        next_id: std::cell::Cell::new(100),
    }
}

#[derive(Default)]
struct Mirror {
    observed: [bool; SLOTS],
    ties: Vec<Item>,
    quota: Vec<Item>,
    sortp: Vec<Item>,
    tags: Vec<Tag>,
    done: u32,
    ties_len: u32,
}

fn apply_list<T: Decode + Clone>(list: &mut Vec<T>, e: &ChangeEntry) {
    match e.op {
        ChangeOp::Full => *list = Vec::<T>::decode_exact(&e.value).expect("a full value decodes"),
        ChangeOp::KeyedPatch => patch_of::<T>(e)
            .apply(list)
            .expect("a delivered patch applies to what the host has"),
        other => panic!("unexpected op {other:?} for signal {}", e.signal_id),
    }
}

impl Mirror {
    fn apply_entry(&mut self, e: &ChangeEntry) {
        match e.signal_id {
            TIES => apply_list(&mut self.ties, e),
            QUOTA => apply_list(&mut self.quota, e),
            SORTP => apply_list(&mut self.sortp, e),
            TAGS => apply_list(&mut self.tags, e),
            DONE => self.done = value_of(e),
            TIES_LEN => self.ties_len = value_of(e),
            ROWS | CUT | DIR => {}
            other => panic!("unexpected signal {other}"),
        }
    }

    fn apply_set(&mut self, set: &ChangeSet) {
        assert!(!set.entries.is_empty(), "an empty change-set is not sent");
        for e in &set.entries {
            assert!(
                self.observed[e.signal_id as usize],
                "an unobserved signal was delivered"
            );
            self.apply_entry(e);
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Change {
    Toggle,
    Score(u16),
    Grp(u8),
}

#[derive(Clone, Copy, Debug)]
enum BurstKind {
    /// Sort-key changes: two derived ops each in `ties` and `sortp`.
    Scores,
    /// Membership flips.
    Toggles,
    /// Moves.
    Moves,
}

#[derive(Clone, Debug)]
enum Edit {
    Push(u8),
    Insert(usize, u8),
    Remove(usize),
    UpdateAt(usize, Change),
    Move(usize, usize),
    Clear,
    Replace(Vec<u8>),
    RawToggle(usize),
    Cut(u16),
    FlipDir,
    Read(u8),
    /// `n` recorded operations in a row, with a Rust read every `read_every` of them (0: none),
    /// then `tail` more single operations.
    Burst {
        n: usize,
        kind: BurstKind,
        read_every: usize,
        tail: u8,
    },
}

#[derive(Clone, Debug)]
enum Step {
    Edit(Edit),
    Txn(Vec<Edit>),
    ObserveOn(u32),
    ObserveOff(u32),
    ObserveAllOn,
    ObserveAllOff,
}

impl Core {
    fn item(&self, seed: u8) -> Item {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        Item {
            id,
            grp: seed % 5,
            done: seed % 4 == 0,
            score: u16::from(seed) % 23,
        }
    }

    fn burst_op(&self, i: usize, kind: BurstKind) {
        let len = self.rows.with(Vec::len);
        if len == 0 {
            self.rows.push(self.item(u8::try_from(i % 251).unwrap()));
            return;
        }
        let at = (i * 7) % len;
        match kind {
            BurstKind::Scores => self
                .rows
                .update_at(at, |r| r.score = (r.score + 5 + (i % 3) as u16) % 23),
            BurstKind::Toggles => self.rows.update_at(at, |r| r.done = !r.done),
            BurstKind::Moves => self.rows.move_item(at, (i * 3) % len),
        }
    }

    fn edit(&self, e: &Edit) {
        let len = self.rows.with(Vec::len);
        match e {
            Edit::Push(seed) => self.rows.push(self.item(*seed)),
            Edit::Insert(at, seed) => self.rows.insert(at % (len + 1), self.item(*seed)),
            Edit::Remove(at) if len > 0 => {
                self.rows.remove(at % len);
            }
            Edit::UpdateAt(at, change) if len > 0 => {
                self.rows.update_at(at % len, |r| match *change {
                    Change::Toggle => r.done = !r.done,
                    Change::Score(s) => r.score = s % 23,
                    Change::Grp(g) => r.grp = g % 5,
                });
            }
            Edit::Move(a, b) if len > 0 => self.rows.move_item(a % len, b % len),
            Edit::Remove(_) | Edit::UpdateAt(..) | Edit::Move(..) => {}
            Edit::Clear => self.rows.clear(),
            Edit::Replace(seeds) => {
                let fresh = seeds.iter().map(|s| self.item(*s)).collect();
                self.rows.replace(fresh);
            }
            Edit::RawToggle(at) => self.rows.update(|l| {
                if !l.is_empty() {
                    let at = at % l.len();
                    l[at].done = !l[at].done;
                }
            }),
            Edit::Cut(c) => self.cut.set(c % 25),
            Edit::FlipDir => self.dir.update(|d| *d = !*d),
            Edit::Read(which) => self.check_reads(*which),
            Edit::Burst {
                n,
                kind,
                read_every,
                tail,
            } => {
                for i in 0..*n {
                    self.burst_op(i, *kind);
                    if *read_every > 0 && i % read_every == read_every - 1 {
                        self.check_reads(u8::try_from(i % 6).unwrap());
                    }
                }
                for i in 0..usize::from(*tail) {
                    self.burst_op(n + i, BurstKind::Toggles);
                }
            }
        }
    }

    /// Every Rust read agrees with the reference, inside a transaction or not (read-your-writes).
    fn check_reads(&self, which: u8) {
        let rows = self.rows.get();
        match which % 6 {
            0 => assert_eq!(self.ties.get(), ref_ties(&rows)),
            1 => assert_eq!(self.quota.get(), ref_quota(&rows)),
            2 => assert_eq!(
                self.sortp.get(),
                ref_sortp(&rows, self.cut.get(), self.dir.get())
            ),
            3 => assert_eq!(self.tags.len(), rows.len()),
            4 => assert_eq!(self.done.get(), ref_done_count(&rows)),
            _ => assert_eq!(
                self.ties_len.get(),
                u32::try_from(ref_ties(&rows).len()).unwrap()
            ),
        }
    }
}

fn change() -> impl Strategy<Value = Change> {
    prop_oneof![
        3 => Just(Change::Toggle),
        3 => any::<u16>().prop_map(Change::Score),
        2 => any::<u8>().prop_map(Change::Grp),
    ]
}

fn burst() -> impl Strategy<Value = Edit> {
    (
        prop_oneof![
            Just(PARAM_WALK_LIMIT),
            Just(PARAM_WALK_LIMIT + 1),
            Just(OUT_LIMIT / 2 + 3),
            Just(TAP_LIMIT - 1),
            Just(TAP_LIMIT),
            Just(TAP_LIMIT + 1),
            Just(TAP_LIMIT + 300),
        ],
        prop_oneof![
            Just(BurstKind::Scores),
            Just(BurstKind::Toggles),
            Just(BurstKind::Moves)
        ],
        prop_oneof![Just(0_usize), Just(1_000), Just(TAP_LIMIT - 1)],
        0_u8..4,
    )
        .prop_map(|(n, kind, read_every, tail)| Edit::Burst {
            n,
            kind,
            read_every,
            tail,
        })
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        4 => any::<u8>().prop_map(Edit::Push),
        3 => (0_usize..60, any::<u8>()).prop_map(|(a, s)| Edit::Insert(a, s)),
        3 => (0_usize..60).prop_map(Edit::Remove),
        7 => (0_usize..60, change()).prop_map(|(a, c)| Edit::UpdateAt(a, c)),
        3 => (0_usize..60, 0_usize..60).prop_map(|(a, b)| Edit::Move(a, b)),
        1 => Just(Edit::Clear),
        1 => proptest::collection::vec(any::<u8>(), 0..30).prop_map(Edit::Replace),
        1 => (0_usize..60).prop_map(Edit::RawToggle),
        2 => any::<u16>().prop_map(Edit::Cut),
        2 => Just(Edit::FlipDir),
        2 => any::<u8>().prop_map(Edit::Read),
        1 => burst(),
    ]
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        8 => edit().prop_map(Step::Edit),
        6 => proptest::collection::vec(edit(), 1..8).prop_map(Step::Txn),
        2 => (0_u32..SLOTS as u32).prop_map(Step::ObserveOn),
        2 => (0_u32..SLOTS as u32).prop_map(Step::ObserveOff),
        1 => Just(Step::ObserveAllOn),
        1 => Just(Step::ObserveAllOff),
    ]
}

fn run_script(steps: &[Step]) {
    let core = core();
    let mut mirror = Mirror::default();
    let observe_step = |mirror: &mut Mirror, id: u32, on: bool| {
        let entries = observe(&core.cell, id, on);
        if id == ALL_SIGNALS {
            mirror.observed = [on; SLOTS];
        } else {
            mirror.observed[id as usize] = on;
        }
        for e in &entries {
            assert_eq!(e.op, ChangeOp::Full, "observe sends full values");
            mirror.apply_entry(e);
        }
    };
    observe_step(&mut mirror, ALL_SIGNALS, true);
    for step in steps {
        let edits: &[Edit] = match step {
            Step::Edit(e) => std::slice::from_ref(e),
            Step::Txn(es) => es,
            Step::ObserveOn(id) => {
                observe_step(&mut mirror, *id, true);
                &[]
            }
            Step::ObserveOff(id) => {
                observe_step(&mut mirror, *id, false);
                &[]
            }
            Step::ObserveAllOn => {
                observe_step(&mut mirror, ALL_SIGNALS, true);
                &[]
            }
            Step::ObserveAllOff => {
                observe_step(&mut mirror, ALL_SIGNALS, false);
                &[]
            }
        };
        if !edits.is_empty() {
            with_sink(core.sink.clone(), || {
                txn(|| {
                    for e in edits {
                        core.edit(e);
                    }
                });
            });
            let sets = core.sink.take_decoded();
            assert!(sets.len() <= 1, "one change-set per transaction");
            for set in &sets {
                mirror.apply_set(set);
            }
        }
        check(&core, &mirror, step);
    }
    observe_step(&mut mirror, ALL_SIGNALS, true);
    check(&core, &mirror, &Step::ObserveAllOn);
}

fn check(core: &Core, mirror: &Mirror, step: &Step) {
    let rows = core.rows.get();
    let (cut, dir) = (core.cut.get(), core.dir.get());
    let o = |id: u32| mirror.observed[id as usize];
    if o(TIES) {
        assert_eq!(mirror.ties, ref_ties(&rows), "host ties after {step:?}");
    }
    if o(QUOTA) {
        assert_eq!(mirror.quota, ref_quota(&rows), "host quota after {step:?}");
    }
    if o(SORTP) {
        assert_eq!(
            mirror.sortp,
            ref_sortp(&rows, cut, dir),
            "host sortp after {step:?}"
        );
    }
    if o(TAGS) {
        assert_eq!(mirror.tags, ref_tags(&rows), "host tags after {step:?}");
    }
    if o(DONE) {
        assert_eq!(
            mirror.done,
            ref_done_count(&rows),
            "host count after {step:?}"
        );
    }
    if o(TIES_LEN) {
        assert_eq!(
            mirror.ties_len,
            u32::try_from(ref_ties(&rows).len()).unwrap(),
            "host ties_len after {step:?}"
        );
    }
    assert_eq!(core.ties.get(), ref_ties(&rows));
    assert_eq!(core.quota.get(), ref_quota(&rows));
    assert_eq!(core.sortp.get(), ref_sortp(&rows, cut, dir));
    assert_eq!(core.tags.get(), ref_tags(&rows));
    assert_eq!(core.done.get(), ref_done_count(&rows));
    for view in VIEWS {
        assert!(!core.cell.is_failed(view), "no view failed");
    }
}

fn cases() -> u32 {
    std::env::var("UNDRA_DERIVED_REVIEW_CASES")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(200)
}

proptest! {
    #![proptest_config(ProptestConfig { cases: cases(), ..ProptestConfig::default() })]

    #[test]
    fn four_views_a_count_parameter_and_overflowing_transactions_match_the_reference(
        steps in proptest::collection::vec(step(), 1..24)
    ) {
        run_script(&steps);
    }
}

#[test]
fn a_fixed_script_overflows_the_tap_and_keeps_writing_in_the_same_transaction() {
    use Edit::*;
    run_script(&[
        Step::Txn(vec![
            Push(3),
            Burst {
                n: TAP_LIMIT + 1,
                kind: BurstKind::Scores,
                read_every: 0,
                tail: 3,
            },
            Read(0),
            UpdateAt(1, Change::Score(4)),
            Read(2),
            Move(0, 3),
        ]),
        Step::Txn(vec![
            Burst {
                n: OUT_LIMIT / 2 + 3,
                kind: BurstKind::Scores,
                read_every: 1_000,
                tail: 0,
            },
            Cut(20),
        ]),
        Step::Edit(UpdateAt(2, Change::Toggle)),
        Step::ObserveOff(SORTP),
        Step::Edit(Burst {
            n: TAP_LIMIT + 300,
            kind: BurstKind::Moves,
            read_every: TAP_LIMIT - 1,
            tail: 2,
        }),
        Step::ObserveOn(SORTP),
        Step::Txn(vec![FlipDir, Cut(7), UpdateAt(0, Change::Grp(0))]),
    ]);
}

// ---------------------------------------------------------------------------------------------
// The tap and the pending derived ops, exactly
// ---------------------------------------------------------------------------------------------

/// An observed sorted view: `score % 3`, nothing filtered.
fn sorted_fixture(n: u32) -> (Rig, Signal<Vec<Item>>, DerivedList<Item>) {
    let rig = Rig::new();
    let rows = Signal::new(
        (1..=n)
            .map(|id| Item {
                id,
                grp: 0,
                done: false,
                score: (id % 3) as u16,
            })
            .collect::<Vec<_>>(),
    );
    let view = rows.derive().sort_by_key(|r: &Item| r.score % 3).build();
    rig.cell.attach_derived(&view, 0, item_key).unwrap();
    assert_eq!(rig.observe_all().len(), 1);
    (rig, rows, view)
}

fn expected_sorted(rows: &[Item]) -> Vec<Item> {
    let mut v = rows.to_vec();
    v.sort_by_key(|r| r.score % 3);
    v
}

#[test]
fn an_op_that_arrives_after_the_tap_overflowed_is_rebuilt_in_the_drain_that_meets_it() {
    let (rig, rows, view) = sorted_fixture(40);
    let mut host = view.get();
    let rebuilds = view.stats().rebuilds;
    rig.run(|| {
        txn(|| {
            for i in 0..=TAP_LIMIT {
                rows.update_at(i % 40, |r| r.score += 1);
            }
            // The tap is stale; these are not recorded on it.
            rows.push(Item {
                id: 1_000,
                grp: 0,
                done: false,
                score: 2,
            });
            rows.move_item(40, 0);
            // A drain in the middle of the transaction rebuilds from the list it sees ...
            assert_eq!(view.get(), expected_sorted(&rows.get()));
            assert_eq!(view.stats().rebuilds, rebuilds + 1);
            // ... and replays what comes after it.
            rows.update_at(0, |r| r.score = 7);
            rows.remove(5);
            assert_eq!(view.get(), expected_sorted(&rows.get()));
        })
    });
    let set = rig.one_set();
    let e = entry(&set, 0);
    assert_eq!(
        e.op,
        ChangeOp::Full,
        "the view was rebuilt: the host gets it whole"
    );
    host = {
        let _ = host;
        value_of(e)
    };
    assert_eq!(host, expected_sorted(&rows.get()));
    assert_eq!(view.stats().rebuilds, rebuilds + 1, "rebuilt once");
    // Patches resume.
    rig.run(|| rows.update_at(3, |r| r.score += 1));
    patch_of::<Item>(entry(&rig.one_set(), 0))
        .apply(&mut host)
        .unwrap();
    assert_eq!(host, expected_sorted(&rows.get()));
}

#[test]
fn derived_ops_overflow_on_their_own_without_a_rebuild() {
    // 2,051 sort-key changes: 2,051 source ops (the tap keeps them) but up to 4,102 view ops (Move +
    // Update each), past the 4,096 the slot keeps: the full value, and no rebuild.
    let (rig, rows, view) = sorted_fixture(60);
    let rebuilds = view.stats().rebuilds;
    let mut moved = 0;
    rig.run(|| {
        txn(|| {
            for i in 0..OUT_LIMIT / 2 + 3 {
                let at = (i * 7) % 60;
                let before = rows.with(|l| l[at].score % 3);
                rows.update_at(at, |r| r.score += 1);
                if before != rows.with(|l| l[at].score % 3) {
                    moved += 1;
                }
            }
        })
    });
    assert!(moved > OUT_LIMIT / 4, "{moved} rows changed key");
    let set = rig.one_set();
    let e = entry(&set, 0);
    let host: Vec<Item> = match e.op {
        ChangeOp::Full => value_of(e),
        ChangeOp::KeyedPatch => panic!("{} ops kept", patch_of::<Item>(e).len()),
        other => panic!("{other:?}"),
    };
    assert_eq!(host, expected_sorted(&rows.get()));
    assert_eq!(
        view.stats().rebuilds,
        rebuilds,
        "the index never stopped being current"
    );
}

// ---------------------------------------------------------------------------------------------
// The 256-op parameter walk, on top of replayed source ops
// ---------------------------------------------------------------------------------------------

/// A filter parameter that drops exactly `k` rows, in the same transaction as 10 source ops that
/// reach the view. Returns the entry's op and the number of ops of a patch.
fn filter_walk_with_replay(k: u32) -> (ChangeOp, usize) {
    let rig = Rig::new();
    let rows = Signal::new((0..1_000_u32).collect::<Vec<_>>());
    let cut = Signal::new(0_u32);
    let view = rows
        .derive()
        .filter_with(&cut, |cut: &u32, n: &u32| *n >= *cut)
        .build();
    rig.cell
        .attach_derived(&view, 0, |n: &u32| u64::from(*n))
        .unwrap();
    rig.cell.attach(&cut, 1).unwrap();
    let mut host: Vec<u32> = value_of(&rig.observe_all()[0]);
    rig.run(|| {
        txn(|| {
            for i in 0..10_u32 {
                rows.push(5_000 + i);
            }
            cut.set(k);
        })
    });
    let set = rig.one_set();
    let e = entry(&set, 0);
    let ops = match e.op {
        ChangeOp::Full => {
            host = value_of(e);
            0
        }
        ChangeOp::KeyedPatch => {
            let patch = patch_of::<u32>(e);
            patch.apply(&mut host).unwrap();
            patch.len()
        }
        other => panic!("{other:?}"),
    };
    let expected: Vec<u32> = rows.get().into_iter().filter(|n| *n >= k).collect();
    assert_eq!(host, expected);
    assert_eq!(view.get(), expected);
    (e.op, ops)
}

#[test]
fn a_filter_walk_of_256_ops_after_replayed_ops_is_one_patch_and_257_is_the_full_value() {
    let limit = u32::try_from(PARAM_WALK_LIMIT).unwrap();
    assert_eq!(
        filter_walk_with_replay(limit),
        (ChangeOp::KeyedPatch, 10 + PARAM_WALK_LIMIT),
        "the replayed ops and exactly 256 walk ops"
    );
    assert_eq!(filter_walk_with_replay(limit + 1).0, ChangeOp::Full);
}

/// A sort parameter that sends exactly `k` rows from the front of a sorted view to its end (one
/// `Move` each), after two source ops in the same transaction.
fn sort_walk_with_replay(k: u16) -> (ChangeOp, usize) {
    let rig = Rig::new();
    let rows = Signal::new((0..1_000_u16).collect::<Vec<_>>());
    let bump = Signal::new(0_u16);
    let view = rows
        .derive()
        .sort_by_key_with(&bump, |bump: &u16, n: &u16| {
            if *n < *bump {
                u32::from(*n) + 10_000
            } else {
                u32::from(*n)
            }
        })
        .build();
    rig.cell
        .attach_derived(&view, 0, |n: &u16| u64::from(*n))
        .unwrap();
    let mut host: Vec<u16> = value_of(&rig.observe_all()[0]);
    rig.run(|| {
        txn(|| {
            rows.push(5_000);
            rows.remove(500);
            bump.set(k);
        })
    });
    let set = rig.one_set();
    let e = entry(&set, 0);
    let ops = match e.op {
        ChangeOp::Full => {
            host = value_of(e);
            0
        }
        ChangeOp::KeyedPatch => {
            let patch = patch_of::<u16>(e);
            assert!(
                patch.ops[2..]
                    .iter()
                    .all(|op| matches!(op, PatchOp::Move { .. })),
                "a sort parameter moves rows, it does not change them"
            );
            patch.apply(&mut host).unwrap();
            patch.len()
        }
        other => panic!("{other:?}"),
    };
    let mut expected = rows.get();
    expected.sort_by_key(|n| {
        if *n < k {
            u32::from(*n) + 10_000
        } else {
            u32::from(*n)
        }
    });
    assert_eq!(host, expected);
    (e.op, ops)
}

#[test]
fn a_sort_walk_of_256_moves_after_replayed_ops_is_one_patch_and_257_is_the_full_value() {
    let limit = u16::try_from(PARAM_WALK_LIMIT).unwrap();
    assert_eq!(
        sort_walk_with_replay(limit),
        (ChangeOp::KeyedPatch, 2 + PARAM_WALK_LIMIT)
    );
    assert_eq!(sort_walk_with_replay(limit + 1).0, ChangeOp::Full);
}

// ---------------------------------------------------------------------------------------------
// One row: moved, changed and removed in one transaction
// ---------------------------------------------------------------------------------------------

#[test]
fn a_row_that_moves_changes_and_leaves_in_one_transaction_is_three_ops_that_apply() {
    let (rig, rows, view) = sorted_fixture(9);
    let mut host = view.get();
    // Row 1 (score 1) sorts among the ones; give it score 2 (Move + Update), then remove it.
    rig.run(|| {
        txn(|| {
            rows.update_at(0, |r| r.score = 2);
            rows.remove(0);
        })
    });
    let patch = patch_of::<Item>(entry(&rig.one_set(), 0));
    assert!(
        matches!(
            patch.ops.as_slice(),
            [
                PatchOp::Move { .. },
                PatchOp::Update { .. },
                PatchOp::Remove { .. }
            ]
        ),
        "{:?}",
        patch.ops
    );
    patch.apply(&mut host).unwrap();
    assert_eq!(host, expected_sorted(&rows.get()));

    // The same three ops spread over two change-sets, applied as one concatenated patch (what a
    // mirror does with the patches of one signal between two drains, ADR-031).
    let mut merged = view.get();
    let mut ops = Vec::new();
    rig.run(|| rows.update_at(1, |r| r.score = 1));
    ops.extend(patch_of::<Item>(entry(&rig.one_set(), 0)).ops);
    rig.run(|| rows.remove(1));
    ops.extend(patch_of::<Item>(entry(&rig.one_set(), 0)).ops);
    assert_eq!(ops.len(), 3, "{ops:?}");
    KeyedPatch { ops }.apply(&mut merged).unwrap();
    assert_eq!(merged, expected_sorted(&rows.get()));
}

// ---------------------------------------------------------------------------------------------
// Duplicate keys in the source
// ---------------------------------------------------------------------------------------------

#[test]
fn duplicate_keys_never_corrupt_the_view() {
    let rig = Rig::new();
    let rows = Signal::new(vec![
        Item {
            id: 1,
            grp: 0,
            done: false,
            score: 2,
        },
        Item {
            id: 2,
            grp: 0,
            done: false,
            score: 1,
        },
    ]);
    let tag = Signal::new(0_u32);
    let view = rows.derive().sort_by_key(|r: &Item| r.score).build();
    rig.cell.attach_derived(&view, 0, item_key).unwrap();
    rig.cell.attach(&tag, 1).unwrap();
    let mut host: Vec<Item> = value_of(&rig.observe_all()[0]);
    let dup = Item {
        id: 1,
        grp: 9,
        done: false,
        score: 0,
    };
    let expected = |rows: &[Item]| {
        let mut v = rows.to_vec();
        v.sort_by_key(|r| r.score);
        v
    };

    // A recorded insert of a second row with key 1: maintenance does not look at keys.
    rig.run(|| {
        txn(|| {
            rows.push(dup.clone());
            tag.set(1);
        })
    });
    let set = rig.one_set();
    patch_of::<Item>(entry(&set, 0)).apply(&mut host).unwrap();
    assert_eq!(host, expected(&rows.get()));
    assert_eq!(view.get(), expected(&rows.get()), "two rows, both kept");

    // A full value with a duplicate key: debug builds refuse to send it (the platforms identify rows
    // by key) and hold the view back like a failed computed; release builds send it as it is.
    rig.run(|| {
        txn(|| {
            rows.update(|l| l[0].score = 5);
            tag.set(2);
        })
    });
    let set = rig.one_set();
    if cfg!(debug_assertions) {
        assert!(set.entries.iter().all(|e| e.signal_id == 1), "only the tag");
        let failed = rig.cell.failed_signals();
        assert_eq!(failed.len(), 1);
        assert!(failed[0].1.contains("same key"), "{failed:?}");
    } else {
        host = value_of(entry(&set, 0));
        assert_eq!(host, expected(&rows.get()));
        assert!(rig.cell.failed_signals().is_empty());
    }
    assert_eq!(view.get(), expected(&rows.get()), "the index is intact");

    // The duplicate goes: the view recovers (debug) or keeps patching (release).
    rig.run(|| rows.remove(2));
    let set = rig.one_set();
    let e = entry(&set, 0);
    match e.op {
        ChangeOp::Full => host = value_of(e),
        _ => patch_of::<Item>(e).apply(&mut host).unwrap(),
    }
    assert_eq!(host, expected(&rows.get()));
    assert!(rig.cell.failed_signals().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Read-your-writes
// ---------------------------------------------------------------------------------------------

#[test]
fn a_read_inside_a_transaction_sees_the_writes_made_before_it_and_the_commit_sends_them_all() {
    let rig = Rig::new();
    let rows = Signal::new(vec![
        Item {
            id: 1,
            grp: 0,
            done: false,
            score: 3,
        },
        Item {
            id: 2,
            grp: 0,
            done: true,
            score: 1,
        },
    ]);
    let open = rows.derive().filter(|r: &Item| !r.done).build();
    let count = rows.derive().filter(|r: &Item| !r.done).count();
    rig.cell.attach_derived(&open, 0, item_key).unwrap();
    rig.cell.attach_computed(&count, 1).unwrap();
    let mut host: Vec<Item> = value_of(&rig.observe_all()[0]);
    rig.run(|| {
        txn(|| {
            rows.update_at(1, |r| r.done = false);
            // The view and the count include the write this transaction just made ...
            assert_eq!(open.len(), 2);
            assert_eq!(count.get(), 2);
            rows.push(Item {
                id: 3,
                grp: 0,
                done: false,
                score: 0,
            });
            rows.update_at(0, |r| r.done = true);
            // ... and every later one; nothing was sent yet.
            assert_eq!(open.get().iter().map(|r| r.id).collect::<Vec<_>>(), [2, 3]);
            assert_eq!(count.get(), 2);
            assert!(rig.sink.take_decoded().is_empty());
        })
    });
    // One change-set: the ops of the whole transaction, in order, in one patch.
    let set = rig.one_set();
    let patch = patch_of::<Item>(entry(&set, 0));
    assert_eq!(patch.len(), 3, "{:?}", patch.ops);
    patch.apply(&mut host).unwrap();
    assert_eq!(host, open.get());
    assert_eq!(value_of::<u32>(entry(&set, 1)), 2);
}

// ---------------------------------------------------------------------------------------------
// A drain part-way through its walk while another thread writes and lets go
// ---------------------------------------------------------------------------------------------

#[test]
fn writers_never_wait_for_a_drain_and_dropping_every_other_handle_mid_walk_is_safe() {
    let rows = Signal::new((0..2_000_u32).collect::<Vec<_>>());
    let cut = Signal::new(0_u32);
    let entered = Arc::new(AtomicBool::new(false));
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let release_rx = Arc::new(parking_lot::Mutex::new(release_rx));
    let evaluated = Arc::new(AtomicUsize::new(0));
    let view = {
        let (entered, release_rx, evaluated) = (
            Arc::clone(&entered),
            Arc::clone(&release_rx),
            Arc::clone(&evaluated),
        );
        rows.derive()
            .filter_with(&cut, move |cut: &u32, n: &u32| {
                // Block once, half-way through the walk a parameter change starts.
                if *cut == 7 && *n == 1_000 && !entered.swap(true, Ordering::SeqCst) {
                    release_rx.lock().recv().unwrap();
                }
                evaluated.fetch_add(1, Ordering::Relaxed);
                *n >= *cut
            })
            .build()
    };
    let cell = StoreCell::new(0xD00D);
    cell.set_handle(HANDLE);
    // Attached but not observed: no commit drains it, so only the two threads below do.
    cell.attach_derived(&view, 0, |n: &u32| u64::from(*n))
        .unwrap();
    let sink = CaptureSink::new();

    let reader = {
        let view = view.clone();
        std::thread::spawn(move || view.get())
    };
    // The first drain on the other thread builds the index; then the parameter changes and a
    // second read walks.
    let _ = reader.join().unwrap();
    cut.set(7);
    let walker = {
        let view = view.clone();
        std::thread::spawn(move || view.len())
    };
    while !entered.load(Ordering::SeqCst) {
        std::thread::yield_now();
    }
    // The walk holds the list's drain lock. Recorded and raw writes of the source never take it,
    // commits of other stores do not either, and every other handle can go.
    with_sink(sink.clone(), || {
        rows.push(5_000);
        rows.update_at(0, |n| *n = 9_000);
        rows.update(|l| l.push(6_000));
    });
    drop(cell);
    drop(view);
    let probe = rows.derive().filter(|n: &u32| n % 2 == 0).build();
    assert_eq!(
        probe.len(),
        2_000 / 2 + 2,
        "another view of the source still works"
    );
    release_tx.send(()).unwrap();
    // The walk finishes on the list it snapshotted when it started.
    assert_eq!(walker.join().unwrap(), 2_000 - 7);
    assert!(evaluated.load(Ordering::Relaxed) > 0);
}

// ---------------------------------------------------------------------------------------------
// Isolation: a panicking closure poisons only its own list, reported once
// ---------------------------------------------------------------------------------------------

/// Captures change-sets and counts failure and recovery reports.
struct Reports {
    capture: Arc<CaptureSink>,
    failed: AtomicUsize,
    recovered: AtomicUsize,
}

impl undra_signals::ChangeSink for Reports {
    fn deliver(&self, change_set: &[u8]) {
        self.capture.deliver(change_set);
    }

    fn computed_failed(&self, _owner: u64, _handle: u64, _signal_id: u32, _message: &str) {
        self.failed.fetch_add(1, Ordering::SeqCst);
    }

    fn computed_recovered(&self, _owner: u64, _handle: u64, _signal_id: u32) {
        self.recovered.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn a_panicking_map_poisons_only_its_list_is_reported_once_and_recovers() {
    let cell = StoreCell::new(0xFA11);
    cell.set_handle(HANDLE);
    let rows = Signal::new(vec![
        Item {
            id: 1,
            grp: 0,
            done: false,
            score: 1,
        },
        Item {
            id: 2,
            grp: 0,
            done: false,
            score: 2,
        },
    ]);
    let bad = rows
        .derive()
        .map(|r: &Item| {
            assert!(r.score != 13, "map failure");
            tag_of(r)
        })
        .sort_by_key(|t: &Tag| t.text.clone())
        .build();
    let good = rows.derive().filter(|r: &Item| !r.done).build();
    let count = rows.derive().filter(|r: &Item| r.score > 1).count();
    cell.attach_keyed(&rows, 0, item_key).unwrap();
    cell.attach_derived(&bad, 1, |t: &Tag| u64::from(t.id))
        .unwrap();
    cell.attach_derived(&good, 2, item_key).unwrap();
    cell.attach_computed(&count, 3).unwrap();
    let reports = Arc::new(Reports {
        capture: CaptureSink::new(),
        failed: AtomicUsize::new(0),
        recovered: AtomicUsize::new(0),
    });
    let mut good_host: Vec<Item> = Vec::new();
    for e in observe(&cell, ALL_SIGNALS, true) {
        if e.signal_id == 2 {
            good_host = value_of(&e);
        }
    }
    let run = |f: &dyn Fn()| -> Vec<ChangeSet> {
        with_sink(reports.clone(), || txn(f));
        reports.capture.take_decoded()
    };
    let ids = |set: &ChangeSet| set.entries.iter().map(|e| e.signal_id).collect::<Vec<_>>();

    // Two commits in a row hit the panicking row: the source, the other view and the count are
    // delivered both times; the failure is reported once.
    for (i, score) in [(0, 13), (1, 5)] {
        let sets = run(&|| rows.update_at(i, |r| r.score = score));
        assert_eq!(sets.len(), 1);
        assert_eq!(ids(&sets[0]), [0, 2, 3], "everything but the failed view");
        apply_list(&mut good_host, entry(&sets[0], 2));
        assert_eq!(good_host, good.get());
    }
    assert_eq!(reports.failed.load(Ordering::SeqCst), 1, "reported once");
    assert!(cell.is_failed(1));
    assert_eq!(count.get(), 2);
    // A Rust read of the failed view panics too, and leaves it usable afterwards.
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| bad.len())).is_err());

    // The input changes so the closure succeeds: the full value, one recovery report.
    let sets = run(&|| rows.update_at(0, |r| r.score = 4));
    let e = entry(&sets[0], 1);
    assert_eq!(e.op, ChangeOp::Full);
    let mut expected: Vec<Tag> = rows.get().iter().map(tag_of).collect();
    expected.sort_by_key(|t| t.text.clone());
    assert_eq!(value_of::<Vec<Tag>>(e), expected);
    assert!(!cell.is_failed(1));
    assert_eq!(reports.recovered.load(Ordering::SeqCst), 1);
    // Then patches again.
    let sets = run(&|| rows.update_at(1, |r| r.grp = 3));
    assert_eq!(entry(&sets[0], 1).op, ChangeOp::KeyedPatch);
}
