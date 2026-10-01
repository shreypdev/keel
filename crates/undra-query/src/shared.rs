//! The query cache and its engine: entries, observers, fetches, staleness, retries,
//! invalidation, garbage collection, persistence and the refetch triggers (SPEC 9).
//!
//! # Structure
//!
//! One [`Shared`] lives on each runtime (`Runtime::extension`). It owns a single mutex around
//! the [`State`]: the cache (`HashMap<QueryKey, Entry>`), the persisted entries waiting for
//! their query to be observed, the compiled key templates and the offline queue.
//!
//! # Rules the code keeps
//!
//! * **The state lock is a leaf.** Nothing the lock guards calls out: no signal write, no host
//!   callback, no port call, no task cancellation (which drops a future, whose `Drop` may take
//!   the lock again). A critical section instead collects what must happen afterwards in an
//!   [`Fx`] and runs it once the lock is released.
//! * **Every change to what a handle shows is published.** Publishing bumps the entry's `seq`
//!   in the same critical section as the change, so a handle can tell a newer view from an
//!   older one and never goes backwards.
//! * **One publish is one transaction.** [`Fx::run`] applies every publication inside a single
//!   `ctx.txn`, so however many entries and handles changed, each store gets one change-set.
//! * **No state is left half-changed by a panic or a cancellation.** A fetch task carries a
//!   guard that clears its in-flight marker if the future is dropped before it finished.

use core::task::Waker;
use core::time::Duration;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use parking_lot::Mutex;
use undra_meta::TypeClosure;
use undra_meta::ids::fnv1a64;
use undra_runtime::executor::TaskId;
use undra_runtime::log::ERROR;
use undra_runtime::{Ctx, Runtime, WeakCtx};

use crate::erased::{Erased, Failure, Outcome, QueryVTable};
use crate::key::{Invalidate, QueryKey};
use crate::queue::QueueState;
use crate::retry::{now_ms, with_retries};
use crate::status::QueryStatus;
use crate::storage::{Counters, Current, Persisted, StorageState};
use crate::walk::KeyPlan;

/// How long an unobserved entry stays cached: 5 minutes (SPEC 9).
pub const DEFAULT_GC_MS: u64 = 300_000;

/// Persisted entries are written this long after the last successful fetch (SPEC 9).
pub const PERSIST_DEBOUNCE_MS: u64 = 250;

// -------------------------------------------------------------------------------------------
// Entries
// -------------------------------------------------------------------------------------------

/// What a handle shows at one moment. Cheap to clone: everything is behind an `Arc`.
#[derive(Clone)]
pub(crate) struct View {
    /// Increases with every publication of the entry.
    pub(crate) seq: u64,
    pub(crate) data: Option<Erased>,
    /// Changes whenever `data` does.
    pub(crate) data_ver: u64,
    pub(crate) error: Option<Erased>,
    /// Changes whenever `error` does.
    pub(crate) error_ver: u64,
    pub(crate) status: QueryStatus,
    pub(crate) fetching: bool,
    pub(crate) updated_at: Option<i64>,
}

/// Something that shows an entry: a query handle.
pub(crate) trait Sink: Send + Sync {
    /// Shows `view` (writes the signals; the caller has a transaction open).
    fn apply(&self, view: &View);
}

pub(crate) struct Inflight {
    /// Identifies the fetch, so a result that arrives after the fetch was replaced is dropped.
    pub(crate) serial: u64,
    pub(crate) task: TaskId,
}

/// The compiled key template of query or mutation `id`, made on first use.
fn plan_for(
    plans: &mut HashMap<u32, Arc<KeyPlan>>,
    schema: &undra_meta::Schema,
    id: u32,
    template: &str,
) -> Arc<KeyPlan> {
    plans
        .entry(id)
        .or_insert_with(|| {
            let meta = schema.queries.iter().find(|q| q.query_id == id);
            Arc::new(KeyPlan::new(template, meta.map(|m| m.params.as_slice())))
        })
        .clone()
}

/// One cached query result (SPEC 9's `Entry`).
pub(crate) struct Entry {
    pub(crate) vt: &'static QueryVTable,
    /// The cache key template with the parameters spliced in, for prefix invalidation.
    pub(crate) rendered: String,
    pub(crate) data: Option<Erased>,
    pub(crate) error: Option<Erased>,
    pub(crate) data_ver: u64,
    pub(crate) error_ver: u64,
    /// When `data` was last confirmed, in wall-clock milliseconds.
    pub(crate) updated_at: Option<i64>,
    /// Marked stale by an invalidation; cleared by the next successful fetch.
    pub(crate) invalidated: bool,
    /// The last fetch died without a typed error (it panicked).
    pub(crate) failed: bool,
    pub(crate) observers: u32,
    pub(crate) inflight: Option<Inflight>,
    /// The garbage-collection timer, present while nobody observes the entry.
    pub(crate) gc: Option<TaskId>,
    pub(crate) persist_dirty: bool,
    pub(crate) persist_task: Option<TaskId>,
    pub(crate) sinks: Vec<(u64, Weak<dyn Sink>)>,
    /// Tasks waiting for the fetch to finish.
    pub(crate) settle: Vec<Waker>,
    pub(crate) seq: u64,
    /// The write stamp: identifies the last write of the entry's data, error and freshness. Every
    /// such write (a fetch result, `set`, an optimistic write) takes a new, never-used stamp from
    /// the client, so an entry that shows the same stamp twice has not been written in between.
    /// A rollback puts the stamp back together with the state it restores. `0` until the entry is
    /// first written.
    pub(crate) stamp: u64,
    /// The optimistic mutations that wrote this entry and have not settled yet, oldest first.
    pub(crate) layers: Vec<Layer>,
}

impl Entry {
    pub(crate) fn new(vt: &'static QueryVTable, rendered: String) -> Entry {
        Entry {
            vt,
            rendered,
            data: None,
            error: None,
            data_ver: 0,
            error_ver: 0,
            updated_at: None,
            invalidated: false,
            failed: false,
            observers: 0,
            inflight: None,
            gc: None,
            persist_dirty: false,
            persist_task: None,
            sinks: Vec::new(),
            settle: Vec::new(),
            seq: 0,
            stamp: 0,
            layers: Vec::new(),
        }
    }

    /// The status a handle shows, derived from what the entry holds (see [`QueryStatus`]).
    pub(crate) fn status(&self) -> QueryStatus {
        if self.inflight.is_some() && self.data.is_none() {
            QueryStatus::Fetching
        } else if self.error.is_some() || self.failed {
            QueryStatus::Error
        } else if self.data.is_some() {
            QueryStatus::Success
        } else {
            QueryStatus::Idle
        }
    }

