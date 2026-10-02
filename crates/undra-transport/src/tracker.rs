//! What one connection has asked of the core, so that a disconnect can be cleaned up.
//!
//! The tracker never talks to the runtime; it only remembers. [`Session`](crate::session)
//! consults it at teardown to cancel calls, stop observations, release handles and fail port
//! calls that will never be answered.

use std::collections::{HashMap, HashSet};

use undra_wire::payload::{ReplyStatus, StreamFlag};

/// `Observe.signal_id` meaning "every signal of the store" (SPEC 3.2, kind 9).
pub(crate) const ALL_SIGNALS: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CallState {
    /// Answered by exactly one `Reply`. `constructor` calls yield a new handle.
    Pending { constructor: bool },
    /// Answered `stream_opened`; items follow until `End`, `Error` or `Failed` (ADR-036).
    Stream,
}

/// The core-side state one connection has created and not yet given back.
#[derive(Debug, Default)]
pub(crate) struct Tracker {
    calls: HashMap<u32, CallState>,
    observed: HashSet<(u64, u32)>,
    /// The references this connection's constructors made and nobody released, per handle: a
    /// constructor that returns an interned object (an `Arc<Self>` singleton) makes one more of
    /// the same handle each time it is called. (What the client's other calls returned is the
    /// runtime's to count, per origin.)
    constructed: HashMap<u64, u32>,
    port_calls: HashSet<u32>,
}

/// What [`Tracker::drain`] hands the teardown: everything sorted, so the cleanup is
/// deterministic.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Leftovers {
    /// Calls and streams still open.
    pub calls: Vec<u32>,
    /// `(handle, signal_id)` pairs still observed.
    pub observed: Vec<(u64, u32)>,
    /// The references this connection's constructors made and nobody released: a handle once per
    /// reference.
    pub constructed: Vec<u64>,
    /// Port calls the core is still waiting on.
    pub port_calls: Vec<u32>,
}

impl Tracker {
    /// Records a new call. `false` if `call_id` is already open (the caller must not run it
    /// again: the first call's reply is still to come).
    pub(crate) fn begin_call(&mut self, call_id: u32, constructor: bool) -> bool {
        if self.calls.contains_key(&call_id) {
            return false;
        }
        self.calls
            .insert(call_id, CallState::Pending { constructor });
        true
    }

    /// How many plain calls (not streams) are waiting for their reply.
    pub(crate) fn open_plain_calls(&self) -> usize {
        self.calls
            .values()
            .filter(|state| matches!(state, CallState::Pending { .. }))
            .count()
    }

    /// Forgets a call (cancelled, or refused without a reply).
    pub(crate) fn end_call(&mut self, call_id: u32) {
        self.calls.remove(&call_id);
    }

    /// The core replied to `call_id`. A successful constructor reply carries the new handle
    /// as the first eight bytes of its body (SPEC 3.4; the body is the returned `u64`).
    pub(crate) fn on_reply(&mut self, call_id: u32, status: ReplyStatus, body: &[u8]) {
        if status == ReplyStatus::StreamOpened {
            if let Some(state) = self.calls.get_mut(&call_id) {
                *state = CallState::Stream;
            }
            return;
        }
        let state = self.calls.remove(&call_id);
        if state == Some(CallState::Pending { constructor: true }) && status == ReplyStatus::Ok {
            if let Some(handle) = body.first_chunk::<8>() {
                *self
                    .constructed
                    .entry(u64::from_le_bytes(*handle))
                    .or_insert(0) += 1;
            }
        }
    }

    /// The core sent a stream item; the end and error markers close the stream.
    pub(crate) fn on_stream_item(&mut self, call_id: u32, flag: StreamFlag) {
        if flag != StreamFlag::Item {
            self.calls.remove(&call_id);
        }
    }

    /// The client (un)observed a signal.
    pub(crate) fn observe(&mut self, handle: u64, signal_id: u32, on: bool) {
        if on {
            self.observed.insert((handle, signal_id));
        } else if signal_id == ALL_SIGNALS {
            self.observed.retain(|&(h, _)| h != handle);
        } else {
            self.observed.remove(&(handle, signal_id));
        }
    }

