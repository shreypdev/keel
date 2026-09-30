//! Ports: how the core calls the platform (SPEC 5.7, 6.3, 8) and how the platform's events
//! reach the core.
//!
//! * A [`Port`] is a `#[keel::port]` trait. The macro generates a proxy type per trait that
//!   encodes its arguments and calls [`Runtime::port_call`](crate::Runtime::port_call)
//!   (async, returns a [`PortFuture`]) or [`port_call_sync`] (sync methods).
//! * A port id is either **foreign** (the default: calls go to
//!   [`Host::port_call`](crate::Host::port_call)) or bound to a **Rust** implementation with
//!   [`Runtime::bind_port`](crate::Runtime::bind_port) (fakes, built-ins), conventionally an
//!   `Arc<Arc<dyn Trait>>`. Typed code fetches the binding with
//!   [`Ctx::rust_port`](crate::Ctx::rust_port) and calls it directly; a raw port call
//!   (what a generated proxy makes) is routed to the implementation through the
//!   [`PortDispatcher`] that `#[keel::port]` registered, so the proxy works the same against a
//!   fake. A Rust-bound port without a dispatcher is [`PortError::Unavailable`].
//! * [`Events`] fans host-to-core events (`Connectivity`, `Lifecycle`) out to subscribers.
//!
//! # Abandoned calls
//!
//! Dropping a [`PortFuture`] before it completes (a cancelled call, a dropped task) marks its
//! `port_call_id` abandoned; a late reply from the host is recognised and discarded, and the id
//! is forgotten. The host is never told about the abandonment (v1), so a host that never answers
//! such a call would leave its id in the set for good: the set is **capped at
//! [`MAX_ABANDONED`] ids** and the oldest is forgotten first (FIFO), with a WARN log. A late
//! reply for a forgotten id is then logged as unknown instead of discarded quietly.

use core::any::Any;
use core::fmt;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use keel_wire::payload::{PortReply, PortStatus};
use keel_wire::{Reader, WireError};
use parking_lot::{Mutex, RwLock};

use crate::log::WARN;
use crate::runtime::Runtime;

/// The most abandoned port call ids kept waiting for a late host reply (see the module
/// documentation). Far above the number of calls a healthy host leaves unanswered.
pub const MAX_ABANDONED: usize = 4096;

/// A `#[keel::port]` trait.
pub trait Port: Send + Sync + 'static {
    /// `fnv1a32("port.<TraitName>")`.
    const PORT_ID: u32;
    /// The trait name.
    const NAME: &'static str;
    /// Sync, Async or Event.
    const KIND: keel_meta::PortKind;
}

/// Why a port call did not produce a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortError {
    /// The platform does not implement the port, has not registered it, or a synchronous
    /// call could not be answered synchronously.
    Unavailable,
    /// The call was abandoned before it completed.
    Cancelled,
    /// The platform's reply was malformed.
    Decode(WireError),
    /// The port method returned an error; the bytes are the encoded `E`.
    Failed(Vec<u8>),
}

impl fmt::Display for PortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PortError::Unavailable => f.write_str("port unavailable"),
            PortError::Cancelled => f.write_str("port call cancelled"),
            PortError::Decode(e) => write!(f, "malformed port reply: {e}"),
            PortError::Failed(body) => write!(f, "port call failed ({} byte error)", body.len()),
        }
    }
}

impl std::error::Error for PortError {}

impl From<WireError> for PortError {
    fn from(e: WireError) -> Self {
        PortError::Decode(e)
    }
}

/// Interprets a `PortReply` payload as the answer to call `expected_id`.
pub(crate) fn decode_port_reply(expected_id: u32, payload: &[u8]) -> Result<Vec<u8>, PortError> {
    let mut r = Reader::new(payload);
    let reply = PortReply::decode(&mut r)?;
    if reply.port_call_id != expected_id {
        return Err(PortError::Decode(WireError::InvalidTag {
            tag: reply.port_call_id,
            at: 0,
            ty: "PortReply.port_call_id",
        }));
    }
    match reply.status {
        PortStatus::Ok => Ok(reply.body.to_vec()),
        PortStatus::Error => Err(PortError::Failed(reply.body.to_vec())),
        PortStatus::Unavailable => Err(PortError::Unavailable),
    }
}

