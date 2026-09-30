//! Transactions and the commit loop.
//!
//! Every write happens inside a transaction: either one opened with [`txn`] or an implicit
//! one-write transaction around a bare `set`. Transactions are per thread. A thread keeps a
//! depth counter and a list of the store slots (and effects) its writes made dirty; when the
//! outermost transaction ends, that list is committed:
//!
//! 1. the dirty slots are grouped by store;
//! 2. for each store, the slots that are observed (or `no_coalesce`) are encoded into **one**
//!    change-set and handed to the sink;
//! 3. effects whose inputs changed run;
//! 4. if any of that wrote signals, those writes form a new transaction and the loop repeats.
//!
//! Commit cost is proportional to the number of dirty slots, never to the number of signals in
//! a store, because the list is built by the writes themselves.
//!
//! # Re-entrancy
//!
//! No lock of this crate is held while user code (a sink, a computed closure, an effect, an
//! encoder) runs. A write made by such code while a commit is in progress is recorded but not
//! committed on the spot; the commit loop picks it up as a **new transaction** when the current
//! round is done. That keeps change-sets in commit order and rules out re-entrant commits.
//!
//! # The round cap
//!
//! Effects (or computeds, or sinks) that keep writing signals that trigger themselves would
//! keep the loop going forever, so a commit is cut off after [`MAX_COMMIT_ROUNDS`] rounds. The
//! cut-off does not strand anything: the queued effects are dropped from the queue, the changes
//! already dirty are delivered one last time, whatever that queued in turn is released (its slots
//! are remembered as unsent for the next commit of the store), and the sink is told through
//! [`ChangeSink::round_cap_hit`]. A slot left dirty with nobody to deliver it, or an effect left
//! marked as queued, would otherwise be skipped by every other thread's writes for good.
//!
//! # Panics
//!
//! Every place that runs user code inside a commit is wrapped in `catch_unwind`, so a panicking
//! sink or effect cannot leave the thread's transaction state half-updated or make the commit
//! lose other stores' change-sets. The first caught panic is re-raised once the commit has
//! finished (unless the thread is already unwinding).

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::effect::EffectInner;
use crate::sink::{self, ChangeSink};
use crate::store::StoreCell;

/// Upper bound on commit rounds in one outermost commit. Effects or sinks that keep writing
/// signals that trigger themselves would otherwise loop forever. After this many rounds the
/// commit stops running effects, delivers the changes that are already dirty one last time,
/// releases everything else (so that no slot or effect stays claimed by this thread) and reports
/// the cut-off through [`ChangeSink::round_cap_hit`](crate::ChangeSink::round_cap_hit).
pub(crate) const MAX_COMMIT_ROUNDS: usize = 1000;

static TXN_ID: AtomicU64 = AtomicU64::new(1);

/// Returns a fresh transaction id: monotonic, starting at 1, unique within the process.
///
/// Used by commits, and by the runtime for the change-set it emits on `observe`.
pub fn next_txn_id() -> u64 {
    TXN_ID.fetch_add(1, Ordering::Relaxed)
}

struct TxnState {
    /// Open [`txn`] scopes on this thread.
    depth: usize,
    /// A commit loop is running on this thread.
    committing: bool,
    /// Slots dirtied since the last commit round, in the order they were dirtied.
    writes: Vec<(Arc<StoreCell>, u32)>,
    /// An empty `writes` vector kept around so that steady-state commits do not reallocate.
    spare: Vec<(Arc<StoreCell>, u32)>,
    /// Effects whose inputs changed since the last commit round.
    effects: Vec<Arc<EffectInner>>,
}

impl TxnState {
    const fn new() -> Self {
        TxnState {
            depth: 0,
            committing: false,
            writes: Vec::new(),
            spare: Vec::new(),
            effects: Vec::new(),
        }
    }

    fn has_pending(&self) -> bool {
        !self.writes.is_empty() || !self.effects.is_empty()
    }
}

