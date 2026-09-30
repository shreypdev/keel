//! [`Signal`]: a shared, observable value.

use std::cell::RefCell;
use std::fmt;
use std::sync::{Arc, OnceLock, Weak};

use parking_lot::RwLock;

use crate::graph::{Binding, Dependents, Reactive, add_dependent, notify_dependents, record};
use crate::oplog::ListLog;
use crate::txn::TxnGuard;
use crate::value::SignalValue;

mod list;

/// A shared value that other parts of the system can react to.
///
/// A `Signal<T>` is a cheap handle: [`Clone`] gives another handle to the **same** signal, not a
/// copy of the value. Writes ([`set`](Signal::set), [`update`](Signal::update)) mark every
/// [`Computed`](crate::Computed) and [`Effect`](crate::Effect) built on the signal as stale and,
/// if the signal has been attached to a [`StoreCell`](crate::StoreCell), record the change in
/// the current transaction so it can be delivered to the host.
///
/// A signal that is not attached to a store still works as a local reactive value; its writes
/// simply have nothing to deliver.
///
/// # Lists
///
/// A `Signal<Vec<T>>` also has list operations: [`push`](Signal::push),
/// [`insert`](Signal::insert), [`remove`](Signal::remove), [`update_at`](Signal::update_at),
/// [`move_item`](Signal::move_item), [`clear`](Signal::clear) and
/// [`replace`](Signal::replace). On a list attached with
/// [`StoreCell::attach_keyed`](crate::StoreCell::attach_keyed) that the host observes, there are
/// two ways the commit can find out what to send (a keyed patch, SPEC 3.8), and the write picks
/// one:
///
/// * **Recorded operations** (`push`, `insert`, `remove`, `update_at`, `move_item`, `clear`) note
///   the SPEC 3.8 op they perform as they perform it, and the commit sends the notes: the cost is
///   proportional to the number of operations, however long the list is. The only part that
///   still grows with the list is the `memmove` of the vector's tail that a `Vec::insert` or
///   `Vec::remove` in the middle needs anyway (the host pays it too when it applies the op).
/// * **Raw writes** (`set`, `update`, `replace`) hand over a list that is compared with what the
///   host has: the commit hashes every key of both lists and compares the items that survived,
///   O(list), and sends the full value when more than half of the items were removed or the keys
///   do not overlap (SPEC 3.8).
///
/// Prefer the recorded operations for edits of a few items, which is what nearly every list
/// mutation is. Use `set` or `replace` to load or refresh a whole list, and `update` for an edit
/// none of the operations can express. A transaction that mixes the two is sent by the raw path
/// (the raw write invalidates the notes of the whole transaction), so the mix is correct and
/// costs what the raw write costs.
///
/// The recorded operations do not look at keys: the host replays them by position. Keys must
/// still be unique within the list (`KeyFn`); a list that breaks that is not noticed by the
/// recorded path, where the raw path would have sent the full value. On a signal that is not
/// attached as a keyed list, or whose slot the host does not observe, the operations are plain
/// mutations (and, like `Vec`'s methods, panic on an index out of range).
///
/// # Threading and re-entrancy
///
/// `Signal` is `Send + Sync`. Readers work on an immutable snapshot of the value (the signal
/// stores it behind an `Arc` and writers replace or copy-on-write it), so **no lock is held
/// while a reader runs**: the closure given to [`with`](Signal::with), a
/// [`Computed`](crate::Computed) or an [`Effect`](crate::Effect) may write any signal, including
/// the one it is reading (a clamp effect that writes back into its own input is fine; the
/// reader keeps seeing the snapshot it started with).
///
/// The one exception is [`update`](Signal::update): its closure runs with the value
/// write-locked, so it must not read or write the signal it is updating, **nor read a
/// [`Computed`](crate::Computed) (or anything else) that reads it**: recomputing the computed
/// would read the write-locked value. The restriction is transitive. Read what the closure needs
/// before calling `update`. A violation would deadlock; it is detected and panics instead (see
/// `update`).
///
/// # Example
///
/// ```
/// use keel_signals::Signal;
///
/// let count = Signal::new(1);
/// let same = count.clone();
/// same.set(2);
/// assert_eq!(count.get(), 2);
/// count.update(|n| *n += 1);
/// assert_eq!(same.with(|n| *n * 10), 30);
/// ```
pub struct Signal<T> {
    pub(crate) inner: Arc<SignalInner<T>>,
}