    pub(crate) fn view(&self) -> View {
        View {
            seq: self.seq,
            data: self.data.clone(),
            data_ver: self.data_ver,
            error: self.error.clone(),
            error_ver: self.error_ver,
            status: self.status(),
            fetching: self.inflight.is_some(),
            updated_at: self.updated_at,
        }
    }

    /// Whether the entry's data is out of date at `now`.
    pub(crate) fn is_stale(&self, now: i64) -> bool {
        if self.data.is_none() || self.invalidated || self.error.is_some() || self.failed {
            return true;
        }
        match (self.vt.stale_ms, self.updated_at) {
            (Some(window), Some(at)) => {
                let age = now.saturating_sub(at);
                // An age below zero means the wall clock went backwards since the data was
                // confirmed (or it was persisted by a device with a wrong clock): trust
                // nothing and fetch.
                age < 0 || age >= i64::try_from(window).unwrap_or(i64::MAX)
            }
            // No staleness window: the data is always stale.
            _ => true,
        }
    }

    /// Whether observing (or a trigger) should start a fetch now.
    pub(crate) fn needs_fetch(&self, now: i64) -> bool {
        self.inflight.is_none() && self.is_stale(now)
    }

    /// Starts showing a persisted entry (in the current form). `false` if its bytes no longer
    /// decode.
    fn seed(&mut self, persisted: &Persisted) -> bool {
        match (self.vt.decode_data)(&persisted.data) {
            Ok(data) => {
                self.data = Some(data);
                self.data_ver += 1;
                self.updated_at = Some(persisted.updated_at);
                true
            }
            Err(_) => false,
        }
    }

    /// Registers a publication: bumps `seq` and, if anything shows this entry, returns who
    /// and what.
    pub(crate) fn publication(&mut self) -> Option<Publication> {
        self.seq += 1;
        self.sinks.retain(|(_, sink)| sink.strong_count() > 0);
        if self.sinks.is_empty() {
            return None;
        }
        let sinks: Vec<Arc<dyn Sink>> = self
            .sinks
            .iter()
            .filter_map(|(_, sink)| sink.upgrade())
            .collect();
        Some((sinks, self.view()))
    }
}

/// Who shows an entry, and what to show them.
pub(crate) type Publication = (Vec<Arc<dyn Sink>>, View);

/// The state of an entry before an optimistic write, kept so the write can be undone.
#[derive(Clone)]
pub(crate) struct EntrySnapshot {
    data: Option<Erased>,
    error: Option<Erased>,
    updated_at: Option<i64>,
    invalidated: bool,
    failed: bool,
    /// The write stamp the entry had.
    stamp: u64,
}

/// Whether two optional values have the same encoding.
fn same_bytes(a: &Option<Erased>, b: &Option<Erased>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => a.bytes == b.bytes,
        _ => false,
    }
}

impl EntrySnapshot {
    /// What an entry that did not exist looked like.
    pub(crate) const EMPTY: EntrySnapshot = EntrySnapshot {
        data: None,
        error: None,
        updated_at: None,
        invalidated: false,
        failed: false,
        stamp: 0,
    };

    pub(crate) fn of(entry: &Entry) -> EntrySnapshot {
        EntrySnapshot {
            data: entry.data.clone(),
            error: entry.error.clone(),
            updated_at: entry.updated_at,
            invalidated: entry.invalidated,
            failed: entry.failed,
            stamp: entry.stamp,
        }
    }
}

/// What one optimistic mutation did to one entry, and what undoes it.
///
/// A mutation's layer exists from its first write of the entry until it settles (succeeds, or
/// rolls back). Layers are what make a rollback the inverse of the mutation's *own* changes: the
/// `stamp` says whether the entry is still exactly as the mutation left it, and `before` is where
/// to go back to if it is.
pub(crate) struct Layer {
    /// The mutation (`UndoLog::owner`).
    owner: u64,
    /// The entry as it was before the owner's first write.
    before: EntrySnapshot,
    /// The write stamp of the owner's latest write.
    stamp: u64,
}

/// What a rollback did to one entry.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Unwound {
    /// The mutation has no layer here (the entry was collected and made again, say).
    NoLayer,
    /// The entry was exactly as the mutation left it and is now as it was before.
    Restored,
    /// Something wrote the entry after the mutation, so it is left as it is.
    Kept,
}

impl Entry {
    /// Records that mutation `owner` wrote this entry, which was `before` then, and that the
    /// write left it with write stamp `stamp`.
    pub(crate) fn record_optimistic_write(
        &mut self,
        owner: u64,
        before: EntrySnapshot,
        stamp: u64,
    ) {
        self.stamp = stamp;
        match self.layers.iter_mut().find(|l| l.owner == owner) {
            // A second write by the same mutation: the restore point stays the first one.
            Some(layer) => layer.stamp = stamp,
            None => self.layers.push(Layer {
                owner,
                before,
                stamp,
            }),
        }
    }

    /// The mutation `owner` succeeded: its writes are final and there is nothing to undo.
    pub(crate) fn commit(&mut self, owner: u64) {
        self.layers.retain(|l| l.owner != owner);
    }

    /// Undoes mutation `owner`'s writes of this entry, if nothing has written it since.
    ///
    /// The entry is compared by write stamp, not by content: a later optimistic write, a fetch
    /// result or a `set` each leave a different stamp even when they wrote the same bytes, and
    /// each is newer than the owner's write, so none of them is overwritten. An entry that is
    /// left as it is may still show the owner's change inside the later write's value; when the
    /// later write is another optimistic mutation's and was made directly on top of the owner's,
    /// that mutation inherits the owner's restore point, so if it fails too the entry goes back
    /// to before both.
    pub(crate) fn unwind(&mut self, owner: u64) -> Unwound {
        let Some(at) = self.layers.iter().position(|l| l.owner == owner) else {
            return Unwound::NoLayer;
        };
        let layer = self.layers.remove(at);
        if self.stamp != layer.stamp {
            if let Some(next) = self.layers.get_mut(at) {
                if next.before.stamp == layer.stamp {
                    next.before = layer.before;
                }
            }
            return Unwound::Kept;
        }
        let before = layer.before;
        if !same_bytes(&self.data, &before.data) {
            self.data_ver += 1;
        }
        if !same_bytes(&self.error, &before.error) {
            self.error_ver += 1;
        }
        self.data = before.data;
        self.error = before.error;
        self.updated_at = before.updated_at;
        self.invalidated = before.invalidated;
        self.failed = before.failed;
        self.stamp = before.stamp;
        Unwound::Restored
    }
}