// ----- port table ------------------------------------------------------------------------

/// How a port id is bound.
#[derive(Clone)]
pub(crate) enum PortBinding {
    /// Implemented by the platform: calls go through [`Host::port_call`](crate::Host).
    Foreign,
    /// Implemented by a Rust value (an `Arc<T>` behind `dyn Any`).
    Rust(Arc<dyn Any + Send + Sync>),
}

struct SlotInner {
    result: Option<Result<Vec<u8>, PortError>>,
    waker: Option<Waker>,
}

/// The rendezvous between a pending port call and its reply.
pub(crate) struct PortSlot {
    inner: Mutex<SlotInner>,
}

impl PortSlot {
    fn new() -> Arc<PortSlot> {
        Arc::new(PortSlot {
            inner: Mutex::new(SlotInner {
                result: None,
                waker: None,
            }),
        })
    }

    fn complete(&self, result: Result<Vec<u8>, PortError>) {
        let waker = {
            let mut inner = self.inner.lock();
            if inner.result.is_some() {
                return;
            }
            inner.result = Some(result);
            inner.waker.take()
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }

    fn take(&self) -> Option<Result<Vec<u8>, PortError>> {
        self.inner.lock().result.take()
    }
}

struct PendingCall {
    slot: Arc<PortSlot>,
    started_ns: u64,
}

#[derive(Default)]
struct Pending {
    calls: HashMap<u32, PendingCall>,
    abandoned: HashSet<u32>,
    /// `abandoned` in the order the ids were added, oldest first. May hold ids whose late reply
    /// already arrived (removed from the set); they are skipped when evicting and dropped when
    /// the queue outgrows twice the cap.
    abandoned_order: VecDeque<u32>,
    /// How many abandoned ids have ever been evicted, for rate-limiting the eviction warning
    /// (a host that never answers is exactly the case the cap is for; one WARN per eviction
    /// would flood the log — re-review NF2).
    evicted_total: u64,
    next_id: u32,
}

/// The eviction warning fires on the first eviction and then once per this many.
const EVICTION_LOG_EVERY: u64 = 1024;

/// What became of a reply handed to [`PortTable::complete`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Completion {
    /// A pending call received its result; the call was issued at `started_ns` on the
    /// runtime's clock.
    Delivered { started_ns: u64 },
    /// The call had been abandoned; the reply was dropped.
    Discarded,
    /// No such call.
    Unknown,
}

/// Port bindings plus the table of in-flight calls.
#[derive(Default)]
pub(crate) struct PortTable {
    bindings: RwLock<HashMap<u32, PortBinding>>,
    pending: Mutex<Pending>,
    /// The runtime the table belongs to, to log an eviction through.
    owner: Weak<Runtime>,
}

impl PortTable {
    /// A table whose warnings go through `owner`'s host.
    pub(crate) fn with_owner(owner: Weak<Runtime>) -> PortTable {
        PortTable {
            owner,
            ..PortTable::default()
        }
    }

    pub(crate) fn bind(&self, port_id: u32, binding: PortBinding) {
        self.bindings.write().insert(port_id, binding);
    }

    pub(crate) fn unbind(&self, port_id: u32) -> bool {
        self.bindings.write().remove(&port_id).is_some()
    }

    /// The binding of `port_id`; unbound ports are foreign.
    pub(crate) fn binding(&self, port_id: u32) -> PortBinding {
        self.bindings
            .read()
            .get(&port_id)
            .cloned()
            .unwrap_or(PortBinding::Foreign)
    }

    /// Registers a new in-flight call (issued at `started_ns` on the runtime's clock) and
    /// returns its id and slot.
    pub(crate) fn begin(&self, started_ns: u64) -> (u32, Arc<PortSlot>) {
        let mut pending = self.pending.lock();
        let id = loop {
            pending.next_id = pending.next_id.wrapping_add(1);
            let candidate = pending.next_id;
            if candidate != 0
                && !pending.calls.contains_key(&candidate)
                && !pending.abandoned.contains(&candidate)
            {
                break candidate;
            }
        };
        let slot = PortSlot::new();
        pending.calls.insert(
            id,
            PendingCall {
                slot: slot.clone(),
                started_ns,
            },
        );
        (id, slot)
    }

