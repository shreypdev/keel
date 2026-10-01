//! A host that does nothing but count, and the helpers that build a runtime around it.
//!
//! The runtime's own `RecordingHost` keeps every payload it is handed, which is what a test
//! wants and what a benchmark must not do (a few million change-sets would be gigabytes). The
//! [`CountingHost`] is what a platform runtime looks like from the core's side at its cheapest:
//! it is told about each reply and change-set and keeps two integers.
#![allow(missing_docs, dead_code)]

use std::collections::VecDeque;
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use undra::meta::ids;
use undra::runtime::testing::{call_payload, decode_reply};
use undra::runtime::{Host, PortCallOutcome};
use undra::runtime::{Runtime, RuntimeConfig};
use undra::wire::payload::{CallTarget, ChangeEntryRef, ChangeOp, ChangeSetRef, ReplyStatus};
use undra::wire::{Decode, Handle, KeyedPatch, Reader};
use undra_bench::stats::Histogram;

use super::fixtures::Item;

#[derive(Default)]
pub struct CountingHost {
    pub replies: AtomicU64,
    pub reply_bytes: AtomicU64,
    pub change_sets: AtomicU64,
    pub change_set_bytes: AtomicU64,
    /// Log records at warn or above: a benchmark that provokes one is measuring a failure.
    pub warnings: AtomicU64,
    /// The first of them, for the failure message.
    pub first_warning: Mutex<Option<String>>,
    /// Stream items (not the end or error markers) handed to the host.
    pub stream_items: AtomicU64,
    pub stream_item_bytes: AtomicU64,
}

impl CountingHost {
    pub fn change_sets(&self) -> u64 {
        self.change_sets.load(Ordering::Relaxed)
    }

    pub fn change_set_bytes(&self) -> u64 {
        self.change_set_bytes.load(Ordering::Relaxed)
    }

    pub fn replies(&self) -> u64 {
        self.replies.load(Ordering::Relaxed)
    }

    pub fn warnings(&self) -> u64 {
        self.warnings.load(Ordering::Relaxed)
    }

    /// The first warning or error logged, if any.
    pub fn first_warning(&self) -> Option<String> {
        self.first_warning
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn stream_items(&self) -> u64 {
        self.stream_items.load(Ordering::Relaxed)
    }
}

impl Host for CountingHost {
    fn reply(&self, _call_id: u32, payload: &[u8]) {
        self.replies.fetch_add(1, Ordering::Relaxed);
        self.reply_bytes
            .fetch_add(payload.len() as u64, Ordering::Relaxed);
    }

    fn change_set(&self, payload: &[u8]) {
        self.change_sets.fetch_add(1, Ordering::Relaxed);
        self.change_set_bytes
            .fetch_add(payload.len() as u64, Ordering::Relaxed);
    }

    fn stream_item(&self, _call_id: u32, payload: &[u8]) {
        // `call_id u32, flag u8, body`: flag 0 is an item, 1 and 2 are the end and error markers.
        if payload.get(4) == Some(&0) {
            self.stream_items.fetch_add(1, Ordering::Relaxed);
            self.stream_item_bytes
                .fetch_add(payload.len() as u64, Ordering::Relaxed);
        }
    }

    fn port_call(&self, _: u32, _: u32, _: u32, _: &[u8]) -> PortCallOutcome {
        PortCallOutcome::Unavailable
    }