/// What a critical section decided must happen once the lock is released.
#[must_use = "an Fx does nothing until it is run"]
#[derive(Default)]
pub(crate) struct Fx {
    publications: Vec<Publication>,
    wake: Vec<Waker>,
    cancel: Vec<TaskId>,
}

impl Fx {
    /// Publishes `entry`'s current state, and releases whoever waits for it to settle.
    pub(crate) fn publish(&mut self, entry: &mut Entry) {
        if let Some(publication) = entry.publication() {
            self.publications.push(publication);
        }
        if entry.inflight.is_none() {
            self.wake.append(&mut entry.settle);
        }
    }

    pub(crate) fn cancel(&mut self, task: TaskId) {
        self.cancel.push(task);
    }

    /// Runs what was collected. Publications are applied in one transaction.
    pub(crate) fn run(self, ctx: &Ctx) {
        for task in self.cancel {
            ctx.cancel_task(task);
        }
        for waker in self.wake {
            waker.wake();
        }
        if !self.publications.is_empty() {
            ctx.txn(|| {
                for (sinks, view) in &self.publications {
                    for sink in sinks {
                        sink.apply(view);
                    }
                }
            });
        }
    }
}

// -------------------------------------------------------------------------------------------
// The shared state
// -------------------------------------------------------------------------------------------

#[derive(Default)]
pub(crate) struct State {
    pub(crate) entries: HashMap<QueryKey, Entry>,
    /// Persisted entries read at hydration, waiting for their query to be observed:
    /// `(query_id, fnv1a64(params))` to what was stored.
    pub(crate) hydrated: HashMap<(u32, u64), Persisted>,
    /// Compiled key templates by query or mutation id.
    pub(crate) plans: HashMap<u32, Arc<KeyPlan>>,
    pub(crate) queue: QueueState,
    /// What the client knows about the `Kv` store (`storage`).
    pub(crate) storage: StorageState,
}

/// The per-runtime query client state: what `ctx.query()` shares.
pub(crate) struct Shared {
    pub(crate) state: Mutex<State>,
    /// The connectivity and lifecycle subscriptions are installed.
    started: AtomicBool,
    /// What the client believes about connectivity. Starts `true`: an app that has heard
    /// nothing from the platform yet is assumed to be able to reach the network, so its first
    /// requests are attempted rather than parked. Platforms report the real state right after
    /// start-up (SPEC 11), and only an `online = false` event makes the client queue mutations.
    pub(crate) online: AtomicBool,
    pub(crate) gc_ms: AtomicU64,
    next_gen: AtomicU64,
    next_sink: AtomicU64,
    /// Source of write stamps and of optimistic-mutation ids (see [`Entry::stamp`]).
    next_stamp: AtomicU64,
    /// The last time read from the `Clock` port, used if the port fails.
    last_now: AtomicI64,
    /// The persistence counters (`stats_json`'s `query.persist`).
    pub(crate) counters: Counters,
    /// Closures read from the store or written by this build, by fingerprint.
    pub(crate) closures: Mutex<HashMap<u64, Arc<TypeClosure>>>,
    /// The current closures of queries and mutations, by id.
    pub(crate) current: Mutex<HashMap<u32, Current>>,
}

impl Shared {
    pub(crate) fn new() -> Shared {
        Shared {
            state: Mutex::new(State::default()),
            started: AtomicBool::new(false),
            online: AtomicBool::new(true),
            gc_ms: AtomicU64::new(DEFAULT_GC_MS),
            next_gen: AtomicU64::new(0),
            next_sink: AtomicU64::new(0),
            next_stamp: AtomicU64::new(0),
            last_now: AtomicI64::new(0),
            counters: Counters::default(),
            closures: Mutex::new(HashMap::new()),
            current: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn now(&self, ctx: &Ctx) -> i64 {
        let now = now_ms(ctx, self.last_now.load(Ordering::Relaxed));
        self.last_now.store(now, Ordering::Relaxed);
        now
    }

    pub(crate) fn new_sink_id(&self) -> u64 {
        self.next_sink.fetch_add(1, Ordering::Relaxed)
    }

    /// A number nobody has been given before (never `0`): a write stamp, or the id of an
    /// optimistic mutation.
    pub(crate) fn new_stamp(&self) -> u64 {
        self.next_stamp.fetch_add(1, Ordering::Relaxed) + 1
    }

    pub(crate) fn is_online(&self) -> bool {
        self.online.load(Ordering::SeqCst)
    }

    pub(crate) fn log(ctx: &Ctx, level: u8, message: &str) {
        ctx.runtime().log(level, "undra::query", message);
    }

    /// The key template of `template` for the encoded `params`.
    pub(crate) fn render_key(&self, ctx: &Ctx, id: u32, template: &str, params: &[u8]) -> String {
        let schema = ctx.runtime().schema();
        let plan = plan_for(&mut self.state.lock().plans, schema, id, template);
        plan.render(schema, params)
    }

    // ----- observers -----------------------------------------------------------------------

    /// Registers an observer of `vt` with the encoded, canonical `params`: creates the entry
    /// if needed, cancels its garbage collection, shows it persisted data it had not seen, and
    /// starts a fetch if it is stale and none is running. `sink` (a handle) is published to
    /// from now on. Returns the entry's key and what it shows.
    pub(crate) fn observe(
        self: &Arc<Self>,
        ctx: &Ctx,
        vt: &'static QueryVTable,
        params: Arc<[u8]>,
        sink: Option<(u64, Weak<dyn Sink>)>,
    ) -> (QueryKey, View) {
        let now = self.now(ctx);
        let key = QueryKey::new(vt.id, params);
        let schema = ctx.runtime().schema();
        let mut fx = Fx::default();
        let view = {
            let mut state = self.state.lock();
            let State {
                entries,
                hydrated,
                plans,
                ..
            } = &mut *state;
            let entry = entries.entry(key.clone()).or_insert_with(|| {
                let rendered = plan_for(plans, schema, vt.id, vt.key).render(schema, &key.params);
                Entry::new(vt, rendered)
            });
            if entry.data.is_none() && vt.persist {
                if let Some(persisted) = hydrated.remove(&(vt.id, fnv1a64(&key.params))) {
                    entry.seed(&persisted);
                }
            }
            entry.observers += 1;
            if let Some(task) = entry.gc.take() {
                fx.cancel(task);
            }
            if let Some(sink) = sink {
                entry.sinks.push(sink);
            }
            if entry.needs_fetch(now) {
                self.start_fetch(ctx, &key, entry, &mut fx);
            }
            entry.view()
        };
        fx.run(ctx);
        (key, view)
    }

    /// An observer is gone. When the last one leaves, the in-flight fetch is cancelled and the
    /// entry is scheduled for garbage collection.
    pub(crate) fn release(self: &Arc<Self>, ctx: &Ctx, key: &QueryKey, sink_id: u64) {
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            let Some(entry) = state.entries.get_mut(key) else {
                return;
            };
            entry.sinks.retain(|(id, _)| *id != sink_id);
            entry.observers = entry.observers.saturating_sub(1);
            if entry.observers == 0 {
                if let Some(inflight) = entry.inflight.take() {
                    fx.cancel(inflight.task);
                }
                fx.publish(entry);
                self.schedule_gc(ctx, key, entry);
            }
        }
        fx.run(ctx);
    }

    // ----- fetching ------------------------------------------------------------------------

    /// Starts a fetch of `entry` unless one is running.
    pub(crate) fn start_fetch(
        self: &Arc<Self>,
        ctx: &Ctx,
        key: &QueryKey,
        entry: &mut Entry,
        fx: &mut Fx,
    ) {
        if entry.inflight.is_some() {
            return;
        }
        let serial = self.next_gen.fetch_add(1, Ordering::Relaxed) + 1;
        let task = ctx.spawn(run_fetch(
            self.clone(),
            ctx.downgrade(),
            key.clone(),
            entry.vt,
            serial,
        ));
        entry.inflight = Some(Inflight { serial, task });
        fx.publish(entry);
    }

    /// Fetches now, even if the data is fresh; joins a fetch that is already running.
    pub(crate) fn refetch(self: &Arc<Self>, ctx: &Ctx, key: &QueryKey) {
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            if let Some(entry) = state.entries.get_mut(key) {
                self.start_fetch(ctx, key, entry, &mut fx);
            }
        }
        fx.run(ctx);
    }