thread_local! {
    static TXN: RefCell<TxnState> = const { RefCell::new(TxnState::new()) };
    /// Scratch buffer reused for change-set payloads so a commit allocates only when a payload
    /// outgrows every earlier one on this thread.
    static BUFFER: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Buffers larger than this are not kept for reuse.
const MAX_KEPT_BUFFER: usize = 64 * 1024;

/// Write lists with more capacity than this are not kept for reuse.
const MAX_KEPT_WRITES: usize = 4096;

/// Records that slot `signal_id` of `cell` was dirtied by the current transaction.
pub(crate) fn push_write(cell: Arc<StoreCell>, signal_id: u32) {
    let _ = TXN.try_with(|t| t.borrow_mut().writes.push((cell, signal_id)));
}

/// Records that `effect` has to run when the current transaction commits.
pub(crate) fn push_effect(effect: Arc<EffectInner>) {
    let _ = TXN.try_with(|t| t.borrow_mut().effects.push(effect));
}

/// Keeps a transaction open for as long as it lives; commits when the outermost one drops.
///
/// Being a guard makes transactions exception-safe: if the closure passed to [`txn`] panics,
/// the depth is still restored and the writes already applied are still committed, so the next
/// write on the thread starts from a clean state.
pub(crate) struct TxnGuard(());

impl TxnGuard {
    pub(crate) fn enter() -> TxnGuard {
        let _ = TXN.try_with(|t| t.borrow_mut().depth += 1);
        TxnGuard(())
    }
}

impl Drop for TxnGuard {
    fn drop(&mut self) {
        let commit_now = TXN
            .try_with(|t| {
                let mut t = t.borrow_mut();
                t.depth = t.depth.saturating_sub(1);
                t.depth == 0 && !t.committing && t.has_pending()
            })
            .unwrap_or(false);
        if commit_now {
            commit();
        }
    }
}

/// Runs `f` as one transaction: every signal written inside is committed together, producing
/// one change-set per store, when the outermost `txn` returns.
///
/// Calls nest; an inner `txn` joins the outer transaction and commits nothing itself. The
/// transaction is exception-safe: if `f` panics, writes that were already applied are still
/// committed and the thread is left in a clean state before the panic continues.
///
/// # Example
///
/// ```
/// use keel_signals::{txn, Signal};
///
/// let a = Signal::new(1);
/// let b = Signal::new(2);
/// let sum = txn(|| {
///     a.set(10);
///     b.set(20);
///     a.get() + b.get()
/// });
/// assert_eq!(sum, 30);
/// ```
pub fn txn<R>(f: impl FnOnce() -> R) -> R {
    let _guard = TxnGuard::enter();
    f()
}

/// Marks a commit loop as running on this thread for as long as it lives.
struct CommitFlag;

impl CommitFlag {
    /// `None` if a commit loop is already running (which cannot happen through the public
    /// API: writes made during a commit are queued, not committed).
    fn set() -> Option<CommitFlag> {
        let acquired = TXN
            .try_with(|t| {
                let mut t = t.borrow_mut();
                if t.committing {
                    false
                } else {
                    t.committing = true;
                    true
                }
            })
            .unwrap_or(false);
        acquired.then_some(CommitFlag)
    }
}

impl Drop for CommitFlag {
    fn drop(&mut self) {
        let _ = TXN.try_with(|t| t.borrow_mut().committing = false);
    }
}

type PanicPayload = Box<dyn Any + Send>;

/// The work of one commit round.
struct Batch {
    writes: Vec<(Arc<StoreCell>, u32)>,
    effects: Vec<Arc<EffectInner>>,
}

fn take_batch() -> Option<Batch> {
    TXN.try_with(|t| {
        let mut t = t.borrow_mut();
        if !t.has_pending() {
            return None;
        }
        let spare = std::mem::take(&mut t.spare);
        Some(Batch {
            writes: std::mem::replace(&mut t.writes, spare),
            effects: std::mem::take(&mut t.effects),
        })
    })
    .unwrap_or(None)
}

/// Commits everything the thread's transactions have queued.
fn commit() {
    let Some(flag) = CommitFlag::set() else {
        return;
    };
    let mut first_panic: Option<PanicPayload> = None;
    let mut rounds = 0;
    while let Some(batch) = take_batch() {
        if rounds == MAX_COMMIT_ROUNDS {
            cut_off(batch, &mut first_panic);
            break;
        }
        rounds += 1;
        run_round(batch, &mut first_panic);
    }
    drop(flag);
    if let Some(payload) = first_panic {
        if !std::thread::panicking() {
            resume_unwind(payload);
        }
    }
}

/// One store's share of a round.
struct Group {
    cell: Arc<StoreCell>,
    ids: Vec<u32>,
}

/// Groups `writes` by store, keeping stores in the order they were first written and each
/// store's ids in write order. Leaves `writes` empty (with its capacity) so it can be reused.
fn group_by_store(writes: &mut Vec<(Arc<StoreCell>, u32)>) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    // Built only once a second store shows up: the common case is one store per round.
    let mut index: HashMap<usize, usize> = HashMap::new();
    for (cell, id) in writes.drain(..) {
        let key = Arc::as_ptr(&cell) as usize;
        if let Some(last) = groups.last_mut() {
            if Arc::as_ptr(&last.cell) as usize == key {
                last.ids.push(id);
                continue;
            }
        }
        if index.is_empty() && groups.len() == 1 {
            index.insert(Arc::as_ptr(&groups[0].cell) as usize, 0);
        }
        match index.get(&key) {
            Some(&i) => groups[i].ids.push(id),
            None => {
                if !groups.is_empty() {
                    index.insert(key, groups.len());
                }
                groups.push(Group {
                    cell,
                    ids: vec![id],
                });
            }
        }
    }
    groups
}

