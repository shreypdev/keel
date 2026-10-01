//! The node behind a [`DerivedList`](super::DerivedList): the index, the tap it replays, the
//! pending derived ops, and the one routine (the drain) every read, commit and observe goes
//! through.
//!
//! # The drain
//!
//! A drain brings the index up to date with the source: it takes the ops the tap recorded and
//! replays them on the index with the parameter values the index was built with, emitting the
//! derived ops each one causes (ADR-039 section 2); then, if a parameter changed, it walks every
//! row with the new values (section 4). A drain that needs the items (to materialise, rebuild or
//! walk) takes the ops and an `Arc` of the list together under the source's read lock. A stale or
//! disarmed tap, or a drain that unwound, rebuilds the index from the list instead.
//!
//! # Locks and re-entrancy
//!
//! The drain lock (`state`) is held while pipeline closures run. Before taking it, a drain checks
//! that this thread is not already draining the same list (a closure that reads its own list: a
//! panic, ADR-021 L1, instead of a deadlock) and not inside the source's `update` closure (ADR-021
//! L3). It runs inside a transaction, so a write some closure makes (a computed parameter's
//! closure may write) is committed after the drain lock is released, never under it.
//!
//! A drain that unwinds (a closure panicked) leaves `in_flight` set: the next one rebuilds the
//! index and the next commit sends the full value, so a half-applied op cannot survive.

use std::borrow::Cow;
use std::cell::RefCell;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use parking_lot::Mutex;
use undra_wire::{Encode, KeyedPatch, PatchOp, Writer};

use super::index::{DerivedIndex, NIL, Step};
use super::pipeline::{EvalFn, ParamSlot, Params};
use super::slot::Emitted;
use super::tap::{SourceTap, TapTaken};
use super::{DerivedStats, DerivingScope, OUT_LIMIT, PARAM_WALK_LIMIT, purity_message};
use crate::graph::{Binding, Dependents, Reactive, add_dependent, propagate, record};
use crate::signal::Signal;
use crate::txn::TxnGuard;
use crate::value::SignalValue;

/// What a derived list is to the rest of the crate, whatever its source item and key types.
pub(crate) trait DerivedNode<U>: Send + Sync {
    /// Rows in the view, after a drain. O(1) once caught up.
    fn view_len(&self) -> usize;
    /// The view as a `Vec`, cached until it changes.
    fn materialise(&self) -> Arc<Vec<U>>;
    /// The counters.
    fn stats(&self) -> DerivedStats;
    /// Where the list lives in a store.
    fn binding(&self) -> &OnceLock<Binding>;
    /// Registers a node to invalidate when the view may have changed.
    fn add_dependent(&self, dependent: Weak<dyn Reactive>);
    /// Store delivery: see [`DerivedSlot`](super::slot::DerivedSlot).
    fn commit(&self, w: &mut Writer, retain: bool, key: fn(&U) -> u64) -> Emitted;
    /// Store delivery: the full value, and patches from this instant.
    fn resync(&self, w: &mut Writer, key: fn(&U) -> u64);
    /// Store delivery: stop keeping patches.
    fn forget(&self);
    /// The node's identity (its address).
    fn id(&self) -> usize;
}

/// The node of one derived list over a `Signal<Vec<S>>`, producing `U`s sorted by `K` (`()`
/// without a sort stage).
pub(crate) struct DerivedInner<S: Clone, U: Clone, K> {
    source: Signal<Vec<S>>,
    tap: Arc<SourceTap<S>>,
    eval: Box<EvalFn<S, U, K>>,
    params: Vec<Box<dyn ParamSlot>>,
    sorted: bool,
    /// The drain lock.
    state: Mutex<State<U, K>>,
    /// The last invalidation walk that reached this node (diamond de-duplication).
    visited: AtomicU64,
    dependents: Dependents,
    binding: OnceLock<Binding>,
    stats: Stats,
}

#[derive(Default)]
struct Stats {
    rebuilds: AtomicU64,
    full_values: AtomicU64,
    patches: AtomicU64,
    ops_emitted: AtomicU64,
}