    /// A fetch finished (successfully or not) after its retries.
    fn complete(self: &Arc<Self>, ctx: &Ctx, key: &QueryKey, serial: u64, outcome: Outcome) {
        let now = self.now(ctx);
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            let Some(entry) = state.entries.get_mut(key) else {
                return;
            };
            if entry.inflight.as_ref().map(|i| i.serial) != Some(serial) {
                // Superseded or cancelled while the result was on its way.
                return;
            }
            entry.inflight = None;
            // Whatever the answer is, it is newer than any optimistic write before it, so a
            // rollback of one must not put its older snapshot on top.
            entry.stamp = self.new_stamp();
            match outcome {
                Ok(value) => {
                    let unchanged = entry
                        .data
                        .as_ref()
                        .is_some_and(|old| old.bytes == value.bytes);
                    if !unchanged {
                        entry.data = Some(value);
                        entry.data_ver += 1;
                    }
                    if entry.error.take().is_some() {
                        entry.error_ver += 1;
                    }
                    entry.failed = false;
                    entry.invalidated = false;
                    entry.updated_at = Some(now);
                    if entry.vt.persist {
                        self.schedule_persist(ctx, key, entry);
                    }
                }
                Err(Failure::Error(error)) => {
                    entry.error = Some(error);
                    entry.error_ver += 1;
                    entry.failed = false;
                }
                Err(Failure::Broken(message)) => {
                    entry.failed = true;
                    Shared::log(ctx, ERROR, &message);
                }
            }
            fx.publish(entry);
        }
        fx.run(ctx);
    }

    /// The fetch task of `serial` was dropped before it finished (it panicked). The entry shows
    /// an error and can be fetched again.
    fn abort(self: &Arc<Self>, ctx: &Ctx, key: &QueryKey, serial: u64) {
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            let Some(entry) = state.entries.get_mut(key) else {
                return;
            };
            if entry.inflight.as_ref().map(|i| i.serial) != Some(serial) {
                return;
            }
            entry.inflight = None;
            entry.failed = true;
            entry.stamp = self.new_stamp();
            fx.publish(entry);
        }
        Shared::log(
            ctx,
            ERROR,
            "a query fetch panicked; the entry shows an error",
        );
        fx.run(ctx);
    }

    // ----- invalidation --------------------------------------------------------------------

    /// Marks every entry that `targets` match as stale and refetches the observed ones (an
    /// in-flight fetch of such an entry is restarted: its answer may predate the change).
    pub(crate) fn invalidate(self: &Arc<Self>, ctx: &Ctx, targets: &[Invalidate]) {
        if targets.is_empty() {
            return;
        }
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            for (key, entry) in &mut state.entries {
                if !targets.iter().any(|t| t.matches(key, entry)) {
                    continue;
                }
                entry.invalidated = true;
                if entry.observers > 0 {
                    if let Some(inflight) = entry.inflight.take() {
                        fx.cancel(inflight.task);
                    }
                    self.start_fetch(ctx, key, entry, &mut fx);
                }
            }
        }
        fx.run(ctx);
    }

    // ----- triggers ------------------------------------------------------------------------

    /// Fetches every observed entry (that is stale, if `only_stale`).
    fn refetch_observed(self: &Arc<Self>, ctx: &Ctx, only_stale: bool) {
        let now = self.now(ctx);
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            for (key, entry) in &mut state.entries {
                if entry.observers > 0 && (!only_stale || entry.is_stale(now)) {
                    self.start_fetch(ctx, key, entry, &mut fx);
                }
            }
        }
        fx.run(ctx);
    }

    /// Connectivity changed. Going online refetches everything observed and replays the
    /// offline queue (SPEC 9).
    pub(crate) fn on_connectivity(self: &Arc<Self>, ctx: &Ctx, online: bool) {
        let was = self.online.swap(online, Ordering::SeqCst);
        if online && !was {
            self.refetch_observed(ctx, false);
            self.replay_queue(ctx);
        }
    }

    /// The app became active: observed entries that went stale while it was away refetch, and a
    /// queue that could not be read (an app launched before the device's first unlock) is read
    /// again (ADR-049 decision 1.4).
    pub(crate) fn on_active(self: &Arc<Self>, ctx: &Ctx) {
        self.refetch_observed(ctx, true);
        self.retry_unreadable_queue(ctx);
    }

    /// Subscribes to the `Connectivity` and `Lifecycle` events, once per runtime, and hydrates
    /// the cache if no start-up hook will.
    ///
    /// The hook ([`crate::__private::HYDRATE`]) is submitted by `#[undra::query]` and
    /// `#[undra::mutation]` (ADR-052), so a core whose queries are all written by hand does not
    /// link it: there the first use of the client (this call) reads the persisted entries and
    /// the offline queue, instead of nothing ever reading them.
    pub(crate) fn start(self: &Arc<Self>, ctx: &Ctx) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        if !hydrate_hook_linked() {
            self.spawn_hydration(ctx);
        }
        // The subscribers use the `Ctx` they are given (ADR-034): the runtime owns them, so one
        // they captured would keep it alive. (`Shared` holds no `Ctx`.)
        let shared = self.clone();
        undra_ports::on_connectivity_changed(ctx, move |ctx, online, _kind| {
            shared.on_connectivity(ctx, online);
        })
        .detach();
        let shared = self.clone();
        undra_ports::on_lifecycle_changed(ctx, move |ctx, state| match state {
            undra_ports::AppState::Active => shared.on_active(ctx),
            // A background run is another chance to read a queue that was unreadable.
            undra_ports::AppState::Background => shared.retry_unreadable_queue(ctx),
            undra_ports::AppState::Inactive => {}
        })
        .detach();
    }

    /// Starts [`Shared::hydrate`] on the core. The task holds the runtime weakly (ADR-034), so
    /// an idle runtime whose owner lets go is freed even while hydration still waits for a late
    /// `Kv` adapter. The start-up hook and [`Shared::start`] share it (one task type, one copy).
    pub(crate) fn spawn_hydration(self: &Arc<Self>, ctx: &Ctx) {
        let (shared, weak) = (self.clone(), ctx.downgrade());
        ctx.spawn(async move { shared.hydrate(&weak).await });
    }

    // ----- garbage collection --------------------------------------------------------------

    /// Schedules the removal of an unobserved entry `gc_ms` from now.
    pub(crate) fn schedule_gc(self: &Arc<Self>, ctx: &Ctx, key: &QueryKey, entry: &mut Entry) {
        if entry.gc.is_some() {
            return;
        }
        let delay = Duration::from_millis(self.gc_ms.load(Ordering::Relaxed));
        let shared = self.clone();
        // Weak (ADR-034): a pending collection must not keep a dropped runtime alive for `gc_ms`.
        let (weak, key2) = (ctx.downgrade(), key.clone());
        entry.gc = Some(ctx.spawn(async move {
            if weak.sleep(delay).await.is_ok() {
                shared.collect(&key2);
            }
        }));
    }

    /// Removes `key` if nobody observes it. The persisted copy stays in the `Kv` store.
    fn collect(&self, key: &QueryKey) {
        let mut state = self.state.lock();
        if state.entries.get(key).is_some_and(|e| e.observers == 0) {
            let removed = state.entries.remove(key);
            drop(state);
            drop(removed);
        }
    }

    // ----- persistence ---------------------------------------------------------------------

    /// Marks `entry` for writing to the `Kv` store 250 ms from now (later writes within the
    /// window are folded into the one pending).
    fn schedule_persist(self: &Arc<Self>, ctx: &Ctx, key: &QueryKey, entry: &mut Entry) {
        entry.persist_dirty = true;
        if entry.persist_task.is_some() {
            return;
        }
        entry.persist_task =
            Some(ctx.spawn(run_persist(self.clone(), ctx.downgrade(), key.clone())));
    }

    /// Reads the persisted cache entries, the offline queue and its dead letters from the `Kv`
    /// port (SPEC 9's `QueryClient::hydrate`), migrating what an older build wrote (ADR-037),
    /// then deletes the stored type descriptions nothing references any more, if everything
    /// could be read.
    pub(crate) async fn hydrate(self: &Arc<Self>, weak: &WeakCtx) {
        // Every step upgrades the weak context and lets go of it before waiting again
        // (ADR-034): hydration, with its retries for a late `Kv`, must not keep a runtime alive
        // that its owner has dropped.
        let cache = self.hydrate_cache(weak).await;
        let queue = self.hydrate_queue(weak).await;
        if cache && queue {
            self.collect_types(weak).await;
        }
    }

    /// Keeps a persisted entry for its query, or shows it right away if the query is already
    /// being observed and has nothing to show yet.
    pub(crate) fn adopt_persisted(
        &self,
        ctx: &Ctx,
        query_id: u32,
        params_hash: u64,
        persisted: Persisted,
    ) {
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            let State {
                entries, hydrated, ..
            } = &mut *state;
            let waiting = entries.iter_mut().find(|(key, entry)| {
                key.query_id == query_id
                    && entry.data.is_none()
                    && fnv1a64(&key.params) == params_hash
            });
            match waiting {
                Some((_, entry)) => {
                    if entry.seed(&persisted) {
                        fx.publish(entry);
                    }
                }
                None => {
                    hydrated.insert((query_id, params_hash), persisted);
                }
            }
        }
        fx.run(ctx);
    }

    // ----- the cache as data ---------------------------------------------------------------

    /// The data of `key`, if it has any.
    pub(crate) fn read(&self, key: &QueryKey) -> Option<Erased> {
        self.state
            .lock()
            .entries
            .get(key)
            .and_then(|entry| entry.data.clone())
    }

    /// Writes `value` as the data of the entry (created if missing), as if a fetch had
    /// returned it. An in-flight fetch is cancelled: its answer would overwrite the write with
    /// older data. With an `undo` log the write is an optimistic one: the entry's previous state
    /// is recorded first (in a [`Layer`] of the entry), and the log remembers the key.
    pub(crate) fn write(
        self: &Arc<Self>,
        ctx: &Ctx,
        vt: &'static QueryVTable,
        params: Arc<[u8]>,
        value: Erased,
        now: i64,
        undo: Option<&mut crate::mutation::UndoLog>,
    ) {
        let key = QueryKey::new(vt.id, params);
        let schema = ctx.runtime().schema();
        let persist_now = undo.is_none();
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            let State {
                entries,
                hydrated,
                plans,
                ..
            } = &mut *state;
            let fresh = !entries.contains_key(&key);
            let entry = entries.entry(key.clone()).or_insert_with(|| {
                let rendered = plan_for(plans, schema, vt.id, vt.key).render(schema, &key.params);
                Entry::new(vt, rendered)
            });
            if fresh && entry.data.is_none() && vt.persist {
                if let Some(persisted) = hydrated.remove(&(vt.id, fnv1a64(&key.params))) {
                    entry.seed(&persisted);
                }
            }
            let before = undo.is_some().then(|| {
                if fresh {
                    EntrySnapshot::EMPTY
                } else {
                    EntrySnapshot::of(entry)
                }
            });
            if let Some(inflight) = entry.inflight.take() {
                fx.cancel(inflight.task);
            }
            let unchanged = entry
                .data
                .as_ref()
                .is_some_and(|old| old.bytes == value.bytes);
            if !unchanged {
                entry.data = Some(value);
                entry.data_ver += 1;
            }
            if entry.error.take().is_some() {
                entry.error_ver += 1;
            }
            entry.failed = false;
            entry.updated_at = Some(now);
            let stamp = self.new_stamp();
            match (undo, before) {
                (Some(undo), Some(before)) => {
                    undo.touch(&key);
                    entry.record_optimistic_write(undo.owner(), before, stamp);
                }
                _ => entry.stamp = stamp,
            }
            if persist_now && vt.persist {
                self.schedule_persist(ctx, &key, entry);
            }
            if entry.observers == 0 {
                self.schedule_gc(ctx, &key, entry);
            }
            fx.publish(entry);
        }
        fx.run(ctx);
    }

    /// Undoes an optimistic mutation's writes, all in one transaction: every entry that is still
    /// exactly as the mutation left it (by write stamp, see [`Entry::unwind`]) goes back to what it
    /// was; an entry something else has written since is left alone. Entries the mutation created
    /// are removed (or emptied, if something observes them); an observed entry that ends up stale,
    /// or whose fetch the write had cancelled, fetches again.
    pub(crate) fn rollback(self: &Arc<Self>, ctx: &Ctx, undo: crate::mutation::UndoLog) {
        let now = self.now(ctx);
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            let owner = undo.owner();
            for key in undo.into_keys().into_iter().rev() {
                let Some(entry) = state.entries.get_mut(&key) else {
                    continue;
                };
                if entry.unwind(owner) != Unwound::Restored {
                    continue;
                }
                if entry.observers > 0 && entry.needs_fetch(now) {
                    self.start_fetch(ctx, &key, entry, &mut fx);
                }
                fx.publish(entry);
            }
            // Entries the mutation created and nobody observes are dropped with their timers
            // (unless another optimistic mutation still has a write on them).
            state.entries.retain(|_, entry| {
                entry.observers > 0
                    || entry.data.is_some()
                    || entry.error.is_some()
                    || !entry.layers.is_empty()
            });
        }
        fx.run(ctx);
    }

    /// An optimistic mutation succeeded: its writes stay, and it no longer has anything to undo.
    pub(crate) fn commit(&self, undo: &crate::mutation::UndoLog) {
        let mut state = self.state.lock();
        for key in undo.keys() {
            if let Some(entry) = state.entries.get_mut(key) {
                entry.commit(undo.owner());
            }
        }
    }

    /// Waits until the fetch of `key` has finished (returns at once if none is running).
    pub(crate) fn poll_settled(&self, key: &QueryKey, waker: &Waker) -> bool {
        let mut state = self.state.lock();
        match state.entries.get_mut(key) {
            Some(entry) if entry.inflight.is_some() => {
                if !entry.settle.iter().any(|w| w.will_wake(waker)) {
                    entry.settle.push(waker.clone());
                }
                false
            }
            _ => true,
        }
    }
}