    /// Delivers a result to call `id`.
    pub(crate) fn complete(&self, id: u32, result: Result<Vec<u8>, PortError>) -> Completion {
        let call = {
            let mut pending = self.pending.lock();
            match pending.calls.remove(&id) {
                Some(call) => call,
                None => {
                    return if pending.abandoned.remove(&id) {
                        Completion::Discarded
                    } else {
                        Completion::Unknown
                    };
                }
            }
        };
        call.slot.complete(result);
        Completion::Delivered {
            started_ns: call.started_ns,
        }
    }

    /// Marks call `id` abandoned (its future was dropped) so a late reply is discarded. Keeps at
    /// most [`MAX_ABANDONED`] ids: the oldest are forgotten first, with a warning.
    pub(crate) fn abandon(&self, id: u32) {
        let evicted = {
            let mut guard = self.pending.lock();
            let pending = &mut *guard;
            if pending.calls.remove(&id).is_none() {
                return;
            }
            pending.abandoned.insert(id);
            pending.abandoned_order.push_back(id);
            let mut evicted = 0_u32;
            while pending.abandoned.len() > MAX_ABANDONED {
                let Some(oldest) = pending.abandoned_order.pop_front() else {
                    break;
                };
                if pending.abandoned.remove(&oldest) {
                    evicted += 1;
                }
            }
            if pending.abandoned_order.len() > 2 * MAX_ABANDONED {
                let live = &pending.abandoned;
                pending.abandoned_order.retain(|id| live.contains(id));
            }
            let before = pending.evicted_total;
            pending.evicted_total += u64::from(evicted);
            (evicted, before, pending.evicted_total)
        };
        let (evicted, before, total) = evicted;
        // First eviction, then once per EVICTION_LOG_EVERY, so an unresponsive host cannot
        // flood the log through the very mechanism that protects against it.
        if evicted > 0 && (before == 0 || before / EVICTION_LOG_EVERY != total / EVICTION_LOG_EVERY)
        {
            // Outside the lock: logging calls the host.
            if let Some(runtime) = self.owner.upgrade() {
                runtime.log(
                    WARN,
                    "keel::runtime",
                    &format!(
                        "port calls: {MAX_ABANDONED} abandoned calls are still waiting for a late \
                         host reply; forgot the {total} oldest so far (a late reply to one of \
                         them will be logged as unknown; warned once per {EVICTION_LOG_EVERY})"
                    ),
                );
            }
        }
    }

    pub(crate) fn pending_count(&self) -> usize {
        self.pending.lock().calls.len()
    }

    pub(crate) fn abandoned_count(&self) -> usize {
        self.pending.lock().abandoned.len()
    }

    /// Removes every binding (shutdown): they may hold a `Ctx`. Returned for the caller to drop
    /// outside the lock.
    pub(crate) fn clear_bindings(&self) -> Vec<PortBinding> {
        self.bindings
            .write()
            .drain()
            .map(|(_, binding)| binding)
            .collect()
    }

    /// Fails every pending call with `Cancelled` (shutdown).
    pub(crate) fn cancel_all(&self) {
        let slots: Vec<_> = {
            let mut pending = self.pending.lock();
            pending.calls.drain().map(|(_, call)| call.slot).collect()
        };
        for slot in slots {
            slot.complete(Err(PortError::Cancelled));
        }
    }
}

// ----- Rust-side dispatch ----------------------------------------------------------------

/// How a Rust implementation of a port answers an *encoded* call: a status byte followed by the
/// body, exactly like a `PortReply` without its `port_call_id`. `#[keel::port]` generates the
/// function that produces it.
///
/// `status` is `0` (the body is the encoded return value), `1` (the body is the encoded typed
/// error) or `2` (unavailable: unknown method or undecodable arguments).
pub enum PortDispatch {
    /// The implementation answered synchronously.
    Sync(Vec<u8>),
    /// The implementation answers later.
    Async(Pin<Box<dyn Future<Output = Vec<u8>> + Send>>),
}