pub(crate) struct SignalInner<T> {
    /// The current value. Readers clone the `Arc` and let go of the lock; writers replace it
    /// (`set`) or mutate it in place, copying first if a reader still holds a snapshot
    /// (`update`).
    value: RwLock<Arc<T>>,
    pub(crate) binding: OnceLock<Binding>,
    pub(crate) dependents: Dependents,
    /// The op log of a list signal attached with [`StoreCell::attach_keyed`](crate::StoreCell):
    /// what the recorded list operations append to and every raw write invalidates.
    pub(crate) log: OnceLock<Arc<dyn ListLog>>,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        Signal {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T: SignalValue> Signal<T> {
    /// Creates a signal holding `value`.
    pub fn new(value: T) -> Signal<T> {
        Signal {
            inner: Arc::new(SignalInner {
                value: RwLock::new(Arc::new(value)),
                binding: OnceLock::new(),
                dependents: parking_lot::Mutex::new(Vec::new()),
                log: OnceLock::new(),
            }),
        }
    }

    /// Returns a clone of the current value.
    pub fn get(&self) -> T {
        T::clone(&self.snapshot())
    }

    /// Calls `f` with a reference to the current value and returns its result, without cloning
    /// the value.
    ///
    /// `f` sees a snapshot: writes made while it runs (even by `f` itself) do not change what it
    /// is looking at, and no lock is held, so `f` may write any signal.
    pub fn with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        f(&self.snapshot())
    }

    /// Replaces the value.
    ///
    /// Outside a [`txn`](crate::txn) this is a transaction of its own: the change is committed
    /// (delivered, effects run) before `set` returns. Inside one, it is committed when the
    /// outermost `txn` ends.
    ///
    /// Every call counts as a change, even if the new value equals the old one: `T` is not
    /// required to be comparable.
    ///
    /// # Panics
    ///
    /// Only if something the commit triggered panicked (a sink, an effect, a computed closure
    /// or an encoder): the value is already written, the commit has run to completion, and the
    /// first such panic is re-raised. See the crate docs.
    pub fn set(&self, value: T) {
        // The replaced value is handed back so it is dropped after the lock is released.
        let _old = self.write_with(|slot| {
            self.invalidate_log();
            std::mem::replace(slot, Arc::new(value))
        });
    }

    /// Mutates the value in place. Transaction behaviour is the same as [`set`](Signal::set).
    ///
    /// `f` runs with the value write-locked, so it must not read or write this signal, **and it
    /// must not read a [`Computed`](crate::Computed) that depends on it** (or an effect's
    /// inputs, or anything else that ends up reading this signal): the computed would recompute
    /// and read the locked value. Read what `f` needs before calling `update`.
    ///
    /// # Panics
    ///
    /// If `f` (on this thread) reads or writes the signal it is updating, directly or through a
    /// computed. That would otherwise deadlock forever, so it is detected and reported instead;
    /// the value is left as it was before `f` ran. Contention with *other* threads is not a
    /// violation and simply waits.
    ///
    /// If a reader is holding a snapshot at that moment (a computed or effect that is mid-run),
    /// the value is copied first so the reader is undisturbed; otherwise it is mutated in place.
    pub fn update(&self, f: impl FnOnce(&mut T)) {
        let me = self.id();
        self.write_with(|slot| {
            // Before `f` runs, so that a panic inside it cannot leave a half-changed list that
            // a keyed slot still believes its recorded ops describe.
            self.invalidate_log();
            let _updating = Updating::enter(me);
            f(Arc::make_mut(slot));
        });
    }

    /// Returns `true` if `self` and `other` are handles to the same signal.
    pub fn ptr_eq(&self, other: &Signal<T>) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Returns `true` once the signal has been attached to a store.
    pub fn is_attached(&self) -> bool {
        self.inner.binding.get().is_some()
    }

    /// The current value, as a snapshot that stays valid (and unchanged) however long the caller
    /// keeps it.
    fn snapshot(&self) -> Arc<T> {
        // `read_recursive` never waits for a queued writer, so a thread that already holds a
        // read guard cannot deadlock against one.
        if let Some(value) = self.inner.value.try_read_recursive() {
            return Arc::clone(&value);
        }
        // A writer holds the lock. If it is this very thread (inside `update`'s closure) waiting
        // would never end.
        self.assert_not_updating("read");
        Arc::clone(&self.inner.value.read_recursive())
    }

    /// Runs `f` with the value read-locked: no writer, recorded or raw, can change the value or
    /// its op log while `f` runs. Readers that take it with `snapshot` do not see the log.
    pub(crate) fn read_locked<R>(&self, f: impl FnOnce(&Arc<T>) -> R) -> R {
        if let Some(value) = self.inner.value.try_read_recursive() {
            return f(&value);
        }
        self.assert_not_updating("read");
        f(&self.inner.value.read_recursive())
    }

    /// Tells the op log of a keyed list (if any) that the list is about to be changed in a way
    /// the recorded ops cannot describe. Called with the value write-locked.
    fn invalidate_log(&self) {
        if let Some(log) = self.inner.log.get() {
            log.invalidate();
        }
    }

    /// The signal's identity for the "being updated on this thread" list.
    fn id(&self) -> usize {
        Arc::as_ptr(&self.inner) as usize
    }

    /// Panics if the calling thread is inside this signal's `update` closure: taking the lock
    /// again would deadlock.
    fn assert_not_updating(&self, what: &str) {
        let me = self.id();
        let reentered = UPDATING
            .try_with(|list| list.borrow().contains(&me))
            .unwrap_or(false);
        assert!(
            !reentered,
            "keel-signals: the closure passed to Signal::update {what} the signal it is \
             updating, directly or through a Computed that depends on it. That would deadlock: \
             `update` holds the value write-locked while the closure runs. Read what the closure \
             needs before calling `update`."
        );
    }

    fn write_with<R>(&self, f: impl FnOnce(&mut Arc<T>) -> R) -> R {
        // Before anything changes: a write that reaches the host or other nodes must come from
        // a thread the embedder allows to mutate (debug builds; see `set_write_checker`).
        #[cfg(debug_assertions)]
        if self.inner.binding.get().is_some() || !self.inner.dependents.lock().is_empty() {
            crate::context::assert_write_allowed();
        }
        // Drop order matters: the lock guard goes first, then the change is announced, then
        // the transaction ends (and commits if it was the outermost). Announcing from a guard
        // means a panicking `f` still marks whatever it managed to change.
        let _txn = TxnGuard::enter();
        let _announce = Announce(&self.inner);
        let mut value = match self.inner.value.try_write() {
            Some(value) => value,
            None => {
                // Another thread holds the lock (wait), or this thread does, inside `update`'s
                // closure (waiting would never end).
                self.assert_not_updating("wrote");
                self.inner.value.write()
            }
        };
        f(&mut value)
    }

    /// The weak handle other nodes register on to be invalidated by this signal.
    pub(crate) fn add_dependent(&self, dependent: Weak<dyn Reactive>) {
        add_dependent(&self.inner.dependents, dependent);
    }
}

thread_local! {
    /// The signals whose `update` closure is running on this thread (by address). Only consulted
    /// when the value lock is found taken, so it costs nothing on the uncontended path.
    static UPDATING: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Marks a signal as being updated on this thread until dropped.
struct Updating;

impl Updating {
    fn enter(signal: usize) -> Updating {
        let _ = UPDATING.try_with(|list| list.borrow_mut().push(signal));
        Updating
    }
}

impl Drop for Updating {
    fn drop(&mut self) {
        let _ = UPDATING.try_with(|list| list.borrow_mut().pop());
    }
}

/// Announces a change on drop: records the slot in the transaction and invalidates dependents.
struct Announce<'a, T>(&'a SignalInner<T>);

impl<T> Drop for Announce<'_, T> {
    fn drop(&mut self) {
        if let Some(binding) = self.0.binding.get() {
            record(binding);
        }
        notify_dependents(&self.0.dependents);
    }
}