impl Stats {
    fn bump(counter: &AtomicU64, by: u64) {
        counter.fetch_add(by, Ordering::Relaxed);
    }
}

struct State<U, K> {
    index: DerivedIndex<K>,
    /// The parameter values the index reflects.
    params: Params,
    /// An emptied snapshot buffer, reused by the next drain.
    spare_params: Params,
    /// The index does not describe the source: the next drain rebuilds it.
    needs_rebuild: bool,
    /// A section that runs pipeline closures is in progress; still set when a lock is taken, it
    /// unwound.
    in_flight: bool,
    /// The derived ops the host has not received.
    out: Out<U>,
    /// The materialised view, until it changes.
    cache: Option<Arc<Vec<U>>>,
    /// What the host has, replayed from every full value and patch sent (debug builds check every
    /// patch against the view with it).
    #[cfg(debug_assertions)]
    shadow: Option<Vec<U>>,
}

/// The derived ops waiting for the next commit, kept only while the host observes the list.
struct Out<U> {
    /// The host has the view as of the last full value plus `ops`: patches are being kept.
    armed: bool,
    /// Kept ops overflowed (or the view was rebuilt): the next commit sends the full value.
    stale: bool,
    ops: Vec<PatchOp<U>>,
}

impl<U> Out<U> {
    fn recording(&self) -> bool {
        self.armed && !self.stale
    }

    fn push(&mut self, op: PatchOp<U>) {
        if !self.recording() {
            return;
        }
        if self.ops.len() >= OUT_LIMIT {
            self.go_stale();
            return;
        }
        self.ops.push(op);
    }

    fn go_stale(&mut self) {
        if self.armed {
            self.stale = true;
            self.ops.clear();
        }
    }

    fn arm(&mut self) {
        self.armed = true;
        self.stale = false;
        self.ops.clear();
    }

    fn disarm(&mut self) {
        self.armed = false;
        self.stale = false;
        self.ops = Vec::new();
    }
}

/// A view rank as the wire carries it; `None` past `u32::MAX`.
fn wire(rank: usize) -> Option<u32> {
    u32::try_from(rank).ok()
}

