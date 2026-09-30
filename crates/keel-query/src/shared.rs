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

use keel_meta::ids::fnv1a64;
use keel_ports::CtxPorts;
use keel_runtime::executor::TaskId;
use keel_runtime::log::{DEBUG, ERROR};
use keel_runtime::{Ctx, Runtime};
use keel_wire::Bytes;
use parking_lot::Mutex;

use crate::erased::{Erased, Failure, Outcome, QueryVTable};
use crate::key::{Invalidate, QueryKey};
use crate::persist::{
    CACHE_KEY_PREFIX, Persisted, cache_key, decode_persisted, encode_persisted, parse_cache_key,
};
use crate::queue::QueueState;
use crate::retry::{now_ms, with_retries};
use crate::status::QueryStatus;
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
                now.saturating_sub(at) >= i64::try_from(window).unwrap_or(i64::MAX)
            }
            // No staleness window: the data is always stale.
            _ => true,
        }
    }

    /// Whether observing (or a trigger) should start a fetch now.
    pub(crate) fn needs_fetch(&self, now: i64) -> bool {
        self.inflight.is_none() && self.is_stale(now)
    }

    /// Starts showing a persisted entry. `false` if its bytes no longer decode.
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

/// The pre-mutation state of an entry, kept by an optimistic write so it can be restored.
#[derive(Clone)]
pub(crate) struct EntrySnapshot {
    data: Option<Erased>,
    error: Option<Erased>,
    updated_at: Option<i64>,
    invalidated: bool,
    failed: bool,
}

