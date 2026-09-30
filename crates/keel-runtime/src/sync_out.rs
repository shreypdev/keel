//! The synchronous reply slot (ADR-028): how a `call_sync` reply is built without touching the
//! heap.
//!
//! A synchronous call used to allocate three times on its way out: the dispatcher's encoded
//! result (`Vec<u8>`), the `Box<dyn Any>` of its `DispatchOutcome`, and the `Reply` payload
//! that wraps the result with its header. The slot replaces all three with one buffer that
//! lives in a thread-local and is reused by every call on the thread:
//!
//! 1. [`Runtime::call_sync_with`](crate::Runtime::call_sync_with) *arms* the slot for its
//!    runtime and call id ([`SyncLease::acquire`]);
//! 2. the generated dispatcher answers with [`Runtime::sync_ok`](crate::Runtime::sync_ok) or
//!    [`sync_err`](crate::Runtime::sync_err), which find the armed slot, write the complete
//!    `Reply` payload (`call_id`, status, encoded body) into the buffer and return a
//!    [`Written`] outcome: a zero-sized marker, so boxing it into the `DispatchOutcome` does
//!    not allocate;
//! 3. the runtime lends the buffer to the caller's closure ([`SyncLease::read`]).
//!
//! Anything that cannot use the slot falls back to the allocating [`DispatchResult::Sync`]
//! path, which stays the contract for `call`, for layers and for hand-written dispatchers:
//! the slot is busy (a call made from the `read` closure, or from a method that was itself
//! called through the slot), armed for another runtime, or gone (thread teardown). The two
//! paths produce byte-identical replies.
//!
//! The slot is per thread, so it needs no lock, and it is a plain `Cell` state machine: the
//! buffer is *moved out* of the slot while anyone uses it, so there is no borrow to conflict
//! and a re-entrant call can only ever see "busy". On `wasm32` (one thread) the thread-local
//! is a static.
//!
//! [`DispatchResult::Sync`]: crate::DispatchResult::Sync

use core::cell::Cell;
use core::marker::PhantomData;

use keel_wire::Writer;
use keel_wire::payload::ReplyStatus;

/// The largest buffer the slot keeps between calls. A reply bigger than this is served (the
/// buffer grows to fit) and the allocation is released afterwards, so one huge reply does not
/// pin memory on every thread that ever made a call.
const MAX_KEPT: usize = 64 * 1024;

/// The marker a dispatcher's `DispatchOutcome` holds when its reply was written into the slot.
///
/// Zero-sized on purpose: `Box::new` of a zero-sized value does not allocate, which is the
/// whole point. The only place it is built is [`write`], after the slot took the bytes.
pub(crate) struct Written(());

/// Where the slot is in the life of one synchronous call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Nobody uses the slot.
    Free,
    /// `call_sync_with` of the runtime with this id is running its dispatcher; the first
    /// answer of that runtime's dispatcher is written into the buffer.
    Armed(u64),
    /// The dispatcher wrote the reply; the buffer holds it.
    Written,
    /// The reply is lent to the caller's closure.
    Reading,
}

struct Slot {
    state: Cell<State>,
    call_id: Cell<u32>,
    /// The reply buffer. Moved out (`take`) while it is written or read, so nothing ever
    /// aliases it; a panic in between just loses it and the next call regrows one.
    buf: Cell<Vec<u8>>,
}

impl Slot {
    const fn new() -> Slot {
        Slot {
            state: Cell::new(State::Free),
            call_id: Cell::new(0),
            buf: Cell::new(Vec::new()),
        }
    }
}

thread_local! {
    static SLOT: Slot = const { Slot::new() };
}

/// Ownership of the armed slot for the duration of one `call_sync_with`. Dropping it frees the
/// slot again, on every path including a panic. Not `Send`: the slot is this thread's.
#[must_use = "the slot is armed until the lease is dropped"]
pub(crate) struct SyncLease {
    /// Whether the slot still has to be freed on drop (`read` frees it itself, in the same
    /// thread-local access it reads in, so the common path touches the thread-local three
    /// times in all: arm, write, read).
    armed: bool,
    _not_send: PhantomData<*const ()>,
}

impl SyncLease {
    /// Arms the slot for `runtime`'s call `call_id`, or `None` when the slot is busy (this
    /// call then takes the allocating path).
    pub(crate) fn acquire(runtime: u64, call_id: u32) -> Option<SyncLease> {
        SLOT.try_with(|slot| {
            if slot.state.get() != State::Free {
                return None;
            }
            slot.state.set(State::Armed(runtime));
            slot.call_id.set(call_id);
            Some(SyncLease {
                armed: true,
                _not_send: PhantomData,
            })
        })
        .ok()
        .flatten()
    }