impl fmt::Debug for PortDispatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PortDispatch::Sync(bytes) => f.debug_tuple("Sync").field(bytes).finish(),
            PortDispatch::Async(_) => f.write_str("Async(..)"),
        }
    }
}

/// The erased entry point of a Rust-bound port: `#[keel::port]` submits one per port trait.
///
/// When a raw [`Runtime::port_call`](crate::Runtime::port_call) targets a port bound to a Rust
/// implementation, the runtime finds the dispatcher by `port_id` and calls it with the bound
/// `Arc<Arc<dyn Trait>>` (as `dyn Any`), the `method_id` and the encoded arguments, so a
/// generated proxy behaves the same whether the implementation is the platform's or a fake.
pub struct PortDispatcher {
    /// The port id (`fnv1a32("port.<TraitName>")`).
    pub port_id: u32,
    /// Runs the call on the implementation (`imp` is the value passed to `bind_port`).
    pub dispatch: fn(imp: &(dyn Any + Send + Sync), method_id: u32, args: &[u8]) -> PortDispatch,
}

inventory::collect!(PortDispatcher);

/// Interprets a [`PortDispatch`] reply (`status u8` + body).
pub(crate) fn decode_dispatch_reply(bytes: &[u8]) -> Result<Vec<u8>, PortError> {
    match bytes.split_first() {
        Some((0, body)) => Ok(body.to_vec()),
        Some((1, body)) => Err(PortError::Failed(body.to_vec())),
        Some((2, _)) => Err(PortError::Unavailable),
        Some((&tag, _)) => Err(PortError::Decode(WireError::InvalidTag {
            tag: u32::from(tag),
            at: 0,
            ty: "PortDispatch.status",
        })),
        None => Err(PortError::Decode(WireError::UnexpectedEof {
            needed: 1,
            at: 0,
        })),
    }
}

// ----- PortFuture ------------------------------------------------------------------------

type BoxedPortFuture = Pin<Box<dyn Future<Output = Result<Vec<u8>, PortError>> + Send>>;

enum Kind {
    /// A call to the host: completed through the port table.
    Foreign {
        slot: Arc<PortSlot>,
        table: Arc<PortTable>,
        id: u32,
        done: bool,
    },
    /// A call to a Rust implementation that answers asynchronously.
    Rust(BoxedPortFuture),
}

/// The result of an asynchronous port call: resolves to the encoded return value
/// (`Result<T, E>` collapses into `Ok(T bytes)` / [`PortError::Failed`]).
///
/// Created by [`Runtime::port_call`](crate::Runtime::port_call); the call is sent to the
/// host (or run on the Rust implementation) when the future is created, not when it is first
/// polled. Dropping it before it completes abandons the call: a late host reply is discarded, an
/// asynchronous Rust implementation is dropped.
#[must_use = "a PortFuture does nothing unless awaited; dropping it abandons the call"]
pub struct PortFuture {
    kind: Kind,
}

impl PortFuture {
    pub(crate) fn new(slot: Arc<PortSlot>, table: Arc<PortTable>, id: u32) -> PortFuture {
        PortFuture {
            kind: Kind::Foreign {
                slot,
                table,
                id,
                done: false,
            },
        }
    }

    /// A future that is already complete.
    pub(crate) fn ready(table: Arc<PortTable>, result: Result<Vec<u8>, PortError>) -> PortFuture {
        let slot = PortSlot::new();
        slot.complete(result);
        PortFuture {
            kind: Kind::Foreign {
                slot,
                table,
                id: 0,
                done: false,
            },
        }
    }

    /// A call answered by a Rust implementation.
    pub(crate) fn rust(future: BoxedPortFuture) -> PortFuture {
        PortFuture {
            kind: Kind::Rust(future),
        }
    }

    /// The `port_call_id` this future waits on (`0` for an already-complete future or a call
    /// answered by a Rust implementation).
    pub fn port_call_id(&self) -> u32 {
        match &self.kind {
            Kind::Foreign { id, .. } => *id,
            Kind::Rust(_) => 0,
        }
    }