    /// Whether the client observes `signal_id` of `handle` (one by one, or all of the store's).
    pub(crate) fn covers(&self, handle: u64, signal_id: u32) -> bool {
        self.observed.contains(&(handle, signal_id)) || self.observed.contains(&(handle, ALL_SIGNALS))
    }

    /// The signals of `handle` the client observes, sorted.
    pub(crate) fn signals_of(&self, handle: u64) -> Vec<u32> {
        let mut signals: Vec<u32> = self
            .observed
            .iter()
            .filter(|&&(h, _)| h == handle)
            .map(|&(_, s)| s)
            .collect();
        signals.sort_unstable();
        signals
    }

    /// The client released one reference to `handle`. Returns whether it was one of the
    /// references this connection's constructors made (counted here); `false` means it was one a
    /// call returned, which the runtime counts for the session's origin.
    pub(crate) fn release(&mut self, handle: u64) -> bool {
        let Some(count) = self.constructed.get_mut(&handle) else {
            return false;
        };
        *count -= 1;
        if *count == 0 {
            self.constructed.remove(&handle);
        }
        true
    }

    /// The client's observation of `handle` ends with the object (the last reference to it is
    /// gone): there is nothing left to stop at a disconnect.
    pub(crate) fn forget_observed(&mut self, handle: u64) {
        self.observed.retain(|&(h, _)| h != handle);
    }

    /// Takes over references another connection of the same session left (a resumed session): a
    /// handle once per reference.
    pub(crate) fn adopt(&mut self, handles: &[u64]) {
        for handle in handles {
            *self.constructed.entry(*handle).or_insert(0) += 1;
        }
    }

    /// The core issued port call `id` to this client.
    pub(crate) fn begin_port_call(&mut self, id: u32) {
        self.port_calls.insert(id);
    }

    /// The client answered port call `id`.
    pub(crate) fn end_port_call(&mut self, id: u32) {
        self.port_calls.remove(&id);
    }

