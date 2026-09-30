// Recorded list operations on `Signal<Vec<T>>` (ADR-027). The guidance on when to use them
// instead of `set` / `update` lives in the `# Lists` section of the `Signal` docs, where rustdoc
// shows it; the op log they append to is `crate::oplog`.

use std::sync::Arc;

use keel_wire::PatchOp;

use super::{Signal, Updating};
use crate::oplog::{KeyedLog, move_within};
use crate::value::SignalValue;

/// One recorded operation in progress: samples once whether the log is recording, builds the op
/// only if it is, and appends it after the list has been changed.
struct Recording<'a, I> {
    log: Option<&'a KeyedLog<I>>,
}

impl<'a, I> Recording<'a, I> {
    /// A recording of the operation about to run, on `log` if there is one and it is recording.
    /// Sampled once: the value is write-locked for the whole operation, so nothing but the slot
    /// being forgotten can change it, and `KeyedLog::push` copes with that.
    fn begin(log: Option<&'a KeyedLog<I>>) -> Recording<'a, I> {
        Recording {
            log: log.filter(|log| log.is_recording()),
        }
    }

    /// Builds the op with `build`, if the log is recording. `None` from `build` (an index that
    /// does not fit the wire's `u32`) makes the log stale, so that the commit diffs.
    fn prepare(&self, build: impl FnOnce() -> Option<PatchOp<I>>) -> Option<PatchOp<I>> {
        let log = self.log?;
        let op = build();
        if op.is_none() {
            log.invalidate();
        }
        op
    }

    /// Appends an op made by `prepare`, now that the list has been changed.
    fn record(&self, op: Option<PatchOp<I>>) {
        if let (Some(log), Some(op)) = (self.log, op) {
            log.push(op);
        }
    }

    /// `prepare` and `record` for an op that needs the list as it is after the change.
    fn push_with(&self, build: impl FnOnce() -> Option<PatchOp<I>>) {
        self.record(self.prepare(build));
    }

    /// While alive, makes the log stale when dropped: for an operation whose closure can panic
    /// after it has changed the list but before its op is recorded.
    fn guard(&self) -> StaleOnUnwind<'a, I> {
        StaleOnUnwind { log: self.log }
    }
}

/// Marks the log stale on drop unless [disarmed](StaleOnUnwind::disarm).
struct StaleOnUnwind<'a, I> {
    log: Option<&'a KeyedLog<I>>,
}

impl<I> StaleOnUnwind<'_, I> {
    fn disarm(mut self) {
        self.log = None;
    }
}

impl<I> Drop for StaleOnUnwind<'_, I> {
    fn drop(&mut self) {
        if let Some(log) = self.log {
            log.invalidate();
        }
    }
}

/// A list position as the wire carries it.
fn wire_index(index: usize) -> Option<u32> {
    u32::try_from(index).ok()
}

impl<I: SignalValue> Signal<Vec<I>> {
    /// The typed op log, if this signal was attached as a keyed list.
    fn op_log(&self) -> Option<&KeyedLog<I>> {
        self.inner
            .log
            .get()
            .and_then(|log| log.as_any().downcast_ref::<KeyedLog<I>>())
    }

    /// Appends `item` to the end of the list.
    ///
    /// Recorded: on a keyed list the host receives one `Insert` op, however long the list is (see
    /// [Lists](Signal#lists) on O(change) versus O(list)). Transaction behaviour is the same as
    /// [`set`](Signal::set).
    ///
    /// # Example
    ///
    /// ```
    /// use keel_signals::Signal;
    ///
    /// let names = Signal::new(vec![String::from("ada")]);
    /// names.push(String::from("grace"));
    /// assert_eq!(names.get(), ["ada", "grace"]);
    /// ```
    pub fn push(&self, item: I) {
        self.write_with(|slot| {
            let index = slot.len();
            let recording = Recording::begin(self.op_log());
            // Built before the list changes: a panicking `Clone` leaves everything as it was.
            let op = recording.prepare(|| {
                wire_index(index).map(|index| PatchOp::Insert {
                    index,
                    item: item.clone(),
                })
            });
            Arc::make_mut(slot).push(item);
            recording.record(op);
        });
    }

    /// Inserts `item` so that it ends up at `index`, shifting the items after it.
    ///
    /// Recorded: one `Insert` op on a keyed list. See [Lists](Signal#lists).
    ///
    /// # Panics
    ///
    /// If `index > len`, as `Vec::insert` does; the list is left unchanged.
    ///
    /// # Example
    ///
    /// ```
    /// use keel_signals::Signal;
    ///
    /// let list = Signal::new(vec![1, 3]);
    /// list.insert(1, 2);
    /// assert_eq!(list.get(), [1, 2, 3]);
    /// ```
    pub fn insert(&self, index: usize, item: I) {
        self.write_with(|slot| {
            let len = slot.len();
            assert!(
                index <= len,
                "keel-signals: Signal::insert at index {index}, but the list has {len} item(s)"
            );
            let recording = Recording::begin(self.op_log());
            let op = recording.prepare(|| {
                wire_index(index).map(|index| PatchOp::Insert {
                    index,
                    item: item.clone(),
                })
            });
            Arc::make_mut(slot).insert(index, item);
            recording.record(op);
        });
    }

