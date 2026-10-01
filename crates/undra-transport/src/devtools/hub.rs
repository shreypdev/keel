//! The devtools hub: what makes a dev server observable (ADR-054).
//!
//! The hub exists for the whole life of a dev server and is **idle** until a devtools page
//! attaches. While at least one page is attached it
//!
//! * observes every store through [`Runtime::observe`] (so the page sees signals no app client
//!   asked for), and the [`Bridge`](crate::Bridge) routes what the runtime then emits: every
//!   change-set reaches the pages whole, the app client receives only the entries it observed
//!   (see [`Route`]);
//! * records the calls the core makes to platform-implemented ports, with arguments, reply and
//!   latency;
//! * runs one worker thread that, after a burst of commits, takes a
//!   [`Runtime::snapshot`] into the [`Ring`] (a *step*), samples the query cache and the
//!   counters, and carries out time travel (a [`Runtime::restore`] of a step).
//!
//! # Locks
//!
//! The runtime calls the bridge, and so the hub, from core threads that may hold the core lock
//! and a store's delivery lock. Nothing here holds a hub lock while it calls into the runtime:
//! the locks below guard small tables, and every method that observes, snapshots or restores
//! first copies what it needs out of them.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use undra_runtime::log::{INFO, WARN};
use undra_runtime::Runtime;
use undra_wire::payload::Snapshot;
use undra_wire::{Handle, Reader, Writer};

use super::DevtoolsConfig;
use super::proto::{
    Cause, ClientMsg, Delivery, PROTOCOL, ServerMsg, StepInfo, StoreRef, Traveled, Welcome, encode_change_set,
    encode_port_end, encode_port_start,
};
use super::ring::Ring;
use crate::bridge::Bridge;
use crate::conn::Conn;
use crate::notice::NOTICE_TARGET;

const TARGET: &str = "undra::devtools";

/// How long the worker waits after the first commit of a burst before it snapshots: long enough
/// for a click's transactions to land together, short enough to feel immediate.
const COALESCE: Duration = Duration::from_millis(10);

/// How often the worker looks for new stores and a changed query cache when nothing else wakes it.
const TICK: Duration = Duration::from_millis(250);

/// The most of the worker's time a snapshot or a query sample may take: the next one waits for
/// this many times as long as the last one took. A snapshot holds the core lock, so on a big
/// state a burst of commits would otherwise keep the core busy for the page's sake.
const DUTY: u32 = 9;

/// The longest the worker waits between two snapshots whatever one cost.
const MAX_GAP: Duration = Duration::from_secs(2);

/// How many port calls the hub keeps open at once before it forgets the old ones.
const MAX_OPEN_PORT_CALLS: usize = 2048;

/// How often it sends the counters.
const STATS_EVERY: Duration = Duration::from_secs(1);

/// What a change-set going through the bridge on this thread means. Set with [`RouteGuard`]
/// around the runtime call that causes it; read by [`Bridge`](crate::Bridge) while a hub is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Route {
    /// An ordinary delivery: the app client gets the entries it observed, the pages get all of it
    /// as news, labelled with the cause.
    Commit(Cause),
    /// The hub's own observation of a store (or a page's resync): the pages get the values as
    /// state (one page when `only` names it); the app client gets nothing.
    Initial { only: Option<u64> },
    /// The app client's own `Observe`: it gets the values; the pages already hold them.
    AppObserve,
    /// Nobody: the hub switching a store's observation off.
    Silent,
}

thread_local! {
    static ROUTE: Cell<Route> = const { Cell::new(Route::Commit(Cause::Other)) };
}

/// The route for change-sets delivered on this thread right now.
pub(crate) fn current_route() -> Route {
    ROUTE.with(Cell::get)
}

/// Sets the [`Route`] of this thread until dropped.
pub(crate) struct RouteGuard(Route);

impl RouteGuard {
    pub(crate) fn set(route: Route) -> RouteGuard {
        RouteGuard(ROUTE.with(|r| r.replace(route)))
    }
}

impl Drop for RouteGuard {
    fn drop(&mut self) {
        ROUTE.with(|r| r.set(self.0));
    }
}