    fn log(&self, level: u8, target: &str, message: &str) {
        // The query client warns once that the `Kv` port never became available: nothing here
        // binds a `Kv`, so it is the expected answer, not a failure.
        if target == "undra::query" && message.contains("Kv port never became available") {
            return;
        }
        if level >= 3 && self.warnings.fetch_add(1, Ordering::Relaxed) == 0 {
            *self.first_warning.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(format!("[{target}] {message}"));
        }
    }
}

/// A runtime that is shut down when it is dropped.
///
/// `Runtime::new` hands out an `Arc<Runtime>` whose last drop does **not** stop it: the
/// query client (an extension) holds a `Ctx`, a reference cycle, so a runtime that is merely
/// dropped keeps itself, and its `undra-core` thread, alive for the life of the process (see
/// `bench/RESULTS.md`, findings). `shutdown()` is what releases it, so the harness calls it, and
/// a benchmark that builds hundreds of runtimes does not accumulate threads.
pub struct Core(Arc<Runtime>);

impl Core {
    /// Builds a runtime for `config` around `host`.
    pub fn new(config: RuntimeConfig, host: Arc<CountingHost>) -> Core {
        Core::with_host(config, host)
    }

    /// Builds a runtime for `config` around any host.
    pub fn with_host(config: RuntimeConfig, host: Arc<dyn Host>) -> Core {
        Core(Runtime::new(config, host).expect("a runtime"))
    }
}

impl std::ops::Deref for Core {
    type Target = Runtime;

