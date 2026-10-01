//! The counter: the smallest store, and the one that shows transactions.
//!
//! `add` changes two signals (`count`, `changes`) and, through the computed `parity`, a third.
//! It does so inside one [`Ctx::txn`], so every platform receives exactly one change-set however
//! many signals move (R5: writes cross once per transaction).

use undra::prelude::*;

/// Whether the count is even or odd: a value derived from `count` in the core.
#[undra::api]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parity {
    /// The count is divisible by two.
    Even,
    /// The count is not divisible by two.
    Odd,
}

/// A counter with a change tally and a computed parity.
#[undra::store(restore = "Self::assemble")]
pub struct Counter {
    ctx: Ctx,
    count: Signal<i32>,
    changes: Signal<u32>,
    parity: Computed<Parity>,
}

#[undra::api(store)]
impl Counter {
    /// A counter at zero with no changes made.
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(0), Signal::new(0))
    }

    // Used by `new` and, through `restore = ".."`, to rebuild the store from a snapshot.
    fn assemble(ctx: Ctx, count: Signal<i32>, changes: Signal<u32>) -> Self {
        let parity = Computed::new(&count, |count| {
            if count % 2 == 0 {
                Parity::Even
            } else {
                Parity::Odd
            }
        });
        Self {
            ctx,
            count,
            changes,
            parity,
        }
    }

    /// Adds one.
    pub fn increment(&self) {
        self.add(1);
    }

    /// Subtracts one.
    pub fn decrement(&self) {
        self.add(-1);
    }

    /// Adds `amount` (which may be negative) and counts the change: one transaction, so one
    /// change-set for `count`, `changes` and `parity` together. The count saturates instead of
    /// overflowing.
    pub fn add(&self, amount: i32) {
        self.ctx.txn(|| {
            self.count
                .update(|count| *count = count.saturating_add(amount));
            self.changes
                .update(|changes| *changes = changes.saturating_add(1));
        });
    }

    /// Sets the count back to zero and forgets the changes, in one transaction.
    pub fn reset(&self) {
        self.ctx.txn(|| {
            self.count.set(0);
            self.changes.set(0);
        });
    }
}

#[cfg(test)]
mod tests {
    use undra::meta::ids;
    use undra::runtime::testing::TestRuntime;
    use undra::signals::ALL_SIGNALS;
    use undra::wire::Decode;
    use undra::wire::payload::{CallTarget, ChangeSet, ReplyStatus};

    use super::*;

    fn construct(t: &TestRuntime) -> Handle {
        let reply = t.call_sync(
            CallTarget::Constructor {
                type_id: ids::type_id("Counter"),
                method_id: ids::method_id("Counter", "new"),
            },
            1,
            &[],
        );
        assert_eq!(reply.status, ReplyStatus::Ok);
        Handle::decode_exact(&reply.body).expect("a handle")
    }

    fn call(t: &TestRuntime, store: Handle, method: &str, args: &[u8]) {
        let reply = t.call_sync(
            CallTarget::Method {
                handle: store,
                method_id: ids::method_id("Counter", method),
            },
            2,
            args,
        );
        assert_eq!(reply.status, ReplyStatus::Ok, "Counter.{method}: {reply:?}");
    }

    fn observed(t: &TestRuntime) -> (Handle, ChangeSet) {
        let store = construct(t);
        t.take_change_sets();
        t.runtime().observe(store.0, ALL_SIGNALS, true);
        let mut sets = t.host().take_decoded_change_sets();
        assert_eq!(sets.len(), 1);
        (store, sets.remove(0))
    }

    #[test]
    fn a_new_counter_starts_even_at_zero() {
        let t = TestRuntime::new();
        let (_, initial) = observed(&t);
        let values: Vec<(u32, Vec<u8>)> = initial
            .entries
            .iter()
            .map(|e| (e.signal_id, e.value.clone()))
            .collect();
        assert_eq!(
            values,
            [
                (0, 0i32.to_le_bytes().to_vec()),
                (1, 0u32.to_le_bytes().to_vec()),
                (2, vec![0, 0]),
            ]
        );
    }

    #[test]
    fn add_is_one_transaction_with_three_entries() {
        let t = TestRuntime::new();
        let (store, _) = observed(&t);
        call(&t, store, "add", &5i32.to_le_bytes());
        let sets = t.host().take_decoded_change_sets();
        assert_eq!(sets.len(), 1, "one transaction, one change-set: {sets:?}");
        let ids: Vec<u32> = sets[0].entries.iter().map(|e| e.signal_id).collect();
        assert_eq!(ids, [0, 1, 2]);
        assert_eq!(sets[0].entries[0].value, 5i32.to_le_bytes());
        assert_eq!(sets[0].entries[1].value, 1u32.to_le_bytes());
        assert_eq!(sets[0].entries[2].value, [1, 0], "5 is odd");
    }

    #[test]
    fn increment_and_decrement_move_parity() {
        let t = TestRuntime::new();
        let (store, _) = observed(&t);
        call(&t, store, "increment", &[]);
        call(&t, store, "decrement", &[]);
        call(&t, store, "decrement", &[]);
        let sets = t.host().take_decoded_change_sets();
        assert_eq!(sets.len(), 3);
        let counts: Vec<i32> = sets
            .iter()
            .map(|cs| i32::from_le_bytes(cs.entries[0].value.clone().try_into().unwrap()))
            .collect();
        assert_eq!(counts, [1, 0, -1]);
        assert_eq!(sets[2].entries[2].value, [1, 0], "-1 is odd");
    }

    #[test]
    fn reset_is_one_transaction() {
        let t = TestRuntime::new();
        let (store, _) = observed(&t);
        call(&t, store, "add", &7i32.to_le_bytes());
        t.host().take_decoded_change_sets();
        call(&t, store, "reset", &[]);
        let sets = t.host().take_decoded_change_sets();
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].entries[0].value, 0i32.to_le_bytes());
        assert_eq!(sets[0].entries[1].value, 0u32.to_le_bytes());
        assert_eq!(sets[0].entries[2].value, [0, 0]);
    }

    #[test]
    fn the_count_saturates() {
        let t = TestRuntime::new();
        let (store, _) = observed(&t);
        call(&t, store, "add", &i32::MAX.to_le_bytes());
        call(&t, store, "add", &i32::MAX.to_le_bytes());
        let last = t
            .host()
            .take_decoded_change_sets()
            .pop()
            .expect("a change-set");
        assert_eq!(last.entries[0].value, i32::MAX.to_le_bytes());
    }

    #[test]
    fn a_restore_rebuilds_the_computed() {
        let t = TestRuntime::new();
        let (store, _) = observed(&t);
        call(&t, store, "add", &3i32.to_le_bytes());
        let snapshot = t.runtime().snapshot();
        call(&t, store, "add", &1i32.to_le_bytes());
        t.host().take_decoded_change_sets();
        t.runtime().restore(&snapshot).expect("restores");
        let sets = t.host().take_decoded_change_sets();
        let set = sets.last().expect("the restore re-sends observed signals");
        assert_eq!(set.entries[0].value, 3i32.to_le_bytes());
        assert_eq!(
            set.entries[2].value,
            [1, 0],
            "3 is odd; parity is recomputed"
        );
    }
}
