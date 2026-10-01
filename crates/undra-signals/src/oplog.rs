//! The op log behind the recorded list operations of `Signal<Vec<T>>` (ADR-027).
//!
//! A keyed list slot normally finds out what changed by diffing the list against a copy of what
//! the host has: O(list). A list written through the recorded operations (`push`, `insert`,
//! `remove`, `update_at`, `move_item`, `clear`) leaves a **log** of the SPEC 3.8 ops instead, and
//! the commit sends the log: O(ops).
//!
//! # The invariant
//!
//! While the log is *armed* and not *stale*, `baseline + ops == the list`: replaying the logged
//! ops on the baseline (the list as the host last saw it) gives the list itself. Everything in
//! this file exists to keep that true or to notice that it no longer is:
//!
//! * a recorded operation appends its op in the same critical section (the signal's value write
//!   lock) that mutates the list, so the two cannot be seen apart;
//! * a raw write (`set`, `update`, `replace`) marks the log stale before it touches the list, so a
//!   commit that finds it stale diffs instead;
//! * the commit takes the log and looks at the list under the value's **read** lock, which no
//!   writer can hold, so the ops it takes are exactly the ones the list it sees includes;
//! * arming, disarming and taking all happen under the baseline lock of the slot.
//!
//! Lock order, everywhere: slot baseline, then the signal's value lock, then this log's lock. The
//! log's lock is a leaf: nothing is called and no other lock is taken while it is held.

use std::any::Any;
use std::sync::Arc;

use parking_lot::Mutex;
use undra_wire::PatchOp;

/// The fewest ops a log keeps before it gives up and lets the commit diff: a list so long that
/// this many ops is a small fraction of it raises the limit to its length (see `arm`).
const MIN_OP_LIMIT: usize = 4096;

/// What a signal knows about its log without knowing the item type.
pub(crate) trait ListLog: Send + Sync {
    /// A raw write is about to change the list: the recorded ops no longer describe it.
    fn invalidate(&self);
    /// For recovering the typed log inside `Signal<Vec<I>>`.
    fn as_any(&self) -> &dyn Any;
}

/// The log of one keyed list: the ops recorded since the last commit took it.
pub(crate) struct KeyedLog<I> {
    state: Mutex<LogState<I>>,
}

struct LogState<I> {
    /// A baseline exists and the ops are being recorded.
    armed: bool,
    /// A raw write (or overflow) made the ops useless; the commit diffs.
    stale: bool,
    /// Ops recorded before the log gives up (and goes stale): bounds its memory when nothing
    /// commits.
    limit: usize,
    /// The recorded ops, oldest first.
    ops: Vec<PatchOp<I>>,
    /// An emptied `ops` kept for reuse, so steady-state commits do not allocate.
    spare: Vec<PatchOp<I>>,
}

/// What a commit found when it took the log.
pub(crate) enum Taken<T, I> {
    /// The slot has no baseline: send the full value, which becomes the baseline.
    Full(Arc<T>),
    /// Something other than recorded ops changed the list, or the log did not add up: diff the
    /// baseline against this value.
    Diff(Arc<T>),
    /// Send these ops. They replay on the baseline to a list of the current length.
    Recorded {
        /// The ops, oldest first.
        ops: Vec<PatchOp<I>>,
        /// The list the ops lead to, kept in debug builds so the baseline can be checked
        /// against it after the ops were applied.
        #[cfg(debug_assertions)]
        current: Arc<T>,
    },
}

impl<I> KeyedLog<I> {
    /// A disarmed log: nothing is recorded until the slot is baselined.
    pub(crate) fn new() -> KeyedLog<I> {
        KeyedLog {
            state: Mutex::new(LogState {
                armed: false,
                stale: false,
                limit: MIN_OP_LIMIT,
                ops: Vec::new(),
                spare: Vec::new(),
            }),
        }
    }

    /// Whether a recorded operation should build its op now.
    pub(crate) fn is_recording(&self) -> bool {
        let state = self.state.lock();
        state.armed && !state.stale
    }

    /// Appends `op`. Dropped when the log stopped recording meanwhile (the slot was forgotten
    /// or a raw write made the ops useless), and turns the log stale when it is full.
    pub(crate) fn push(&self, op: PatchOp<I>) {
        let mut state = self.state.lock();
        if !state.armed || state.stale {
            return;
        }
        if state.ops.len() >= state.limit {
            state.stale = true;
            state.ops = Vec::new();
            return;
        }
        state.ops.push(op);
    }

    /// The baseline is about to be replaced by the list of length `len`: start recording from
    /// an empty log. The caller holds the value's read lock, so no write is half done.
    pub(crate) fn arm(&self, len: usize) {
        let mut state = self.state.lock();
        state.armed = true;
        state.stale = false;
        state.ops.clear();
        state.limit = len.max(MIN_OP_LIMIT);
    }

    /// The slot has no baseline any more (unobserved, or its delivery was abandoned): stop
    /// recording and drop what was recorded.
    pub(crate) fn disarm(&self) {
        let mut state = self.state.lock();
        state.armed = false;
        state.stale = false;
        state.ops = Vec::new();
    }

