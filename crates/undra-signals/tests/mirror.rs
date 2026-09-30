//! Model-based test: a host mirror that applies every delivered change-set must always agree
//! with the core's signals, whatever mix of list edits, transactions and observation changes
//! is thrown at it.

mod common;

use std::sync::Arc;

use common::*;
use proptest::prelude::*;
use undra_signals::testing::CaptureSink;
use undra_signals::{ALL_SIGNALS, Computed, Signal, StoreCell, txn, with_sink};
use undra_wire::Reader;
use undra_wire::payload::{ChangeEntry, ChangeOp, ChangeSet};

const LIST: u32 = 0;
const COUNT: u32 = 1;
const OPEN: u32 = 2;
const NAME: u32 = 3;

struct Core {
    cell: Arc<StoreCell>,
    list: Signal<Vec<Todo>>,
    count: Signal<u32>,
    open: Computed<u32>,
    name: Signal<String>,
    sink: Arc<CaptureSink>,
}

fn core() -> Core {
    let cell = StoreCell::new(0xABCD);
    cell.set_handle(HANDLE);
    let list = Signal::new(todos(3));
    let count = Signal::new(0_u32);
    let name = Signal::new(String::new());
    let open = Computed::new((&list, &count), |(l, c): (&Vec<Todo>, &u32)| {
        u32::try_from(l.iter().filter(|t| !t.done).count()).unwrap() + c
    });
    cell.attach_keyed(&list, LIST, todo_key).unwrap();
    cell.attach(&count, COUNT).unwrap();
    cell.attach_computed(&open, OPEN).unwrap();
    cell.attach(&name, NAME).unwrap();
    Core {
        cell,
        list,
        count,
        open,
        name,
        sink: CaptureSink::new(),
    }
}

/// What the host knows.
#[derive(Default)]
struct Mirror {
    observed: [bool; 4],
    list: Vec<Todo>,
    count: u32,
    open: u32,
    name: String,
    last_txn: u64,
}

impl Mirror {
    fn apply_set(&mut self, set: &ChangeSet) {
        assert!(
            !set.entries.is_empty(),
            "empty change-sets are never delivered"
        );
        assert!(
            set.txn_id > self.last_txn,
            "transactions arrive in commit order"
        );
        self.last_txn = set.txn_id;
        let mut seen = [false; 4];
        for e in &set.entries {
            let slot = e.signal_id as usize;
            assert!(!seen[slot], "a signal appears at most once per change-set");
            seen[slot] = true;
            assert!(
                self.observed[slot],
                "unobserved signal {slot} was delivered"
            );
            self.apply_entry(e);
        }
    }

    fn apply_entry(&mut self, e: &ChangeEntry) {
        match (e.signal_id, e.op) {
            (LIST, ChangeOp::Full) => self.list = value_of(e),
            (LIST, ChangeOp::KeyedPatch) => {
                let mut r = Reader::new(&e.value);
                let patch = undra_wire::KeyedPatch::<Todo>::decode(&mut r).unwrap();
                r.finish().unwrap();
                patch
                    .apply(&mut self.list)
                    .expect("a delivered patch always applies");
            }
            (COUNT, ChangeOp::Full) => self.count = value_of(e),
            (OPEN, ChangeOp::Full) => self.open = value_of(e),
            (NAME, ChangeOp::Full) => self.name = value_of(e),
            other => panic!("unexpected entry {other:?}"),
        }
    }
}

#[derive(Clone, Debug)]
enum Edit {
    Push(u32),
    Remove(usize),
    Toggle(usize),
    Move(usize, usize),
    Replace(Vec<u32>),
    Clear,
    Count(u32),
    Name(String),
}

#[derive(Clone, Debug)]
enum Op {
    Edit(Edit),
    Txn(Vec<Edit>),
    ObserveOn(u32),
    ObserveOff(u32),
    ObserveAllOn,
    ObserveAllOff,
}

fn edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        3 => (0_u32..20).prop_map(Edit::Push),
        2 => (0_usize..50).prop_map(Edit::Remove),
        2 => (0_usize..50).prop_map(Edit::Toggle),
        2 => (0_usize..50, 0_usize..50).prop_map(|(a, b)| Edit::Move(a, b)),
        1 => proptest::collection::vec(0_u32..20, 0..8).prop_map(Edit::Replace),
        1 => Just(Edit::Clear),
        2 => (0_u32..5).prop_map(Edit::Count),
        1 => "[a-zA-Z\u{e9}\u{65e5}]{0,4}".prop_map(Edit::Name),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        6 => edit().prop_map(Op::Edit),
        3 => proptest::collection::vec(edit(), 1..6).prop_map(Op::Txn),
        2 => (0_u32..4).prop_map(Op::ObserveOn),
        1 => (0_u32..4).prop_map(Op::ObserveOff),
        1 => Just(Op::ObserveAllOn),
        1 => Just(Op::ObserveAllOff),
    ]
}