    fn deref(&self) -> &Runtime {
        &self.0
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

/// A runtime with no `undra-core` thread (the host drives the executor with `run_pending`, as the
/// wasm shell does), so the numbers do not include a thread hop; `bench/RESULTS.md` has the
/// `undra_call` numbers that do.
pub fn runtime() -> (Arc<Core>, Arc<CountingHost>) {
    let host = Arc::new(CountingHost::default());
    let config = RuntimeConfig {
        platform: "bench".to_owned(),
        core_threads: 0,
        ..RuntimeConfig::default()
    };
    (Arc::new(Core::new(config, host.clone())), host)
}

/// A runtime around `host` with `core_threads` `undra-core` threads (0: the host drives the
/// executor, 1: a real core thread, as a platform has).
pub fn runtime_with(host: Arc<dyn Host>, core_threads: u8) -> Arc<Core> {
    let config = RuntimeConfig {
        platform: "bench".to_owned(),
        core_threads,
        ..RuntimeConfig::default()
    };
    Arc::new(Core::with_host(config, host))
}

/// Calls `Type::new(args)` and returns the new object's handle.
pub fn construct(rt: &Runtime, type_name: &str, args: &[u8]) -> Handle {
    let payload = call_payload(
        CallTarget::Constructor {
            type_id: ids::type_id(type_name),
            method_id: ids::method_id(type_name, "new"),
        },
        1,
        args,
    );
    let reply = decode_reply(&rt.call_sync(&payload));
    assert_eq!(
        reply.status,
        ReplyStatus::Ok,
        "{type_name}::new answered {reply:?}"
    );
    Handle::decode_exact(&reply.body).expect("a constructor answers a handle")
}

/// The prebuilt `Call` payload for a method call on `handle`.
pub fn method_call(
    handle: Handle,
    type_name: &str,
    method: &str,
    call_id: u32,
    args: &[u8],
) -> Vec<u8> {
    call_payload(
        CallTarget::Method {
            handle,
            method_id: ids::method_id(type_name, method),
        },
        call_id,
        args,
    )
}

/// Runs a prebuilt sync call and panics unless it succeeded.
pub fn call_ok(rt: &Runtime, payload: &[u8]) -> Vec<u8> {
    let reply = rt.call_sync(payload);
    let status = decode_reply(&reply).status;
    assert_eq!(status, ReplyStatus::Ok, "call failed: {status:?}");
    reply
}

// ---------------------------------------------------------------------------------------------
// Hosts for the sustained scenarios
// ---------------------------------------------------------------------------------------------

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// A [`CountingHost`] that also copies every change-set into one reused buffer: the copy each
/// platform's FFI callback makes before it hands the bytes to its own thread
/// (`InprocTransport.swift`, `InprocTransport.kt`, `wasm-main.ts`), without growing memory.
#[derive(Default)]
pub struct CopyingHost {
    pub counts: CountingHost,
    copy: Mutex<Vec<u8>>,
}

impl Host for CopyingHost {
    fn reply(&self, call_id: u32, payload: &[u8]) {
        self.counts.reply(call_id, payload);
    }

    fn change_set(&self, payload: &[u8]) {
        self.counts.change_set(payload);
        let mut copy = locked(&self.copy);
        copy.clear();
        copy.extend_from_slice(payload);
        black_box(copy.as_slice());
    }

    fn stream_item(&self, call_id: u32, payload: &[u8]) {
        self.counts.stream_item(call_id, payload);
    }

    fn port_call(&self, port: u32, method: u32, id: u32, args: &[u8]) -> PortCallOutcome {
        self.counts.port_call(port, method, id, args)
    }

    fn log(&self, level: u8, target: &str, message: &str) {
        self.counts.log(level, target, message);
    }
}

/// The host-side mirror of one keyed list: what a platform's list state does with the entries of
/// a change-set for its store (a full value replaces the list, a keyed patch is replayed on it).
/// The Rust stand-in for a SwiftUI, Compose or React list model.
pub struct ListMirror {
    handle: u64,
    signal_id: u32,
    pub list: Vec<Item>,
    /// Keyed patches applied.
    pub patches: u64,
    /// Full values applied.
    pub fulls: u64,
    /// Entries that did not decode or apply: a mirror that has desynchronised.
    pub errors: u64,
    /// Fault injection: every n-th keyed patch is dropped (0 never), so a scenario's equality
    /// invariant can be shown to fail.
    skip_every: u64,
    patches_seen: u64,
}

impl ListMirror {
    /// A mirror of signal `signal_id` of the store `handle`, empty until the first full value.
    pub fn new(handle: Handle, signal_id: u32) -> ListMirror {
        ListMirror {
            handle: handle.0,
            signal_id,
            list: Vec::new(),
            patches: 0,
            fulls: 0,
            errors: 0,
            skip_every: 0,
            patches_seen: 0,
        }
    }

    /// Drops every `n`-th keyed patch (fault injection; `0` turns it off).
    pub fn skip_every(&mut self, n: u64) {
        self.skip_every = n;
    }

    /// Applies one entry if it is for this list.
    pub fn apply_entry(&mut self, entry: &ChangeEntryRef<'_>) {
        if entry.handle.0 != self.handle || entry.signal_id != self.signal_id {
            return;
        }
        match entry.op {
            ChangeOp::Full => match Vec::<Item>::decode_exact(entry.value) {
                Ok(list) => {
                    self.list = list;
                    self.fulls += 1;
                }
                Err(_) => self.errors += 1,
            },
            ChangeOp::KeyedPatch => {
                self.patches_seen += 1;
                if self.patches_seen.checked_rem(self.skip_every) == Some(0) {
                    return;
                }
                let patch = KeyedPatch::<Item>::decode(&mut Reader::new(entry.value));
                match patch.map(|patch| patch.apply(&mut self.list)) {
                    Ok(Ok(())) => self.patches += 1,
                    _ => self.errors += 1,
                }
            }
            ChangeOp::LazyInvalidated => {}
        }
    }

    /// Validates a change-set and applies its entries for this list.
    pub fn apply(&mut self, change_set: &[u8]) {
        match ChangeSetRef::decode(&mut Reader::new(change_set)) {
            Ok(set) => {
                for entry in &set {
                    self.apply_entry(&entry);
                }
            }
            Err(_) => self.errors += 1,
        }
    }

    /// The ids of the mirrored rows, in order.
    pub fn ids(&self) -> Vec<u64> {
        self.list.iter().map(|row| row.id).collect()
    }
}

/// A [`CountingHost`] that applies every change-set to a [`ListMirror`] inside the callback:
/// the commit, the delivery and the list apply of one operation, on the committing thread.
#[derive(Default)]
pub struct ApplyingHost {
    pub counts: CountingHost,
    mirror: Mutex<Option<ListMirror>>,
}

impl ApplyingHost {
    /// Starts mirroring; change-sets delivered from now on are applied.
    pub fn watch(&self, mirror: ListMirror) {
        *locked(&self.mirror) = Some(mirror);
    }

    /// Reads the mirror (panics if nothing is watched).
    pub fn with_mirror<R>(&self, f: impl FnOnce(&ListMirror) -> R) -> R {
        f(locked(&self.mirror).as_ref().expect("a watched list"))
    }
}

impl Host for ApplyingHost {
    fn reply(&self, call_id: u32, payload: &[u8]) {
        self.counts.reply(call_id, payload);
    }

    fn change_set(&self, payload: &[u8]) {
        self.counts.change_set(payload);
        if let Some(mirror) = locked(&self.mirror).as_mut() {
            mirror.apply(payload);
        }
    }

    fn stream_item(&self, call_id: u32, payload: &[u8]) {
        self.counts.stream_item(call_id, payload);
    }

    fn port_call(&self, port: u32, method: u32, id: u32, args: &[u8]) -> PortCallOutcome {
        self.counts.port_call(port, method, id, args)
    }

    fn log(&self, level: u8, target: &str, message: &str) {
        self.counts.log(level, target, message);
    }
}

/// How often the platform's "main thread" drains what the core delivered: one frame at 60 Hz.
pub const FRAME: Duration = Duration::from_nanos(16_666_667);

/// Bytes of backlog [`Frame`] makes room for before the run starts: 32,768 change-sets of the
/// soak's size (about ten frames of its load).
///
/// The platform's queue is unbounded (ADR-031), so a busy moment deepens it, and the resident
/// set of this process would grow by the size of that backlog once, at its deepest. That is the
/// harness's own buffer and not a leak in the core, so the buffer's pages are touched up front:
/// a scheduling hiccup shows as latency, and RSS growth after the warm-up means something else.
const BACKLOG_BYTES: usize = 32_768 * 56;

/// The change-sets one drain found: each copied behind a four-byte length into one buffer, the
/// copy a platform's FFI callback makes into its own queue. One buffer rather than an
/// allocation per change-set keeps the harness's own allocator traffic (a block allocated on the
/// core thread and freed on the main thread, 170,000 times a second) out of the RSS the soak
/// watches.
#[derive(Default)]
pub struct Frame {
    bytes: Vec<u8>,
    count: usize,
}

impl Frame {
    /// An empty frame whose room for a backlog is already resident.
    pub fn with_room() -> Frame {
        // Not zeroes: the allocator would hand out untouched zero pages, which are not resident.
        let mut bytes = vec![0xA5_u8; BACKLOG_BYTES];
        bytes.clear();
        Frame { bytes, count: 0 }
    }

    /// How many change-sets the frame holds.
    pub fn len(&self) -> usize {
        self.count
    }

    /// Whether the frame holds none.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Empties the frame, keeping its room.
    pub fn clear(&mut self) {
        self.bytes.clear();
        self.count = 0;
    }

    fn push(&mut self, change_set: &[u8]) {
        self.bytes
            .extend_from_slice(&(change_set.len() as u32).to_le_bytes());
        self.bytes.extend_from_slice(change_set);
        self.count += 1;
    }

    /// The change-sets, oldest first.
    pub fn iter(&self) -> FrameIter<'_> {
        FrameIter { rest: &self.bytes }
    }
}

/// The change-sets of a [`Frame`], oldest first.
pub struct FrameIter<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for FrameIter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        let (len, rest) = self.rest.split_first_chunk::<4>()?;
        let len = u32::from_le_bytes(*len) as usize;
        let (change_set, rest) = rest.split_at_checked(len)?;
        self.rest = rest;
        Some(change_set)
    }
}

