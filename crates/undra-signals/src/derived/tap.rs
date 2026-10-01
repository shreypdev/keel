//! Source taps (ADR-039 section 4): the recorded operations of a `Signal<Vec<T>>`, delivered to
//! every derived list built on it.
//!
//! ADR-027's op log has one consumer (the keyed slot). A derived list is another, and a source can
//! have several, so a list signal keeps a [`TapList`]: one [`SourceTap`] per derived list, held by
//! `Weak` (dropping a derived list drops its tap). A recorded operation builds its op once per tap
//! that is recording and appends it inside the value's write lock, after the list was changed; a
//! raw write marks every tap stale before it touches the list.
//!
//! # The invariant
//!
//! While a tap is armed and not stale, the index of its derived list plus the tap's ops is the
//! list: replaying them on the index gives the list as it is. A drain that needs the items too
//! (materialising, rebuilding, a parameter walk) takes the ops and an `Arc` of the list together
//! under the value's read lock ([`SourceTap::take`]), which no writer can hold, so the ops it
//! replays are exactly the ones the list it sees includes.
//!
//! Lock order: a derived list's drain lock, then the source's value lock (read), then a tap (a
//! leaf). Writers take the value lock (write), then the tap list (read, recursive) and each tap,
//! one leaf at a time. Registering a tap takes the tap list's write lock and never runs under the
//! value lock.

use std::any::Any;
use std::sync::{Arc, Weak};

use parking_lot::{Mutex, RwLock};
use undra_wire::PatchOp;

use super::TAP_LIMIT;
use crate::oplog::ListLog;

/// Every tap of one list signal.
pub(crate) struct TapList<I> {
    taps: RwLock<Vec<Weak<SourceTap<I>>>>,
}

impl<I> TapList<I> {
    /// A list with no taps.
    pub(crate) fn new() -> TapList<I> {
        TapList {
            taps: RwLock::new(Vec::new()),
        }
    }

    /// Adds `tap`. Dead entries are swept when the list is about to grow, as dependents are.
    pub(crate) fn register(&self, tap: &Arc<SourceTap<I>>) {
        let mut taps = self.taps.write();
        if taps.len() == taps.capacity() {
            taps.retain(|weak| weak.strong_count() > 0);
        }
        taps.push(Arc::downgrade(tap));
    }

    /// Records one operation on every tap that is recording: `build` makes the op (`None`: an
    /// index that does not fit the wire's `u32`, which makes the tap stale). Called with the value
    /// write-locked, after the list was changed. If `build` panics (an item's `Clone`), every tap
    /// goes stale: the list has changed and the op that says so was not recorded.
    pub(crate) fn record(&self, build: impl Fn() -> Option<PatchOp<I>>) {
        let taps = self.taps.read_recursive();
        if taps.is_empty() {
            return;
        }
        let unwinding = StaleOnUnwind(Some(&taps));
        for weak in taps.iter() {
            let Some(tap) = weak.upgrade() else { continue };
            if !tap.is_recording() {
                continue;
            }
            match build() {
                Some(op) => tap.push(op),
                None => tap.invalidate(),
            }
        }
        unwinding.disarm();
    }

    /// Marks every live tap stale (a recorded operation unwound after changing the list).
    pub(crate) fn invalidate_taps(&self) {
        TapList::invalidate_all(&self.taps.read_recursive());
    }

    /// Marks every live tap stale.
    fn invalidate_all(taps: &[Weak<SourceTap<I>>]) {
        for weak in taps {
            if let Some(tap) = weak.upgrade() {
                tap.invalidate();
            }
        }
    }

    /// How many taps are registered, dead ones included (tests).
    #[cfg(test)]
    pub(crate) fn registered(&self) -> usize {
        self.taps.read().len()
    }
}

/// Marks every tap stale on drop unless disarmed.
struct StaleOnUnwind<'a, I>(Option<&'a [Weak<SourceTap<I>>]>);

impl<I> StaleOnUnwind<'_, I> {
    fn disarm(mut self) {
        self.0 = None;
    }
}

impl<I> Drop for StaleOnUnwind<'_, I> {
    fn drop(&mut self) {
        if let Some(taps) = self.0 {
            TapList::invalidate_all(taps);
        }
    }
}