/// Keeps an emptied `writes` vector for the next round, unless it grew unusually large.
fn recycle_writes(writes: Vec<(Arc<StoreCell>, u32)>) {
    if writes.capacity() <= MAX_KEPT_WRITES {
        let _ = TXN.try_with(|t| t.borrow_mut().spare = writes);
    }
}

fn run_round(batch: Batch, first_panic: &mut Option<PanicPayload>) {
    let sink = sink::current();
    let Batch { writes, effects } = batch;
    commit_stores(writes, sink.as_ref(), first_panic);

    for effect in effects {
        if let Err(payload) = catch_unwind(AssertUnwindSafe(|| effect.run_if_dirty())) {
            first_panic.get_or_insert(payload);
        }
    }
}

/// Builds and delivers the change-set of every store in `writes`, one store at a time. A panic
/// abandons that store's change-set only (see `StoreCell::commit_slots`) and is kept to be
/// re-raised.
fn commit_stores(
    mut writes: Vec<(Arc<StoreCell>, u32)>,
    sink: Option<&Arc<dyn ChangeSink>>,
    first_panic: &mut Option<PanicPayload>,
) {
    let mut txn_id: Option<u64> = None;
    let groups = group_by_store(&mut writes);
    recycle_writes(writes);
    for Group { cell, ids } in groups {
        let outcome = catch_unwind(AssertUnwindSafe(|| {
            cell.commit_slots(ids, sink, &mut txn_id);
        }));
        if let Err(payload) = outcome {
            first_panic.get_or_insert(payload);
        }
    }
}

/// The round cap was reached: something keeps re-triggering itself.
///
/// The work still queued is dealt with so that it does not stay claimed by this thread. Every
/// slot a queued write dirtied is a slot other threads would skip (they see it dirty and assume
/// this thread will deliver it), and every queued effect is one that would never be queued
/// again. So: the queued effects are cancelled (a later write queues them again), the queued
/// writes get one last delivery, and whatever that delivery queued in turn is released with its
/// slots remembered as unsent, for the next commit of the store. Finally the sink is told.
fn cut_off(batch: Batch, first_panic: &mut Option<PanicPayload>) {
    let sink = sink::current();
    let Batch { writes, effects } = batch;
    for effect in effects {
        effect.cancel_queued_run();
    }
    commit_stores(writes, sink.as_ref(), first_panic);
    if let Some(rest) = take_batch() {
        for effect in rest.effects {
            effect.cancel_queued_run();
        }
        for (cell, signal_id) in rest.writes {
            cell.defer(signal_id);
        }
    }
    if let Some(sink) = sink {
        let reported = catch_unwind(AssertUnwindSafe(|| sink.round_cap_hit(MAX_COMMIT_ROUNDS)));
        if let Err(payload) = reported {
            first_panic.get_or_insert(payload);
        }
    }
}

/// Takes this thread's reusable payload buffer (empty, with whatever capacity it kept).
pub(crate) fn take_buffer() -> Vec<u8> {
    BUFFER
        .try_with(|b| std::mem::take(&mut *b.borrow_mut()))
        .unwrap_or_default()
}

