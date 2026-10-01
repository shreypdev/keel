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
    constructed: HashSet<u64>,
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
    /// Handles this connection's constructors returned and nobody released.
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
                self.constructed.insert(u64::from_le_bytes(*handle));
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

    /// The client released `handle`: nothing about it is left to clean up.
    pub(crate) fn release(&mut self, handle: u64) {
        self.observed.retain(|&(h, _)| h != handle);
        self.constructed.remove(&handle);
    }

    /// Takes over objects another connection of the same session left (a resumed session).
    pub(crate) fn adopt(&mut self, handles: &[u64]) {
        self.constructed.extend(handles.iter().copied());
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
            constructed: sorted(self.constructed.drain()),
            port_calls: sorted(self.port_calls.drain()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn release_forgets_everything_about_the_handle() {
        let mut t = Tracker::default();
        t.begin_call(1, true);
        t.on_reply(1, ReplyStatus::Ok, &5_u64.to_le_bytes());
        t.observe(5, 0, true);
        t.observe(6, 0, true);
        t.release(5);
        let left = t.drain();
        assert!(left.constructed.is_empty());
        assert_eq!(left.observed, [(6, 0)]);
    }

    #[test]
    fn adopted_objects_are_this_connections_to_give_back() {
        let mut t = Tracker::default();
        t.adopt(&[7, 3]);
        t.begin_call(1, true);
        t.on_reply(1, ReplyStatus::Ok, &9_u64.to_le_bytes());
        t.release(3);
        assert_eq!(t.drain().constructed, [7, 9]);
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