thread_local! {
    /// The derived lists draining on this thread (by address), innermost last.
    static DRAINING: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Marks a list as draining on this thread until dropped.
struct Draining;

impl Draining {
    fn enter(node: usize) -> Draining {
        let _ = DRAINING.try_with(|stack| stack.borrow_mut().push(node));
        Draining
    }
}

impl Drop for Draining {
    fn drop(&mut self) {
        let _ = DRAINING.try_with(|stack| stack.borrow_mut().pop());
    }
}

/// Panics if this thread is draining `node`: waiting for its drain lock would never end.
pub(crate) fn check_cycle(node: usize) {
    let cycle = DRAINING
        .try_with(|stack| stack.borrow().contains(&node))
        .unwrap_or(false);
    assert!(
        !cycle,
        "undra-signals: derived list cycle detected: a derived list's filter, sort-key or map \
         closure read the derived list it is computing, directly or through a computed or another \
         derived list. That would deadlock: the list's index is locked while its closures run. \
         Pipeline closures must be pure functions of their arguments: pass other values as \
         parameters (`filter_with`, `sort_by_key_with`)."
    );
}

impl<S, U, K> DerivedInner<S, U, K>
where
    S: SignalValue,
    U: SignalValue,
    K: Ord + Clone + Send + Sync + 'static,
{
    /// Builds the node and wires it to its source and parameters.
    pub(crate) fn build(
        source: Signal<Vec<S>>,
        eval: Box<EvalFn<S, U, K>>,
        params: Vec<Box<dyn ParamSlot>>,
        sorted: bool,
    ) -> Arc<DerivedInner<S, U, K>> {
        let tap = Arc::new(SourceTap::new());
        source.tap_list().register(&tap);
        let inner = Arc::new(DerivedInner {
            source,
            tap,
            eval,
            params,
            sorted,
            state: Mutex::new(State {
                index: DerivedIndex::new(sorted),
                params: Params::default(),
                spare_params: Params::default(),
                needs_rebuild: true,
                in_flight: false,
                out: Out {
                    armed: false,
                    stale: false,
                    ops: Vec::new(),
                },
                cache: None,
                #[cfg(debug_assertions)]
                shadow: None,
            }),
            visited: AtomicU64::new(0),
            dependents: parking_lot::Mutex::new(Vec::new()),
            binding: OnceLock::new(),
            stats: Stats::default(),
        });
        // Subscribe only once the node is fully built, as `Computed::new` does.
        let weak: Weak<dyn Reactive> = Arc::downgrade(&inner) as Weak<dyn Reactive>;
        inner.source.add_dependent(weak.clone());
        for param in &inner.params {
            param.subscribe(weak.clone());
        }
        inner
    }

    fn addr(&self) -> usize {
        std::ptr::from_ref(self) as usize
    }

    /// Runs `f` with the state locked and drained, inside a transaction (writes some closure makes
    /// commit after the lock is released) and with this list marked as draining on this thread.
    /// `f` receives the list snapshot when `need_snapshot` (or a rebuild) took one.
    fn drained<R>(
        &self,
        need_snapshot: bool,
        f: impl FnOnce(&Self, &mut State<U, K>, Option<Arc<Vec<S>>>) -> R,
    ) -> R {
        // Declared first so it is dropped last: after the drain lock is released.
        let _txn = TxnGuard::enter();
        check_cycle(self.addr());
        // ADR-021 L3: inside the source's `update` / `update_at` closure the list is half changed
        // (and its lock held), whether or not this drain would need it.
        self.source.assert_not_updating("read");
        let mut state = self.state.lock();
        let _draining = Draining::enter(self.addr());
        if state.in_flight {
            // The last section that ran closures unwound: nothing it left can be trusted.
            state.needs_rebuild = true;
            state.out.go_stale();
            state.cache = None;
        }
        state.in_flight = true;
        let current = self.drain(&mut state, need_snapshot);
        let result = f(self, &mut state, current);
        state.in_flight = false;
        result
    }

    /// Brings the index up to date (see the module docs). Returns the list snapshot when one was
    /// taken: always when `need_snapshot`.
    fn drain(&self, st: &mut State<U, K>, need_snapshot: bool) -> Option<Arc<Vec<S>>> {
        let params_changed = self.snapshot_params(st);
        if !st.needs_rebuild && !need_snapshot && !params_changed {
            match self.tap.take_ops() {
                Some(ops) => {
                    self.replay(st, ops);
                    self.adopt_params(st);
                    return None;
                }
                None => st.needs_rebuild = true,
            }
        }
        let taken = self.source.read_locked(|current| self.tap.take(current));
        match taken {
            TapTaken::Ops { ops, current } if !st.needs_rebuild => {
                // Source ops first, with the values the index was built with; then the walk.
                self.replay(st, ops);
                if params_changed && !st.needs_rebuild {
                    self.walk(st, &current);
                }
                if st.needs_rebuild {
                    self.rebuild(st, &current);
                } else {
                    self.adopt_params(st);
                }
                Some(current)
            }
            TapTaken::Ops { ops, current } => {
                self.tap.recycle(ops);
                self.rebuild(st, &current);
                Some(current)
            }
            TapTaken::Fresh { current } => {
                self.rebuild(st, &current);
                Some(current)
            }
        }
    }

    /// Snapshots every parameter into `st.spare_params` (outside the deriving scope: reading a
    /// parameter may run a computed's closure, which may read signals) and says whether any value
    /// differs from the one the index reflects.
    fn snapshot_params(&self, st: &mut State<U, K>) -> bool {
        if self.params.is_empty() {
            return false;
        }
        let mut fresh = std::mem::take(&mut st.spare_params);
        fresh.0.clear();
        fresh
            .0
            .extend(self.params.iter().map(|param| param.snapshot()));
        let changed = st.params.0.len() != fresh.0.len()
            || self
                .params
                .iter()
                .zip(st.params.0.iter().zip(&fresh.0))
                .any(|(param, (old, new))| !param.same(old, new));
        st.spare_params = fresh;
        changed
    }

    /// The snapshot just taken becomes the values the index reflects (when they are equal this
    /// keeps the newest allocation, so the next comparison is a pointer check).
    fn adopt_params(&self, st: &mut State<U, K>) {
        if self.params.is_empty() {
            return;
        }
        std::mem::swap(&mut st.params, &mut st.spare_params);
        st.spare_params.0.clear();
    }

    /// Evaluates one row inside the deriving scope.
    fn evaluate<'a>(&self, params: &Params, row: Cow<'a, S>) -> Option<(K, Cow<'a, U>)> {
        let _scope = DerivingScope::enter();
        (self.eval)(params, row)
    }

    /// Replays the source's ops on the index, oldest first, emitting at most two derived ops per
    /// source op (ADR-039 section 2).
    fn replay(&self, st: &mut State<U, K>, mut ops: Vec<PatchOp<S>>) {
        let mut produced = 0_usize;
        for op in ops.drain(..) {
            if st.needs_rebuild {
                // An op did not fit the index (it cannot, by the tap invariant): the rest is moot.
                break;
            }
            produced += self.apply(st, op);
        }
        self.tap.recycle(ops);
        if produced > 0 {
            st.cache = None;
        }
    }

    /// One source op; returns how many derived ops it caused.
    fn apply(&self, st: &mut State<U, K>, op: PatchOp<S>) -> usize {
        let State {
            index,
            params,
            needs_rebuild,
            out,
            ..
        } = st;
        let len = index.source_len();
        match op {
            PatchOp::Insert { index: at, item } => {
                let at = at as usize;
                if at > len {
                    *needs_rebuild = true;
                    return 0;
                }
                match self.evaluate(params, Cow::Owned(item)) {
                    None => {
                        index.insert(at, None);
                        0
                    }
                    Some((key, value)) => {
                        let rank = index.insert(at, Some(key)).unwrap_or_default();
                        emit_insert(out, rank, value);
                        1
                    }
                }
            }
            PatchOp::Remove { index: at } => {
                let at = at as usize;
                if at >= len {
                    *needs_rebuild = true;
                    return 0;
                }
                match index.remove(at) {
                    Some(rank) => {
                        emit(out, rank, |index| PatchOp::Remove { index });
                        1
                    }
                    None => 0,
                }
            }
            PatchOp::Update { index: at, item } => {
                let at = at as usize;
                if at >= len {
                    *needs_rebuild = true;
                    return 0;
                }
                let (key, value) = split(self.evaluate(params, Cow::Owned(item)));
                match index.update(at, key) {
                    Step::None => 0,
                    Step::Insert(rank) => {
                        emit_insert(out, rank, value.expect("a row that enters has a value"));
                        1
                    }
                    Step::Remove(rank) => {
                        emit(out, rank, |index| PatchOp::Remove { index });
                        1
                    }
                    Step::Stay(rank) => {
                        emit_update(out, rank, value.expect("a row that stays has a value"));
                        1
                    }
                    Step::Move { from, to } => {
                        emit_move(out, from, to);
                        emit_update(out, to, value.expect("a row that moves has a value"));
                        2
                    }
                }
            }
            PatchOp::Move { from, to } => {
                let (from, to) = (from as usize, to as usize);
                if from >= len || to >= len {
                    *needs_rebuild = true;
                    return 0;
                }
                match index.move_row(from, to) {
                    Some((before, after)) => {
                        emit_move(out, before, after);
                        1
                    }
                    None => 0,
                }
            }
            PatchOp::Clear => {
                if index.clear() {
                    out.push(PatchOp::Clear);
                    1
                } else {
                    0
                }
            }
        }
    }

    /// A parameter changed: re-evaluates every row with the new values (in `st.spare_params`) and
    /// applies the transition of each row whose membership or sort key changed. At most
    /// [`PARAM_WALK_LIMIT`] ops are sent; beyond that the view goes as a full value. O(n)
    /// evaluations plus O(k log n) for k changed rows.
    fn walk(&self, st: &mut State<U, K>, current: &Arc<Vec<S>>) {
        let State {
            index,
            spare_params: params,
            out,
            cache,
            ..
        } = st;
        debug_assert_eq!(
            index.source_len(),
            current.len(),
            "the walk sees the list the index has"
        );
        let mut sent = 0_usize;
        let mut changed = false;
        let mut row = index.first_row();
        for item in current.iter() {
            if row == NIL {
                break;
            }
            let (key, value) = split(self.evaluate(params, Cow::Borrowed(item)));
            let step = index.transition(row, key);
            let ops = match step {
                Step::None | Step::Stay(_) => 0,
                // No stage maps with a parameter in v1, so a row that stays did not change.
                Step::Insert(_) | Step::Remove(_) | Step::Move { .. } => 1,
            };
            if ops > 0 {
                changed = true;
                sent += ops;
                if sent > PARAM_WALK_LIMIT {
                    out.go_stale();
                } else {
                    match step {
                        Step::Insert(rank) => {
                            emit_insert(out, rank, value.expect("a row that enters has a value"));
                        }
                        Step::Remove(rank) => emit(out, rank, |index| PatchOp::Remove { index }),
                        Step::Move { from, to } => emit_move(out, from, to),
                        Step::None | Step::Stay(_) => {}
                    }
                }
            }
            row = index.next_row(row);
        }
        if changed {
            *cache = None;
        }
    }

    /// Rebuilds the index from `current` with the parameter values just snapshotted (or the ones
    /// it has, when none were taken). The host's copy is then sent whole.
    fn rebuild(&self, st: &mut State<U, K>, current: &Arc<Vec<S>>) {
        if !self.params.is_empty() && st.spare_params.0.len() == self.params.len() {
            std::mem::swap(&mut st.params, &mut st.spare_params);
            st.spare_params.0.clear();
        } else if st.params.0.len() != self.params.len() {
            st.params.0 = self.params.iter().map(|param| param.snapshot()).collect();
        }
        st.needs_rebuild = true;
        let keys: Vec<Option<K>> = current
            .iter()
            .map(|item| {
                self.evaluate(&st.params, Cow::Borrowed(item))
                    .map(|(key, _)| key)
            })
            .collect();
        st.index.rebuild(keys);
        st.needs_rebuild = false;
        st.out.go_stale();
        st.cache = None;
        Stats::bump(&self.stats.rebuilds, 1);
    }

    /// The view as a `Vec`: the cache, or the passing rows evaluated in view order (no sort: the
    /// index has the order). O(n) and cached until the view changes.
    fn materialise_from(&self, st: &mut State<U, K>, current: &Arc<Vec<S>>) -> Arc<Vec<U>> {
        if let Some(cache) = &st.cache {
            return Arc::clone(cache);
        }
        let sources = st.index.view_sources();
        let mut view = Vec::with_capacity(sources.len());
        let mut pure = true;
        for at in sources {
            match self.evaluate(&st.params, Cow::Borrowed(&current[at])) {
                Some((_, value)) => view.push(value.into_owned()),
                None => {
                    pure = false;
                    break;
                }
            }
        }
        if !pure {
            // The index says the row passes and the pipeline now says it does not: a closure
            // that is not a pure function of its arguments.
            debug_assert!(
                pure,
                "{}",
                purity_message("gave a different answer for a row that did not change")
            );
            st.needs_rebuild = true;
            st.out.go_stale();
            return Arc::new(self.reference(st, current));
        }
        let view = Arc::new(view);
        st.cache = Some(Arc::clone(&view));
        view
    }

    /// `stable_sort_by_key(filter_map(source))` computed directly, without the index: what an
    /// impure pipeline is answered with in release builds until the next drain rebuilds.
    fn reference(&self, st: &State<U, K>, current: &Arc<Vec<S>>) -> Vec<U> {
        let mut rows: Vec<(K, U)> = current
            .iter()
            .filter_map(|item| {
                self.evaluate(&st.params, Cow::Borrowed(item))
                    .map(|(key, value)| (key, value.into_owned()))
            })
            .collect();
        if self.sorted {
            rows.sort_by(|a, b| a.0.cmp(&b.0));
        }
        rows.into_iter().map(|(_, value)| value).collect()
    }

    /// Materialises with the snapshot a drain just took, or takes one.
    fn materialise_in(
        &self,
        st: &mut State<U, K>,
        current: Option<Arc<Vec<S>>>,
    ) -> (Arc<Vec<U>>, Arc<Vec<S>>) {
        let current = match current {
            Some(current) => current,
            None => self
                .drain(st, true)
                .expect("a drain that needs a snapshot takes one"),
        };
        (self.materialise_from(st, &current), current)
    }

    /// Debug builds: the view's keys are unique (a duplicate breaks `ForEach` / `LazyColumn`
    /// identity on the platform; maintenance does not care).
    #[cfg(debug_assertions)]
    fn check_keys(view: &[U], key: fn(&U) -> u64) {
        let mut keys: Vec<(u64, usize)> = view
            .iter()
            .enumerate()
            .map(|(at, row)| (key(row), at))
            .collect();
        keys.sort_unstable();
        if let Some(pair) = keys.windows(2).find(|pair| pair[0].0 == pair[1].0) {
            let (first, second) = (pair[0].1.min(pair[1].1), pair[0].1.max(pair[1].1));
            panic!(
                "undra-signals: a derived list's view has two rows with the same key, at positions \
                 {first} and {second}. The platforms identify rows by key (SwiftUI `ForEach`, \
                 Compose `LazyColumn`, React keys): keep the source's keys unique, or map to rows \
                 whose key field is."
            );
        }
    }

    /// The host was just sent `view` in full.
    #[allow(unused_variables)]
    fn sent_full(st: &mut State<U, K>, view: &Arc<Vec<U>>, key: fn(&U) -> u64) {
        #[cfg(debug_assertions)]
        {
            Self::check_keys(view, key);
            st.shadow = Some(view.as_ref().clone());
        }
    }
}