/// Gives a payload buffer back for reuse.
pub(crate) fn recycle_buffer(mut buf: Vec<u8>) {
    if buf.capacity() > MAX_KEPT_BUFFER {
        return;
    }
    buf.clear();
    let _ = BUFFER.try_with(|b| *b.borrow_mut() = buf);
}

#[cfg(test)]
pub(crate) fn pending_len() -> usize {
    TXN.with(|t| {
        let t = t.borrow();
        t.writes.len() + t.effects.len()
    })
}

#[cfg(test)]
pub(crate) fn depth() -> usize {
    TXN.with(|t| t.borrow().depth)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Signal;
    use crate::testing::CaptureSink;

    #[test]
    fn txn_ids_increase() {
        let a = next_txn_id();
        let b = next_txn_id();
        let c = next_txn_id();
        assert!(a >= 1);
        assert!(b > a && c > b);
    }

    #[test]
    fn txn_returns_the_closure_value() {
        assert_eq!(txn(|| 41 + 1), 42);
    }

    #[test]
    fn depth_tracks_nesting_and_returns_to_zero() {
        assert_eq!(depth(), 0);
        txn(|| {
            assert_eq!(depth(), 1);
            txn(|| {
                assert_eq!(depth(), 2);
                txn(|| assert_eq!(depth(), 3));
            });
            assert_eq!(depth(), 1);
        });
        assert_eq!(depth(), 0);
    }

    #[test]
    fn depth_is_restored_when_the_closure_panics() {
        let result = std::panic::catch_unwind(|| {
            txn(|| {
                txn(|| panic!("boom"));
            });
        });
        assert!(result.is_err());
        assert_eq!(depth(), 0);
        assert_eq!(pending_len(), 0);
    }

    #[test]
    fn a_thread_starts_clean_and_bare_writes_leave_nothing_pending() {
        let sink = CaptureSink::new();
        crate::with_sink(sink, || {
            let s = Signal::new(1);
            s.set(2);
            assert_eq!(pending_len(), 0);
            assert_eq!(depth(), 0);
        });
    }

    #[test]
    fn buffer_is_reused_up_to_the_cap() {
        let mut buf = take_buffer();
        buf.extend_from_slice(&[0; 1024]);
        let cap = buf.capacity();
        recycle_buffer(buf);
        let again = take_buffer();
        assert!(again.is_empty());
        assert!(again.capacity() >= cap.min(1024));
        // Oversized buffers are dropped rather than hoarded.
        recycle_buffer(Vec::with_capacity(MAX_KEPT_BUFFER + 1));
        assert!(take_buffer().capacity() <= MAX_KEPT_BUFFER);
    }

    #[test]
    fn grouping_preserves_first_seen_order_and_dedups_stores() {
        let a = StoreCell::new(1);
        let b = StoreCell::new(2);
        let mut writes = vec![
            (a.clone(), 3),
            (a.clone(), 1),
            (b.clone(), 0),
            (a.clone(), 2),
            (b.clone(), 5),
        ];
        let groups = group_by_store(&mut writes);
        assert!(writes.is_empty(), "the vector is drained for reuse");
        assert!(writes.capacity() >= 5, "and keeps its capacity");
        assert_eq!(groups.len(), 2);
        assert!(Arc::ptr_eq(&groups[0].cell, &a));
        assert_eq!(groups[0].ids, vec![3, 1, 2]);
        assert!(Arc::ptr_eq(&groups[1].cell, &b));
        assert_eq!(groups[1].ids, vec![0, 5]);
    }

    #[test]
    fn grouping_many_stores() {
        let cells: Vec<_> = (0..50).map(StoreCell::new).collect();
        let mut writes = Vec::new();
        for round in 0..3_u32 {
            for cell in &cells {
                writes.push((cell.clone(), round));
            }
        }
        let groups = group_by_store(&mut writes);
        assert_eq!(groups.len(), 50);
        for (g, cell) in groups.iter().zip(&cells) {
            assert!(Arc::ptr_eq(&g.cell, cell));
            assert_eq!(g.ids, vec![0, 1, 2]);
        }
    }
}