/// A host that looks like a platform runtime from the core's side: change-sets are copied into
/// a queue (the copy every FFI callback makes) for a "main thread" to drain once a frame, port
/// calls are answered `Async` and handed to completer threads, and replies are matched to the
/// time their call was issued.
///
/// A host callback must not call back into the runtime (E_REENTRANT), so this host only copies
/// and queues; the completer threads call `Runtime::port_reply` from their own threads.
pub struct DrainHost {
    pub counts: CountingHost,
    /// The one foreign port this host answers later; every other port (the query client's
    /// `Kv`, say) is answered "unavailable" at once, as a platform without it would.
    answered_port: u32,
    queue: Mutex<Frame>,
    ports: Mutex<VecDeque<u32>>,
    port_ready: Condvar,
    ports_closed: AtomicBool,
    port_calls: AtomicU64,
    epoch: Instant,
    issued_ns: Box<[AtomicU64]>,
    latency: Mutex<Histogram>,
    reply_errors: AtomicU64,
}

impl DrainHost {
    /// A host that can stamp up to `slots` call ids at once (call ids are reused modulo
    /// `slots`, so it must exceed the number of calls in flight) and that hands the calls of
    /// `answered_port` to its completer threads.
    pub fn new(slots: usize, answered_port: u32) -> DrainHost {
        DrainHost {
            counts: CountingHost::default(),
            answered_port,
            queue: Mutex::new(Frame::with_room()),
            ports: Mutex::new(VecDeque::new()),
            port_ready: Condvar::new(),
            ports_closed: AtomicBool::new(false),
            port_calls: AtomicU64::new(0),
            epoch: Instant::now(),
            issued_ns: (0..slots.max(1)).map(|_| AtomicU64::new(0)).collect(),
            latency: Mutex::new(Histogram::new()),
            reply_errors: AtomicU64::new(0),
        }
    }