impl EntrySnapshot {
    pub(crate) fn of(entry: &Entry) -> EntrySnapshot {
        EntrySnapshot {
            data: entry.data.clone(),
            error: entry.error.clone(),
            updated_at: entry.updated_at,
            invalidated: entry.invalidated,
            failed: entry.failed,
        }
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
    /// The last time read from the `Clock` port, used if the port fails.
    last_now: AtomicI64,
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
            last_now: AtomicI64::new(0),
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

    pub(crate) fn is_online(&self) -> bool {
        self.online.load(Ordering::SeqCst)
    }

    pub(crate) fn log(ctx: &Ctx, level: u8, message: &str) {
        ctx.runtime().log(level, "keel::query", message);
    }

    /// The key template of `template` for the encoded `params`.
    pub(crate) fn render_key(&self, ctx: &Ctx, id: u32, template: &str, params: &[u8]) -> String {
        let schema = ctx.runtime().schema();
        let plan = {
            let mut state = self.state.lock();
            state
                .plans
                .entry(id)
                .or_insert_with(|| {
                    let meta = schema.queries.iter().find(|q| q.query_id == id);
                    Arc::new(KeyPlan::new(template, meta.map(|m| m.params.as_slice())))
                })
                .clone()
        };
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
        let rendered = self.render_key(ctx, vt.id, vt.key, &key.params);
        let mut fx = Fx::default();
        let view = {
            let mut state = self.state.lock();
            let State {
                entries, hydrated, ..
            } = &mut *state;
            let entry = entries
                .entry(key.clone())
                .or_insert_with(|| Entry::new(vt, rendered));
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
            ctx.clone(),
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

    /// The app became active: observed entries that went stale while it was away refetch.
    pub(crate) fn on_active(self: &Arc<Self>, ctx: &Ctx) {
        self.refetch_observed(ctx, true);
    }

    /// Subscribes to the `Connectivity` and `Lifecycle` events, once per runtime.
    pub(crate) fn start(self: &Arc<Self>, ctx: &Ctx) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = self.clone();
        keel_ports::on_connectivity_changed(ctx, move |online, _kind| {
            // Event callbacks run on the core loop, where the runtime is current.
            if let Some(ctx) = Ctx::try_current() {
                shared.on_connectivity(&ctx, online);
            }
        })
        .detach();
        let shared = self.clone();
        keel_ports::on_lifecycle_changed(ctx, move |state| {
            if state == keel_ports::AppState::Active {
                if let Some(ctx) = Ctx::try_current() {
                    shared.on_active(&ctx);
                }
            }
        })
        .detach();
    }

    // ----- garbage collection --------------------------------------------------------------

    /// Schedules the removal of an unobserved entry `gc_ms` from now.
    pub(crate) fn schedule_gc(self: &Arc<Self>, ctx: &Ctx, key: &QueryKey, entry: &mut Entry) {
        if entry.gc.is_some() {
            return;
        }
        let delay = Duration::from_millis(self.gc_ms.load(Ordering::Relaxed));
        let shared = self.clone();
        let (ctx2, key2) = (ctx.clone(), key.clone());
        entry.gc = Some(ctx.spawn(async move {
            ctx2.sleep(delay).await;
            shared.collect(&key2);
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
        entry.persist_task = Some(ctx.spawn(run_persist(self.clone(), ctx.clone(), key.clone())));
    }

    /// Reads the persisted cache entries and the offline queue from the `Kv` port
    /// (SPEC 9's `QueryClient::hydrate`).
    ///
    /// An entry written under another schema hash is deleted; one whose query has not been
    /// observed yet waits in memory until it is. A queue written under another schema hash
    /// is deleted; otherwise its mutations are replayed at once if the client is online.
    pub(crate) async fn hydrate(self: &Arc<Self>, ctx: &Ctx) {
        let kv = ctx.kv();
        let schema_hash = ctx.runtime().schema_hash();
        let keys = kv.list(CACHE_KEY_PREFIX.to_owned()).await;
        for key in keys {
            let Some((query_id, params_hash)) = parse_cache_key(&key) else {
                continue;
            };
            let Some(Bytes(raw)) = kv.get(key.clone()).await else {
                continue;
            };
            match decode_persisted(&raw) {
                Ok(persisted) if persisted.schema_hash == schema_hash => {
                    self.adopt_persisted(ctx, query_id, params_hash, persisted);
                }
                _ => {
                    Shared::log(
                        ctx,
                        DEBUG,
                        &format!("dropping the stale cache entry `{key}`"),
                    );
                    kv.delete(key).await;
                }
            }
        }
        self.hydrate_queue(ctx).await;
    }

    /// Keeps a persisted entry for its query, or shows it right away if the query is already
    /// being observed and has nothing to show yet.
    fn adopt_persisted(
        self: &Arc<Self>,
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
    /// older data. With an `undo` log the entry's previous state is recorded first.
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
        let rendered = self.render_key(ctx, vt.id, vt.key, &key.params);
        let persist_now = undo.is_none();
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            let State {
                entries, hydrated, ..
            } = &mut *state;
            let fresh = !entries.contains_key(&key);
            let entry = entries
                .entry(key.clone())
                .or_insert_with(|| Entry::new(vt, rendered));
            if fresh && entry.data.is_none() && vt.persist {
                if let Some(persisted) = hydrated.remove(&(vt.id, fnv1a64(&key.params))) {
                    entry.seed(&persisted);
                }
            }
            if let Some(undo) = undo {
                undo.record(
                    &key,
                    if fresh {
                        None
                    } else {
                        Some(EntrySnapshot::of(entry))
                    },
                );
            }
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

    /// Puts entries back as an optimistic mutation found them, all in one transaction. Entries
    /// the mutation created are removed (or emptied, if something observes them); an observed
    /// entry that ends up stale, or whose fetch the write had cancelled, fetches again.
    pub(crate) fn rollback(self: &Arc<Self>, ctx: &Ctx, undo: crate::mutation::UndoLog) {
        let now = self.now(ctx);
        let mut fx = Fx::default();
        {
            let mut state = self.state.lock();
            for (key, before) in undo.into_entries().into_iter().rev() {
                let Some(entry) = state.entries.get_mut(&key) else {
                    continue;
                };
                match before {
                    Some(snapshot) => {
                        entry.data_ver += 1;
                        entry.error_ver += 1;
                        entry.data = snapshot.data;
                        entry.error = snapshot.error;
                        entry.updated_at = snapshot.updated_at;
                        entry.invalidated = snapshot.invalidated;
                        entry.failed = snapshot.failed;
                    }
                    None => {
                        entry.data_ver += 1;
                        entry.error_ver += 1;
                        entry.data = None;
                        entry.error = None;
                        entry.updated_at = None;
                        entry.invalidated = false;
                        entry.failed = false;
                    }
                }
                if entry.observers > 0 && entry.needs_fetch(now) {
                    self.start_fetch(ctx, &key, entry, &mut fx);
                }
                fx.publish(entry);
            }
            // Entries the mutation created and nobody observes are dropped with their timers.
            state.entries.retain(|_, entry| {
                entry.observers > 0 || entry.data.is_some() || entry.error.is_some()
            });
        }
        fx.run(ctx);
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
    ctx: Ctx,
    key: QueryKey,
    serial: u64,
    armed: bool,
}

impl Drop for FetchGuard {
    fn drop(&mut self) {
        if self.armed && !self.ctx.runtime().is_shut_down() {
            self.shared.abort(&self.ctx, &self.key, self.serial);
        }
    }
}

/// The fetch task of one entry: runs the query with retries and reports the outcome.
async fn run_fetch(
    shared: Arc<Shared>,
    ctx: Ctx,
    key: QueryKey,
    vt: &'static QueryVTable,
    serial: u64,
) {
    let mut guard = FetchGuard {
        shared: shared.clone(),
        ctx: ctx.clone(),
        key: key.clone(),
        serial,
        armed: true,
    };
    let outcome = with_retries(&ctx, vt.retry, Failure::retryable, || {
        (vt.fetch)(ctx.clone(), &key.params)
    })
    .await;
    guard.armed = false;
    shared.complete(&ctx, &key, serial, outcome);
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
async fn run_persist(shared: Arc<Shared>, ctx: Ctx, key: QueryKey) {
    let mut guard = PersistGuard {
        shared: shared.clone(),
        key: key.clone(),
        armed: true,
    };
    let schema_hash = ctx.runtime().schema_hash();
    loop {
        ctx.sleep(Duration::from_millis(PERSIST_DEBOUNCE_MS)).await;
        let write = {
            let mut state = shared.state.lock();
            let Some(entry) = state.entries.get_mut(&key) else {
                guard.armed = false;
                return;
            };
            entry.persist_dirty = false;
            match (&entry.data, entry.updated_at) {
                (Some(data), Some(updated_at)) => Some((data.bytes.clone(), updated_at)),
                _ => None,
            }
        };
        if let Some((bytes, updated_at)) = write {
            let value = encode_persisted(schema_hash, updated_at, &bytes);
            ctx.kv()
                .set(cache_key(key.query_id, &key.params), Bytes(value))
                .await;
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

/// The `Shared` of `ctx`'s runtime, created on first use.
pub(crate) fn shared_of(runtime: &Runtime) -> Arc<Shared> {
    struct Ext(Arc<Shared>);
    runtime
        .extension_with(|| Ext(Arc::new(Shared::new())))
        .0
        .clone()
}