    /// Lends the reply payload to `f`, then frees the slot. `f` may call the runtime again:
    /// that call sees a busy slot and allocates, it cannot disturb the bytes being read.
    ///
    /// If the dispatcher never wrote a reply (the runtime only calls this after a written
    /// outcome, so this is a bug guard), `f` gets a well-formed status 5 reply instead.
    pub(crate) fn read<R>(mut self, f: impl FnOnce(&[u8]) -> R) -> R {
        // The lease proves the thread-local is alive: its destructor only runs at thread exit.
        let result = SLOT.with(|slot| {
            if slot.state.get() != State::Written {
                let mut w = Writer::new();
                w.write_u32(slot.call_id.get());
                w.write_u8(ReplyStatus::BadRequest.as_u8());
                w.write_str("internal: a synchronous reply was promised but not written");
                slot.state.set(State::Free);
                return f(w.as_slice());
            }
            slot.state.set(State::Reading);
            let buf = slot.buf.take();
            let result = f(&buf);
            // Keep the allocation for the next call unless it is unreasonably large.
            if buf.capacity() <= MAX_KEPT {
                slot.buf.set(buf);
            }
            slot.state.set(State::Free);
            result
        });
        // Freed above; if `f` had panicked, `armed` is still set and `drop` frees the slot.
        self.armed = false;
        result
    }
}

impl Drop for SyncLease {
    fn drop(&mut self) {
        if self.armed {
            let _ = SLOT.try_with(|slot| slot.state.set(State::Free));
        }
    }
}

/// Writes the complete `Reply` payload (`call_id`, `status`, `value` encoded by `encode`) into
/// the slot if it is armed for `runtime`; `false` (nothing written) otherwise.
#[inline]
pub(crate) fn write<T>(
    runtime: u64,
    status: ReplyStatus,
    value: &T,
    encode: fn(&T, &mut Writer),
) -> bool {
    SLOT.try_with(|slot| {
        if slot.state.get() != State::Armed(runtime) {
            return false;
        }
        let mut w = Writer::from_vec(slot.buf.take());
        w.clear();
        w.write_u32(slot.call_id.get());
        w.write_u8(status.as_u8());
        encode(value, &mut w);
        slot.buf.set(w.into_vec());
        slot.state.set(State::Written);
        true
    })
    .unwrap_or(false)
}

impl Written {
    /// The marker, for a reply [`write`] just stored.
    pub(crate) fn new() -> Written {
        Written(())
    }
}

/// Whether the slot is in use on this thread, for tests of the fallback.
#[cfg(test)]
pub(crate) fn is_free() -> bool {
    SLOT.with(|slot| slot.state.get() == State::Free)
}

/// Keeps the slot busy until dropped, so every `call_sync` made meanwhile on this thread takes
/// the allocating path. The reference the tests compare the fast path against.
pub(crate) struct Occupied {
    _lease: Option<SyncLease>,
}

impl Occupied {
    /// Takes the slot (a no-op if it is busy already).
    pub(crate) fn new() -> Occupied {
        Occupied {
            _lease: SyncLease::acquire(0, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use keel_wire::Encode;

    use super::*;

    fn put<T: Encode>(runtime: u64, status: ReplyStatus, value: &T) -> bool {
        write(runtime, status, value, T::encode)
    }

    fn fresh() {
        // Tests share threads with nothing else, but a failed one must not poison the next.
        SLOT.with(|slot| slot.state.set(State::Free));
    }

    #[test]
    fn write_needs_an_armed_slot_for_the_same_runtime() {
        fresh();
        assert!(!put(1, ReplyStatus::Ok, &7_u32));
        let lease = SyncLease::acquire(1, 9).expect("free");
        assert!(!put(2, ReplyStatus::Ok, &7_u32), "another runtime's slot");
        assert!(put(1, ReplyStatus::Ok, &7_u32));
        assert!(!put(1, ReplyStatus::Ok, &8_u32), "one answer per arming");
        let bytes = lease.read(<[u8]>::to_vec);
        assert_eq!(bytes, [9, 0, 0, 0, 0, 7, 0, 0, 0]);
        assert!(is_free());
    }

    #[test]
    fn an_error_status_is_in_the_header() {
        fresh();
        let lease = SyncLease::acquire(1, 1).expect("free");
        assert!(put(1, ReplyStatus::Error, &String::from("no")));
        assert_eq!(
            lease.read(<[u8]>::to_vec),
            [1, 0, 0, 0, 1, 2, 0, 0, 0, b'n', b'o']
        );
    }

    #[test]
    fn a_second_lease_is_refused_until_the_first_is_dropped() {
        fresh();
        let first = SyncLease::acquire(1, 1).expect("free");
        assert!(SyncLease::acquire(1, 2).is_none());
        assert!(SyncLease::acquire(2, 3).is_none());
        drop(first);
        assert!(SyncLease::acquire(1, 2).is_some());
        fresh();
    }

    #[test]
    fn the_slot_is_busy_while_the_reply_is_being_read() {
        fresh();
        let lease = SyncLease::acquire(1, 1).expect("free");
        assert!(put(1, ReplyStatus::Ok, &1_u8));
        lease.read(|_| {
            assert!(SyncLease::acquire(1, 2).is_none(), "re-entrant call");
            assert!(!put(1, ReplyStatus::Ok, &2_u8), "not armed while reading");
        });
        assert!(is_free());
    }

    #[test]
    fn a_panicking_reader_frees_the_slot() {
        fresh();
        let lease = SyncLease::acquire(1, 1).expect("free");
        assert!(put(1, ReplyStatus::Ok, &1_u8));
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            lease.read(|_| panic!("reader"));
        }));
        assert!(caught.is_err());
        assert!(is_free());
        // And the next call works.
        let lease = SyncLease::acquire(1, 2).expect("free again");
        assert!(put(1, ReplyStatus::Ok, &3_u8));
        assert_eq!(lease.read(<[u8]>::to_vec), [2, 0, 0, 0, 0, 3]);
    }