    /// Removes and returns the item at `index`, shifting the items after it.
    ///
    /// Recorded: one `Remove` op on a keyed list. See [Lists](Signal#lists).
    ///
    /// # Panics
    ///
    /// If `index >= len`, as `Vec::remove` does; the list is left unchanged.
    ///
    /// # Example
    ///
    /// ```
    /// use keel_signals::Signal;
    ///
    /// let list = Signal::new(vec![10_u32, 20, 30]);
    /// assert_eq!(list.remove(1), 20);
    /// assert_eq!(list.get(), [10, 30]);
    /// ```
    pub fn remove(&self, index: usize) -> I {
        self.write_with(|slot| {
            let len = slot.len();
            assert!(
                index < len,
                "keel-signals: Signal::remove at index {index}, but the list has {len} item(s)"
            );
            let recording = Recording::begin(self.op_log());
            let removed = Arc::make_mut(slot).remove(index);
            recording.push_with(|| wire_index(index).map(|index| PatchOp::Remove { index }));
            removed
        })
    }

    /// Changes the item at `index` in place with `f`.
    ///
    /// Recorded: one `Update` op carrying the changed item (SPEC 3.8 `Update` carries the whole
    /// item) on a keyed list. See [Lists](Signal#lists). Like [`update`](Signal::update), `f`
    /// runs with the value write-locked: it must not read or write this signal, nor a computed
    /// that depends on it (that is detected and panics instead of deadlocking).
    ///
    /// If `f` panics, the item keeps whatever `f` did to it and the commit that follows is
    /// computed by diffing, so the host is not left with a stale item.
    ///
    /// # Panics
    ///
    /// If `index >= len`; the list is left unchanged.
    ///
    /// # Example
    ///
    /// ```
    /// use keel_signals::Signal;
    ///
    /// let list = Signal::new(vec![String::from("a"), String::from("b")]);
    /// list.update_at(1, |item| item.push('!'));
    /// assert_eq!(list.get(), ["a", "b!"]);
    /// ```
    pub fn update_at(&self, index: usize, f: impl FnOnce(&mut I)) {
        let me = self.id();
        self.write_with(|slot| {
            let len = slot.len();
            assert!(
                index < len,
                "keel-signals: Signal::update_at at index {index}, but the list has {len} item(s)"
            );
            let recording = Recording::begin(self.op_log());
            // Between `f` changing the item and the op being recorded the log describes a list
            // that no longer is the list.
            let unwinding = recording.guard();
            let _updating = Updating::enter(me);
            let list = Arc::make_mut(slot);
            f(&mut list[index]);
            recording.push_with(|| {
                wire_index(index).map(|wire| PatchOp::Update {
                    index: wire,
                    item: list[index].clone(),
                })
            });
            unwinding.disarm();
        });
    }

    /// Takes the item at `from` out of the list and puts it back so that it ends up at `to`
    /// (SPEC 3.8 `Move`: `to` is the item's final position).
    ///
    /// Recorded: one `Move` op, which carries two indices and no item, on a keyed list. See
    /// [Lists](Signal#lists). `from == to` changes nothing.
    ///
    /// # Panics
    ///
    /// If `from >= len` or `to >= len`; the list is left unchanged.
    ///
    /// # Example
    ///
    /// ```
    /// use keel_signals::Signal;
    ///
    /// let list = Signal::new(vec![1_u32, 2, 3, 4]);
    /// list.move_item(0, 2);
    /// assert_eq!(list.get(), [2, 3, 1, 4]);
    /// ```
    pub fn move_item(&self, from: usize, to: usize) {
        self.write_with(|slot| {
            let len = slot.len();
            assert!(
                from < len && to < len,
                "keel-signals: Signal::move_item from {from} to {to}, but the list has {len} item(s)"
            );
            if from == to {
                return;
            }
            let recording = Recording::begin(self.op_log());
            move_within(Arc::make_mut(slot).as_mut_slice(), from, to);
            recording.push_with(|| {
                Some(PatchOp::Move {
                    from: wire_index(from)?,
                    to: wire_index(to)?,
                })
            });
        });
    }