impl<T: SignalValue + Default> Default for Signal<T> {
    fn default() -> Self {
        Signal::new(T::default())
    }
}

impl<T: SignalValue + fmt::Debug> fmt::Debug for Signal<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut s = f.debug_struct("Signal");
        match self.inner.value.try_read_recursive() {
            Some(value) => s.field("value", &**value),
            None => s.field("value", &format_args!("<locked>")),
        };
        s.field("attached", &self.is_attached()).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::CaptureSink;
    use crate::{Computed, with_sink};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn get_returns_the_initial_value() {
        assert_eq!(Signal::new(7).get(), 7);
        assert_eq!(Signal::new(String::from("hi")).get(), "hi");
        assert_eq!(Signal::new(Vec::<u8>::new()).get(), Vec::<u8>::new());
    }

    #[test]
    fn set_replaces_the_value() {
        let s = Signal::new(1);
        s.set(2);
        assert_eq!(s.get(), 2);
        s.set(2);
        assert_eq!(s.get(), 2);
    }

    #[test]
    fn update_mutates_in_place() {
        let s = Signal::new(vec![1, 2]);
        s.update(|v| v.push(3));
        s.update(|v| v.retain(|x| x % 2 == 1));
        assert_eq!(s.get(), vec![1, 3]);
    }

    #[test]
    fn with_borrows_without_cloning() {
        #[derive(Debug)]
        struct CountClones(Arc<AtomicUsize>);
        impl Clone for CountClones {
            fn clone(&self) -> Self {
                self.0.fetch_add(1, Ordering::SeqCst);
                CountClones(Arc::clone(&self.0))
            }
        }
        impl keel_wire::Encode for CountClones {
            fn encode(&self, _w: &mut keel_wire::Writer) {}
        }

        let clones = Arc::new(AtomicUsize::new(0));
        let s = Signal::new(CountClones(clones.clone()));
        let n = s.with(|v| Arc::strong_count(&v.0));
        assert_eq!(n, 2);
        assert_eq!(clones.load(Ordering::SeqCst), 0, "with must not clone");
        let _ = s.get();
        assert_eq!(clones.load(Ordering::SeqCst), 1, "get clones once");
    }

    #[test]
    fn with_returns_the_closure_result() {
        let s = Signal::new(String::from("hello"));
        assert_eq!(s.with(|v| v.len()), 5);
        assert_eq!(s.with(|v| v.to_uppercase()), "HELLO");
    }

    #[test]
    fn clone_shares_the_signal() {
        let a = Signal::new(1);
        let b = a.clone();
        assert!(a.ptr_eq(&b));
        b.set(5);
        assert_eq!(a.get(), 5);
        a.update(|v| *v += 1);
        assert_eq!(b.get(), 6);
    }

    #[test]
    fn separate_signals_are_independent() {
        let a = Signal::new(1);
        let b = Signal::new(1);
        assert!(!a.ptr_eq(&b));
        a.set(2);
        assert_eq!(b.get(), 1);
    }

    #[test]
    fn default_uses_the_type_default() {
        assert_eq!(Signal::<u32>::default().get(), 0);
        assert_eq!(Signal::<String>::default().get(), "");
    }

    #[test]
    fn debug_shows_the_value() {
        let s = Signal::new(3);
        let text = format!("{s:?}");
        assert!(text.contains("value: 3"), "{text}");
        assert!(text.contains("attached: false"), "{text}");
    }

    #[test]
    fn debug_while_write_locked_does_not_block() {
        let s = Signal::new(3);
        s.update(|_| {
            let text = format!("{s:?}");
            assert!(text.contains("<locked>"), "{text}");
        });
    }

    #[test]
    fn unattached_signals_produce_no_change_sets() {
        let sink = CaptureSink::new();
        with_sink(sink.clone(), || {
            let s = Signal::new(1);
            s.set(2);
            s.update(|v| *v += 1);
        });
        assert!(sink.is_empty());
    }

    #[test]
    fn signals_are_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Signal<i32>>();
        assert_send_sync::<Signal<Vec<String>>>();
    }

    #[test]
    fn concurrent_updates_are_not_lost() {
        let s = Signal::new(0_u64);
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let s = s.clone();
                scope.spawn(move || {
                    for _ in 0..1000 {
                        s.update(|v| *v += 1);
                    }
                });
            }
        });
        assert_eq!(s.get(), 4000);
    }

    #[test]
    fn with_holds_no_lock_so_it_may_write_the_same_signal() {
        let s = Signal::new(vec![1, 2, 3]);
        let seen = s.with(|v| {
            s.set(vec![9]);
            s.update(|w| w.push(10));
            v.clone()
        });
        assert_eq!(
            seen,
            vec![1, 2, 3],
            "the reader keeps the snapshot it started with"
        );
        assert_eq!(s.get(), vec![9, 10]);
    }

    #[test]
    fn update_copies_the_value_when_a_reader_holds_a_snapshot() {
        let s = Signal::new(vec![1, 2, 3]);
        s.with(|before| {
            s.update(|v| v.push(4));
            assert_eq!(
                before,
                &vec![1, 2, 3],
                "the outstanding snapshot is untouched"
            );
        });
        assert_eq!(s.get(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn update_does_not_copy_when_nobody_reads() {
        #[derive(Debug)]
        struct CountClones(Arc<AtomicUsize>);
        impl Clone for CountClones {
            fn clone(&self) -> Self {
                self.0.fetch_add(1, Ordering::SeqCst);
                CountClones(Arc::clone(&self.0))
            }
        }
        impl keel_wire::Encode for CountClones {
            fn encode(&self, _w: &mut keel_wire::Writer) {}
        }
        let clones = Arc::new(AtomicUsize::new(0));
        let s = Signal::new(CountClones(clones.clone()));
        for _ in 0..10 {
            s.update(|_| {});
        }
        assert_eq!(
            clones.load(Ordering::SeqCst),
            0,
            "in-place updates never clone"
        );
    }

    #[test]
    fn read_locked_keeps_writers_out_until_it_returns() {
        use std::sync::atomic::AtomicBool;
        use std::time::Duration;
        // A keyed slot takes its op log and looks at the list inside `read_locked`; that is only
        // sound if no write (which appends to the log under the write lock) can land in
        // between.
        let s = Signal::new(vec![1, 2]);
        let written = AtomicBool::new(false);
        std::thread::scope(|scope| {
            s.read_locked(|seen| {
                scope.spawn(|| {
                    s.update(|v| v.push(3));
                    written.store(true, Ordering::SeqCst);
                });
                std::thread::sleep(Duration::from_millis(50));
                assert!(!written.load(Ordering::SeqCst), "the writer got in");
                assert_eq!(**seen, vec![1, 2]);
            });
        });
        assert!(written.load(Ordering::SeqCst));
        assert_eq!(s.get(), vec![1, 2, 3]);
    }

    #[test]
    fn a_raw_write_invalidates_the_op_log_and_a_recorded_one_does_not() {
        use crate::oplog::{KeyedLog, ListLog};
        let s = Signal::new(vec![1_u32, 2]);
        let log = Arc::new(KeyedLog::<u32>::new());
        let erased: Arc<dyn ListLog> = log.clone();
        assert!(s.inner.log.set(erased).is_ok());
        log.arm(2);
        s.push(3);
        s.insert(0, 0);
        s.remove(1);
        s.update_at(0, |n| *n += 10);
        s.move_item(0, 1);
        s.clear();
        assert!(
            log.is_recording(),
            "recorded operations keep the log usable"
        );
        s.update(|v| v.push(1));
        assert!(!log.is_recording(), "update makes it stale");
        log.arm(1);
        s.set(vec![5]);
        assert!(!log.is_recording(), "so does set");
        log.arm(1);
        s.replace(vec![6]);
        assert!(!log.is_recording(), "and replace");
    }

    #[test]
    fn a_write_inside_with_to_another_signal_is_fine() {
        let a = Signal::new(1);
        let b = Signal::new(0);
        a.with(|v| b.set(*v * 2));
        assert_eq!(b.get(), 2);
    }

    #[test]
    fn a_panicking_update_still_marks_dependents_and_releases_the_lock() {
        let a = Signal::new(1);
        let runs = Arc::new(AtomicUsize::new(0));
        let doubled = {
            let runs = runs.clone();
            Computed::new(&a, move |v: &i32| {
                runs.fetch_add(1, Ordering::SeqCst);
                v * 2
            })
        };
        assert_eq!(doubled.get(), 2);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            a.update(|v| {
                *v = 10;
                panic!("boom");
            });
        }));
        assert!(result.is_err());
        // The write that happened before the panic is visible and the computed noticed it.
        assert_eq!(a.get(), 10);
        assert_eq!(doubled.get(), 20);
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }
}