enum Event {
    /// A transaction committed.
    Commit,
    /// A page needs the stores and their values.
    Sync(u64),
    /// A page asked to travel.
    Restore { request_id: u32, step: u32 },
    Stop,
}

struct OpenPort {
    started: Instant,
    port_id: u32,
    method_id: u32,
}

#[derive(Default)]
struct Counters {
    commits: AtomicU64,
    entries: AtomicU64,
    change_set_bytes: AtomicU64,
    steps: AtomicU64,
    skipped_same: AtomicU64,
    port_calls: AtomicU64,
    /// Port calls made while no app client was attached: not listed, only counted.
    unattended_port_calls: AtomicU64,
    travels: AtomicU64,
}

/// The state of the devtools of one dev server.
pub(crate) struct Hub {
    rt: Arc<Runtime>,
    bridge: Weak<Bridge>,
    cfg: DevtoolsConfig,
    epoch: u64,
    started: Instant,
    started_unix_ms: u64,
    active: AtomicBool,
    seq: AtomicU64,
    last_txn: AtomicU64,
    counters: Counters,
    clients: Mutex<Vec<Arc<Conn>>>,
    /// The stores the hub observes.
    held: Mutex<HashSet<u64>>,
    ring: Mutex<Ring>,
    ports: Mutex<HashMap<u32, OpenPort>>,
    /// The stores last announced to the pages.
    announced: Mutex<Vec<StoreRef>>,
    events: Mutex<Option<Sender<Event>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    /// Serialises attach, detach and shutdown (never taken on a path the runtime calls).
    lifecycle: Mutex<()>,
    stopped: AtomicBool,
    /// Set by a restore, taken by the next step: which step it restored.
    restored_from: AtomicU32,
    /// `live_stores` of the last look: a change is a store built or released.
    last_live: AtomicU64,
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

fn json_u64(text: &str, key: &str) -> Option<u64> {
    let at = text.find(&format!("\"{key}\":"))? + key.len() + 3;
    let digits: String = text[at..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

impl Hub {
    pub(crate) fn new(rt: Arc<Runtime>, bridge: &Arc<Bridge>, cfg: DevtoolsConfig) -> Arc<Hub> {
        let started_unix_ms = unix_ms();
        let ring = Ring::new(cfg.max_steps, cfg.max_bytes, cfg.max_step_bytes);
        Arc::new(Hub {
            rt,
            bridge: Arc::downgrade(bridge),
            epoch: started_unix_ms.rotate_left(17) ^ u64::from(std::process::id()),
            started: Instant::now(),
            started_unix_ms,
            active: AtomicBool::new(false),
            seq: AtomicU64::new(0),
            last_txn: AtomicU64::new(0),
            counters: Counters::default(),
            clients: Mutex::new(Vec::new()),
            held: Mutex::new(HashSet::new()),
            ring: Mutex::new(ring),
            ports: Mutex::new(HashMap::new()),
            announced: Mutex::new(Vec::new()),
            events: Mutex::new(None),
            worker: Mutex::new(None),
            lifecycle: Mutex::new(()),
            stopped: AtomicBool::new(false),
            restored_from: AtomicU32::new(0),
            last_live: AtomicU64::new(u64::MAX),
            cfg,
        })
    }

    /// Whether a page is attached (the bridge routes only then).
    pub(crate) fn is_active(&self) -> bool {
        self.active.load(Ordering::Acquire)
    }

    /// Whether the hub keeps store `handle` observed.
    pub(crate) fn holds(&self, handle: u64) -> bool {
        self.held.lock().contains(&handle)
    }

    fn at_ms(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn note(&self, level: u8, message: &str) {
        self.rt.log(level, TARGET, message);
    }

    fn send_event(&self, event: Event) {
        if let Some(tx) = self.events.lock().as_ref() {
            let _ = tx.send(event);
        }
    }

    // ----- pages ---------------------------------------------------------------------------

    fn welcome(&self) -> ServerMsg {
        let config = self.rt.config();
        ServerMsg::Welcome(Welcome {
            protocol: PROTOCOL,
            undra_version: crate::UNDRA_VERSION.to_owned(),
            schema_hash: self.rt.schema_hash(),
            platform: config.platform.clone(),
            mode: config.mode.clone(),
            core_epoch: self.epoch,
            started_unix_ms: self.started_unix_ms,
            ring_steps: u32::try_from(self.cfg.max_steps).unwrap_or(u32::MAX),
            ring_bytes: self.cfg.max_bytes as u64,
            ring_step_bytes: self.cfg.max_step_bytes as u64,
            // The page reads names and types, not doc comments: the schema without them is a fraction of the size.
            schema_json: self.rt.schema().without_docs().to_json(),
        })
    }

    fn app_message(&self) -> ServerMsg {
        let client = self.bridge.upgrade().and_then(|b| b.client());
        ServerMsg::App {
            connected: client.is_some(),
            platform: client.map(|c| c.platform).unwrap_or_default(),
        }
    }

    /// Adds a page. Sends it the welcome first; the worker then syncs it. `false` when the hub
    /// is full or stopped (the caller closes the connection).
    pub(crate) fn attach(self: &Arc<Self>, conn: &Arc<Conn>) -> bool {
        let _life = self.lifecycle.lock();
        if self.stopped.load(Ordering::Acquire) {
            return false;
        }
        if self.clients.lock().len() >= self.cfg.max_clients {
            return false;
        }
        send_msg(conn, &self.welcome());
        let first = {
            let mut clients = self.clients.lock();
            clients.push(conn.clone());
            clients.len() == 1
        };
        if first && !self.start_worker() {
            self.clients.lock().retain(|c| c.id != conn.id);
            return false;
        }
        self.send_event(Event::Sync(conn.id));
        self.note(INFO, &format!("a devtools page attached ({} open)", self.clients.lock().len()));
        true
    }

    /// Removes a page; the last one out stops the recording.
    pub(crate) fn detach(self: &Arc<Self>, id: u64) {
        let _life = self.lifecycle.lock();
        let left = {
            let mut clients = self.clients.lock();
            let before = clients.len();
            clients.retain(|c| c.id != id);
            if clients.len() == before {
                return;
            }
            clients.len()
        };
        self.note(INFO, &format!("a devtools page left ({left} open)"));
        if left == 0 {
            // The hub stays active (the bridge keeps filtering) until the runtime observes only
            // what the app client asked for again.
            self.stop_worker();
            self.release_observation();
            self.active.store(false, Ordering::Release);
            self.ring.lock().clear();
            self.ports.lock().clear();
            self.announced.lock().clear();
        }
    }

    /// Stops everything for good (the server is stopping or suspending): pages are closed, the
    /// worker is joined, the observation is undone.
    pub(crate) fn shutdown(self: &Arc<Self>, reason: &str) {
        let _life = self.lifecycle.lock();
        self.stopped.store(true, Ordering::Release);
        let clients = std::mem::take(&mut *self.clients.lock());
        for c in &clients {
            c.close(crate::ws::close::GOING_AWAY, reason);
        }
        self.stop_worker();
        if !clients.is_empty() {
            self.release_observation();
        }
        self.active.store(false, Ordering::Release);
    }

    fn start_worker(self: &Arc<Self>) -> bool {
        let (tx, rx) = channel();
        *self.events.lock() = Some(tx);
        self.active.store(true, Ordering::Release);
        let hub = self.clone();
        match thread::Builder::new()
            .name("undra-devtools".to_owned())
            .spawn(move || hub.work(&rx))
        {
            Ok(handle) => {
                *self.worker.lock() = Some(handle);
                true
            }
            Err(e) => {
                self.active.store(false, Ordering::Release);
                *self.events.lock() = None;
                self.note(WARN, &format!("could not start the devtools thread: {e}"));
                false
            }
        }
    }

    fn stop_worker(&self) {
        self.send_event(Event::Stop);
        *self.events.lock() = None;
        if let Some(handle) = self.worker.lock().take() {
            let _ = handle.join();
        }
    }

    /// Undoes the hub's observation: the runtime's observed set goes back to what the app client
    /// asked for. Switching a store's observation off is all-or-nothing, so the app's own signals
    /// are observed again, and the client is sent their current values like any `Observe` (a
    /// write in the instant between the two steps is then not lost; the values it already has
    /// arrive again, which a mirror applies as no change).
    fn release_observation(&self) {
        let held: Vec<u64> = std::mem::take(&mut *self.held.lock()).into_iter().collect();
        let app = self.bridge.upgrade().and_then(|b| b.current());
        let _restate = RouteGuard::set(Route::AppObserve);
        for handle in held {
            if self.rt.objects().type_of(Handle(handle)).is_err() {
                continue;
            }
            {
                // Off: nobody gets anything.
                let _off = RouteGuard::set(Route::Silent);
                self.rt.observe(handle, u32::MAX, false);
            }
            if let Some(app) = &app {
                for signal in app.observed_signals(handle) {
                    self.rt.observe(handle, signal, true);
                }
            }
        }
    }

    // ----- what the bridge and the sessions tell the hub -----------------------------------

    fn broadcast_bytes(&self, bytes: &[u8], only: Option<u64>) {
        let clients = self.clients.lock();
        for c in clients.iter().filter(|c| only.is_none_or(|id| id == c.id)) {
            c.send_plain(bytes.to_vec());
        }
    }

    fn broadcast(&self, msg: &ServerMsg) {
        let mut w = Writer::new();
        msg.encode(&mut w);
        self.broadcast_bytes(w.as_slice(), None);
    }

    /// A change-set the runtime delivered, routed as `route` says.
    pub(crate) fn on_change_set(&self, payload: &[u8], route: Route) {
        let (delivery, cause, only) = match route {
            Route::Commit(cause) => (Delivery::Commit, cause, None),
            Route::Initial { only } => (Delivery::Initial, Cause::Other, only),
            Route::AppObserve | Route::Silent => return,
        };
        let seq = self.seq.fetch_add(1, Ordering::AcqRel) + 1;
        if delivery == Delivery::Commit {
            let count = payload.get(8..12).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
            if let Some(txn) = payload.get(..8) {
                let mut raw = [0_u8; 8];
                raw.copy_from_slice(txn);
                self.last_txn.store(u64::from_le_bytes(raw), Ordering::Release);
            }
            self.counters.commits.fetch_add(1, Ordering::Relaxed);
            self.counters.entries.fetch_add(u64::from(count), Ordering::Relaxed);
            self.counters
                .change_set_bytes
                .fetch_add(payload.len() as u64, Ordering::Relaxed);
        }
        let mut w = Writer::with_capacity(40 + payload.len());
        encode_change_set(&mut w, seq, self.at_ms(), delivery, cause, payload);
        self.broadcast_bytes(w.as_slice(), only);
        if delivery == Delivery::Commit {
            self.send_event(Event::Commit);
        }
    }

    /// The core asked the client to run a port method.
    pub(crate) fn port_start(&self, id: u32, port_id: u32, method_id: u32, args: &[u8]) {
        self.counters.port_calls.fetch_add(1, Ordering::Relaxed);
        {
            let mut ports = self.ports.lock();
            // A call the platform never answers stays open; keep the table bounded (a page that is
            // open for hours against an app that drops its replies must not grow the server).
            if ports.len() >= MAX_OPEN_PORT_CALLS {
                ports.retain(|_, open| open.started.elapsed() < Duration::from_secs(60));
            }
            ports.insert(
                id,
                OpenPort {
                    started: Instant::now(),
                    port_id,
                    method_id,
                },
            );
        }
        let mut w = Writer::with_capacity(32 + args.len());
        encode_port_start(&mut w, id, port_id, method_id, self.at_ms(), args);
        self.broadcast_bytes(w.as_slice(), None);
    }

    /// The core made a port call while no app client was attached: it was answered `Unavailable`.
    pub(crate) fn port_unattended(&self) {
        self.counters.unattended_port_calls.fetch_add(1, Ordering::Relaxed);
    }

    /// A port call ended (`status`: 0 ok, 1 typed error, 2 unavailable).
    pub(crate) fn port_end(&self, id: u32, status: u8, reply: &[u8]) {
        let Some(open) = self.ports.lock().remove(&id) else {
            return;
        };
        let latency = u64::try_from(open.started.elapsed().as_micros()).unwrap_or(u64::MAX);
        let mut w = Writer::with_capacity(48 + reply.len());
        encode_port_end(&mut w, id, open.port_id, open.method_id, self.at_ms(), status, latency, reply);
        self.broadcast_bytes(w.as_slice(), None);
    }

    /// The app slot was taken or vacated.
    pub(crate) fn app_changed(&self) {
        if self.is_active() {
            self.broadcast(&self.app_message());
        }
    }

    /// A message from a page.
    pub(crate) fn on_client(&self, conn: u64, msg: ClientMsg) {
        match msg {
            ClientMsg::Restore { request_id, step } => {
                self.send_event(Event::Restore { request_id, step });
            }
            ClientMsg::Resync => self.send_event(Event::Sync(conn)),
        }
    }

    // ----- the worker ----------------------------------------------------------------------

    fn work(self: &Arc<Self>, rx: &Receiver<Event>) {
        // A panic here must not take the dev server with it (R6); the pages notice the silence.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.work_loop(rx)));
        if outcome.is_err() {
            self.note(WARN, "the devtools thread panicked; the page stops updating");
            self.active.store(false, Ordering::Release);
        }
    }

    fn work_loop(self: &Arc<Self>, rx: &Receiver<Event>) {
        let mut due: Option<Instant> = None;
        let mut last_stats = Instant::now().checked_sub(STATS_EVERY).unwrap_or_else(Instant::now);
        let mut queries = QuerySampler::default();
        // How long to coalesce a burst before the snapshot: at least `COALESCE`, and long enough
        // that the snapshots take at most a tenth of the worker's time (they hold the core lock).
        // The state the page attached to is step 1: the first thing it can travel back to.
        let mut gap = COALESCE.max(self.capture_timed()).min(MAX_GAP);
        loop {
            let wait = due.map_or(TICK, |at| at.saturating_duration_since(Instant::now()));
            match rx.recv_timeout(wait) {
                Ok(Event::Stop) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(Event::Commit) => {
                    due.get_or_insert_with(|| Instant::now() + gap);
                }
                Ok(Event::Sync(conn)) => self.sync_client(conn),
                Ok(Event::Restore { request_id, step }) => self.travel(request_id, step),
                Err(RecvTimeoutError::Timeout) => {}
            }
            if due.is_some_and(|at| Instant::now() >= at) {
                due = None;
                gap = COALESCE.max(self.capture_timed()).min(MAX_GAP);
            }
            if due.is_none() {
                // The backstop: a store built while nothing committed is noticed here, and a
                // query that changed without a commit (its gc, its staleness) is sampled.
                if self.live_stores_changed() {
                    self.capture();
                }
                self.sample_queries(&mut queries);
                if last_stats.elapsed() >= STATS_EVERY {
                    last_stats = Instant::now();
                    self.broadcast(&ServerMsg::Stats(self.stats_json()));
                }
            }
        }
    }

    /// [`capture`](Self::capture), and the gap the next burst should be coalesced over: `DUTY`
    /// times what this took.
    fn capture_timed(&self) -> Duration {
        let started = Instant::now();
        self.capture();
        started.elapsed().saturating_mul(DUTY)
    }

    fn live_stores_changed(&self) -> bool {
        let live = json_u64(&self.rt.stats_json(), "live_stores").unwrap_or(0);
        self.last_live.swap(live, Ordering::AcqRel) != live
    }

    /// The stores in `snapshot`, in handle order.
    fn stores_of(bytes: &[u8]) -> Option<Vec<StoreRef>> {
        let snapshot = Snapshot::decode(&mut Reader::new(bytes)).ok()?;
        let mut stores: Vec<StoreRef> = snapshot
            .stores
            .iter()
            .map(|s| StoreRef {
                handle: s.handle.0,
                type_id: s.type_id,
            })
            .collect();
        stores.sort_by_key(|s| s.handle);
        Some(stores)
    }

    /// Starts observing the stores the hub does not hold yet, forgets the ones that are gone and
    /// tells the pages when the set changed. The values of a store the hub starts to observe go
    /// to every page as state.
    fn observe_stores(&self, stores: &[StoreRef]) {
        let fresh: Vec<u64> = {
            let mut held = self.held.lock();
            let now: HashSet<u64> = stores.iter().map(|s| s.handle).collect();
            held.retain(|h| now.contains(h));
            stores.iter().map(|s| s.handle).filter(|h| held.insert(*h)).collect()
        };
        // The pages learn what the stores are before they get the stores' values.
        let changed = {
            let mut announced = self.announced.lock();
            if announced.as_slice() == stores {
                false
            } else {
                *announced = stores.to_vec();
                true
            }
        };
        if changed {
            self.broadcast(&ServerMsg::Stores(stores.to_vec()));
        }
        for handle in fresh {
            let _initial = RouteGuard::set(Route::Initial { only: None });
            self.rt.observe(handle, u32::MAX, true);
        }
    }

    /// Takes a snapshot: finds new stores, and when the state differs from the newest step adds
    /// a step to the ring and tells the pages.
    fn capture(&self) {
        let through = self.seq.load(Ordering::Acquire);
        let txn = self.last_txn.load(Ordering::Acquire);
        let bytes = self.rt.snapshot();
        let Some(stores) = Self::stores_of(&bytes) else {
            self.note(WARN, "a snapshot did not decode; no step was taken");
            return;
        };
        self.observe_stores(&stores);
        let restored_from = self.restored_from.swap(0, Ordering::AcqRel);
        let pushed = {
            let mut ring = self.ring.lock();
            if ring.newest_bytes().is_some_and(|b| b.as_slice() == bytes.as_slice()) {
                self.counters.skipped_same.fetch_add(1, Ordering::Relaxed);
                return;
            }
            ring.push(
                StepInfo {
                    step: 0,
                    through_seq: through,
                    txn,
                    at_ms: self.at_ms(),
                    bytes: 0,
                    stores: u32::try_from(stores.len()).unwrap_or(u32::MAX),
                    restorable: false,
                    restored_from,
                },
                bytes,
            )
        };
        self.counters.steps.fetch_add(1, Ordering::Relaxed);
        self.broadcast(&ServerMsg::Step(pushed.info));
        if let Some(below_step) = pushed.evicted_below {
            self.broadcast(&ServerMsg::Evicted { below_step });
        }
    }

    /// Brings one page up to date: the stores, their values, the steps, who the app is.
    fn sync_client(&self, id: u64) {
        let Some(conn) = self.clients.lock().iter().find(|c| c.id == id).cloned() else {
            return;
        };
        let bytes = self.rt.snapshot();
        let stores = Self::stores_of(&bytes).unwrap_or_default();
        // Stores the hub already observes are re-observed for this page alone: the runtime
        // answers an observe with the current values.
        let known: Vec<u64> = {
            let held = self.held.lock();
            stores.iter().map(|s| s.handle).filter(|h| held.contains(h)).collect()
        };
        send_msg(&conn, &ServerMsg::Stores(stores.clone()));
        for handle in known {
            let _initial = RouteGuard::set(Route::Initial { only: Some(id) });
            self.rt.observe(handle, u32::MAX, true);
        }
        // Whatever is new is observed now, for every page (this one included).
        self.observe_stores(&stores);
        let steps: Vec<StepInfo> = self.ring.lock().infos().cloned().collect();
        for info in steps {
            send_msg(&conn, &ServerMsg::Step(info));
        }
        send_msg(&conn, &self.app_message());
        send_msg(&conn, &ServerMsg::Stats(self.stats_json()));
        if let Some(json) = self.rt.inspect("queries") {
            send_msg(&conn, &ServerMsg::Queries { at_ms: self.at_ms(), json });
        }
    }

    /// Time travel: restores the snapshot of `step` into the core.
    fn travel(&self, request_id: u32, step: u32) {
        let outcome = self.restore_step(step);
        let msg = match outcome {
            Ok(dropped) => {
                self.counters.travels.fetch_add(1, Ordering::Relaxed);
                self.note(INFO, &format!("time travel: restored step {step} ({dropped} store(s) dropped)"));
                self.notify_app(&if dropped == 0 {
                    format!("time travel: step {step}")
                } else {
                    format!("time travel: step {step} ({dropped} store(s) built since are gone)")
                });
                self.restored_from.store(step, Ordering::Release);
                // The restore's change-sets carry its new state; the step that records it is
                // taken now rather than after the burst, so it is labelled with its origin.
                self.capture();
                Traveled {
                    request_id,
                    ok: true,
                    step,
                    dropped,
                    message: if dropped == 0 {
                        format!("restored step {step}")
                    } else {
                        format!(
                            "restored step {step}; {dropped} store(s) built since are gone, and the app's references to them are stale"
                        )
                    },
                }
            }
            Err(message) => Traveled {
                request_id,
                ok: false,
                step,
                dropped: 0,
                message,
            },
        };
        self.broadcast(&ServerMsg::Traveled(msg));
    }

    fn restore_step(&self, step: u32) -> Result<u32, String> {
        let bytes = {
            let ring = self.ring.lock();
            match ring.get(step) {
                None => return Err(format!("step {step} is not in the history any more")),
                Some(s) => match &s.bytes {
                    None => {
                        return Err(format!(
                            "step {step} was not kept: its state ({} bytes) is over the limit of {} bytes",
                            s.info.bytes, self.cfg.max_step_bytes
                        ));
                    }
                    Some(bytes) => bytes.clone(),
                },
            }
        };
        let target: HashSet<u64> = Self::stores_of(&bytes)
            .ok_or_else(|| format!("step {step}'s snapshot is damaged"))?
            .into_iter()
            .map(|s| s.handle)
            .collect();
        let now = Self::stores_of(&self.rt.snapshot()).unwrap_or_default();
        let dropped = now.iter().filter(|s| !target.contains(&s.handle)).count();
        let result = {
            let _restore = RouteGuard::set(Route::Commit(Cause::Restore(step)));
            self.rt.restore(&bytes)
        };
        match result {
            Ok(()) => Ok(u32::try_from(dropped).unwrap_or(u32::MAX)),
            Err(e) => Err(format!("the core refused the snapshot of step {step}: {e}")),
        }
    }

    fn notify_app(&self, text: &str) {
        if let Some(conn) = self.bridge.upgrade().and_then(|b| b.current()) {
            conn.on_log(INFO, NOTICE_TARGET, text);
        }
    }

    /// Samples the query cache and sends it when it changed. At most once per `TICK`, and when a
    /// sample is slow (a big cache: it is built on this thread, and takes the cache's lock to copy
    /// the rows) the next waits `DUTY` times as long, so the cost stays a tenth of a core at most.
    fn sample_queries(&self, sampler: &mut QuerySampler) {
        if Instant::now() < sampler.not_before {
            return;
        }
        let started = Instant::now();
        let json = self.rt.inspect("queries");
        sampler.not_before = Instant::now() + TICK.max(started.elapsed().saturating_mul(DUTY));
        let Some(json) = json else {
            return;
        };
        if json != sampler.last {
            sampler.last.clone_from(&json);
            self.broadcast(&ServerMsg::Queries {
                at_ms: self.at_ms(),
                json,
            });
        }
    }

    /// The counters: the core's own (`undra_stats_json`) and the server's, as one JSON object. The
    /// page labels them as what they are: the *server's* view, not the app mirror's (SPEC 11.1).
    fn stats_json(&self) -> String {
        let c = &self.counters;
        let (ring_steps, ring_bytes) = {
            let ring = self.ring.lock();
            (ring.len(), ring.bytes())
        };
        let app = self.bridge.upgrade().and_then(|b| b.current());
        let commits = c.commits.load(Ordering::Relaxed);
        let steps = c.steps.load(Ordering::Relaxed);
        format!(
            "{{\"at_ms\":{},\"core\":{},\"server\":{{\"commits\":{commits},\"entries\":{},\"change_set_bytes\":{},\"steps\":{steps},\"unchanged_captures\":{},\"port_calls\":{},\"unattended_port_calls\":{},\"travels\":{},\"open_port_calls\":{},\"ring_steps\":{ring_steps},\"ring_bytes\":{ring_bytes},\"pages\":{},\"app_connected\":{},\"app_backlog_bytes\":{}}}}}",
            self.at_ms(),
            self.rt.stats_json(),
            c.entries.load(Ordering::Relaxed),
            c.change_set_bytes.load(Ordering::Relaxed),
            c.skipped_same.load(Ordering::Relaxed),
            c.port_calls.load(Ordering::Relaxed),
            c.unattended_port_calls.load(Ordering::Relaxed),
            c.travels.load(Ordering::Relaxed),
            self.ports.lock().len(),
            self.clients.lock().len(),
            app.is_some(),
            app.as_ref().map_or(0, |a| a.queued_bytes()),
        )
    }
}

/// What the worker remembers about sampling the query cache.
struct QuerySampler {
    /// The last document sent.
    last: String,
    /// The earliest the cache is read again.
    not_before: Instant,
}

impl Default for QuerySampler {
    fn default() -> QuerySampler {
        QuerySampler {
            last: String::new(),
            not_before: Instant::now(),
        }
    }
}

/// Encodes `msg` and queues it for one connection.
pub(crate) fn send_msg(conn: &Conn, msg: &ServerMsg) {
    let mut w = Writer::new();
    msg.encode(&mut w);
    conn.send_plain(w.into_vec());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_is_per_thread_and_restored_when_the_guard_drops() {
        assert_eq!(current_route(), Route::Commit(Cause::Other));
        {
            let _outer = RouteGuard::set(Route::Initial { only: Some(3) });
            assert_eq!(current_route(), Route::Initial { only: Some(3) });
            {
                let _inner = RouteGuard::set(Route::Silent);
                assert_eq!(current_route(), Route::Silent);
            }
            assert_eq!(current_route(), Route::Initial { only: Some(3) });
            std::thread::spawn(|| assert_eq!(current_route(), Route::Commit(Cause::Other)))
                .join()
                .unwrap();
        }
        assert_eq!(current_route(), Route::Commit(Cause::Other));
    }

    #[test]
    fn the_worker_and_the_observation_exist_only_while_a_page_is_attached() {
        use undra_runtime::RuntimeConfig;
        let bridge = Bridge::new();
        let rt = Runtime::new(
            RuntimeConfig {
                platform: "rust".into(),
                mode: "dev".into(),
                core_threads: 1,
                blocking_threads: 1,
                log_level: 0,
            },
            bridge.clone(),
        )
        .unwrap();
        let hub = Hub::new(rt.clone(), &bridge, DevtoolsConfig::new("0123456789abcdef", &[]));
        bridge.set_hub(Some(hub.clone()));
        let idle = |hub: &Hub| {
            hub.worker.lock().is_none()
                && hub.events.lock().is_none()
                && !hub.is_active()
                && hub.held.lock().is_empty()
                && hub.ring.lock().len() == 0
                && hub.ports.lock().is_empty()
        };
        assert!(idle(&hub), "a hub with no page owns no thread, no step and no observation");
        assert!(!bridge.devtools_attached());

        let (conn, _queue) = Conn::new(1, rt.schema_hash(), 1 << 20, None);
        let conn = Arc::new(conn);
        assert!(hub.attach(&conn));
        assert!(hub.worker.lock().is_some(), "the first page starts the worker");
        assert!(hub.is_active() && bridge.devtools_attached());
        // A second page does not start a second worker.
        let (second, _queue2) = Conn::new(2, rt.schema_hash(), 1 << 20, None);
        let second = Arc::new(second);
        assert!(hub.attach(&second));
        hub.detach(1);
        assert!(hub.worker.lock().is_some(), "one page is still attached");
        hub.detach(2);
        assert!(idle(&hub), "the last page out stops the worker and frees the ring");
        assert!(!bridge.devtools_attached());
        // Detaching a page that is not attached is a no-op; so is a second shutdown.
        hub.detach(2);
        hub.shutdown("test");
        hub.shutdown("test");
        assert!(idle(&hub));
        assert!(!hub.attach(&conn), "a stopped hub takes no page");
        bridge.set_hub(None);
        rt.shutdown();
    }

    #[test]
    fn a_counter_is_read_out_of_the_stats_document() {
        let text = "{\"live_handles\":4,\"live_stores\":12,\"tasks\":0}";
        assert_eq!(json_u64(text, "live_stores"), Some(12));
        assert_eq!(json_u64(text, "tasks"), Some(0));
        assert_eq!(json_u64(text, "nothing"), None);
    }
}