/// Splits an evaluation into the index's part and the emitted value.
fn split<'a, K, U: Clone>(evaluated: Option<(K, Cow<'a, U>)>) -> (Option<K>, Option<Cow<'a, U>>) {
    match evaluated {
        Some((key, value)) => (Some(key), Some(value)),
        None => (None, None),
    }
}

/// Emits an op that carries only a rank.
fn emit<U>(out: &mut Out<U>, rank: usize, op: impl FnOnce(u32) -> PatchOp<U>) {
    if !out.recording() {
        return;
    }
    match wire(rank) {
        Some(index) => out.push(op(index)),
        None => out.go_stale(),
    }
}

/// Emits an `Insert`, cloning a borrowed value only when it is kept.
fn emit_insert<U: Clone>(out: &mut Out<U>, rank: usize, value: Cow<'_, U>) {
    emit(out, rank, |index| PatchOp::Insert {
        index,
        item: value.into_owned(),
    });
}

/// Emits an `Update`, cloning a borrowed value only when it is kept.
fn emit_update<U: Clone>(out: &mut Out<U>, rank: usize, value: Cow<'_, U>) {
    emit(out, rank, |index| PatchOp::Update {
        index,
        item: value.into_owned(),
    });
}

fn emit_move<U>(out: &mut Out<U>, from: usize, to: usize) {
    if !out.recording() {
        return;
    }
    match (wire(from), wire(to)) {
        (Some(from), Some(to)) => out.push(PatchOp::Move { from, to }),
        _ => out.go_stale(),
    }
}