// -------------------------------------------------------------------------------------------
// Tasks
// -------------------------------------------------------------------------------------------

/// Clears the in-flight marker of a fetch whose task was dropped before it finished.
struct FetchGuard {
    shared: Arc<Shared>,
    ctx: WeakCtx,
    key: QueryKey,
    serial: u64,
    armed: bool,
}

impl Drop for FetchGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // A runtime that is shutting down or gone has no entry left to show the error on.
        if let Ok(ctx) = self.ctx.upgrade() {
            self.shared.abort(&ctx, &self.key, self.serial);
        }
    }
}

/// The fetch task of one entry: runs the query with retries and reports the outcome. It holds the
/// runtime weakly (ADR-034): each attempt upgrades for as long as the query's own future runs, and
/// the backoff between attempts holds nothing.
async fn run_fetch(
    shared: Arc<Shared>,
    weak: WeakCtx,
    key: QueryKey,
    vt: &'static QueryVTable,
    serial: u64,
) {
    let mut guard = FetchGuard {
        shared: shared.clone(),
        ctx: weak.clone(),
        key: key.clone(),
        serial,
        armed: true,
    };
    let outcome = with_retries(&weak, vt.retry, Failure::retryable, || {
        match weak.upgrade() {
            Ok(ctx) => (vt.fetch)(ctx, &key.params),
            Err(gone) => Box::pin(async move { Err(Failure::Broken(gone.to_string())) }),
        }
    })
    .await;
    guard.armed = false;
    if let Ok(ctx) = weak.upgrade() {
        shared.complete(&ctx, &key, serial, outcome);
    }
}