    fn now_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64 + 1
    }

    /// Records that `call_id` is being issued now; its reply will add call to reply latency to
    /// [`latency`](DrainHost::latency).
    pub fn stamp(&self, call_id: u32) {
        let slot = call_id as usize % self.issued_ns.len();
        self.issued_ns[slot].store(self.now_ns(), Ordering::Relaxed);
    }

    /// Call to reply latencies recorded so far.
    pub fn latency(&self) -> Histogram {
        locked(&self.latency).clone()
    }

    /// Takes the latencies recorded so far and starts a fresh histogram.
    pub fn take_latency(&self) -> Histogram {
        std::mem::take(&mut *locked(&self.latency))
    }

    /// Replies whose status was neither `Ok` nor `StreamOpened`.
    pub fn reply_errors(&self) -> u64 {
        self.reply_errors.load(Ordering::Relaxed)
    }

    /// Port calls the core has made.
    pub fn port_calls(&self) -> u64 {
        self.port_calls.load(Ordering::Relaxed)
    }

    /// Swaps the queue of copied change-sets with `out`, which must be empty: the caller keeps
    /// the change-sets and the queue inherits `out`'s room, so a steady drain allocates nothing.
    pub fn take_change_sets(&self, out: &mut Frame) {
        debug_assert!(out.is_empty());
        std::mem::swap(&mut *locked(&self.queue), out);
    }

    /// The next port call id a completer should answer; blocks until one arrives, and returns
    /// `None` once [`close_ports`](DrainHost::close_ports) was called and the queue is empty.
    pub fn next_port_call(&self) -> Option<u32> {
        let mut ports = locked(&self.ports);
        loop {
            if let Some(id) = ports.pop_front() {
                return Some(id);
            }
            if self.ports_closed.load(Ordering::Acquire) {
                return None;
            }
            ports = self
                .port_ready
                .wait(ports)
                .unwrap_or_else(|e| e.into_inner());
        }
    }

    /// Wakes every completer waiting in [`next_port_call`](DrainHost::next_port_call) so it can
    /// finish what is queued and stop.
    pub fn close_ports(&self) {
        self.ports_closed.store(true, Ordering::Release);
        let _guard = locked(&self.ports);
        self.port_ready.notify_all();
    }
}