impl<S, U, K> DerivedNode<U> for DerivedInner<S, U, K>
where
    S: SignalValue,
    U: SignalValue,
    K: Ord + Clone + Send + Sync + 'static,
{
    fn view_len(&self) -> usize {
        self.drained(false, |_, st, _| st.index.view_len())
    }

    fn materialise(&self) -> Arc<Vec<U>> {
        self.drained(false, |node, st, current| {
            if current.is_none() {
                if let Some(cache) = &st.cache {
                    return Arc::clone(cache);
                }
            }
            node.materialise_in(st, current).0
        })
    }

    fn stats(&self) -> DerivedStats {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        DerivedStats {
            rebuilds: load(&self.stats.rebuilds),
            full_values: load(&self.stats.full_values),
            patches: load(&self.stats.patches),
            ops_emitted: load(&self.stats.ops_emitted),
        }
    }

    fn binding(&self) -> &OnceLock<Binding> {
        &self.binding
    }

    fn add_dependent(&self, dependent: Weak<dyn Reactive>) {
        add_dependent(&self.dependents, dependent);
    }

    fn commit(&self, w: &mut Writer, retain: bool, key: fn(&U) -> u64) -> Emitted {
        // Debug builds check every patch against the view, which needs the snapshot the drain
        // replayed up to; an unretained slot always sends the full value.
        let need_snapshot = cfg!(debug_assertions) || !retain;
        self.drained(need_snapshot, |node, st, current| {
            if !retain {
                let (view, _) = node.materialise_in(st, current);
                view.encode(w);
                Stats::bump(&node.stats.full_values, 1);
                return Emitted::Full;
            }
            if !st.out.armed || st.out.stale {
                let (view, _) = node.materialise_in(st, current);
                view.encode(w);
                st.out.arm();
                Self::sent_full(st, &view, key);
                Stats::bump(&node.stats.full_values, 1);
                return Emitted::Full;
            }
            if st.out.ops.is_empty() {
                return Emitted::Nothing;
            }
            let patch = KeyedPatch {
                ops: std::mem::take(&mut st.out.ops),
            };
            patch.encode(w);
            Stats::bump(&node.stats.patches, 1);
            Stats::bump(&node.stats.ops_emitted, patch.ops.len() as u64);
            #[cfg(debug_assertions)]
            {
                let (view, _) = node.materialise_in(st, current);
                if let Some(shadow) = st.shadow.as_mut() {
                    assert!(
                        patch.apply(shadow).is_ok(),
                        "undra-signals: a derived list's patch does not apply to what the host has"
                    );
                    assert!(
                        same_encoding(shadow, &view),
                        "undra-signals: a derived list's patches drifted from its view"
                    );
                }
            }
            let mut ops = patch.ops;
            ops.clear();
            st.out.ops = ops;
            Emitted::Patch
        })
    }

    fn resync(&self, w: &mut Writer, key: fn(&U) -> u64) {
        self.drained(true, |node, st, current| {
            let (view, _) = node.materialise_in(st, current);
            view.encode(w);
            st.out.arm();
            Self::sent_full(st, &view, key);
            Stats::bump(&node.stats.full_values, 1);
        });
    }

    fn forget(&self) {
        check_cycle(self.addr());
        self.state.lock().out.disarm();
    }

    fn id(&self) -> usize {
        self.addr()
    }
}

impl<S, U, K> Reactive for DerivedInner<S, U, K>
where
    S: SignalValue,
    U: SignalValue,
    K: Ord + Clone + Send + Sync + 'static,
{
    fn invalidate(self: Arc<Self>, pass: u64) {
        if self.visited.swap(pass, Ordering::Relaxed) == pass {
            return;
        }
        if let Some(binding) = self.binding.get() {
            record(binding);
        }
        propagate(&self.dependents, pass);
    }
}

impl<S: Clone, U: Clone, K> fmt::Debug for DerivedInner<S, U, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DerivedInner")
            .field("sorted", &self.sorted)
            .field("parameters", &self.params.len())
            .finish_non_exhaustive()
    }
}

/// Whether `a` and `b` are the same list as the host would see it.
#[cfg(debug_assertions)]
fn same_encoding<U: SignalValue>(a: &[U], b: &[U]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            let (mut left, mut right) = (Writer::new(), Writer::new());
            x.encode(&mut left);
            y.encode(&mut right);
            left.as_slice() == right.as_slice()
        })
}