    /// Takes the result if it has arrived, without polling (used by `port_call_sync`).
    pub(crate) fn try_take(&mut self) -> Option<Result<Vec<u8>, PortError>> {
        match &mut self.kind {
            Kind::Foreign { slot, done, .. } => {
                let result = slot.take();
                if result.is_some() {
                    *done = true;
                }
                result
            }
            Kind::Rust(_) => None,
        }
    }
}

impl Future for PortFuture {
    type Output = Result<Vec<u8>, PortError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match &mut self.kind {
            Kind::Rust(future) => future.as_mut().poll(cx),
            Kind::Foreign { slot, done, .. } => {
                let mut inner = slot.inner.lock();
                if let Some(result) = inner.result.take() {
                    drop(inner);
                    *done = true;
                    return Poll::Ready(result);
                }
                match &inner.waker {
                    Some(w) if w.will_wake(cx.waker()) => {}
                    _ => inner.waker = Some(cx.waker().clone()),
                }
                Poll::Pending
            }
        }
    }
}

impl Drop for PortFuture {
    fn drop(&mut self) {
        if let Kind::Foreign {
            table, id, done, ..
        } = &self.kind
        {
            if !*done && *id != 0 {
                table.abandon(*id);
            }
        }
    }
}

impl fmt::Debug for PortFuture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PortFuture")
            .field("port_call_id", &self.port_call_id())
            .finish_non_exhaustive()
    }
}

/// Calls a synchronous port method and returns its encoded result.
///
/// The host must answer synchronously ([`PortCallOutcome::Sync`](crate::PortCallOutcome) or an
/// `Async` that has already been replied to by the time `Host::port_call` returns); any other
/// answer fails with [`PortError::Unavailable`] and the call is abandoned.
pub fn port_call_sync(
    rt: &Runtime,
    port_id: u32,
    method_id: u32,
    args: &[u8],
) -> Result<Vec<u8>, PortError> {
    rt.port_call_sync(port_id, method_id, args)
}

// ----- events ----------------------------------------------------------------------------

type Callback = dyn Fn(&[u8]) + Send + Sync;

/// A boxed event subscriber, as passed to [`Events::subscribe`].
pub type EventHandler = Box<dyn Fn(&[u8]) + Send + Sync>;

/// `(port_id, method_id)`.
type EventKey = (u32, u32);
/// `(subscription id, callback)`.
type Subscriber = (u64, Arc<Callback>);

#[derive(Default)]
struct EventsInner {
    subs: Mutex<HashMap<EventKey, Vec<Subscriber>>>,
    next: AtomicU64,
}

/// Host-to-core events (`Connectivity::changed`, `Lifecycle::changed`, ...): subscribers are
/// called on the core loop, with the core lock held, in subscription order.
///
/// A subscriber must not block, and must not call back into the runtime through the host
/// entry points (`call_sync` and friends); it may write signals and spawn tasks. A panicking
/// subscriber is logged and skipped; the others still run.
#[derive(Default)]
pub struct Events {
    inner: Arc<EventsInner>,
}

impl Events {
    /// Subscribes to `(port_id, method_id)`. The callback receives the event's encoded
    /// parameters. Dropping the returned [`Subscription`] unsubscribes.
    pub fn subscribe(&self, port_id: u32, method_id: u32, callback: EventHandler) -> Subscription {
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed);
        self.inner
            .subs
            .lock()
            .entry((port_id, method_id))
            .or_default()
            .push((id, Arc::from(callback)));
        Subscription {
            events: Arc::downgrade(&self.inner),
            key: (port_id, method_id),
            id,
        }
    }

    /// Number of subscribers to `(port_id, method_id)`.
    pub fn subscriber_count(&self, port_id: u32, method_id: u32) -> usize {
        self.inner
            .subs
            .lock()
            .get(&(port_id, method_id))
            .map_or(0, Vec::len)
    }

    /// Removes every subscriber (shutdown); a [`Subscription`] dropped later finds nothing to
    /// remove. The callbacks are returned for the caller to drop outside the lock: they may
    /// hold a `Ctx`, which would otherwise keep the runtime alive.
    pub(crate) fn clear(&self) -> Vec<Arc<Callback>> {
        let drained: Vec<_> = self.inner.subs.lock().drain().collect();
        drained
            .into_iter()
            .flat_map(|(_, subs)| subs.into_iter().map(|(_, callback)| callback))
            .collect()
    }

    /// The callbacks for an event, cloned out so none runs under the table's lock.
    pub(crate) fn callbacks(&self, port_id: u32, method_id: u32) -> Vec<Arc<Callback>> {
        self.inner
            .subs
            .lock()
            .get(&(port_id, method_id))
            .map(|subs| subs.iter().map(|(_, cb)| cb.clone()).collect())
            .unwrap_or_default()
    }
}