impl Core {
    fn edit(&self, e: &Edit) {
        match e {
            Edit::Push(id) => self.list.update(|l| l.push(todo(*id, "p", false))),
            Edit::Remove(i) => self.list.update(|l| {
                if !l.is_empty() {
                    let at = i % l.len();
                    l.remove(at);
                }
            }),
            Edit::Toggle(i) => self.list.update(|l| {
                if !l.is_empty() {
                    let at = i % l.len();
                    l[at].done = !l[at].done;
                }
            }),
            Edit::Move(a, b) => self.list.update(|l| {
                if !l.is_empty() {
                    let from = a % l.len();
                    let item = l.remove(from);
                    let to = b % (l.len() + 1);
                    l.insert(to, item);
                }
            }),
            Edit::Replace(ids) => self
                .list
                .set(ids.iter().map(|id| todo(*id, "r", false)).collect()),
            Edit::Clear => self.list.set(Vec::new()),
            Edit::Count(v) => self.count.set(*v),
            Edit::Name(s) => self.name.set(s.clone()),
        }
    }
}

fn observe_into(core: &Core, mirror: &mut Mirror, id: u32, on: bool) {
    let entries = observe(&core.cell, id, on);
    let targets: Vec<usize> = if id == ALL_SIGNALS {
        (0..4).collect()
    } else {
        vec![id as usize]
    };
    for t in targets {
        mirror.observed[t] = on;
    }
    for e in &entries {
        assert_eq!(e.op, ChangeOp::Full, "observe always sends full values");
        mirror.apply_entry(e);
    }
    if on {
        assert_eq!(entries.len(), if id == ALL_SIGNALS { 4 } else { 1 });
    }
}

fn check(core: &Core, mirror: &Mirror) {
    if mirror.observed[LIST as usize] {
        assert_eq!(mirror.list, core.list.get(), "list");
    }
    if mirror.observed[COUNT as usize] {
        assert_eq!(mirror.count, core.count.get(), "count");
    }
    if mirror.observed[OPEN as usize] {
        assert_eq!(mirror.open, core.open.get(), "open");
    }
    if mirror.observed[NAME as usize] {
        assert_eq!(mirror.name, core.name.get(), "name");
    }
}

fn run_script(ops: &[Op]) {
    let core = core();
    let mut mirror = Mirror::default();
    // Mirrors start out observing everything, like a freshly bound store.
    observe_into(&core, &mut mirror, ALL_SIGNALS, true);
    check(&core, &mirror);

    for op in ops {
        match op {
            Op::Edit(e) => with_sink(core.sink.clone(), || core.edit(e)),
            Op::Txn(edits) => with_sink(core.sink.clone(), || {
                txn(|| edits.iter().for_each(|e| core.edit(e)));
            }),
            Op::ObserveOn(id) => observe_into(&core, &mut mirror, *id, true),
            Op::ObserveOff(id) => observe_into(&core, &mut mirror, *id, false),
            Op::ObserveAllOn => observe_into(&core, &mut mirror, ALL_SIGNALS, true),
            Op::ObserveAllOff => observe_into(&core, &mut mirror, ALL_SIGNALS, false),
        }
        for set in core.sink.take_decoded() {
            mirror.apply_set(&set);
        }
        check(&core, &mirror);
    }

    // A final resync must always bring the mirror fully up to date.
    observe_into(&core, &mut mirror, ALL_SIGNALS, true);
    check(&core, &mirror);
    assert_eq!(mirror.list, core.list.get());
    assert_eq!(mirror.count, core.count.get());
    assert_eq!(mirror.open, core.open.get());
    assert_eq!(mirror.name, core.name.get());
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 300, ..ProptestConfig::default() })]

    #[test]
    fn the_host_mirror_always_agrees_with_the_core(ops in proptest::collection::vec(op(), 1..40)) {
        run_script(&ops);
    }
}

#[test]
fn a_fixed_regression_script() {
    // A hand-written script covering patch, full, txn and resync paths deterministically.
    run_script(&[
        Op::Edit(Edit::Push(7)),
        Op::Edit(Edit::Toggle(0)),
        Op::Txn(vec![
            Edit::Remove(1),
            Edit::Push(8),
            Edit::Count(3),
            Edit::Move(0, 2),
        ]),
        Op::ObserveOff(LIST),
        Op::Edit(Edit::Clear),
        Op::Edit(Edit::Push(1)),
        Op::ObserveOn(LIST),
        Op::Edit(Edit::Push(2)),
        Op::Edit(Edit::Replace(vec![1, 1, 2])),
        Op::Edit(Edit::Replace(vec![1, 2, 3])),
        Op::Edit(Edit::Name("ünï".to_string())),
        Op::ObserveAllOff,
        Op::Edit(Edit::Count(4)),
        Op::ObserveAllOn,
    ]);
}