impl Host for DrainHost {
    fn reply(&self, call_id: u32, payload: &[u8]) {
        self.counts.reply(call_id, payload);
        // `call_id u32, status u8, body`: 0 is Ok and 4 is StreamOpened.
        if !matches!(payload.get(4), Some(0 | 4)) {
            self.reply_errors.fetch_add(1, Ordering::Relaxed);
        }
        let slot = call_id as usize % self.issued_ns.len();
        let issued = self.issued_ns[slot].swap(0, Ordering::Relaxed);
        if issued != 0 {
            locked(&self.latency).record(self.now_ns().saturating_sub(issued));
        }
    }

    fn change_set(&self, payload: &[u8]) {
        self.counts.change_set(payload);
        locked(&self.queue).push(payload);
    }

    fn stream_item(&self, call_id: u32, payload: &[u8]) {
        self.counts.stream_item(call_id, payload);
    }

    fn port_call(&self, port: u32, _method: u32, id: u32, _args: &[u8]) -> PortCallOutcome {
        if port != self.answered_port {
            return PortCallOutcome::Unavailable;
        }
        self.port_calls.fetch_add(1, Ordering::Relaxed);
        locked(&self.ports).push_back(id);
        self.port_ready.notify_one();
        PortCallOutcome::Async
    }

    fn log(&self, level: u8, target: &str, message: &str) {
        self.counts.log(level, target, message);
    }
}

/// What the platform's "main thread" found while draining: counters another thread can read
/// while the drain runs.
#[derive(Default)]
pub struct MainStats {
    /// Change-sets walked.
    pub change_sets: AtomicU64,
    /// Entries walked.
    pub entries: AtomicU64,
    /// Change-sets whose `txn_id` did not increase for a store they touch: a delivery that
    /// overtook another.
    pub out_of_order: AtomicU64,
    /// Change-sets that did not validate.
    pub malformed: AtomicU64,
    /// Drains that found something.
    pub frames: AtomicU64,
    /// The most change-sets one drain found: how deep the queue got.
    pub largest_batch: AtomicUsize,
}

/// Checks that, for every store, the `txn_id`s of the change-sets arrive in increasing order.
#[derive(Default)]
pub struct OrderChecker {
    last: Vec<(u64, u64)>,
}

impl OrderChecker {
    /// Feeds one change-set; returns `false` if any store it touches saw a `txn_id` that did not
    /// exceed the previous one (or if the payload is malformed).
    pub fn check(&mut self, change_set: &[u8]) -> bool {
        let Ok(set) = ChangeSetRef::decode(&mut Reader::new(change_set)) else {
            return false;
        };
        let mut ordered = true;
        let mut previous_handle = u64::MAX;
        for entry in &set {
            // Consecutive entries of one store are checked once.
            if entry.handle.0 == previous_handle {
                continue;
            }
            previous_handle = entry.handle.0;
            match self
                .last
                .iter_mut()
                .find(|(handle, _)| *handle == entry.handle.0)
            {
                Some((_, last)) => {
                    if set.txn_id <= *last {
                        ordered = false;
                    }
                    *last = set.txn_id;
                }
                None => self.last.push((entry.handle.0, set.txn_id)),
            }
        }
        ordered
    }
}

/// The platform's main thread, as a model: it takes what the host queued once a frame, walks
/// every change-set, checks the per-store order, applies a list mirror, and remembers the last
/// `u64` value it saw for the stores it was asked to track.
pub struct MainThread {
    pub stats: Arc<MainStats>,
    order: OrderChecker,
    mirror: Option<ListMirror>,
    tracked: Vec<(u64, Option<u64>)>,
    /// Fault injection: swap the first two change-sets of the first frame that has two, so the
    /// order check can be shown to fail.
    swap_first_pair: bool,
}

/// What a [`MainThread`] ends with.
pub struct MainOutcome {
    pub mirror: Option<ListMirror>,
    /// The last full `u64` value seen for each tracked handle.
    pub last_values: Vec<(u64, Option<u64>)>,
}