    /// Takes what a commit needs: the recorded ops when they are usable, else the value to diff
    /// or send. `current` is the list, `len` its length and `base_len` the length of the
    /// baseline, `None` when there is none. The caller holds the value's read lock and the
    /// slot's baseline lock, and afterwards the log records from empty: it is armed and current
    /// for the baseline the caller is about to leave behind.
    pub(crate) fn take<T>(
        &self,
        current: &Arc<T>,
        len: usize,
        base_len: Option<usize>,
    ) -> Taken<T, I> {
        let (was_armed, was_stale, ops) = {
            let mut state = self.state.lock();
            let fresh = std::mem::take(&mut state.spare);
            let taken = (
                state.armed,
                state.stale,
                std::mem::replace(&mut state.ops, fresh),
            );
            state.armed = true;
            state.stale = false;
            state.limit = len.max(MIN_OP_LIMIT);
            taken
        };
        let Some(base_len) = base_len else {
            return Taken::Full(Arc::clone(current));
        };
        if was_armed && !was_stale && replayed_len(base_len, &ops) == Some(len) {
            return Taken::Recorded {
                ops,
                #[cfg(debug_assertions)]
                current: Arc::clone(current),
            };
        }
        Taken::Diff(Arc::clone(current))
    }

    /// Hands back the (emptied) ops of a commit so their buffer is reused.
    pub(crate) fn recycle(&self, mut ops: Vec<PatchOp<I>>) {
        ops.clear();
        let mut state = self.state.lock();
        if state.spare.capacity() < ops.capacity() {
            state.spare = ops;
        }
    }
}

impl<I> KeyedLog<I> {
    /// The recorded ops no longer describe the list (a raw write, an overflowing index, a
    /// panic halfway through an operation): the commit has to diff. Does nothing while the log
    /// is disarmed, since there is nothing to be wrong about.
    pub(crate) fn invalidate(&self) {
        let mut state = self.state.lock();
        if state.armed {
            state.stale = true;
            state.ops.clear();
        }
    }
}