    /// Removes every item.
    ///
    /// Recorded: one `Clear` op on a keyed list (an empty list records nothing). See
    /// [Lists](Signal#lists).
    ///
    /// # Example
    ///
    /// ```
    /// use keel_signals::Signal;
    ///
    /// let list = Signal::new(vec![1, 2, 3]);
    /// list.clear();
    /// assert!(list.get().is_empty());
    /// ```
    pub fn clear(&self) {
        // The removed items are dropped after the lock is released, as `set` does.
        let _removed = self.write_with(|slot| {
            if slot.is_empty() {
                return None;
            }
            let recording = Recording::begin(self.op_log());
            let removed = std::mem::replace(slot, Arc::new(Vec::new()));
            recording.push_with(|| Some(PatchOp::Clear));
            Some(removed)
        });
    }

    /// Replaces the whole list with `items`.
    ///
    /// This is [`set`](Signal::set) under a name that belongs with the other list operations: it
    /// is a raw write, so a keyed list sends the host whatever
    /// [`KeyedPatch::diff`](keel_wire::KeyedPatch::diff) makes of the old and the new list (a
    /// patch when they overlap, the full value when they do not or when more than half of the
    /// items went away), which costs O(list). To change a few items, use the recorded
    /// operations. See [Lists](Signal#lists).
    ///
    /// # Example
    ///
    /// ```
    /// use keel_signals::Signal;
    ///
    /// let list = Signal::new(vec![1, 2]);
    /// list.replace(vec![7, 8, 9]);
    /// assert_eq!(list.get(), [7, 8, 9]);
    /// ```
    pub fn replace(&self, items: Vec<I>) {
        self.set(items);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_mutations_on_an_unattached_list() {
        let list = Signal::new(vec![1_u32, 2, 3]);
        list.push(4);
        list.insert(0, 0);
        assert_eq!(list.remove(2), 2);
        list.update_at(0, |n| *n += 10);
        list.move_item(0, 3);
        assert_eq!(list.get(), vec![1, 3, 4, 10]);
        list.clear();
        assert!(list.get().is_empty());
        list.replace(vec![5]);
        assert_eq!(list.get(), vec![5]);
    }

    #[test]
    fn out_of_range_indices_panic_and_leave_the_list_alone() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        let list = Signal::new(vec![1_u32, 2]);
        let panics = |f: &dyn Fn()| catch_unwind(AssertUnwindSafe(f)).is_err();
        assert!(panics(&|| list.insert(3, 9)));
        assert!(panics(&|| {
            let _ = list.remove(2);
        }));
        assert!(panics(&|| list.update_at(2, |n| *n = 9)));
        assert!(panics(&|| list.move_item(0, 2)));
        assert!(panics(&|| list.move_item(2, 0)));
        assert_eq!(list.get(), vec![1, 2]);
        // The signal is still usable: no lock was left held.
        list.push(3);
        assert_eq!(list.get(), vec![1, 2, 3]);
    }

    #[test]
    fn clear_drops_the_items_after_the_lock_is_released() {
        use std::sync::atomic::{AtomicBool, Ordering};

        /// Notes whether the list's lock was free when it was dropped.
        #[derive(Clone)]
        struct Probe(Arc<AtomicBool>, Arc<Signal<Vec<Probe>>>);
        impl keel_wire::Encode for Probe {
            fn encode(&self, _w: &mut keel_wire::Writer) {}
        }
        impl Drop for Probe {
            fn drop(&mut self) {
                let free = self.1.inner.value.try_read_recursive().is_some();
                self.0.store(free, Ordering::SeqCst);
            }
        }
        let lock_was_free = Arc::new(AtomicBool::new(false));
        let list = Arc::new(Signal::new(Vec::new()));
        list.push(Probe(Arc::clone(&lock_was_free), Arc::clone(&list)));
        list.clear();
        assert!(lock_was_free.load(Ordering::SeqCst));
    }

    #[test]
    fn a_panicking_update_at_keeps_what_the_closure_did() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        let list = Signal::new(vec![1_u32, 2]);
        let result = catch_unwind(AssertUnwindSafe(|| {
            list.update_at(1, |n| {
                *n = 20;
                panic!("boom");
            });
        }));
        assert!(result.is_err());
        assert_eq!(list.get(), vec![1, 20]);
    }

    #[test]
    fn update_at_closure_reading_the_signal_is_detected() {
        use std::panic::{AssertUnwindSafe, catch_unwind};
        let list = Signal::new(vec![1_u32]);
        let result = catch_unwind(AssertUnwindSafe(|| {
            list.update_at(0, |_| {
                list.with(|l| l.len());
            });
        }));
        assert!(
            result.is_err(),
            "reading inside the closure must not deadlock"
        );
        list.push(2);
        assert_eq!(list.get(), vec![1, 2]);
    }
}