    #[test]
    fn a_panicking_encoder_frees_the_slot_and_the_next_call_works() {
        struct Bomb;
        impl Encode for Bomb {
            fn encode(&self, _: &mut Writer) {
                panic!("encode");
            }
        }
        fresh();
        let lease = SyncLease::acquire(1, 1).expect("free");
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            put(1, ReplyStatus::Ok, &Bomb)
        }));
        assert!(caught.is_err());
        drop(lease);
        assert!(is_free());
        let lease = SyncLease::acquire(1, 2).expect("free again");
        assert!(put(1, ReplyStatus::Ok, &3_u8));
        assert_eq!(lease.read(<[u8]>::to_vec), [2, 0, 0, 0, 0, 3]);
    }

    #[test]
    fn the_buffer_is_reused_and_a_huge_reply_is_not_kept() {
        fresh();
        let lease = SyncLease::acquire(1, 1).expect("free");
        assert!(put(1, ReplyStatus::Ok, &1_u8));
        lease.read(|_| ());
        let kept = SLOT.with(|slot| {
            let buf = slot.buf.take();
            let capacity = buf.capacity();
            slot.buf.set(buf);
            capacity
        });
        assert!(kept >= 6, "the allocation is kept for the next call");

        let lease = SyncLease::acquire(1, 2).expect("free");
        assert!(put(1, ReplyStatus::Ok, &vec![0_u8; MAX_KEPT + 1]));
        assert_eq!(lease.read(|reply| reply.len()), 5 + 4 + MAX_KEPT + 1);
        let kept = SLOT.with(|slot| {
            let buf = slot.buf.take();
            let capacity = buf.capacity();
            slot.buf.set(buf);
            capacity
        });
        assert!(kept <= MAX_KEPT, "a huge reply's buffer is released");
    }

    #[test]
    fn sync_ok_and_sync_err_use_the_slot_when_armed_and_build_a_result_otherwise() {
        use crate::DispatchResult;
        use crate::testing::TestRuntime;

        fresh();
        let t = TestRuntime::new();
        let rt = t.runtime();

        // Not armed (a `call`, a layer, a test calling the dispatcher): the classic result.
        match rt.sync_ok(&5_u32, u32::encode).downcast::<DispatchResult>() {
            Ok(DispatchResult::Sync(Ok(bytes))) => assert_eq!(bytes, [5, 0, 0, 0]),
            other => panic!("expected a classic Ok result, got {other:?}"),
        }
        match rt.sync_err(&6_u8, u8::encode).downcast::<DispatchResult>() {
            Ok(DispatchResult::Sync(Err(bytes))) => assert_eq!(bytes, [6]),
            other => panic!("expected a classic Err result, got {other:?}"),
        }

        // Armed for this runtime: the reply is written into the slot, the outcome is the marker.
        let lease = SyncLease::acquire(rt.id(), 3).expect("free");
        let outcome = rt.sync_ok(&5_u32, u32::encode);
        assert!(outcome.is::<Written>());
        assert!(!outcome.is::<DispatchResult>());
        assert_eq!(lease.read(<[u8]>::to_vec), [3, 0, 0, 0, 0, 5, 0, 0, 0]);

        // Armed for another runtime: not this one's slot.
        let lease = SyncLease::acquire(rt.id() + 1, 4).expect("free");
        assert!(rt.sync_err(&1_u8, u8::encode).is::<DispatchResult>());
        drop(lease);
        assert!(is_free());
    }

    #[test]
    fn a_marker_with_nothing_written_answers_status_5_instead_of_garbage() {
        fresh();
        let lease = SyncLease::acquire(1, 12).expect("free");
        // The runtime never does this (it only reads after a written outcome); the guard is for
        // a bug, and still produces a well-formed reply.
        let reply = lease.read(<[u8]>::to_vec);
        assert_eq!(reply[..5], [12, 0, 0, 0, 5]);
        assert!(is_free());
    }

    #[test]
    fn the_marker_is_zero_sized() {
        assert_eq!(core::mem::size_of::<Written>(), 0);
    }
}