/// Clears the persist marker of an entry whose write task ended abnormally.
struct PersistGuard {
    shared: Arc<Shared>,
    key: QueryKey,
    armed: bool,
}

impl Drop for PersistGuard {
    fn drop(&mut self) {
        if self.armed {
            if let Some(entry) = self.shared.state.lock().entries.get_mut(&self.key) {
                entry.persist_task = None;
            }
        }
    }
}

/// The write task of one entry: waits out the debounce, writes the entry to the `Kv` port, and
/// goes around again if another fetch finished meanwhile.
async fn run_persist(shared: Arc<Shared>, weak: WeakCtx, key: QueryKey) {
    let mut guard = PersistGuard {
        shared: shared.clone(),
        key: key.clone(),
        armed: true,
    };
    loop {
        // The debounce holds only the weak context (ADR-034).
        if weak
            .sleep(Duration::from_millis(PERSIST_DEBOUNCE_MS))
            .await
            .is_err()
        {
            return;
        }
        let write = {
            let mut state = shared.state.lock();
            let Some(entry) = state.entries.get_mut(&key) else {
                guard.armed = false;
                return;
            };
            entry.persist_dirty = false;
            entry.data.is_some() && entry.updated_at.is_some()
        };
        if write {
            let Ok(ctx) = weak.upgrade() else {
                return;
            };
            shared.persist_entry(&ctx, &key).await;
        }
        let mut state = shared.state.lock();
        match state.entries.get_mut(&key) {
            Some(entry) if entry.persist_dirty => {}
            Some(entry) => {
                entry.persist_task = None;
                guard.armed = false;
                return;
            }
            None => {
                guard.armed = false;
                return;
            }
        }
    }
}

/// How the client lives on a runtime (an extension slot).
struct Ext(Arc<Shared>);

/// Whether this program links the start-up hook that hydrates the cache, which it does when the
/// core declares a query or a mutation with the macros (ADR-052). When it does, the runtime (or
/// the test, for a `TestRuntime`) runs it; when it does not, [`Shared::start`] hydrates.
fn hydrate_hook_linked() -> bool {
    undra_runtime::inventory::iter::<undra_runtime::InitHook>
        .into_iter()
        .any(|hook| hook.name == crate::__private::HYDRATE.name)
}