    /// Takes everything out, leaving the tracker empty.
    pub(crate) fn drain(&mut self) -> Leftovers {
        fn sorted<T: Ord>(items: impl IntoIterator<Item = T>) -> Vec<T> {
            let mut v: Vec<T> = items.into_iter().collect();
            v.sort_unstable();
            v
        }
        Leftovers {
            calls: sorted(self.calls.drain().map(|(id, _)| id)),
            observed: sorted(self.observed.drain()),
            constructed: sorted(
                self.constructed
                    .drain()
                    .flat_map(|(handle, count)| std::iter::repeat_n(handle, count as usize)),
            ),
            port_calls: sorted(self.port_calls.drain()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signal_is_covered_when_observed_alone_or_with_its_store() {
        let mut t = Tracker::default();
        t.observe(1, 0, true);
        t.observe(2, ALL_SIGNALS, true);
        assert!(t.covers(1, 0));
        assert!(!t.covers(1, 1), "only the signal that was asked for");
        assert!(t.covers(2, 0) && t.covers(2, 99), "every signal of a store observed whole");
        assert!(!t.covers(3, 0));
        t.observe(1, 0, false);
        assert!(!t.covers(1, 0));
        assert_eq!(t.signals_of(2), [ALL_SIGNALS]);
        t.observe(5, 3, true);
        t.observe(5, 1, true);
        assert_eq!(t.signals_of(5), [1, 3], "sorted, for a deterministic re-statement");
    }

    #[test]
    fn a_call_is_open_until_it_is_answered() {
        let mut t = Tracker::default();
        assert!(t.begin_call(1, false));
        assert!(!t.begin_call(1, false), "a duplicate id is refused");
        t.on_reply(1, ReplyStatus::Ok, &[]);
        assert!(t.begin_call(1, false), "the id is free again");
        assert_eq!(t.drain().calls, [1]);
    }

    #[test]
    fn a_stream_stays_open_until_its_end_error_or_failed_marker() {
        let mut t = Tracker::default();
        for (id, closing) in [
            (1, StreamFlag::End),
            (2, StreamFlag::Error),
            (3, StreamFlag::Failed),
        ] {
            t.begin_call(id, false);
            t.on_reply(id, ReplyStatus::StreamOpened, &[]);
            t.on_stream_item(id, StreamFlag::Item);
            assert_eq!(t.calls.len(), 1, "still open after an item");
            t.on_stream_item(id, closing);
            assert!(t.calls.is_empty());
        }
    }

    #[test]
    fn a_constructor_reply_records_the_new_handle() {
        let mut t = Tracker::default();
        t.begin_call(1, true);
        t.on_reply(1, ReplyStatus::Ok, &0x0000_0002_0000_0007_u64.to_le_bytes());
        t.begin_call(2, true);
        t.on_reply(2, ReplyStatus::Error, &[1, 2, 3, 4, 5, 6, 7, 8]);
        t.begin_call(3, true);
        t.on_reply(3, ReplyStatus::Ok, &[1, 2, 3]);
        t.begin_call(4, false);
        t.on_reply(4, ReplyStatus::Ok, &9_u64.to_le_bytes());
        let left = t.drain();
        assert_eq!(left.constructed, [0x0000_0002_0000_0007]);
        assert!(left.calls.is_empty());
    }

    #[test]
    fn observation_is_per_signal_or_per_store() {
        let mut t = Tracker::default();
        t.observe(1, 0, true);
        t.observe(1, 1, true);
        t.observe(2, 0, true);
        t.observe(1, 0, false);
        t.observe(2, ALL_SIGNALS, false);
        assert_eq!(t.drain().observed, [(1, 1)]);
        t.observe(1, 0, true);
        t.observe(1, 1, true);
        t.observe(1, ALL_SIGNALS, false);
        assert!(t.drain().observed.is_empty());
    }

    #[test]
    fn a_release_gives_back_one_constructor_reference_and_says_whether_there_was_one() {
        let mut t = Tracker::default();
        // The same handle constructed twice (an interned singleton): two references.
        for id in [1, 2] {
            t.begin_call(id, true);
            t.on_reply(id, ReplyStatus::Ok, &5_u64.to_le_bytes());
        }
        t.observe(5, 0, true);
        t.observe(6, 0, true);
        assert!(t.release(5), "the first constructor reference");
        assert_eq!(t.constructed.get(&5), Some(&1), "one reference is left");
        assert!(t.release(5));
        assert!(
            !t.release(5),
            "nothing is left here: a reference a call returned is the runtime's"
        );
        assert!(!t.release(77), "a handle this connection never constructed");
        // Observations end with the object, not with a release (another reference may remain).
        t.forget_observed(5);
        let left = t.drain();
        assert!(left.constructed.is_empty());
        assert_eq!(left.observed, [(6, 0)]);
    }

    #[test]
    fn a_reference_per_constructor_call_is_given_back_at_a_disconnect() {
        let mut t = Tracker::default();
        for id in [1, 2] {
            t.begin_call(id, true);
            t.on_reply(id, ReplyStatus::Ok, &5_u64.to_le_bytes());
        }
        assert_eq!(t.drain().constructed, [5, 5], "once per reference, not once per handle");
    }

    #[test]
    fn adopted_objects_are_this_connections_to_give_back() {
        let mut t = Tracker::default();
        t.adopt(&[7, 3, 7]);
        t.begin_call(1, true);
        t.on_reply(1, ReplyStatus::Ok, &9_u64.to_le_bytes());
        assert!(t.release(3));
        assert_eq!(t.drain().constructed, [7, 7, 9]);
    }

    #[test]
    fn port_calls_are_open_until_answered_and_drain_is_sorted() {
        let mut t = Tracker::default();
        for id in [9, 3, 5] {
            t.begin_port_call(id);
        }
        t.end_port_call(5);
        assert_eq!(t.drain().port_calls, [3, 9]);
        assert_eq!(t.drain(), Leftovers::default(), "drain empties the tracker");
    }
}