impl<I: Send + Sync + 'static> ListLog for TapList<I> {
    fn invalidate(&self) {
        TapList::invalidate_all(&self.taps.read_recursive());
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The ops one derived list has not replayed yet.
pub(crate) struct SourceTap<I> {
    state: Mutex<TapState<I>>,
}

struct TapState<I> {
    /// The derived list has an index the ops apply to, so they are being recorded.
    armed: bool,
    /// A raw write, an overflowing index, a panic halfway through an operation, or more than
    /// [`TAP_LIMIT`] ops made the ops useless: the next drain rebuilds.
    stale: bool,
    /// The recorded ops, oldest first.
    ops: Vec<PatchOp<I>>,
    /// An emptied `ops` kept for reuse, so steady-state drains do not allocate.
    spare: Vec<PatchOp<I>>,
}

/// What a drain that needs the items found in its tap.
pub(crate) enum TapTaken<I> {
    /// Replay these ops (oldest first) on the index: they lead to `current`.
    Ops {
        /// The ops recorded since the last take.
        ops: Vec<PatchOp<I>>,
        /// The list they lead to.
        current: Arc<Vec<I>>,
    },
    /// The tap was disarmed or stale: rebuild the index from `current`.
    Fresh {
        /// The list as it is.
        current: Arc<Vec<I>>,
    },
}

impl<I> SourceTap<I> {
    /// A disarmed tap: nothing is recorded until the first drain arms it with a snapshot, so
    /// building a derived list costs nothing until it is read or observed.
    pub(crate) fn new() -> SourceTap<I> {
        SourceTap {
            state: Mutex::new(TapState {
                armed: false,
                stale: false,
                ops: Vec::new(),
                spare: Vec::new(),
            }),
        }
    }

    /// Whether a recorded operation should build an op for this tap now.
    pub(crate) fn is_recording(&self) -> bool {
        let state = self.state.lock();
        state.armed && !state.stale
    }

    /// Appends `op`; dropped when the tap stopped recording meanwhile. Past [`TAP_LIMIT`] the tap
    /// goes stale and frees its ops: a derived list nobody drains holds at most that many.
    pub(crate) fn push(&self, op: PatchOp<I>) {
        let mut state = self.state.lock();
        if !state.armed || state.stale {
            return;
        }
        if state.ops.len() >= TAP_LIMIT {
            state.stale = true;
            state.ops = Vec::new();
            return;
        }
        state.ops.push(op);
    }

    /// The recorded ops no longer describe the list. Does nothing while disarmed.
    pub(crate) fn invalidate(&self) {
        let mut state = self.state.lock();
        if state.armed {
            state.stale = true;
            state.ops = Vec::new();
        }
    }

    /// Under the source's value read lock: the ops recorded since the last take and the list they
    /// lead to, or `Fresh` when the tap was disarmed or stale. Either way the tap is armed and
    /// empty afterwards, current for `current`.
    pub(crate) fn take(&self, current: &Arc<Vec<I>>) -> TapTaken<I> {
        let mut state = self.state.lock();
        let usable = state.armed && !state.stale;
        state.armed = true;
        state.stale = false;
        let current = Arc::clone(current);
        if !usable {
            state.ops.clear();
            return TapTaken::Fresh { current };
        }
        let fresh = std::mem::take(&mut state.spare);
        let ops = std::mem::replace(&mut state.ops, fresh);
        TapTaken::Ops { ops, current }
    }

    /// The ops recorded since the last take, without a snapshot of the list (they describe a
    /// prefix of the writes, each complete); `None` when the tap is disarmed or stale and the
    /// drain has to rebuild.
    pub(crate) fn take_ops(&self) -> Option<Vec<PatchOp<I>>> {
        let mut state = self.state.lock();
        if !state.armed || state.stale {
            return None;
        }
        let fresh = std::mem::take(&mut state.spare);
        Some(std::mem::replace(&mut state.ops, fresh))
    }

    /// Hands back the (emptied) ops of a drain so their buffer is reused.
    pub(crate) fn recycle(&self, mut ops: Vec<PatchOp<I>>) {
        ops.clear();
        let mut state = self.state.lock();
        if state.spare.capacity() < ops.capacity() {
            state.spare = ops;
        }
    }

    /// The ops waiting, or `None` while not recording (tests).
    #[cfg(test)]
    pub(crate) fn pending(&self) -> Option<usize> {
        let state = self.state.lock();
        (state.armed && !state.stale).then_some(state.ops.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn armed() -> SourceTap<u32> {
        let tap = SourceTap::new();
        assert!(matches!(
            tap.take(&Arc::new(Vec::new())),
            TapTaken::Fresh { .. }
        ));
        tap
    }

    #[test]
    fn a_new_tap_records_nothing_until_a_take_arms_it() {
        let tap = SourceTap::<u32>::new();
        assert!(!tap.is_recording());
        tap.push(PatchOp::Clear);
        assert_eq!(tap.pending(), None);
        assert!(tap.take_ops().is_none(), "disarmed: the drain rebuilds");
        let current = Arc::new(vec![1_u32]);
        assert!(matches!(tap.take(&current), TapTaken::Fresh { .. }));
        assert!(tap.is_recording());
        tap.push(PatchOp::Remove { index: 0 });
        match tap.take(&current) {
            TapTaken::Ops { ops, .. } => assert_eq!(ops, vec![PatchOp::Remove { index: 0 }]),
            TapTaken::Fresh { .. } => panic!("armed and current"),
        }
    }

    #[test]
    fn a_stale_tap_asks_for_a_rebuild_once() {
        let tap = armed();
        tap.push(PatchOp::Clear);
        tap.invalidate();
        assert!(!tap.is_recording());
        assert!(tap.take_ops().is_none());
        assert!(matches!(
            tap.take(&Arc::new(Vec::new())),
            TapTaken::Fresh { .. }
        ));
        assert_eq!(tap.pending(), Some(0), "re-armed empty");
    }

    #[test]
    fn the_limit_is_exact() {
        let tap = armed();
        for i in 0..TAP_LIMIT {
            tap.push(PatchOp::Insert {
                index: u32::try_from(i).unwrap(),
                item: 0,
            });
        }
        assert_eq!(tap.pending(), Some(TAP_LIMIT), "exactly the limit is kept");
        tap.push(PatchOp::Clear);
        assert_eq!(tap.pending(), None, "one more makes it stale");
        assert!(tap.take_ops().is_none());
    }

    #[test]
    fn taking_reuses_the_buffer() {
        let tap = armed();
        tap.push(PatchOp::Clear);
        let ops = tap.take_ops().unwrap();
        let capacity = ops.capacity();
        tap.recycle(ops);
        tap.push(PatchOp::Clear);
        let again = tap.take_ops().unwrap();
        assert!(again.capacity() >= capacity.min(1));
    }

    #[test]
    fn the_list_records_on_live_recording_taps_and_prunes_dead_ones() {
        let list = TapList::<u32>::new();
        let a = Arc::new(armed());
        let b = Arc::new(SourceTap::new()); // disarmed: skipped
        list.register(&a);
        list.register(&b);
        {
            let gone = Arc::new(armed());
            list.register(&gone);
        }
        let built = std::sync::atomic::AtomicUsize::new(0);
        list.record(|| {
            built.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Some(PatchOp::Clear)
        });
        assert_eq!(built.into_inner(), 1, "built once per recording tap");
        assert_eq!(a.pending(), Some(1));
        assert_eq!(b.pending(), None);
        for _ in 0..8 {
            let tap = Arc::new(SourceTap::new());
            list.register(&tap);
        }
        assert!(
            list.registered() <= 9,
            "dead taps are swept as the list grows"
        );
    }

    #[test]
    fn a_panicking_build_makes_every_tap_stale() {
        let list = TapList::<u32>::new();
        let a = Arc::new(armed());
        let b = Arc::new(armed());
        list.register(&a);
        list.register(&b);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            list.record(|| panic!("clone failed"));
        }));
        assert!(result.is_err());
        assert!(!a.is_recording() && !b.is_recording());
    }

    #[test]
    fn an_index_that_does_not_fit_makes_the_tap_stale() {
        let list = TapList::<u32>::new();
        let a = Arc::new(armed());
        list.register(&a);
        list.record(|| None);
        assert!(!a.is_recording());
    }
}