/// The `Shared` of `ctx`'s runtime, created on first use. Creating it also adds the client's
/// section to the runtime's `stats_json` ([`crate::stats_section`]): registered here, at run time,
/// rather than through `inventory`, so a core that never uses the client does not link it.
pub(crate) fn shared_of(runtime: &Runtime) -> Arc<Shared> {
    if let Some(ext) = runtime.try_extension::<Ext>() {
        return ext.0.clone();
    }
    runtime.add_stats_section(undra_runtime::StatsSection {
        name: "query",
        json: crate::stats_section,
    });
    runtime
        .extension_with(|| Ext(Arc::new(Shared::new())))
        .0
        .clone()
}

/// The `Shared` of a runtime, if one was created.
pub(crate) fn existing(runtime: &Runtime) -> Option<Arc<Shared>> {
    runtime.try_extension::<Ext>().map(|ext| ext.0.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defs::{BoxFuture, QueryDef};
    use crate::erased::query_vtable;
    use std::sync::atomic::AtomicUsize;
    use std::task::Wake;
    use undra_runtime::testing::TestRuntime;

    struct Dummy;

    impl QueryDef for Dummy {
        const ID: u32 = 7;
        const KEY: &'static str = "dummy:{x}";
        const STALE_MS: Option<u64> = Some(1_000);
        const PERSIST: bool = false;
        const RETRY: u32 = 0;
        type Params = (u8,);
        type Output = u32;
        type Error = String;
        fn fetch(_: Ctx, _: (u8,)) -> BoxFuture<Result<u32, String>> {
            Box::pin(async { Ok(1) })
        }
    }

    struct Always;

    impl QueryDef for Always {
        const ID: u32 = 8;
        const KEY: &'static str = "always";
        const STALE_MS: Option<u64> = None;
        const PERSIST: bool = false;
        const RETRY: u32 = 0;
        type Params = ();
        type Output = u32;
        type Error = String;
        fn fetch(_: Ctx, _: ()) -> BoxFuture<Result<u32, String>> {
            Box::pin(async { Ok(1) })
        }
    }

    fn entry() -> Entry {
        Entry::new(query_vtable::<Dummy>(), "dummy:1".to_owned())
    }

    fn with_data(mut entry: Entry, at: i64) -> Entry {
        entry.data = Some(Erased::new(5_u32));
        entry.updated_at = Some(at);
        entry
    }

    /// A task id to put in an `Inflight` (ids cannot be made up).
    fn a_task() -> TaskId {
        let t = TestRuntime::new();
        t.ctx().spawn(async {})
    }

    fn inflight() -> Option<Inflight> {
        Some(Inflight {
            serial: 1,
            task: a_task(),
        })
    }

    #[test]
    fn status_is_derived_from_what_the_entry_holds() {
        let mut e = entry();
        assert_eq!(e.status(), QueryStatus::Idle);

        e.inflight = inflight();
        assert_eq!(
            e.status(),
            QueryStatus::Fetching,
            "fetching with nothing to show"
        );

        e.data = Some(Erased::new(1_u32));
        assert_eq!(
            e.status(),
            QueryStatus::Success,
            "a refetch keeps showing the data"
        );

        e.error = Some(Erased::new("boom".to_owned()));
        assert_eq!(
            e.status(),
            QueryStatus::Error,
            "the last fetch failed; stale data stays"
        );

        e.inflight = None;
        e.error = None;
        e.failed = true;
        assert_eq!(
            e.status(),
            QueryStatus::Error,
            "a fetch that died shows as an error too"
        );

        e.failed = false;
        e.data = None;
        e.error = Some(Erased::new("boom".to_owned()));
        e.inflight = inflight();
        assert_eq!(
            e.status(),
            QueryStatus::Fetching,
            "retrying by hand with no data is a spinner again"
        );
    }

    #[test]
    fn staleness_follows_the_window_and_the_flags() {
        let fresh = with_data(entry(), 1_000);
        assert!(!fresh.is_stale(1_000));
        assert!(!fresh.is_stale(1_999));
        assert!(
            fresh.is_stale(2_000),
            "the window is over at exactly its length"
        );
        assert!(fresh.is_stale(5_000));
        // A clock that moved backwards cannot vouch for the data: stale (and no panic).
        assert!(fresh.is_stale(0));
        assert!(fresh.is_stale(i64::MIN));

        assert!(entry().is_stale(0), "no data is always stale");

        let mut invalidated = with_data(entry(), 1_000);
        invalidated.invalidated = true;
        assert!(invalidated.is_stale(1_000));

        let mut errored = with_data(entry(), 1_000);
        errored.error = Some(Erased::new("x".to_owned()));
        assert!(
            errored.is_stale(1_000),
            "a failed last fetch is worth trying again"
        );

        let mut failed = with_data(entry(), 1_000);
        failed.failed = true;
        assert!(failed.is_stale(1_000));

        let mut always = Entry::new(query_vtable::<Always>(), "always".to_owned());
        always.data = Some(Erased::new(1_u32));
        always.updated_at = Some(1_000);
        assert!(always.is_stale(1_000), "no window means always stale");
    }

    #[test]
    fn a_fetch_is_only_needed_when_stale_and_idle() {
        let mut e = entry();
        assert!(e.needs_fetch(0));
        e.inflight = inflight();
        assert!(!e.needs_fetch(0), "one fetch at a time");
        let fresh = with_data(entry(), 0);
        assert!(!fresh.needs_fetch(10));
    }

    struct Recorder(AtomicUsize);

    impl Sink for Recorder {
        fn apply(&self, _: &View) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn publications_bump_the_sequence_and_skip_entries_nobody_shows() {
        let mut e = entry();
        assert!(e.publication().is_none(), "no sinks: nothing to publish to");
        assert_eq!(
            e.seq, 1,
            "the sequence still advances, so a later view is newer"
        );

        let sink = Arc::new(Recorder(AtomicUsize::new(0)));
        let weak: Weak<dyn Sink> = Arc::downgrade(&sink) as Weak<dyn Sink>;
        e.sinks.push((1, weak));
        let (sinks, view) = e.publication().unwrap();
        assert_eq!((sinks.len(), view.seq), (1, 2));

        // A sink that is gone is pruned, not kept alive.
        drop(sinks);
        drop(sink);
        assert!(e.publication().is_none());
        assert!(e.sinks.is_empty());
    }

    struct CountWake(AtomicUsize);

    impl Wake for CountWake {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn waiters_are_released_only_when_the_fetch_is_over() {
        let counter = Arc::new(CountWake(AtomicUsize::new(0)));
        let mut e = entry();
        e.settle.push(Waker::from(counter.clone()));
        e.inflight = inflight();
        let mut fx = Fx::default();
        fx.publish(&mut e);
        assert_eq!(fx.wake.len(), 0, "still fetching: keep waiting");

        e.inflight = None;
        fx.publish(&mut e);
        assert_eq!(fx.wake.len(), 1);
        assert!(e.settle.is_empty());
        for waker in std::mem::take(&mut fx.wake) {
            waker.wake();
        }
        assert_eq!(counter.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_snapshot_restores_every_field_of_an_entry() {
        let mut e = with_data(entry(), 42);
        e.invalidated = true;
        let snapshot = EntrySnapshot::of(&e);
        e.data = None;
        e.updated_at = None;
        e.invalidated = false;
        e.failed = true;
        assert!(snapshot.data.is_some());
        assert_eq!(snapshot.updated_at, Some(42));
        assert!(snapshot.invalidated && !snapshot.failed);
        assert!(EntrySnapshot::EMPTY.data.is_none() && EntrySnapshot::EMPTY.updated_at.is_none());
    }

    // ----- layers and stamps: a rollback is the inverse of the mutation's own writes -----------

    /// What `Shared::write` does to an entry for an optimistic write by `owner`.
    fn optimistic_write(e: &mut Entry, owner: u64, value: u32, stamp: u64) {
        let before = EntrySnapshot::of(e);
        e.data = Some(Erased::new(value));
        e.data_ver += 1;
        e.record_optimistic_write(owner, before, stamp);
    }

    /// What a fetch result (or a plain `set`) does: new data, a new stamp, no layer.
    fn foreign_write(e: &mut Entry, value: u32, stamp: u64) {
        e.data = Some(Erased::new(value));
        e.data_ver += 1;
        e.stamp = stamp;
    }

    fn value(e: &Entry) -> Option<u32> {
        e.data.as_ref().and_then(|d| d.typed::<u32>())
    }

    #[test]
    fn unwinding_an_entry_as_the_owner_left_it_restores_it_with_its_stamp() {
        let mut e = with_data(entry(), 42);
        e.stamp = 3;
        let ver = e.data_ver;
        optimistic_write(&mut e, 10, 6, 11);
        assert_eq!((value(&e), e.stamp), (Some(6), 11));

        assert_eq!(e.unwind(10), Unwound::Restored);
        assert_eq!((value(&e), e.stamp, e.updated_at), (Some(5), 3, Some(42)));
        assert!(e.data_ver > ver, "observers are told the data changed");
        assert!(e.layers.is_empty());
        assert_eq!(
            e.unwind(10),
            Unwound::NoLayer,
            "a second unwind finds nothing"
        );
    }

    #[test]
    fn an_entry_written_since_is_left_alone_even_if_it_was_written_with_the_same_bytes() {
        let mut e = with_data(entry(), 42);
        optimistic_write(&mut e, 10, 6, 11);
        // A fetch returns exactly what the optimistic write said. It is still newer.
        foreign_write(&mut e, 6, 12);
        let ver = e.data_ver;
        assert_eq!(e.unwind(10), Unwound::Kept);
        assert_eq!((value(&e), e.stamp, e.data_ver), (Some(6), 12, ver));
        assert!(e.layers.is_empty(), "the layer is gone either way");
    }

    #[test]
    fn a_later_optimistic_write_survives_and_inherits_the_restore_point() {
        let mut e = with_data(entry(), 42);
        e.stamp = 3;
        optimistic_write(&mut e, 10, 6, 11); // A
        optimistic_write(&mut e, 20, 7, 21); // B, made on top of A's result

        assert_eq!(e.unwind(10), Unwound::Kept, "A fails: B's value stays");
        assert_eq!(value(&e), Some(7));
        assert_eq!(e.layers.len(), 1);

        // B fails too: the entry goes back to before A, not to A's failed value.
        assert_eq!(e.unwind(20), Unwound::Restored);
        assert_eq!((value(&e), e.stamp), (Some(5), 3));
    }

    #[test]
    fn the_later_write_inherits_nothing_if_something_else_wrote_in_between() {
        let mut e = with_data(entry(), 42);
        optimistic_write(&mut e, 10, 6, 11); // A
        foreign_write(&mut e, 9, 12); // a fetch result
        optimistic_write(&mut e, 20, 10, 21); // B, on top of the fetch result

        assert_eq!(e.unwind(10), Unwound::Kept);
        // B's restore point is still the fetch result, not what the entry was before A.
        assert_eq!(e.unwind(20), Unwound::Restored);
        assert_eq!(value(&e), Some(9));
    }

    #[test]
    fn rollbacks_in_reverse_order_restore_step_by_step() {
        let mut e = with_data(entry(), 42);
        optimistic_write(&mut e, 10, 6, 11);
        optimistic_write(&mut e, 20, 7, 21);
        assert_eq!(e.unwind(20), Unwound::Restored);
        assert_eq!(value(&e), Some(6), "B's rollback puts A's value back");
        assert_eq!(e.stamp, 11, "and A's stamp with it");
        assert_eq!(e.unwind(10), Unwound::Restored, "so A can still restore");
        assert_eq!(value(&e), Some(5));
    }

    #[test]
    fn a_committed_mutation_has_nothing_left_to_undo() {
        let mut e = with_data(entry(), 42);
        optimistic_write(&mut e, 10, 6, 11); // A
        optimistic_write(&mut e, 20, 7, 21); // B
        e.commit(20);
        assert_eq!(e.layers.len(), 1);
        assert_eq!(e.unwind(20), Unwound::NoLayer);
        // A fails after B succeeded: B's value stays.
        assert_eq!(e.unwind(10), Unwound::Kept);
        assert_eq!(value(&e), Some(7));
    }

    #[test]
    fn a_second_write_by_the_same_mutation_keeps_its_first_restore_point() {
        let mut e = with_data(entry(), 42);
        optimistic_write(&mut e, 10, 6, 11);
        optimistic_write(&mut e, 10, 7, 12);
        assert_eq!(e.layers.len(), 1);
        assert_eq!(e.unwind(10), Unwound::Restored);
        assert_eq!(value(&e), Some(5), "the value before the first write");
    }

    #[test]
    fn an_entry_the_mutation_created_goes_back_to_empty() {
        let mut e = entry();
        let before = EntrySnapshot::EMPTY;
        e.data = Some(Erased::new(1_u32));
        e.record_optimistic_write(10, before, 4);
        assert_eq!(e.unwind(10), Unwound::Restored);
        assert!(e.data.is_none() && e.updated_at.is_none() && e.stamp == 0);
    }

    #[test]
    fn same_bytes_compares_encodings_not_identity() {
        let a = Some(Erased::new(5_u32));
        let b = Some(Erased::new(5_u32));
        let c = Some(Erased::new(6_u32));
        assert!(same_bytes(&a, &b));
        assert!(!same_bytes(&a, &c));
        assert!(!same_bytes(&a, &None));
        assert!(same_bytes(&None, &None));
    }
}