impl MainThread {
    pub fn new(stats: Arc<MainStats>) -> MainThread {
        MainThread {
            stats,
            order: OrderChecker::default(),
            mirror: None,
            tracked: Vec::new(),
            swap_first_pair: false,
        }
    }

    /// Mirrors one keyed list.
    pub fn mirror(mut self, mirror: ListMirror) -> MainThread {
        self.mirror = Some(mirror);
        self
    }

    /// Remembers the last `u64` value written to any signal of `handle`.
    pub fn track(mut self, handle: Handle) -> MainThread {
        self.tracked.push((handle.0, None));
        self
    }

    /// Swaps the first two change-sets of the first frame that has two (fault injection).
    pub fn swap_first_pair(mut self) -> MainThread {
        self.swap_first_pair = true;
        self
    }

    /// One drain: walks `frame`.
    pub fn frame(&mut self, frame: &Frame) {
        if frame.is_empty() {
            return;
        }
        if self.swap_first_pair && frame.len() >= 2 {
            // Fault injection: the first two change-sets trade places.
            let mut all: Vec<&[u8]> = frame.iter().collect();
            all.swap(0, 1);
            self.swap_first_pair = false;
            self.walk(all.into_iter(), frame.len());
        } else {
            self.walk(frame.iter(), frame.len());
        }
    }

    fn walk<'a>(&mut self, change_sets: impl Iterator<Item = &'a [u8]>, count: usize) {
        let stats = &self.stats;
        stats.frames.fetch_add(1, Ordering::Relaxed);
        stats.largest_batch.fetch_max(count, Ordering::Relaxed);
        for change_set in change_sets {
            if !self.order.check(change_set) {
                stats.out_of_order.fetch_add(1, Ordering::Relaxed);
            }
            let Ok(set) = ChangeSetRef::decode(&mut Reader::new(change_set)) else {
                stats.malformed.fetch_add(1, Ordering::Relaxed);
                continue;
            };
            stats.change_sets.fetch_add(1, Ordering::Relaxed);
            stats.entries.fetch_add(set.len() as u64, Ordering::Relaxed);
            for entry in &set {
                if let Some(mirror) = self.mirror.as_mut() {
                    mirror.apply_entry(&entry);
                }
                // The last `u64` a tracked store was given in full.
                let value = <[u8; 8]>::try_from(entry.value)
                    .ok()
                    .filter(|_| entry.op == ChangeOp::Full)
                    .map(u64::from_le_bytes);
                let slot = value.and_then(|_| {
                    self.tracked
                        .iter_mut()
                        .find(|(handle, _)| *handle == entry.handle.0)
                });
                if let (Some(value), Some(slot)) = (value, slot) {
                    slot.1 = Some(value);
                }
            }
        }
    }

    /// Runs the drain loop on its own thread: a frame every [`FRAME`] until `stop` is set, then
    /// one last drain of whatever is left.
    pub fn spawn(mut self, host: Arc<DrainHost>, stop: Arc<AtomicBool>) -> JoinHandle<MainOutcome> {
        std::thread::Builder::new()
            .name("bench-main".to_owned())
            .spawn(move || {
                let mut batch = Frame::with_room();
                let mut next = Instant::now() + FRAME;
                loop {
                    let stopping = stop.load(Ordering::Acquire);
                    host.take_change_sets(&mut batch);
                    self.frame(&batch);
                    batch.clear();
                    if stopping {
                        break;
                    }
                    let now = Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    }
                    next += FRAME;
                    if next < Instant::now() {
                        // Fell behind (a stopped process, a busy machine): do not burst.
                        next = Instant::now() + FRAME;
                    }
                }
                MainOutcome {
                    mirror: self.mirror,
                    last_values: self.tracked,
                }
            })
            .expect("the main-thread model starts")
    }
}