/// Keeps an [`Events::subscribe`] callback alive; dropping it unsubscribes.
#[must_use = "dropping a Subscription unsubscribes immediately"]
pub struct Subscription {
    events: std::sync::Weak<EventsInner>,
    key: EventKey,
    id: u64,
}

impl Subscription {
    /// Keeps the subscription for the life of the runtime instead of until dropped.
    pub fn detach(self) {
        std::mem::forget(self);
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let Some(inner) = self.events.upgrade() else {
            return;
        };
        let mut subs = inner.subs.lock();
        if let Some(list) = subs.get_mut(&self.key) {
            list.retain(|(id, _)| *id != self.id);
            if list.is_empty() {
                subs.remove(&self.key);
            }
        }
    }
}

impl fmt::Debug for Subscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Subscription")
            .field("port_id", &self.key.0)
            .field("method_id", &self.key.1)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_wire::Writer;

    fn reply(id: u32, status: PortStatus, body: &[u8]) -> Vec<u8> {
        let mut w = Writer::new();
        PortReply {
            port_call_id: id,
            status,
            body,
        }
        .encode(&mut w);
        w.into_vec()
    }

    #[test]
    fn port_reply_statuses_map_to_results() {
        assert_eq!(
            decode_port_reply(3, &reply(3, PortStatus::Ok, &[1, 2])),
            Ok(vec![1, 2])
        );
        assert_eq!(
            decode_port_reply(3, &reply(3, PortStatus::Error, &[9])),
            Err(PortError::Failed(vec![9]))
        );
        assert_eq!(
            decode_port_reply(3, &reply(3, PortStatus::Unavailable, &[])),
            Err(PortError::Unavailable)
        );
    }

    #[test]
    fn port_reply_with_the_wrong_id_or_garbage_is_a_decode_error() {
        assert!(matches!(
            decode_port_reply(3, &reply(4, PortStatus::Ok, &[])),
            Err(PortError::Decode(_))
        ));
        assert!(matches!(
            decode_port_reply(3, &[1, 2]),
            Err(PortError::Decode(_))
        ));
        let mut bad_status = reply(3, PortStatus::Ok, &[]);
        bad_status[4] = 9;
        assert!(matches!(
            decode_port_reply(3, &bad_status),
            Err(PortError::Decode(_))
        ));
    }

    #[test]
    fn begin_allocates_distinct_nonzero_ids_and_complete_delivers_once() {
        let table = PortTable::default();
        let (a, slot_a) = table.begin(0);
        let (b, _slot_b) = table.begin(0);
        assert_ne!(a, 0);
        assert_ne!(a, b);
        assert_eq!(table.pending_count(), 2);
        assert_eq!(
            table.complete(a, Ok(vec![1])),
            Completion::Delivered { started_ns: 0 }
        );
        assert_eq!(slot_a.take(), Some(Ok(vec![1])));
        assert_eq!(table.complete(a, Ok(vec![2])), Completion::Unknown);
        assert_eq!(table.pending_count(), 1);
    }

    #[test]
    fn the_abandoned_set_is_capped_and_forgets_the_oldest_first() {
        let table = PortTable::default();
        let mut ids = Vec::new();
        for _ in 0..MAX_ABANDONED + 25 {
            let (id, _slot) = table.begin(0);
            table.abandon(id);
            ids.push(id);
        }
        assert_eq!(table.pending_count(), 0);
        assert_eq!(table.abandoned_count(), MAX_ABANDONED);
        // The 25 oldest were forgotten: a late reply for one is unknown, for a newer one discarded.
        assert_eq!(table.complete(ids[0], Ok(vec![])), Completion::Unknown);
        assert_eq!(table.complete(ids[24], Ok(vec![])), Completion::Unknown);
        assert_eq!(table.complete(ids[25], Ok(vec![])), Completion::Discarded);
        assert_eq!(
            table.complete(*ids.last().unwrap(), Ok(vec![])),
            Completion::Discarded
        );
        assert_eq!(table.abandoned_count(), MAX_ABANDONED - 2);
    }

    #[test]
    fn answered_abandonments_do_not_grow_the_eviction_queue_without_bound() {
        let table = PortTable::default();
        for _ in 0..4 * MAX_ABANDONED {
            let (id, _slot) = table.begin(0);
            table.abandon(id);
            // The host replies promptly: the id leaves the set at once.
            assert_eq!(table.complete(id, Ok(vec![])), Completion::Discarded);
        }
        assert_eq!(table.abandoned_count(), 0);
        let queued = table.pending.lock().abandoned_order.len();
        assert!(queued <= 2 * MAX_ABANDONED + 1, "{queued} stale ids kept");
    }

    #[test]
    fn abandoned_ids_discard_late_replies_and_are_not_reused_while_abandoned() {
        let table = PortTable::default();
        let (id, _slot) = table.begin(0);
        table.abandon(id);
        assert_eq!(table.pending_count(), 0);
        assert_eq!(table.abandoned_count(), 1);
        // A fresh call never gets the abandoned id.
        let (other, _) = table.begin(0);
        assert_ne!(other, id);
        assert_eq!(table.complete(id, Ok(vec![])), Completion::Discarded);
        assert_eq!(table.abandoned_count(), 0);
        assert_eq!(table.complete(id, Ok(vec![])), Completion::Unknown);
    }

    #[test]
    fn id_counter_wraps_and_skips_zero_and_live_ids() {
        let table = PortTable::default();
        table.pending.lock().next_id = u32::MAX - 1;
        let (a, _sa) = table.begin(0); // u32::MAX
        let (b, _sb) = table.begin(0); // wraps past 0 to 1
        assert_eq!((a, b), (u32::MAX, 1));
        table.pending.lock().next_id = 0; // next candidate is 1, which is live
        let (c, _sc) = table.begin(0);
        assert_eq!(c, 2);
    }

    #[test]
    fn cancel_all_fails_pending_calls() {
        let table = PortTable::default();
        let (_, slot) = table.begin(0);
        table.cancel_all();
        assert_eq!(slot.take(), Some(Err(PortError::Cancelled)));
        assert_eq!(table.pending_count(), 0);
    }

    #[test]
    fn events_fan_out_in_order_and_unsubscribe_on_drop() {
        let events = Events::default();
        let log = Arc::new(Mutex::new(Vec::new()));
        let l1 = log.clone();
        let s1 = events.subscribe(1, 2, Box::new(move |p| l1.lock().push(("a", p.to_vec()))));
        let l2 = log.clone();
        let s2 = events.subscribe(1, 2, Box::new(move |p| l2.lock().push(("b", p.to_vec()))));
        assert_eq!(events.subscriber_count(1, 2), 2);
        assert_eq!(events.subscriber_count(1, 3), 0);
        for cb in events.callbacks(1, 2) {
            cb(&[7]);
        }
        assert_eq!(*log.lock(), [("a", vec![7]), ("b", vec![7])]);
        drop(s1);
        assert_eq!(events.subscriber_count(1, 2), 1);
        drop(s2);
        assert_eq!(events.subscriber_count(1, 2), 0);
        assert!(events.callbacks(1, 2).is_empty());
    }

    #[test]
    fn subscription_may_outlive_the_events_table() {
        let events = Events::default();
        let sub = events.subscribe(1, 1, Box::new(|_| {}));
        drop(events);
        drop(sub);
    }

    #[test]
    fn detached_subscription_stays_subscribed() {
        let events = Events::default();
        events.subscribe(5, 5, Box::new(|_| {})).detach();
        assert_eq!(events.subscriber_count(5, 5), 1);
    }
}