impl<I: Send + Sync + 'static> ListLog for KeyedLog<I> {
    fn invalidate(&self) {
        KeyedLog::invalidate(self);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The length a list of length `base` has after `ops`, or `None` if an op's index is out of
/// range for the list it runs on. Only lengths are tracked: this is the cheap sanity check that
/// the log still describes the baseline, not a comparison of items.
fn replayed_len<I>(base: usize, ops: &[PatchOp<I>]) -> Option<usize> {
    let mut len = base;
    for op in ops {
        match *op {
            PatchOp::Insert { index, .. } => {
                if index as usize > len {
                    return None;
                }
                len += 1;
            }
            PatchOp::Remove { index } => {
                if index as usize >= len {
                    return None;
                }
                len -= 1;
            }
            PatchOp::Update { index, .. } => {
                if index as usize >= len {
                    return None;
                }
            }
            PatchOp::Move { from, to } => {
                if from as usize >= len || to as usize >= len {
                    return None;
                }
            }
            PatchOp::Clear => len = 0,
        }
    }
    Some(len)
}

/// Replays `ops` on `list`, moving the items out of them and leaving `ops` empty. The same
/// semantics as `KeyedPatch::apply` (SPEC 3.8), without cloning and without a second bounds pass:
/// `replayed_len` has already vouched for the indices, and an op that still does not fit is
/// skipped exactly as `apply` skips it.
pub(crate) fn apply_ops<I>(list: &mut Vec<I>, ops: &mut Vec<PatchOp<I>>) {
    for op in ops.drain(..) {
        match op {
            PatchOp::Insert { index, item } => {
                let index = index as usize;
                if index <= list.len() {
                    list.insert(index, item);
                }
            }
            PatchOp::Remove { index } => {
                let index = index as usize;
                if index < list.len() {
                    list.remove(index);
                }
            }
            PatchOp::Update { index, item } => {
                if let Some(slot) = list.get_mut(index as usize) {
                    *slot = item;
                }
            }
            PatchOp::Move { from, to } => {
                let (from, to) = (from as usize, to as usize);
                if from < list.len() && to < list.len() {
                    move_within(list, from, to);
                }
            }
            PatchOp::Clear => list.clear(),
        }
    }
}

/// Takes the item at `from` out and puts it back so that it ends up at `to` (SPEC 3.8 `Move`):
/// one rotation of the span between the two, instead of a removal and an insertion that each
/// shift the whole tail. Both must be in range.
pub(crate) fn move_within<I>(list: &mut [I], from: usize, to: usize) {
    match from.cmp(&to) {
        std::cmp::Ordering::Less => list[from..=to].rotate_left(1),
        std::cmp::Ordering::Greater => list[to..=from].rotate_right(1),
        std::cmp::Ordering::Equal => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Op = PatchOp<u32>;

    #[test]
    fn replayed_len_follows_every_op() {
        let ops: Vec<Op> = vec![
            PatchOp::Insert { index: 3, item: 9 },
            PatchOp::Remove { index: 0 },
            PatchOp::Update { index: 2, item: 1 },
            PatchOp::Move { from: 0, to: 2 },
        ];
        assert_eq!(replayed_len(3, &ops), Some(3));
        assert_eq!(replayed_len(3, &[PatchOp::<u32>::Clear]), Some(0));
        assert_eq!(replayed_len(0, &[] as &[Op]), Some(0));
    }

    #[test]
    fn replayed_len_rejects_out_of_range_ops() {
        assert_eq!(replayed_len(2, &[Op::Insert { index: 3, item: 0 }]), None);
        assert_eq!(replayed_len(2, &[Op::Remove { index: 2 }]), None);
        assert_eq!(replayed_len(2, &[Op::Update { index: 2, item: 0 }]), None);
        assert_eq!(replayed_len(2, &[Op::Move { from: 0, to: 2 }]), None);
        assert_eq!(replayed_len(2, &[Op::Move { from: 2, to: 0 }]), None);
        // The bounds are those of the list at that op, not of the baseline.
        let ops = [Op::Clear, Op::Insert { index: 1, item: 0 }];
        assert_eq!(replayed_len(2, &ops), None);
    }

    #[test]
    fn move_within_matches_remove_then_insert() {
        for len in 1..7_usize {
            for from in 0..len {
                for to in 0..len {
                    let mut expected: Vec<usize> = (0..len).collect();
                    let item = expected.remove(from);
                    expected.insert(to, item);
                    let mut rotated: Vec<usize> = (0..len).collect();
                    move_within(&mut rotated, from, to);
                    assert_eq!(rotated, expected, "len {len}, {from} -> {to}");
                }
            }
        }
    }

    #[test]
    fn apply_ops_agrees_with_keyed_patch_apply() {
        let ops: Vec<Op> = vec![
            PatchOp::Insert { index: 1, item: 10 },
            PatchOp::Move { from: 0, to: 2 },
            PatchOp::Update { index: 0, item: 11 },
            PatchOp::Remove { index: 3 },
            PatchOp::Insert { index: 3, item: 12 },
        ];
        let mut by_patch = vec![1, 2, 3];
        undra_wire::KeyedPatch { ops: ops.clone() }
            .apply(&mut by_patch)
            .unwrap();
        let mut by_move = vec![1, 2, 3];
        apply_ops(&mut by_move, &mut ops.clone());
        assert_eq!(by_move, by_patch);
    }

    #[test]
    fn a_disarmed_log_records_nothing_and_asks_for_the_full_value() {
        let log = KeyedLog::<u32>::new();
        assert!(!log.is_recording());
        log.push(PatchOp::Clear);
        let current = Arc::new(vec![1_u32]);
        assert!(matches!(log.take(&current, 1, None), Taken::Full(_)));
        // Taking armed it.
        assert!(log.is_recording());
    }

    #[test]
    fn a_stale_log_asks_for_a_diff_and_the_next_take_is_clean() {
        let log = KeyedLog::<u32>::new();
        let current = Arc::new(vec![1_u32]);
        log.arm(1);
        log.push(PatchOp::Insert { index: 1, item: 2 });
        log.invalidate();
        assert!(!log.is_recording());
        assert!(matches!(log.take(&current, 1, Some(1)), Taken::Diff(_)));
        assert!(log.is_recording(), "taking re-armed it");
        assert!(matches!(
            log.take(&current, 1, Some(1)),
            Taken::Recorded { .. }
        ));
    }

    #[test]
    fn a_log_that_does_not_add_up_asks_for_a_diff() {
        let log = KeyedLog::<u32>::new();
        log.arm(1);
        log.push(PatchOp::Insert { index: 1, item: 2 });
        // The list is said to have length 5 but baseline 1 + one insert is 2.
        let current = Arc::new(vec![0_u32; 5]);
        assert!(matches!(log.take(&current, 5, Some(1)), Taken::Diff(_)));
    }

    #[test]
    fn a_full_log_goes_stale_instead_of_growing() {
        let log = KeyedLog::<u32>::new();
        log.arm(0);
        for i in 0..MIN_OP_LIMIT {
            log.push(PatchOp::Insert {
                index: u32::try_from(i).unwrap(),
                item: 0,
            });
        }
        assert!(log.is_recording());
        log.push(PatchOp::Clear);
        assert!(!log.is_recording());
        let current = Arc::new(Vec::<u32>::new());
        assert!(matches!(log.take(&current, 0, Some(0)), Taken::Diff(_)));
    }

    #[test]
    fn disarming_drops_the_ops() {
        let log = KeyedLog::<u32>::new();
        log.arm(1);
        log.push(PatchOp::Clear);
        log.disarm();
        assert!(!log.is_recording());
        let current = Arc::new(vec![1_u32]);
        assert!(matches!(log.take(&current, 1, None), Taken::Full(_)));
    }
}
