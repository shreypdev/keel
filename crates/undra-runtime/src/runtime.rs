//! [`Runtime`]: the core lock, the call/reply machinery and every host-facing entry point
//! (SPEC 5, 6, 16.2). See `docs/runtime-internals.md` for the threading model as built.

use crate::atomic_update::cas_update;
use core::any::Any;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Once, Weak};
use std::time::Duration;

use parking_lot::{Mutex, MutexGuard, RwLock};
use undra_meta::{
    ClosureRoot, ClosureSignal, DispatchCall, DispatchFn, DispatchOutcome, Schema, StoresClosure,
    TypeClosure, TypeRef,
};
use undra_signals::ChangeSink;
use undra_wire::payload::{
    Call, CallTarget, PortReply, PortStatus, Reply, ReplyStatus, Snapshot, StoreSnapshot,
    StreamFailure, StreamFlag, StreamItem,
};
use undra_wire::{Handle, Reader, Writer};

use crate::blocking::{Blocking, BlockingTask, default_pool_size};
use crate::config::{
    DroppedStore, InitError, MODE_DEV, MODE_INPROC, RestoreError, RestoreReport, RuntimeConfig,
};
use crate::ctx::{Ctx, CtxScope, Gone, Lifeline, current_runtime};
use crate::dispatch::{DispatchBytes, DispatchResult, DispatchTable, E_REENTRANT, needs_async};
#[cfg(not(target_family = "wasm"))]
use crate::executor::Shared;
use crate::executor::{
    BATCH, BoxFuture, CancelOutcome, EndPoll, Executor, Notify, TaskId, TaskKind,
};
use crate::ext::{Extensions, InitHook, InspectFn, Inspectors};
use crate::guard::{self, PanicReport, drop_guarded, encode_panic_body};
use crate::host::{Host, PortCallOutcome};
use crate::lazy::LazyList;
use crate::log::{DEBUG, ERROR, FATAL, WARN};

/// The `port_call_id` of a fire-and-forget port call: no answer is expected (SPEC 6, host contract 6).
const FIRE_AND_FORGET: u32 = 0;
use crate::object::{AnyObject, StoreObject, StoreRestorer, UndraObject, erased, store};
use crate::object_table::{BadHandle, GENERATION_CEILING, ObjectTable};
use crate::persist::{self, RegisteredHooks};
use crate::ports::{
    Completion, Events, PortBinding, PortDispatch, PortDispatcher, PortError, PortFuture,
    PortTable, decode_dispatch_reply, decode_port_reply,
};
use crate::stats::{Stats, push_json_string};
use crate::sync_out::{self, SyncLease, Written};
use crate::timer::{Sleep, Timers, delay_ms};

static NEXT_RUNTIME_ID: AtomicU64 = AtomicU64::new(1);
static GLOBAL: Mutex<Option<Arc<Runtime>>> = Mutex::new(None);

/// Every live runtime of the process by id (ADR-035): how a change-set finds the runtime that
/// owns its store, whichever thread committed it. Registered when a runtime is built, removed in
/// its `Drop`; weak, so the registry never keeps one alive.
static RUNTIMES: RwLock<Option<HashMap<u64, Weak<Runtime>>>> = RwLock::new(None);

fn register_runtime(id: u64, runtime: Weak<Runtime>) {
    RUNTIMES
        .write()
        .get_or_insert_with(HashMap::new)
        .insert(id, runtime);
}

fn unregister_runtime(id: u64) {
    if let Some(map) = RUNTIMES.write().as_mut() {
        map.remove(&id);
    }
}

/// The live runtime with id `id`, if any.
fn runtime_by_id(id: u64) -> Option<Arc<Runtime>> {
    RUNTIMES.read().as_ref()?.get(&id)?.upgrade()
}

thread_local! {
    /// Ids of the runtimes whose core lock this thread currently holds.
    static HELD: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    /// Ids of the runtimes whose `Host` callback this thread is currently inside, innermost
    /// last (ADR-023, finding M2).
    static IN_HOST: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
    /// Nesting depth of `testing::unchecked_writes` scopes on this thread.
    static UNCHECKED_WRITES: Cell<u32> = const { Cell::new(0) };
    /// This thread created a `TestRuntime`, so it is that test's driver.
    static TEST_DRIVER: Cell<bool> = const { Cell::new(false) };
}

/// Lifts the write-context check on this thread until dropped (`testing::unchecked_writes`).
pub(crate) struct UncheckedWrites(());

impl UncheckedWrites {
    pub(crate) fn enter() -> UncheckedWrites {
        let _ = UNCHECKED_WRITES.try_with(|depth| depth.set(depth.get() + 1));
        UncheckedWrites(())
    }
}

impl Drop for UncheckedWrites {
    fn drop(&mut self) {
        let _ = UNCHECKED_WRITES.try_with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Records that the calling thread drives a `TestRuntime` (it created one): its direct signal
/// writes are the test's own and are allowed.
pub(crate) fn mark_test_driver_thread() {
    let _ = TEST_DRIVER.try_with(|driver| driver.set(true));
}

/// Marks the calling thread as running a `Host` callback of one runtime until it is dropped.
///
/// While the mark is set, the runtime's core-lock entry points refuse the thread with
/// `E_REENTRANT` exactly as they do for a thread that holds the core lock. The core lock alone is
/// not enough: a callback that runs on a thread that does not hold it (an off-core commit
/// delivering a change-set under a store's delivery lock) could otherwise wait for the core while
/// the core waits for that delivery lock.
pub(crate) struct HostCall {
    runtime: u64,
}

impl HostCall {
    /// Enters a callback of runtime `runtime`.
    pub(crate) fn enter(runtime: u64) -> HostCall {
        let _ = IN_HOST.try_with(|stack| stack.borrow_mut().push(runtime));
        HostCall { runtime }
    }

    /// Whether this thread is inside a host callback of runtime `runtime`.
    pub(crate) fn active(runtime: u64) -> bool {
        IN_HOST
            .try_with(|stack| stack.borrow().contains(&runtime))
            .unwrap_or(false)
    }
}

impl Drop for HostCall {
    fn drop(&mut self) {
        let runtime = self.runtime;
        let _ = IN_HOST.try_with(|stack| {
            let mut stack = stack.borrow_mut();
            if let Some(at) = stack.iter().rposition(|&id| id == runtime) {
                stack.remove(at);
            }
        });
    }
}

/// The call the runtime is running, for the FATAL record of a panic on wasm, where nothing can
/// catch the panic and so nothing at a guard can name it (ADR-046 decision 4.4). Only wasm sets it
/// (single-threaded; the cost is not paid on native, where the guards know).
static RUNNING: std::sync::Mutex<Option<CallTarget>> = std::sync::Mutex::new(None);

fn set_running(target: Option<CallTarget>) {
    if let Ok(mut running) = RUNNING.lock() {
        *running = target;
    }
}

/// What the runtime was running when it panicked on wasm, as `Todos.add`.
pub(crate) fn running_operation() -> Option<String> {
    let target = RUNNING.lock().ok().and_then(|running| *running)?;
    current_or_global().map(|rt| rt.operation_of(&target))
}

/// The runtime executing on this thread, else the global one.
pub(crate) fn current_or_global() -> Option<Arc<Runtime>> {
    current_runtime().or_else(|| GLOBAL.lock().clone())
}

/// Reports a panic contained where no runtime is at hand (a timer thread, a waker, a migration
/// hook): through the runtime this thread is in, else the global one. Nothing happens without one.
pub(crate) fn report_current(what: &str, operation: &str, report: &PanicReport) {
    if let Some(rt) = current_or_global() {
        rt.log_panic(what, operation, report);
    }
}

/// Logs a fatal record through the current runtime (the wasm panic hook).
pub(crate) fn log_fatal_current(target: &str, message: &str) {
    if let Some(rt) = current_or_global() {
        rt.log(FATAL, target, message);
    }
}

/// Routes every change-set committed by `undra-signals` to the runtime that **owns the store**
/// (ADR-035), whose host receives it: never to whichever runtime the committing thread happens to
/// be inside, and never to the global one by default. A store no runtime published (owner `0`)
/// delivers nothing.
struct RuntimeSink;

impl ChangeSink for RuntimeSink {
    /// Every commit names its owner ([`deliver_from`](ChangeSink::deliver_from)); a change-set
    /// without one has no runtime to go to.
    fn deliver(&self, _change_set: &[u8]) {}

    fn deliver_from(&self, owner: u64, change_set: &[u8]) {
        if owner == 0 {
            return;
        }
        if let Some(rt) = runtime_by_id(owner) {
            rt.deliver_change_set(change_set);
        }
    }

    /// A write the checker refused: logged at error level through the owning runtime (else the
    /// one the thread is in, else the global one) before the writer panics, so the host hears of
    /// it even when the panicking thread is not one the runtime watches.
    fn off_core_write(&self, owner: u64, message: &str) {
        let rt = if owner == 0 {
            current_or_global()
        } else {
            runtime_by_id(owner)
        };
        if let Some(rt) = rt {
            Stats::inc(&rt.stats.off_core_writes);
            rt.log(ERROR, "undra::signals", message);
        }
    }

    /// A computed panicked on its current inputs (ADR-019 amendment): it is held back while the
    /// rest of its store is delivered. Logged once at error level through the owning runtime,
    /// which marks the store poisoned and lists the signal in `stats_json`.
    fn computed_failed(&self, owner: u64, handle: u64, signal_id: u32, message: &str) {
        if let Some(rt) = runtime_by_id(owner) {
            rt.note_computed_failed(Handle(handle), signal_id, message);
        }
    }

    /// A computed that had failed evaluated again and its value was delivered.
    fn computed_recovered(&self, owner: u64, handle: u64, signal_id: u32) {
        if let Some(rt) = runtime_by_id(owner) {
            rt.note_computed_recovered(Handle(handle), signal_id);
        }
    }

    /// A commit hit `undra-signals`' round cap (effects re-triggering each other): logged at
    /// error level through the runtime the commit belongs to.
    fn round_cap_hit(&self, rounds: usize) {
        if let Some(rt) = current_or_global() {
            rt.log(
                ERROR,
                "undra::signals",
                &format!(
                    "effect loop hit the round cap: a commit was cut off after {rounds} rounds; \
                     the queued effects were dropped and the pending changes delivered"
                ),
            );
        }
    }
}

/// The write-context check installed into `undra-signals`: may the calling thread write a signal
/// of a store owned by runtime `owner` (`0`: a signal of no published store)?
///
/// An allowlist (ADR-023, ADR-035), evaluated **in every build**: yes on a thread that holds
/// **that** runtime's core lock (a dispatched call, a task poll, an event subscriber, `observe`,
/// `restore`, `Ctx::with_core`: everything entered through the runtime's entry points), on a
/// thread that holds any core lock when `owner` is `0`, on a `TestRuntime`'s driver thread (the
/// test's own direct writes) and inside `testing::unchecked_writes`. No on every other thread: a
/// blocking-pool worker, a host or embedder thread, a thread inside no runtime at all, the core of
/// another runtime. Such a write would be delivered without the core lock, unordered against the
/// core's transactions, to the wrong host, or not at all; it is refused before the value changes
/// (E0065). Signal writes belong on the core: send the result back to a task or a dispatched call,
/// or use `Ctx::with_core` on a host thread.
fn write_allowed(owner: u64) -> bool {
    UNCHECKED_WRITES
        .try_with(|depth| depth.get() > 0)
        .unwrap_or(false)
        || HELD
            .try_with(|held| {
                let held = held.borrow();
                if owner == 0 {
                    !held.is_empty()
                } else {
                    held.contains(&owner)
                }
            })
            .unwrap_or(false)
        || TEST_DRIVER.try_with(Cell::get).unwrap_or(false)
}

fn install_sink() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        undra_signals::set_sink(Arc::new(RuntimeSink));
        undra_signals::set_write_checker(write_allowed);
    });
}

/// What the core lock protects. It is deliberately tiny: the lock's job is mutual exclusion
/// of *user code* (dispatchers and task polls), not protection of runtime bookkeeping, which
/// has its own fine-grained locks because it must stay reachable from wakers, the timer
/// thread and host threads. See `docs/runtime-internals.md`.
#[derive(Default)]
pub(crate) struct CoreState {
    /// Executor turns run so far.
    turns: u64,
}

/// The calling thread may not enter the runtime: it already holds this runtime's core lock, or it
/// is inside one of this runtime's host callbacks (`E_REENTRANT`, SPEC 5.1). Waiting for the lock
/// there would deadlock, so the entry is refused instead. Returned by
/// [`Ctx::with_core`](crate::Ctx::with_core).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reentrant;

impl core::fmt::Display for Reentrant {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(E_REENTRANT)
    }
}

impl std::error::Error for Reentrant {}

/// Holds the core lock and makes the runtime current on this thread. Whoever holds one *is*
/// the core loop.
pub(crate) struct CoreGuard<'a> {
    guard: Option<MutexGuard<'a, CoreState>>,
    id: u64,
    scope: Option<CtxScope>,
}

impl CoreGuard<'_> {
    fn state(&mut self) -> Option<&mut CoreState> {
        self.guard.as_deref_mut()
    }

    fn exit(&mut self) {
        if self.scope.take().is_some() {
            let id = self.id;
            let _ = HELD.try_with(|held| {
                let mut held = held.borrow_mut();
                if let Some(pos) = held.iter().rposition(|&x| x == id) {
                    held.remove(pos);
                }
            });
        }
    }

    /// Unlocks, handing the lock to a waiting thread if there is one (used between executor
    /// turns so host calls are not starved).
    fn unlock_fair(mut self) {
        self.exit();
        if let Some(guard) = self.guard.take() {
            MutexGuard::unlock_fair(guard);
        }
    }
}

impl Drop for CoreGuard<'_> {
    fn drop(&mut self) {
        self.exit();
    }
}

/// A call whose completion is still to come.
struct CallEntry {
    task: TaskId,
    /// The handle of the receiver the call was made on (null for free functions and
    /// constructors): what a restore must check before the call may go on running.
    receiver: Handle,
    /// What the call is, to name it in a panic report (ADR-046).
    target: CallTarget,
    stream: Option<Arc<StreamState>>,
}

/// Credit accounting for one open stream (SPEC 3.7).
#[derive(Default)]
struct StreamState {
    credit: AtomicU32,
    notify: Notify,
}

impl StreamState {
    fn add_credit(&self, n: u32) {
        let _ = cas_update(&self.credit, Ordering::AcqRel, Ordering::Acquire, |c| {
            Some(c.saturating_add(n))
        });
        self.notify.notify_one();
    }

    fn try_take(&self) -> bool {
        cas_update(&self.credit, Ordering::AcqRel, Ordering::Acquire, |c| {
            c.checked_sub(1)
        })
        .is_ok()
    }
}

/// How the runtime is assembled; `Runtime::init` and `Runtime::new` use the defaults.
pub(crate) struct BuildOptions {
    /// No core thread and a manual clock (the blocking pool is real): the test runtime.
    pub manual: bool,
    /// Run the [`InitHook`]s.
    pub run_hooks: bool,
    /// Register as the process-global runtime.
    pub register_global: bool,
}

/// What `dispatch` found out about a call.
enum Dispatched {
    Done(DispatchResult, Handle),
    /// A synchronous answer written straight into the thread's reply slot (ADR-028): only
    /// produced while `call_sync_with` has the slot armed.
    Written,
    Panicked(PanicReport, Handle),
    Bad(String),
}

/// The reply of a synchronous call: the armed slot holding it, or an owned payload.
enum SyncReply {
    Slot(SyncLease),
    Owned(Vec<u8>),
}

/// The Undra runtime: one per process (or per embedded instance). See the
/// [crate documentation](crate).
pub struct Runtime {
    id: u64,
    weak: Weak<Runtime>,
    config: RuntimeConfig,
    dev: bool,
    host: Arc<dyn Host>,
    schema: Schema,
    schema_hash: u64,
    core: Mutex<CoreState>,
    objects: ObjectTable,
    exec: Executor,
    pub(crate) ports: Arc<PortTable>,
    events: Events,
    timers: Arc<Timers>,
    blocking: Blocking,
    table: DispatchTable,
    restorers: HashMap<u32, &'static StoreRestorer>,
    /// The fingerprint of a store type's signals (ADR-037), computed when a snapshot or a restore
    /// first needs it (only for the types it holds; a few, so a list, not a map).
    store_fingerprints: Mutex<Vec<(u32, u64)>>,
    /// The last snapshot description: for which store types, and its bytes (a snapshot of the
    /// same set of types reuses it).
    description: Mutex<Option<(Vec<u32>, Arc<str>)>>,
    /// The sections layered crates added to `stats_json`, one per name.
    stats_sections: Mutex<Vec<crate::ext::StatsSection>>,
    port_dispatchers: HashMap<u32, &'static PortDispatcher>,
    calls: Mutex<HashMap<u32, CallEntry>>,
    pub(crate) stats: Stats,
    /// The background tasks registered on this runtime (ADR-046).
    pub(crate) background: crate::background::Registry,
    extensions: Extensions,
    inspectors: Inspectors,
    shut_down: AtomicBool,
    /// How many times user code used this runtime after `shutdown` (only the first few are
    /// logged).
    late_uses: AtomicU32,
    /// Cancelled futures that could not be dropped on the spot without waiting for the core
    /// lock; dropped at the start of the next core turn.
    deferred_drops: Mutex<Vec<BoxFuture>>,
    core_thread: Mutex<Option<std::thread::JoinHandle<()>>>,
    /// Closed at the top of `shutdown` and in `Drop`: what `Ctx::closed`, `WeakCtx::closed` and
    /// `WeakCtx::sleep` wait on without holding the runtime (ADR-034).
    lifeline: Arc<Lifeline>,
    /// The computed signals currently held back because they panicked, as `(store handle,
    /// signal id)` (ADR-019 amendment): what `stats_json` reports as `poisoned_signals`.
    poisoned_signals: Mutex<HashSet<(u64, u32)>>,
}

/// Where an object lives: equal addresses are the same object.
fn object_address(object: &Arc<dyn AnyObject>) -> usize {
    Arc::as_ptr(object).cast::<()>() as usize
}

fn reply_payload(call_id: u32, status: ReplyStatus, body: &[u8]) -> Vec<u8> {
    let mut w = Writer::with_capacity(5 + body.len());
    Reply {
        call_id,
        status,
        body,
    }
    .encode(&mut w);
    w.into_vec()
}

fn encode_to_vec<T>(value: &T, encode: fn(&T, &mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    encode(value, &mut w);
    w.into_vec()
}

fn string_body(s: &str) -> Vec<u8> {
    let mut w = Writer::with_capacity(4 + s.len());
    w.write_str(s);
    w.into_vec()
}

fn stream_payload(call_id: u32, flag: StreamFlag, body: &[u8]) -> Vec<u8> {
    let mut w = Writer::with_capacity(5 + body.len());
    StreamItem {
        call_id,
        flag,
        body,
    }
    .encode(&mut w);
    w.into_vec()
}

/// The migrating half of a restore (ADR-037): the snapshot's description, parsed once when a
/// fingerprint differs, and the per-store conversion by name.
struct Migration<'a> {
    snapshot: &'a Snapshot,
    description: Option<Result<StoresClosure, String>>,
    /// Per store type: the snapshot's closure (checked against its fingerprint) and today's, or
    /// why there are none. Many stores share a type; this is worked out once.
    closures: Vec<(u32, OldAndNew)>,
}

/// A store type's closure in the snapshot and today's, or why there are none.
type OldAndNew = Result<Arc<(TypeClosure, TypeClosure)>, String>;

impl<'a> Migration<'a> {
    fn new(snapshot: &'a Snapshot) -> Migration<'a> {
        Migration {
            snapshot,
            description: None,
            closures: Vec::new(),
        }
    }

    /// The old and the new closure of `type_id`'s signals.
    #[inline(never)]
    fn closures(
        &mut self,
        rt: &Runtime,
        type_id: u32,
    ) -> Result<Arc<(TypeClosure, TypeClosure)>, String> {
        if let Some((_, found)) = self.closures.iter().find(|(id, _)| *id == type_id) {
            return found.clone();
        }
        let recorded = self.snapshot.fingerprint(type_id).unwrap_or(0);
        let found = (|| {
            let description = self.description(rt)?;
            let old = description
                .closure_of(type_id)
                .ok_or_else(|| "the snapshot does not describe this store type".to_owned())?;
            if old.fingerprint() != recorded {
                return Err(
                    "the snapshot's description does not match its fingerprint (the snapshot is damaged)"
                        .to_owned(),
                );
            }
            let new = rt
                .schema
                .store_closure(type_id)
                .ok_or_else(|| "this build has no such store".to_owned())?;
            Ok(Arc::new((old, new)))
        })();
        self.closures.push((type_id, found.clone()));
        found
    }

    /// The parsed description, or why it does not parse.
    fn description(&mut self, _rt: &Runtime) -> Result<&StoresClosure, String> {
        let snapshot = self.snapshot;
        self.description
            .get_or_insert_with(|| {
                StoresClosure::from_json(&snapshot.description)
                    .map_err(|e| format!("the snapshot's description does not parse: {e}"))
            })
            .as_ref()
            .map_err(Clone::clone)
    }

    /// The body `StoreObject::restore` expects (`signal_count u32, signals x { signal_id u32, len
    /// u32, value }`, today's ids), built by name from a record whose store type's fingerprint
    /// differs from today's.
    #[inline(never)]
    fn store_body(
        &mut self,
        rt: &Runtime,
        record: &StoreSnapshot,
    ) -> Result<Vec<u8>, RestoreError> {
        let type_id = record.type_id;
        let store = rt.store_name(type_id);
        let incompatible = |signal: &str, reason: String| RestoreError::Incompatible {
            type_id,
            store: store.clone(),
            signal: signal.to_owned(),
            reason,
        };
        let recorded = self.snapshot.fingerprint(type_id).unwrap_or(0);
        let pair = self
            .closures(rt, type_id)
            .map_err(|why| incompatible("", why))?;
        let (old, new) = (&pair.0, &pair.1);
        let (
            ClosureRoot::Signals {
                signals: old_signals,
            },
            ClosureRoot::Signals {
                signals: new_signals,
            },
        ) = (&old.root, &new.root)
        else {
            return Err(incompatible(
                "",
                "the description is not a store's".to_owned(),
            ));
        };
        let mut out: Vec<(u32, Vec<u8>)> = Vec::with_capacity(new_signals.len());
        for signal in new_signals {
            let stored = old_signals
                .iter()
                .find(|o| o.name == signal.name)
                .and_then(|o| {
                    record
                        .signals
                        .iter()
                        .find(|(id, _)| *id == o.signal_id)
                        .map(|(_, bytes)| (o, bytes.as_slice()))
                });
            let converted = match stored {
                Some((old_signal, bytes)) => {
                    convert_signal(&store, recorded, old_signal, bytes, old, signal, new)
                }
                None => missing_signal(&store, recorded, signal),
            };
            match converted {
                Ok(Some(bytes)) => out.push((signal.signal_id, bytes)),
                Ok(None) => {}
                Err(reason) => return Err(incompatible(&signal.name, reason)),
            }
        }
        let mut body = Writer::new();
        body.write_len(u32::try_from(out.len()).unwrap_or(u32::MAX));
        for (signal_id, value) in &out {
            body.write_u32(*signal_id);
            body.write_bytes(value);
        }
        Ok(body.into_vec())
    }
}

/// One signal the snapshot has, converted to today's type: structurally (with `ty` hooks inside),
/// else the store-and-signal hook, else the hook of its type (decision 5).
#[inline(never)]
fn convert_signal(
    store: &str,
    fingerprint: u64,
    old_signal: &ClosureSignal,
    bytes: &[u8],
    old: &TypeClosure,
    signal: &ClosureSignal,
    new: &TypeClosure,
) -> Result<Option<Vec<u8>>, String> {
    // Streamed, by name: the common case of an update allocates no value tree.
    let error = match persist::migrate(
        bytes,
        &old_signal.ty,
        old,
        &signal.ty,
        new,
        &RegisteredHooks,
    ) {
        Ok(bytes) => return Ok(Some(bytes)),
        Err(error) => error,
    };
    // Not structural: the store-and-signal hook, else the type's, gets the old value, decoded
    // (by the hook's own `support`: a core without hooks does not link the decoder).
    let hook = persist::signal_hook(store, &signal.name, fingerprint).or_else(|| {
        let from = old.narrowed(&old_signal.ty).fingerprint();
        persist::root_type_hook(&signal.ty, from)
    });
    let Some(hook) = hook else {
        return Err(error.to_string());
    };
    let hook = returning(hook, &signal.ty)?;
    (hook.support.offer)(hook, Some((bytes, &old_signal.ty, old)))
        .map(Some)
        .map_err(|e| e.to_string())
}

/// A signal the snapshot lacks: its `#[undra(default)]` (left out of the body, the generated
/// restore fills it), else the store-and-signal hook given `None`.
fn missing_signal(
    store: &str,
    fingerprint: u64,
    signal: &ClosureSignal,
) -> Result<Option<Vec<u8>>, String> {
    if signal.default {
        return Ok(None);
    }
    if let Some(hook) = persist::signal_hook(store, &signal.name, fingerprint) {
        let hook = returning(hook, &signal.ty)?;
        return (hook.support.offer)(hook, None)
            .map(Some)
            .map_err(|e| e.to_string());
    }
    Err(
        "the snapshot has no such signal and it has no `#[undra(default)]` or migration hook"
            .to_owned(),
    )
}

/// `hook`, if it returns `ty`. A store-and-signal hook's return type is not checked by the compiler
/// (the macro cannot see the store); the runtime reports a mismatch with an E0066 ERROR at start-up,
/// and a restore never splices its bytes into a signal of another type, where they could decode as
/// a wrong value of the same width (the misdecode ADR-037 exists to prevent).
fn returning(
    hook: &'static persist::Migration,
    ty: &TypeRef,
) -> Result<&'static persist::Migration, String> {
    match &hook.returns {
        Some(returns) if TypeRef::from(returns) != *ty => Err(format!(
            "the migration hook `{}` returns {} but the signal is {ty} (E0066)",
            hook.name,
            TypeRef::from(returns)
        )),
        _ => Ok(hook),
    }
}

impl Runtime {
    // ----- construction and lifecycle ----------------------------------------------------

    /// Creates the process-global runtime: builds the executor, object table, port table and
    /// dispatch table, installs the change sink, runs the [`InitHook`]s (query hydration) and
    /// starts the `undra-core` thread (unless `config.core_threads == 0` or on wasm, where the
    /// host drives [`poll`](Runtime::poll)).
    ///
    /// Fails with [`InitError::AlreadyInitialized`] while another global runtime is alive;
    /// [`shutdown`](Runtime::shutdown) releases it.
    pub fn init(config: RuntimeConfig, host: Arc<dyn Host>) -> Result<Arc<Runtime>, InitError> {
        Runtime::build(
            config,
            host,
            BuildOptions {
                manual: false,
                run_hooks: true,
                register_global: true,
            },
        )
    }

    /// Like [`init`](Runtime::init) but does not register the runtime as the global one, so
    /// any number can coexist (a transport server with one core per connection, tests).
    /// [`Runtime::global`] does not see it; its own calls and tasks still find it through
    /// [`Ctx::current`].
    pub fn new(config: RuntimeConfig, host: Arc<dyn Host>) -> Result<Arc<Runtime>, InitError> {
        Runtime::build(
            config,
            host,
            BuildOptions {
                manual: false,
                run_hooks: true,
                register_global: false,
            },
        )
    }

    pub(crate) fn build(
        config: RuntimeConfig,
        host: Arc<dyn Host>,
        opts: BuildOptions,
    ) -> Result<Arc<Runtime>, InitError> {
        let dev = match config.mode.as_str() {
            MODE_INPROC => false,
            MODE_DEV => true,
            other => return Err(InitError::InvalidMode(other.to_owned())),
        };
        if opts.register_global && GLOBAL.lock().is_some() {
            return Err(InitError::AlreadyInitialized);
        }
        guard::install_hook();
        install_sink();

        let inline = opts.manual || cfg!(target_family = "wasm") || config.core_threads == 0;
        let pool_size = match config.blocking_threads {
            0 => default_pool_size(),
            n => usize::from(n).min(64),
        };
        let schema = undra_meta::collect_schema("undra-core");
        let schema_hash = schema.hash();
        let table = DispatchTable::collect();
        let mut restorers: HashMap<u32, &'static StoreRestorer> = HashMap::new();
        for restorer in inventory::iter::<StoreRestorer> {
            restorers.entry(restorer.type_id).or_insert(restorer);
        }

        let mut port_dispatchers: HashMap<u32, &'static PortDispatcher> = HashMap::new();
        for dispatcher in inventory::iter::<PortDispatcher> {
            port_dispatchers
                .entry(dispatcher.port_id)
                .or_insert(dispatcher);
        }

        let id = NEXT_RUNTIME_ID.fetch_add(1, Ordering::Relaxed);
        let rt = Arc::new_cyclic(|weak| Runtime {
            id,
            weak: weak.clone(),
            dev,
            exec: Executor::new(id, host.clone(), inline),
            timers: Timers::new(opts.manual),
            // Test runtimes use the real pool too (ADR-023), so a test exercises the rule that a
            // blocking closure never writes signals and never runs on the core.
            blocking: Blocking::threaded(pool_size),
            host,
            config,
            schema,
            schema_hash,
            core: Mutex::new(CoreState::default()),
            // Test runtimes own their generation counter so the handles a test sees are the
            // same on every run; a real runtime shares the process-wide one (ADR-022).
            objects: if opts.manual {
                ObjectTable::isolated()
            } else {
                ObjectTable::new()
            },
            ports: Arc::new(PortTable::with_owner(weak.clone())),
            events: Events::default(),
            table,
            restorers,
            store_fingerprints: Mutex::new(Vec::new()),
            description: Mutex::new(None),
            stats_sections: Mutex::new(Vec::new()),
            port_dispatchers,
            calls: Mutex::new(HashMap::new()),
            stats: Stats::default(),
            background: crate::background::Registry::default(),
            extensions: Extensions::default(),
            inspectors: Inspectors::default(),
            shut_down: AtomicBool::new(false),
            late_uses: AtomicU32::new(0),
            deferred_drops: Mutex::new(Vec::new()),
            core_thread: Mutex::new(None),
            lifeline: Arc::new(Lifeline::default()),
            poisoned_signals: Mutex::new(HashSet::new()),
        });

        register_runtime(rt.id, Arc::downgrade(&rt));
        rt.objects.set_owner(rt.id);
        // `#[undra::migrate]` targets the macro could not check (ADR-037, E0066).
        for problem in persist::check_migrations(&rt.schema) {
            rt.log(ERROR, "undra::persist", &problem);
        }
        for (id, first, second) in &rt.table.collisions {
            rt.log(
                ERROR,
                "undra::runtime",
                &format!(
                    "dispatcher id {id:#010x} is claimed by both `{first}` and `{second}`; `{first}` wins"
                ),
            );
        }

        if !inline {
            rt.start_core_thread()?;
        }
        if opts.register_global {
            let mut global = GLOBAL.lock();
            if global.is_some() {
                drop(global);
                rt.shutdown();
                return Err(InitError::AlreadyInitialized);
            }
            *global = Some(rt.clone());
        }
        if opts.run_hooks {
            rt.run_init_hooks();
        }
        Ok(rt)
    }

    #[cfg(not(target_family = "wasm"))]
    fn start_core_thread(&self) -> Result<(), InitError> {
        let weak = self.weak.clone();
        let shared = self.exec.shared();
        let handle = std::thread::Builder::new()
            .name("undra-core".to_owned())
            .spawn(move || {
                let _alive = crate::testing::ThreadMark::enter();
                core_loop(&weak, &shared);
            })
            .map_err(|e| InitError::Spawn(e.to_string()))?;
        *self.core_thread.lock() = Some(handle);
        Ok(())
    }

    #[cfg(target_family = "wasm")]
    fn start_core_thread(&self) -> Result<(), InitError> {
        Ok(())
    }

    /// The process-global runtime created by [`Runtime::init`], if any.
    pub fn global() -> Option<Arc<Runtime>> {
        GLOBAL.lock().clone()
    }

    /// Runs the registered [`InitHook`]s, one per name (done automatically by `init` and `new`;
    /// the test runtime leaves it to the test, after it has bound its fakes).
    pub fn run_init_hooks(&self) {
        let ctx = self.ctx();
        let Ok(_guard) = self.enter_core() else {
            return;
        };
        let mut ran: Vec<&'static str> = Vec::new();
        for hook in inventory::iter::<InitHook> {
            if ran.contains(&hook.name) {
                continue;
            }
            ran.push(hook.name);
            if let Err(report) = guard::guarded(|| (hook.run)(&ctx)) {
                self.log_panic(
                    &format!("init hook `{}` panicked", hook.name),
                    &format!("init hook {}", hook.name),
                    &report,
                );
            }
        }
    }

    /// Stops the runtime and releases everything it holds (ADR-023, findings L1 and L5):
    ///
    /// 1. every call still in flight is answered with status 3 and every open stream ends with
    ///    a failed item (flag 3, status 3, `"the runtime shut down"`; ADR-036), each exactly once,
    ///    before anything slow is waited for, so a host that is waiting on a reply is released at
    ///    once;
    /// 2. the `undra-core`, timer and blocking threads are stopped and joined;
    /// 3. pending port calls fail with [`PortError::Cancelled`], event subscribers and
    ///    Rust port bindings are cleared (closures that hold a [`Ctx`] would keep the runtime
    ///    alive through a reference cycle), and every task and object is dropped under the core
    ///    lock, so user `Drop` code runs where it expects to;
    /// 4. the global slot is released.
    ///
    /// Idempotent. Calls made afterwards are answered with status 5, and `spawn`, `sleep`,
    /// `port_call` and `event` on a surviving [`Ctx`] are no-ops that log a warning (never a
    /// panic, never queued). [`Runtime::extension`] values are **not** cleared: they are handed
    /// out as `&T` for the life of the runtime, so an extension that holds a `Ctx` has to
    /// release it itself.
    ///
    /// # Must not be called from the core or a host callback
    ///
    /// Not from a dispatched call, a task, an event subscriber or a `Host` callback of this
    /// runtime: it joins the threads it stops, which would wait for itself (from the core
    /// thread) or for a job that is waiting for the core lock (from a blocking closure that
    /// took it). Debug builds assert this; release builds skip the joins that could never
    /// finish (the threads exit on their own) and carry on.
    pub fn shutdown(&self) {
        let inside = self.is_reentrant();
        debug_assert!(
            !inside,
            "undra-runtime: Runtime::shutdown was called from the core thread or from a host \
             callback, which cannot wait for the threads it stops (see its documentation)"
        );
        if self.shut_down.swap(true, Ordering::AcqRel) {
            return;
        }
        // Long-lived work waiting on `closed()` (or a `WeakCtx::sleep`) learns first.
        self.lifeline.close(Gone::ShutDown);
        // Refuses every task spawned from now on; the core thread leaves its loop.
        self.exec.shutdown();
        // Answer and cancel what is in flight, under the core lock (so no poll is running and
        // no call is half way through `dispatch`), before any thread is joined.
        {
            let guard = self.enter_core().ok();
            self.cancel_all_calls("the runtime shut down");
            drop(guard);
        }
        let core_thread = if inside {
            None
        } else {
            self.core_thread.lock().take()
        };
        if let Some(handle) = core_thread {
            if handle.thread().id() != std::thread::current().id() {
                let _ = handle.join();
            }
        }
        self.timers.shutdown();
        self.blocking.shutdown();
        self.ports.cancel_all();
        self.release_user_references();
        let _guard = self.enter_core().ok();
        self.teardown();
        drop(_guard);
        // Take the global reference out under the lock but drop it outside: dropping the
        // last reference runs `Drop`, which runs user destructors, which may log through
        // `current_or_global()` and would deadlock on this lock.
        let released = {
            let mut global = GLOBAL.lock();
            if global.as_ref().is_some_and(|g| g.id == self.id) {
                global.take()
            } else {
                None
            }
        };
        drop(released);
    }

    /// Clears what user code registered with the runtime and may hold a `Ctx` through: event
    /// subscribers and Rust port bindings. Dropped outside their locks, under the panic guard.
    fn release_user_references(&self) {
        for callback in self.events.clear() {
            self.drop_guarded_logged("an event subscriber at shutdown", callback);
        }
        for binding in self.ports.clear_bindings() {
            self.drop_guarded_logged("a port binding at shutdown", binding);
        }
    }

    /// Drops every task, call and object.
    fn teardown(&self) {
        self.drain_deferred_drops();
        for future in self.exec.clear() {
            self.drop_guarded_logged("a task at shutdown", future);
        }
        self.calls.lock().clear();
        for cleared in self.objects.clear() {
            if let Some(cell) = cleared.object.as_store() {
                cell.set_handle(0);
            }
            self.drop_guarded_logged("an object at shutdown", cleared.object);
        }
    }

    /// Whether [`shutdown`](Runtime::shutdown) has been called.
    pub fn is_shut_down(&self) -> bool {
        self.shut_down.load(Ordering::Acquire)
    }

    // ----- small accessors ---------------------------------------------------------------

    fn me(&self) -> Arc<Runtime> {
        match self.weak.upgrade() {
            Some(rt) => rt,
            // A `Runtime` only ever exists inside the `Arc` its constructors return, and no
            // caller can reach `&self` after the last strong reference is gone.
            None => unreachable!("undra-runtime: Runtime used while being dropped"),
        }
    }

    /// A [`Ctx`] for this runtime.
    pub fn ctx(&self) -> Ctx {
        Ctx(self.me())
    }

    /// The runtime's "it is over" signal (ADR-034).
    pub(crate) fn lifeline(&self) -> &Arc<Lifeline> {
        &self.lifeline
    }

    /// A process-unique id for this runtime instance.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// The configuration passed at creation.
    pub fn config(&self) -> &RuntimeConfig {
        &self.config
    }

    /// The schema collected from every registration linked into the process.
    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// The schema hash (`fnv1a64` of the canonical JSON), for the load-time check.
    pub fn schema_hash(&self) -> u64 {
        self.schema_hash
    }

    /// The object table.
    pub fn objects(&self) -> &ObjectTable {
        &self.objects
    }

    /// Host-to-core event subscriptions.
    pub fn events(&self) -> &Events {
        &self.events
    }

    /// The value of type `T` attached to this runtime, created with `T::default()` on first
    /// use: how `undra-query` keeps its `QueryClient` here.
    pub fn extension<T: Default + Send + Sync + 'static>(&self) -> &T {
        self.extensions.get_or_init(T::default)
    }

    /// The `T` of [`extension`](Runtime::extension), if something created it; never creates it.
    pub fn try_extension<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.extensions.get::<T>()
    }

    /// Adds a section to [`stats_json`](Runtime::stats_json) (one per name: a second section with
    /// a name already added is ignored). A layered crate calls it when it first keeps state on
    /// this runtime.
    pub fn add_stats_section(&self, section: crate::ext::StatsSection) {
        let mut sections = self.stats_sections.lock();
        if sections.iter().all(|s| s.name != section.name) {
            sections.push(section);
        }
    }

    /// Like [`extension`](Runtime::extension) with an explicit initializer (which may run
    /// more than once if threads race; only one result is kept).
    pub fn extension_with<T: Send + Sync + 'static>(&self, init: impl FnOnce() -> T) -> &T {
        self.extensions.get_or_init(init)
    }

    /// Registers an inspector: a function that describes part of the runtime's state as one JSON
    /// document, for dev tooling (the devtools page of `undra dev`, ADR-054). `undra-query`
    /// registers `"queries"` (its cache) the first time a cache exists. A later registration under
    /// the same name replaces the earlier. The core never reads inspectors; they hold the state
    /// they describe weakly, or the runtime they live in would never be freed (ADR-034).
    pub fn register_inspector(&self, name: &'static str, inspect: InspectFn) {
        self.inspectors.register(name, inspect);
    }

    /// The document inspector `name` produces now, or `None` when there is none or it panicked.
    ///
    /// The inspector runs on the calling thread, with no lock of the runtime held by this call
    /// (so it may take its own). A panic is contained (R6): it is logged once at level 5 with its
    /// backtrace and counted in `panics`, and the inspector is then skipped (this returns `None`)
    /// until a new one is registered under its name.
    pub fn inspect(&self, name: &str) -> Option<String> {
        match self.inspectors.inspect(name) {
            crate::ext::Answer::Document(document) => Some(document),
            crate::ext::Answer::Panicked(report) => {
                self.log_panic(
                    &format!("inspector `{name}` panicked and is skipped from now on"),
                    &format!("inspector {name}"),
                    &report,
                );
                None
            }
            crate::ext::Answer::None => None,
        }
    }

    /// The names of the registered inspectors.
    pub fn inspectors(&self) -> Vec<&'static str> {
        self.inspectors.names()
    }

    // ----- logging -----------------------------------------------------------------------

    /// Sends a record to the host if `level` is at least the configured level.
    pub fn log(&self, level: u8, target: &str, message: &str) {
        if level >= self.config.log_level {
            // A panicking `Host::log` must not take the runtime down, and there is nowhere
            // left to report it.
            let _ = guard::guarded(|| {
                let _call = HostCall::enter(self.id);
                self.host.log(level, target, message);
            });
        }
    }

    /// Runs a host callback under the panic guard: a host that panics is logged and the
    /// runtime carries on.
    fn guard_host<R>(&self, what: &str, f: impl FnOnce() -> R) -> Option<R> {
        let id = self.id;
        match guard::guarded(move || {
            // Every host callback is marked, so the host cannot re-enter this runtime from it.
            let _call = HostCall::enter(id);
            f()
        }) {
            Ok(value) => Some(value),
            Err(report) => {
                self.log_panic(&format!("{what} panicked"), what, &report);
                None
            }
        }
    }

    fn host_port_call(
        &self,
        port_id: u32,
        method_id: u32,
        port_call_id: u32,
        args: &[u8],
    ) -> PortCallOutcome {
        self.guard_host("Host::port_call", || {
            self.host.port_call(port_id, method_id, port_call_id, args)
        })
        .unwrap_or(PortCallOutcome::Unavailable)
    }

    /// A panic the runtime contained: the FATAL record, the counter, and the structured report
    /// for the app's crash reporter (ADR-046 decision 4). `what` is the sentence of the record,
    /// `operation` what was running (`Todos.add`, `task`).
    fn log_panic(&self, what: &str, operation: &str, report: &PanicReport) {
        Stats::inc(&self.stats.panics);
        self.log(
            FATAL,
            "undra::panic",
            &format!("{what}: {}\n{}", report.message, report.backtrace),
        );
        self.emit_report(operation, report);
    }

    /// Reports a panic an embedder contained at its own boundary (`undra-ffi`'s entries): the
    /// report names `entry` as the operation and carries no frames.
    pub fn report_boundary_panic(&self, entry: &str, message: &str) {
        Stats::inc(&self.stats.panics);
        let report = PanicReport {
            message: message.to_owned(),
            backtrace: String::new(),
            location: String::new(),
            thread: std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_owned(),
            frames: Vec::new(),
        };
        self.emit_report(entry, &report);
    }

    /// Hands the report of a contained panic to the `Diagnostics` port, fire and forget: to a
    /// Rust binding (a fake) if there is one, else to the platform with `port_call_id` 0, like a
    /// log record. A report that itself panics, or one made while a report is being delivered,
    /// is dropped: there is nowhere left to say so but the log, which already has it.
    fn emit_report(&self, operation: &str, report: &PanicReport) {
        thread_local! {
            static REPORTING: Cell<bool> = const { Cell::new(false) };
        }
        if REPORTING.with(|flag| flag.replace(true)) {
            return;
        }
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                let _ = REPORTING.try_with(|flag| flag.set(false));
            }
        }
        let _reset = Reset;
        Stats::inc(&self.stats.panic_reports);
        let bytes = crate::diagnostics::encode_report(report, operation, self.schema_hash);
        let (port, method) = (
            crate::diagnostics::DIAGNOSTICS_PORT,
            crate::diagnostics::PANICKED_METHOD,
        );
        match self.ports.binding(port) {
            PortBinding::Rust(imp, own) => {
                let _ = guard::guarded(|| self.dispatch_to_rust(&imp, own, port, method, &bytes));
            }
            PortBinding::Foreign => {
                let _ = self.host_port_call(port, method, FIRE_AND_FORGET, &bytes);
            }
        }
    }

    /// `verb` and the type of the store at `handle`, for a panic report: `observe Todos`.
    fn store_operation(&self, verb: &str, handle: Handle) -> String {
        match self.objects.type_of(handle) {
            Ok((_, type_name)) => format!("{verb} {type_name}"),
            Err(_) => verb.to_owned(),
        }
    }

    /// What `target` is, for a panic report: `Todos.add`, `add_later`, `Todos.new`.
    fn operation_of(&self, target: &CallTarget) -> String {
        let named = |name: &str, id: u32| {
            if name == "?" {
                format!("{id:#010x}")
            } else {
                name.to_owned()
            }
        };
        match *target {
            CallTarget::Function { method_id } => self
                .table
                .functions
                .get(&method_id)
                .map_or_else(|| format!("fn {method_id:#010x}"), |m| m.name.to_owned()),
            CallTarget::Method { handle, method_id } => match self.objects.type_of(handle) {
                Ok((type_id, type_name)) => {
                    let method = self
                        .table
                        .objects
                        .get(&type_id)
                        .map_or("?", |entry| entry.name_of(method_id, false));
                    format!("{type_name}.{}", named(method, method_id))
                }
                Err(_) => format!("method {method_id:#010x}"),
            },
            CallTarget::Constructor { type_id, method_id } => {
                match self.table.objects.get(&type_id) {
                    Some(entry) => format!(
                        "{}.{}",
                        entry.meta.name,
                        named(entry.name_of(method_id, true), method_id)
                    ),
                    None => format!("constructor {method_id:#010x}"),
                }
            }
            CallTarget::LazyPage { .. } => "lazy page".to_owned(),
        }
    }

    fn drop_guarded_logged<T>(&self, what: &str, value: T) {
        if let Err(report) = drop_guarded(value) {
            self.log_panic(
                &format!("dropping {what} panicked"),
                &format!("drop of {what}"),
                &report,
            );
        }
    }

    // ----- the core lock -----------------------------------------------------------------

    /// Whether this thread holds the core lock.
    fn holds_core(&self) -> bool {
        HELD.try_with(|held| held.borrow().contains(&self.id))
            .unwrap_or(false)
    }

    /// Whether the calling thread may not enter this runtime: it holds the core lock, or it is
    /// inside one of this runtime's host callbacks (whichever lock it holds there).
    fn is_reentrant(&self) -> bool {
        self.holds_core() || HostCall::active(self.id)
    }

    /// Takes the core lock and makes this runtime current on the thread. Fails, instead of
    /// deadlocking, when this thread already holds the lock or is inside a host callback of this
    /// runtime: that is the host (or user code) calling back into the runtime, which SPEC 5.1
    /// forbids (`E_REENTRANT`). The callback test covers host threads that do *not* hold the
    /// core lock, such as an off-core commit delivering a change-set (ADR-023).
    pub(crate) fn enter_core(&self) -> Result<CoreGuard<'_>, Reentrant> {
        if self.is_reentrant() {
            return Err(Reentrant);
        }
        let guard = self.core.lock();
        let _ = HELD.try_with(|held| held.borrow_mut().push(self.id));
        Ok(CoreGuard {
            guard: Some(guard),
            id: self.id,
            scope: Some(CtxScope::enter(self.me())),
        })
    }

    /// Runs `f` as the core on the calling thread: takes the core lock, makes the runtime
    /// current, runs `f` in one transaction and releases the lock. See [`Ctx::with_core`].
    pub(crate) fn with_core<R>(&self, f: impl FnOnce() -> R) -> Result<R, Reentrant> {
        let guard = self.enter_core()?;
        let out = undra_signals::txn(f);
        // The transaction committed (and delivered) inside the lock: in order with the core's.
        drop(guard);
        Ok(out)
    }

    /// Like [`enter_core`](Runtime::enter_core) but never waits: `None` if the thread may not
    /// enter or the lock is taken.
    fn try_enter_core(&self) -> Option<CoreGuard<'_>> {
        if self.is_reentrant() {
            return None;
        }
        let guard = self.core.try_lock()?;
        let _ = HELD.try_with(|held| held.borrow_mut().push(self.id));
        Some(CoreGuard {
            guard: Some(guard),
            id: self.id,
            scope: Some(CtxScope::enter(self.me())),
        })
    }

    fn reentrant(&self, entry_point: &str) {
        Stats::inc(&self.stats.bad_requests);
        self.log(
            ERROR,
            "undra::runtime",
            &format!("{entry_point}: {E_REENTRANT}"),
        );
    }

    // ----- host callbacks ----------------------------------------------------------------

    fn send_reply(&self, call_id: u32, status: ReplyStatus, body: &[u8]) {
        Stats::inc(&self.stats.replies);
        let payload = reply_payload(call_id, status, body);
        self.guard_host("Host::reply", || self.host.reply(call_id, &payload));
    }

    fn send_stream_item(&self, call_id: u32, flag: StreamFlag, body: &[u8]) {
        Stats::inc(&self.stats.stream_items);
        let payload = stream_payload(call_id, flag, body);
        self.guard_host("Host::stream_item", || {
            self.host.stream_item(call_id, &payload);
        });
    }

    /// Ends stream `call_id` with a [`StreamFlag::Failed`] item (ADR-036): the call failed with
    /// `status` (2 panicked, 3 cancelled by the core), in the reply-failure vocabulary, so the
    /// host maps it exactly as a failed reply. Never a flag-2 item, which carries only the
    /// stream's own `E`.
    fn send_stream_failure(&self, call_id: u32, status: ReplyStatus, message: &str, detail: &str) {
        let mut body = Writer::with_capacity(9 + message.len() + detail.len());
        StreamFailure {
            status,
            message,
            detail,
        }
        .encode(&mut body);
        self.send_stream_item(call_id, StreamFlag::Failed, body.as_slice());
    }

    pub(crate) fn deliver_change_set(&self, payload: &[u8]) {
        Stats::inc(&self.stats.change_sets);
        Stats::add(&self.stats.change_set_bytes, payload.len() as u64);
        if self.dev && payload.len() >= 12 {
            let mut txn = [0u8; 8];
            txn.copy_from_slice(&payload[..8]);
            let mut count = [0u8; 4];
            count.copy_from_slice(&payload[8..12]);
            self.log(
                DEBUG,
                "undra::devtools",
                &format!(
                    "commit txn={} entries={} bytes={}",
                    u64::from_le_bytes(txn),
                    u32::from_le_bytes(count),
                    payload.len()
                ),
            );
        }
        self.guard_host("Host::change_set", || self.host.change_set(payload));
    }

    /// Starts observing `signal_ids` of one store and hands the host their current values as one
    /// change-set: the single path that `observe` and `restore` share (ADR-023, findings M1/L9).
    ///
    /// The cell builds the entries and calls [`deliver_change_set`](Runtime::deliver_change_set)
    /// **under the store's delivery lock**, with the transaction id allocated there, so a commit
    /// of the same store on another thread cannot slip its newer values in front of these
    /// (older) ones. Callers run it inside `undra_signals::txn` (the writes of a computed that do
    /// not settle within the cell's passes then commit after the delivery, and the host converges
    /// on the core's values) and under the panic guard.
    fn deliver_observed(&self, cell: &undra_signals::StoreCell, signal_ids: &[u32]) -> u32 {
        cell.observe_and_deliver(signal_ids, |payload| self.deliver_change_set(payload))
    }

    // ----- calls -------------------------------------------------------------------------

    /// Serves a call (SPEC 3.3) and replies through [`Host::reply`]. Sync methods reply before
    /// this returns; async methods and streams are spawned and reply later.
    ///
    /// Returns `0` when the call was accepted (a reply will follow, for anything from success
    /// to status 2 or 5) and `5` when it was rejected without a reply: an undecodable payload
    /// (no `call_id` to answer), `call_id == 0`, a `call_id` that is already in flight, a
    /// shut-down runtime, or a re-entrant call.
    pub fn call(&self, payload: &[u8]) -> u32 {
        Stats::inc(&self.stats.calls);
        let call = match Call::decode(&mut Reader::new(payload)) {
            Ok(call) => call,
            Err(e) => {
                Stats::inc(&self.stats.bad_requests);
                self.log(
                    WARN,
                    "undra::runtime",
                    &format!("call: malformed payload: {e}"),
                );
                return 5;
            }
        };
        let call_id = call.call_id;
        if call_id == 0 {
            Stats::inc(&self.stats.bad_requests);
            self.log(WARN, "undra::runtime", "call: call_id 0 is reserved");
            return 5;
        }
        if self.is_shut_down() {
            Stats::inc(&self.stats.bad_requests);
            return 5;
        }
        let Ok(_guard) = self.enter_core() else {
            self.reentrant("call");
            return 5;
        };
        // `shutdown` tears down under this lock, so once we hold it the answer is final.
        if self.is_shut_down() {
            Stats::inc(&self.stats.bad_requests);
            return 5;
        }
        if self.calls.lock().contains_key(&call_id) {
            Stats::inc(&self.stats.bad_requests);
            self.log(
                WARN,
                "undra::runtime",
                &format!("call: call_id {call_id} is already in flight"),
            );
            return 5;
        }
        match self.dispatch(&call, false) {
            Dispatched::Bad(reason) => self.reply_bad(call_id, &reason),
            Dispatched::Panicked(report, handle) => {
                self.reply_panic(call_id, handle, &call.target, &report);
            }
            Dispatched::Done(result, handle) => match result {
                DispatchResult::Sync(Ok(body)) => self.send_reply(call_id, ReplyStatus::Ok, &body),
                DispatchResult::Sync(Err(body)) => {
                    self.send_reply(call_id, ReplyStatus::Error, &body);
                }
                DispatchResult::Async(future) => {
                    self.spawn_call(call_id, handle, call.target, future);
                }
                DispatchResult::Stream(stream) => {
                    self.open_stream(call_id, handle, call.target, stream);
                }
                DispatchResult::Unknown | DispatchResult::BadRequest(_) => {
                    // `dispatch` maps both to `Dispatched::Bad`.
                    self.reply_bad(call_id, "internal: unmapped dispatch result");
                }
            },
            // `call` never arms the reply slot, so no dispatcher can have written into it.
            Dispatched::Written => self.reply_bad(call_id, "internal: unmapped dispatch result"),
        }
        0
    }

    /// Serves a call synchronously and returns the `Reply` payload (SPEC 3.4). Only for sync
    /// methods: an `async` method or a stream is answered with status 5 without being run.
    /// The reply is *not* passed to [`Host::reply`].
    ///
    /// The returned `Vec` is the one allocation of the call, made at the very end; the work
    /// before it does not touch the heap once the thread's reply buffer has warmed up. A caller
    /// that can read the reply in place (a shim copying it into its own buffer) avoids even that
    /// with [`call_sync_with`](Runtime::call_sync_with).
    pub fn call_sync(&self, payload: &[u8]) -> Vec<u8> {
        match self.serve_sync(payload) {
            SyncReply::Slot(lease) => lease.read(<[u8]>::to_vec),
            // Already an owned payload (an error reply): no second copy.
            SyncReply::Owned(bytes) => bytes,
        }
    }

    /// Like [`call_sync`](Runtime::call_sync), but lends the `Reply` payload to `read` instead of
    /// returning it: no heap allocation on the hot path (ADR-028).
    ///
    /// The bytes are valid only inside `read`, which runs after the core lock is released, on
    /// the calling thread. `read` may call the runtime again (that call allocates its reply
    /// instead of reusing the buffer being read) and may panic (the panic propagates, the
    /// thread's buffer is released).
    ///
    /// ```
    /// use undra_runtime::Runtime;
    /// use undra_runtime::testing::TestRuntime;
    ///
    /// let rt = TestRuntime::new();
    /// // A payload the runtime cannot decode is answered with status 5, like any other reply.
    /// let status = rt.runtime().call_sync_with(&[1, 2, 3], |reply: &[u8]| reply[4]);
    /// assert_eq!(status, 5);
    /// ```
    pub fn call_sync_with<R>(&self, payload: &[u8], read: impl FnOnce(&[u8]) -> R) -> R {
        match self.serve_sync(payload) {
            SyncReply::Slot(lease) => lease.read(read),
            SyncReply::Owned(bytes) => read(&bytes),
        }
    }

    /// Runs a synchronous call under the core lock and returns where its reply is.
    fn serve_sync(&self, payload: &[u8]) -> SyncReply {
        Stats::inc(&self.stats.calls);
        Stats::inc(&self.stats.replies);
        let call = match Call::decode(&mut Reader::new(payload)) {
            Ok(call) => call,
            Err(e) => {
                Stats::inc(&self.stats.bad_requests);
                return SyncReply::Owned(reply_payload(
                    0,
                    ReplyStatus::BadRequest,
                    &string_body(&format!("malformed call payload: {e}")),
                ));
            }
        };
        let call_id = call.call_id;
        let bad = |reason: &str| {
            Stats::inc(&self.stats.bad_requests);
            SyncReply::Owned(reply_payload(
                call_id,
                ReplyStatus::BadRequest,
                &string_body(reason),
            ))
        };
        if self.is_shut_down() {
            return bad("the runtime is shut down");
        }
        let Ok(_guard) = self.enter_core() else {
            self.reentrant("call_sync");
            return bad(E_REENTRANT);
        };
        if self.is_shut_down() {
            return bad("the runtime is shut down");
        }
        // Armed for the dispatcher only; `None` (a call made from inside another call's reply
        // reader) means the dispatcher takes the allocating path.
        let lease = SyncLease::acquire(self.id, call_id);
        match self.dispatch(&call, true) {
            Dispatched::Written => match lease {
                Some(lease) => SyncReply::Slot(lease),
                None => bad("internal: a dispatcher wrote a reply nobody asked for"),
            },
            Dispatched::Bad(reason) => bad(&reason),
            Dispatched::Panicked(report, handle) => {
                self.note_panic(
                    "call_sync",
                    &self.operation_of(&call.target),
                    handle,
                    &report,
                );
                SyncReply::Owned(reply_payload(
                    call_id,
                    ReplyStatus::Panic,
                    &encode_panic_body(&report),
                ))
            }
            Dispatched::Done(result, _) => match result {
                DispatchResult::Sync(Ok(body)) => {
                    SyncReply::Owned(reply_payload(call_id, ReplyStatus::Ok, &body))
                }
                DispatchResult::Sync(Err(body)) => {
                    SyncReply::Owned(reply_payload(call_id, ReplyStatus::Error, &body))
                }
                other => {
                    // A dispatcher that disagrees with its metadata about being async.
                    self.drop_guarded_logged("an unused dispatch result", other);
                    bad("this method is asynchronous; call it with call(), not call_sync()")
                }
            },
        }
    }

    /// Answers a synchronous method with `Ok(value)`: what a generated dispatcher returns for
    /// the success of a method that is not `async` and does not return a stream (status 0).
    /// `encode` is the value's wire encoder, `Encode::encode` for any wire type.
    ///
    /// Under [`call_sync`](Runtime::call_sync) the complete reply is encoded straight into the
    /// thread's reply buffer and the outcome is a zero-sized marker, so no heap allocation
    /// happens (ADR-028). Anywhere else (a `call`, a layer, a test calling the dispatcher
    /// directly) the value is encoded into a `Vec` and the outcome is
    /// [`DispatchResult::Sync`]`(Ok(..))`, exactly as a hand-written dispatcher would build it.
    /// The two are byte-identical on the wire.
    ///
    /// ```
    /// use undra_runtime::undra_wire::Encode;
    /// use undra_runtime::testing::TestRuntime;
    /// use undra_runtime::DispatchResult;
    ///
    /// let t = TestRuntime::new();
    /// // A generated dispatcher ends with `rt.sync_ok(&value, Encode::encode)`. Outside
    /// // `call_sync` no reply slot is armed, so the answer is the classic result:
    /// let outcome = t.runtime().sync_ok(&42_u32, Encode::encode);
    /// match outcome.downcast::<DispatchResult>() {
    ///     Ok(DispatchResult::Sync(Ok(bytes))) => assert_eq!(bytes, [42, 0, 0, 0]),
    ///     _ => unreachable!("not armed, so a DispatchResult::Sync"),
    /// }
    /// ```
    #[inline]
    pub fn sync_ok<T>(&self, value: &T, encode: fn(&T, &mut Writer)) -> DispatchOutcome {
        if sync_out::write(self.id, ReplyStatus::Ok, value, encode) {
            DispatchOutcome::new(Written::new())
        } else {
            DispatchOutcome::new(DispatchResult::Sync(Ok(encode_to_vec(value, encode))))
        }
    }

    /// Answers a synchronous method with its typed error (status 1); the counterpart of
    /// [`sync_ok`](Runtime::sync_ok) for `Err(error)`.
    #[inline]
    pub fn sync_err<E>(&self, error: &E, encode: fn(&E, &mut Writer)) -> DispatchOutcome {
        if sync_out::write(self.id, ReplyStatus::Error, error, encode) {
            DispatchOutcome::new(Written::new())
        } else {
            DispatchOutcome::new(DispatchResult::Sync(Err(encode_to_vec(error, encode))))
        }
    }

    /// Finds the dispatcher for `call` and runs it under the panic guard. The caller holds
    /// the core lock. `sync_only` rejects async-shaped methods (by their metadata) before
    /// running anything.
    fn dispatch(&self, call: &Call<'_>, sync_only: bool) -> Dispatched {
        if cfg!(target_family = "wasm") {
            set_running(Some(call.target));
            let dispatched = self.dispatch_inner(call, sync_only);
            set_running(None);
            dispatched
        } else {
            self.dispatch_inner(call, sync_only)
        }
    }

    fn dispatch_inner(&self, call: &Call<'_>, sync_only: bool) -> Dispatched {
        let async_reason = |name: &str| {
            Dispatched::Bad(format!(
                "`{name}` is asynchronous; call it with call(), not call_sync()"
            ))
        };
        // The route is the generated dispatcher the static table names, or, when the table has
        // no entry, the reason to report if no layer serves the call either.
        let (route, method_id, handle): (Result<DispatchFn, String>, u32, Handle) = match call
            .target
        {
            CallTarget::Function { method_id } => match self.table.functions.get(&method_id) {
                Some(meta) => {
                    if sync_only && needs_async(meta.is_async, &meta.returns) {
                        return async_reason(meta.name);
                    }
                    (Ok(meta.dispatch), method_id, Handle::NULL)
                }
                None => (
                    Err(format!("unknown function {method_id:#010x}")),
                    method_id,
                    Handle::NULL,
                ),
            },
            CallTarget::Method { handle, method_id } => {
                let (type_id, type_name) = match self.objects.type_of(handle) {
                    Ok(found) => found,
                    Err(e) => return Dispatched::Bad(e.to_string()),
                };
                match self.table.objects.get(&type_id) {
                    Some(entry) => {
                        if sync_only && entry.method_needs_async(method_id) {
                            return async_reason(entry.name_of(method_id, false));
                        }
                        (Ok(entry.meta.dispatch), method_id, handle)
                    }
                    None => (
                        Err(format!(
                            "no dispatcher is registered for `{type_name}` ({type_id:#010x})"
                        )),
                        method_id,
                        handle,
                    ),
                }
            }
            CallTarget::Constructor { type_id, method_id } => {
                match self.table.objects.get(&type_id) {
                    Some(entry) => {
                        if sync_only && entry.constructor_needs_async(method_id) {
                            return async_reason(entry.name_of(method_id, true));
                        }
                        (Ok(entry.meta.dispatch), method_id, Handle::NULL)
                    }
                    None => (
                        Err(format!("unknown object type {type_id:#010x}")),
                        method_id,
                        Handle::NULL,
                    ),
                }
            }
            CallTarget::LazyPage {
                handle,
                offset,
                limit,
            } => {
                return match self.objects.get::<LazyList>(handle) {
                    Ok(list) => {
                        Dispatched::Done(DispatchResult::Sync(Ok(list.page(offset, limit))), handle)
                    }
                    Err(e) => Dispatched::Bad(e.to_string()),
                };
            }
        };
        let dispatch_call = DispatchCall {
            method_id,
            call_id: call.call_id,
            handle: handle.0,
            args: call.args,
        };
        match route {
            Ok(dispatch_fn) => self
                .run_dispatcher("generated", dispatch_fn, dispatch_call, handle, false)
                .unwrap_or_else(|| Dispatched::Bad("internal: unrouted call".to_owned())),
            Err(miss) => {
                for layer in &self.table.layers {
                    if let Some(done) =
                        self.run_dispatcher(layer.name, layer.dispatch, dispatch_call, handle, true)
                    {
                        return done;
                    }
                }
                Dispatched::Bad(miss)
            }
        }
    }

    /// Runs one dispatcher under the panic guard and classifies what it answered. A layer
    /// (`layered`) that answers `Unknown` does not serve the id: `None` lets the next one try.
    fn run_dispatcher(
        &self,
        name: &str,
        dispatch_fn: DispatchFn,
        call: DispatchCall<'_>,
        handle: Handle,
        layered: bool,
    ) -> Option<Dispatched> {
        match guard::guarded(|| dispatch_fn(self as &dyn Any, call)) {
            Ok(outcome) => match outcome.downcast::<Written>() {
                Ok(Written { .. }) => Some(Dispatched::Written),
                Err(outcome) => self.classify(name, outcome, call, handle, layered),
            },
            Err(report) => Some(Dispatched::Panicked(report, handle)),
        }
    }

    /// Classifies an outcome that is not a written reply: a [`DispatchResult`], or something a
    /// dispatcher should never have returned.
    fn classify(
        &self,
        name: &str,
        outcome: DispatchOutcome,
        call: DispatchCall<'_>,
        handle: Handle,
        layered: bool,
    ) -> Option<Dispatched> {
        match outcome.downcast::<DispatchResult>() {
            Ok(DispatchResult::BadRequest(reason)) => Some(Dispatched::Bad(reason)),
            Ok(DispatchResult::Unknown) if layered => None,
            Ok(DispatchResult::Unknown) => Some(Dispatched::Bad(format!(
                "unknown method {:#010x}, or its arguments or receiver were not valid",
                call.method_id
            ))),
            Ok(result) => Some(Dispatched::Done(result, handle)),
            Err(_) => Some(Dispatched::Bad(format!(
                "the {name} dispatcher returned something other than an undra_runtime::DispatchResult"
            ))),
        }
    }

    fn reply_bad(&self, call_id: u32, reason: &str) {
        Stats::inc(&self.stats.bad_requests);
        self.send_reply(call_id, ReplyStatus::BadRequest, &string_body(reason));
    }

    /// A computed of the store at `handle` panicked and is held back (ADR-019 amendment): an
    /// error log naming the store and the signal, the store marked poisoned, the signal listed.
    pub(crate) fn note_computed_failed(&self, handle: Handle, signal_id: u32, message: &str) {
        Stats::inc(&self.stats.panics);
        self.objects.mark_poisoned(handle);
        self.poisoned_signals.lock().insert((handle.0, signal_id));
        let store = self
            .objects
            .type_of(handle)
            .map_or("a store", |(_, name)| name);
        self.log(
            ERROR,
            "undra::signals",
            &format!(
                "computed signal {signal_id} of `{store}` ({handle:?}) panicked: {message}; it is held back \
                 (the host keeps the last value it received, every other signal of the store is still \
                 delivered) and evaluated again when its inputs change"
            ),
        );
        let signal = self
            .schema
            .objects
            .iter()
            .find(|o| o.name == store)
            .and_then(|o| o.store.as_ref())
            .and_then(|s| s.signals.iter().find(|s| s.signal_id == signal_id))
            .map_or_else(|| signal_id.to_string(), |s| s.name.clone());
        let report = guard::caught_elsewhere(message);
        self.emit_report(&format!("computed {store}.{signal}"), &report);
    }

    /// A held-back computed evaluated again and was delivered.
    pub(crate) fn note_computed_recovered(&self, handle: Handle, signal_id: u32) {
        self.poisoned_signals.lock().remove(&(handle.0, signal_id));
        self.log(
            crate::log::INFO,
            "undra::signals",
            &format!(
                "computed signal {signal_id} of {handle:?} evaluates again; its value was delivered"
            ),
        );
    }

    /// Accounts for a caught panic: log level 5, counters, store poisoning.
    fn note_panic(&self, what: &str, operation: &str, handle: Handle, report: &PanicReport) {
        self.log_panic(&format!("{what} panicked"), operation, report);
        if !handle.is_null() {
            self.objects.mark_poisoned(handle);
        }
    }

    fn reply_panic(&self, call_id: u32, handle: Handle, target: &CallTarget, report: &PanicReport) {
        self.note_panic("call", &self.operation_of(target), handle, report);
        self.send_reply(call_id, ReplyStatus::Panic, &encode_panic_body(report));
    }

    fn spawn_call(
        &self,
        call_id: u32,
        handle: Handle,
        target: CallTarget,
        future: Pin<Box<dyn Future<Output = DispatchBytes> + Send>>,
    ) {
        // The task holds the runtime weakly (ADR-034): the executor owns the task, so a strong
        // reference here would be a cycle that keeps a dropped runtime alive while the call is
        // in flight. It upgrades only to reply; a runtime that is gone has answered the call
        // already (shutdown and `Drop` answer every call in flight).
        let rt = self.weak.clone();
        let spawned = self.exec.try_spawn(
            Box::pin(async move {
                let result = future.await;
                if let Some(rt) = rt.upgrade() {
                    rt.finish_call(call_id, result);
                }
            }),
            TaskKind::Call { call_id, handle },
        );
        let task = match spawned {
            Ok(task) => task,
            Err(refused) => {
                // The runtime began shutting down after `call` checked: nothing will run this
                // call, so it is answered as cancelled rather than left silent.
                self.drop_guarded_logged("a call refused at shutdown", refused);
                self.send_reply(call_id, ReplyStatus::Cancelled, &[]);
                return;
            }
        };
        self.calls.lock().insert(
            call_id,
            CallEntry {
                task,
                receiver: handle,
                target,
                stream: None,
            },
        );
    }

    /// A call's task finished with `result`: reply, unless the call was cancelled meanwhile.
    fn finish_call(&self, call_id: u32, result: DispatchBytes) {
        if self.calls.lock().remove(&call_id).is_none() {
            return;
        }
        match result {
            Ok(body) => self.send_reply(call_id, ReplyStatus::Ok, &body),
            Err(body) => self.send_reply(call_id, ReplyStatus::Error, &body),
        }
    }

    fn open_stream(
        &self,
        call_id: u32,
        handle: Handle,
        target: CallTarget,
        stream: Pin<Box<dyn futures_core::Stream<Item = DispatchBytes> + Send>>,
    ) {
        let state = Arc::new(StreamState::default());
        let spawned = self.exec.try_spawn(
            Box::pin(drive_stream(
                self.weak.clone(),
                call_id,
                stream,
                state.clone(),
            )),
            TaskKind::Stream { call_id, handle },
        );
        let task = match spawned {
            Ok(task) => task,
            Err(refused) => {
                self.drop_guarded_logged("a stream refused at shutdown", refused);
                self.send_reply(call_id, ReplyStatus::Cancelled, &[]);
                return;
            }
        };
        // The call must be registered *before* the host hears status 4: `stream_credit` does
        // not take the core lock, so a host that grants credit the moment it sees the reply
        // must find the stream. The driver cannot run yet (we hold the core lock), so the
        // reply still precedes the first item.
        self.calls.lock().insert(
            call_id,
            CallEntry {
                task,
                receiver: handle,
                target,
                stream: Some(state),
            },
        );
        self.send_reply(call_id, ReplyStatus::StreamOpened, &[]);
    }

    /// Cancels an in-flight call: the task is dropped and, for an ordinary call, the reply is
    /// status 3. Cancelling a stream closes it without a further message (the host already
    /// knows). Unknown or finished ids are ignored.
    pub fn cancel(&self, call_id: u32) {
        let Ok(_guard) = self.enter_core() else {
            self.reentrant("cancel");
            return;
        };
        let Some(entry) = self.calls.lock().remove(&call_id) else {
            return;
        };
        Stats::inc(&self.stats.cancelled);
        if let CancelOutcome::Dropped(future) = self.exec.cancel(entry.task) {
            self.drop_guarded_logged("a cancelled task", future);
        }
        if entry.stream.is_none() {
            self.send_reply(call_id, ReplyStatus::Cancelled, &[]);
        }
    }

    /// Cancels the in-flight calls and streams whose receiver a restore replaced or invalidated:
    /// a plain call is answered with status 3, a stream ends with an error item saying so, and
    /// the task is dropped, each exactly once (the call table is the gate). Calls with no
    /// receiver, and calls on an object that is still the one its handle names, go on.
    ///
    /// `before` maps every handle that was live before the restore to its object's address.
    /// A call is affected when its handle was live before or is live now and does not name the
    /// same object in both (after a restore that is every call on a store, since each one is
    /// rebuilt); a call on an object that had already been released, whose handle the restore did
    /// not touch, is not.
    fn cancel_calls_replaced_by_restore(&self, before: &HashMap<u64, usize>) {
        let affected: Vec<u32> = {
            let calls = self.calls.lock();
            calls
                .iter()
                .filter(|(_, entry)| !entry.receiver.is_null())
                .filter(|(_, entry)| {
                    let was = before.get(&entry.receiver.0).copied();
                    let now = self
                        .objects
                        .get_dyn(entry.receiver)
                        .ok()
                        .map(|object| object_address(&object));
                    (was.is_some() || now.is_some()) && was != now
                })
                .map(|(&call_id, _)| call_id)
                .collect()
        };
        for call_id in affected {
            self.abort_call(
                call_id,
                "the object it was running on was replaced by a restore",
            );
        }
    }

    /// Ends every in-flight call and stream from the runtime's side ([`abort_call`](Runtime::abort_call)
    /// each). The caller holds the core lock (or is the thread that would).
    fn cancel_all_calls(&self, why: &str) {
        let ids: Vec<u32> = self.calls.lock().keys().copied().collect();
        for call_id in ids {
            self.abort_call(call_id, why);
        }
    }

    /// Ends in-flight call `call_id` from the runtime's side: drops its task and tells the host,
    /// exactly once (a call already answered, or cancelled by the host, is left alone). A plain
    /// call gets status 3 (cancelled); a stream gets a failed item (flag 3) with status 3 and
    /// `why` as its message (ADR-036), because the host did not ask for the end and a clean end
    /// would read as success. The caller holds the core lock (or is `Drop`).
    fn abort_call(&self, call_id: u32, why: &str) {
        let Some(entry) = self.calls.lock().remove(&call_id) else {
            return;
        };
        Stats::inc(&self.stats.cancelled);
        if let CancelOutcome::Dropped(future) = self.exec.cancel(entry.task) {
            self.drop_guarded_logged("a call cancelled by the runtime", future);
        }
        if entry.stream.is_some() {
            self.send_stream_failure(call_id, ReplyStatus::Cancelled, why, "");
        } else {
            self.send_reply(call_id, ReplyStatus::Cancelled, &[]);
        }
    }

    /// Grants a stream more credit. Never takes the core lock. Unknown ids are ignored.
    pub fn stream_credit(&self, call_id: u32, credit: u32) {
        let state = self
            .calls
            .lock()
            .get(&call_id)
            .and_then(|entry| entry.stream.clone());
        if let Some(state) = state {
            state.add_credit(credit);
        }
    }

    // ----- observation and objects -------------------------------------------------------

    /// Starts or stops observing a store's signal (`signal_id == u32::MAX` for all of them).
    /// Starting delivers the current values as a change-set through [`Host::change_set`]
    /// **before this returns**. Unknown handles and non-stores are logged and ignored.
    pub fn observe(&self, handle: u64, signal_id: u32, on: bool) {
        let Ok(_guard) = self.enter_core() else {
            self.reentrant("observe");
            return;
        };
        let handle = Handle(handle);
        let object = match self.objects.get_dyn(handle) {
            Ok(object) => object,
            Err(e) => {
                self.log(WARN, "undra::runtime", &format!("observe: {e}"));
                return;
            }
        };
        let Some(cell) = object.as_store() else {
            self.log(
                WARN,
                "undra::runtime",
                &format!("observe: `{}` is not a store", object.undra_type_name()),
            );
            return;
        };
        let signal_count = cell.signal_count();
        if !on {
            cell.observe(signal_id, false, &mut Writer::new());
            self.objects
                .with_observed(handle, |o| o.record(signal_id, false, signal_count));
            return;
        }
        // The transaction outlives the delivery: writes made by computed closures during
        // `observe` that do not settle within its pass cap commit after the entries went out,
        // so the host converges on the core's values instead of keeping the capped snapshot
        // (signals re-review R2). Entries and delivery are one step under the store's delivery
        // lock (ADR-023, M1).
        match guard::guarded(|| {
            undra_signals::txn(|| {
                self.deliver_observed(cell, &[signal_id]);
            })
        }) {
            // Recorded once the host has the values: a panic leaves nothing remembered that a
            // later restore would re-observe (review N5).
            Ok(()) => {
                self.objects
                    .with_observed(handle, |o| o.record(signal_id, true, signal_count));
            }
            Err(report) => {
                self.note_panic(
                    "observe",
                    &self.store_operation("observe", handle),
                    handle,
                    &report,
                );
            }
        }
    }

    /// Releases a handle. The object is dropped once no task holds it; a released store stops
    /// delivering change-sets. Stale handles are ignored.
    pub fn release(&self, handle: u64) {
        let Ok(_guard) = self.enter_core() else {
            self.reentrant("release");
            return;
        };
        match self.objects.release(Handle(handle)) {
            Ok(object) => {
                if let Some(cell) = object.as_store() {
                    cell.set_handle(0);
                }
                self.drop_guarded_logged("a released object", object);
            }
            Err(e) => self.log(DEBUG, "undra::runtime", &format!("release: {e}")),
        }
    }

    /// Stores `object` and returns its handle. Generated constructors call this (or
    /// [`insert_object`](Runtime::insert_object) / [`insert_store`](Runtime::insert_store)).
    pub fn insert(&self, object: Arc<dyn AnyObject>) -> Handle {
        self.objects.insert(object)
    }

    /// Stores an object and returns its handle: what a generated constructor calls.
    ///
    /// If `T` is a store (a [`StoreRestorer`] is registered for its type id) the runtime finds
    /// its [`StoreCell`](undra_signals::StoreCell) through the restorer's `cell` accessor and
    /// tells it its handle, so change-sets and snapshots work without the caller doing
    /// anything more.
    pub fn insert_object<T: UndraObject>(&self, object: Arc<T>) -> Handle {
        let cell = self.restorers.get(&T::TYPE_ID).map(|r| r.cell);
        self.objects
            .insert(erased(object, T::TYPE_ID, T::NAME, cell))
    }

    /// Stores a store and returns its handle; its cell learns the handle. Unlike
    /// [`insert_object`](Runtime::insert_object) this does not need a registered
    /// [`StoreRestorer`].
    pub fn insert_store<T: StoreObject>(&self, object: Arc<T>) -> Handle {
        self.objects.insert(store(object))
    }

    /// Stores a [`LazyList`] (sharing its state) and returns its handle, which platforms page
    /// through with `LazyPage` calls.
    pub fn insert_lazy_list(&self, list: &LazyList) -> Handle {
        self.insert_object(Arc::new(list.clone()))
    }

    /// Resolves a raw handle to a `T`: what a generated dispatcher does for its receiver.
    pub fn object<T: Send + Sync + 'static>(&self, handle: u64) -> Result<Arc<T>, BadHandle> {
        self.objects.get::<T>(Handle(handle))
    }

    // ----- executor ----------------------------------------------------------------------

    /// Spawns a detached task. After [`shutdown`](Runtime::shutdown) the future is dropped
    /// unpolled, a warning is logged and the returned id names nothing.
    pub fn spawn(&self, future: impl Future<Output = ()> + Send + 'static) -> TaskId {
        if self.is_shut_down() {
            self.warn_shut_down("spawn");
            self.drop_guarded_logged("a task spawned after shutdown", future);
            return TaskId::dead();
        }
        match self.exec.try_spawn(Box::pin(future), TaskKind::Detached) {
            Ok(id) => id,
            Err(refused) => {
                self.warn_shut_down("spawn");
                self.drop_guarded_logged("a task spawned after shutdown", refused);
                TaskId::dead()
            }
        }
    }

    /// Logs (for the first few uses only, so a loop that keeps calling cannot flood the host)
    /// that user code used a runtime that has been shut down. The call is a no-op.
    fn warn_shut_down(&self, what: &str) {
        const LOGGED: u32 = 8;
        let seen = self.late_uses.fetch_add(1, Ordering::Relaxed);
        if seen < LOGGED {
            let more = if seen + 1 == LOGGED {
                " (further warnings of this kind are suppressed)"
            } else {
                ""
            };
            self.log(
                WARN,
                "undra::runtime",
                &format!("{what}: the runtime is shut down; the call was ignored{more}"),
            );
        }
    }

    /// Cancels a task; see [`Ctx::cancel_task`].
    ///
    /// The cancelled future is dropped **on the core** (ADR-023, finding L6): user `Drop` code
    /// (stores, `PortFuture`s, anything the task captured) must not run concurrently with core
    /// user code. On the core already (inside a task or a dispatch) it is dropped on the spot;
    /// elsewhere it is dropped at once under the core lock if that is free, and otherwise
    /// queued and dropped at the start of the core's next turn, so this never waits for the
    /// core.
    pub fn cancel_task(&self, id: TaskId) {
        let CancelOutcome::Dropped(future) = self.exec.cancel(id) else {
            return;
        };
        if self.holds_core() {
            self.drop_guarded_logged("a cancelled task", future);
        } else if let Some(_guard) = self.try_enter_core() {
            self.drop_guarded_logged("a cancelled task", future);
        } else {
            self.deferred_drops.lock().push(future);
            // Wake the core so the drop is not left waiting for unrelated work.
            self.exec.nudge();
        }
    }

    /// Drops the futures `cancel_task` queued. The caller holds the core lock.
    fn drain_deferred_drops(&self) {
        let queued = std::mem::take(&mut *self.deferred_drops.lock());
        for future in queued {
            self.drop_guarded_logged("a cancelled task", future);
        }
    }

    /// Runs `f` on the blocking pool; see [`Ctx::spawn_blocking`].
    pub fn spawn_blocking<T: Send + 'static>(
        &self,
        f: impl FnOnce() -> T + Send + 'static,
    ) -> BlockingTask<T> {
        // Weak (ADR-034): a closure still waiting in the pool's queue does not keep a dropped
        // runtime alive; it becomes current for the closure while it runs.
        self.blocking.spawn(self.ctx().downgrade(), f)
    }

    /// Sleeps; see [`Ctx::sleep`]. After [`shutdown`](Runtime::shutdown) it completes at once
    /// (and logs a warning) instead of registering a timer nobody would fire.
    pub fn sleep(&self, duration: Duration) -> Sleep {
        if duration.is_zero() {
            return Sleep::ready();
        }
        if self.is_shut_down() {
            self.warn_shut_down("sleep");
            return Sleep::ready();
        }
        let (id, slot) = self.timers.register();
        let host_owns = self
            .guard_host("Host::timer_set", || {
                self.host.timer_set(id, delay_ms(duration))
            })
            .unwrap_or(false);
        if !host_owns && !self.timers.arm(id, duration) {
            self.log(
                WARN,
                "undra::runtime",
                "sleep: the host does not own timers and this platform has no internal timer; the sleep will never complete",
            );
        }
        Sleep::armed(self.timers.clone(), id, slot)
    }

    /// The host says timer `timer_id` is due. Never takes the core lock. Unknown ids (a
    /// sleep that was dropped) are ignored.
    pub fn timer_fired(&self, timer_id: u32) {
        self.timers.fire(timer_id);
    }

    /// Drives the executor for one turn: polls at most [`BATCH`] ready tasks, then, if more
    /// are ready, asks the host to call `poll` again ([`Host::schedule`]). This is how wasm
    /// and manually driven runtimes make progress.
    pub fn poll(&self) {
        if self.is_reentrant() {
            self.reentrant("poll");
            return;
        }
        let batch = self.exec.take_ready(BATCH);
        if !batch.is_empty() {
            self.run_batch(batch);
        }
        self.exec.reschedule_if_ready();
    }

    /// Polls until no task is ready, and returns how many polls that took. Tasks that are
    /// waiting for a timer, a port reply or credit stay parked. A task that re-wakes itself
    /// forever makes this run forever.
    pub fn run_pending(&self) -> usize {
        if self.is_reentrant() {
            self.reentrant("run_pending");
            return 0;
        }
        let mut polled = 0;
        loop {
            let batch = self.exec.take_ready(BATCH);
            if batch.is_empty() {
                return polled;
            }
            polled += batch.len();
            self.run_batch(batch);
        }
    }

    /// One turn of the core loop: lock, poll `ids`, unlock fairly.
    pub(crate) fn run_batch(&self, ids: Vec<TaskId>) {
        let Ok(mut guard) = self.enter_core() else {
            self.exec.requeue(ids);
            self.reentrant("executor turn");
            return;
        };
        if self.is_shut_down() {
            return;
        }
        self.drain_deferred_drops();
        if let Some(state) = guard.state() {
            state.turns += 1;
        }
        Stats::inc(&self.stats.turns);
        for id in ids {
            self.poll_one(id);
        }
        guard.unlock_fair();
    }

    fn poll_one(&self, id: TaskId) {
        let Some((mut future, waker, kind)) = self.exec.begin_poll(id) else {
            return;
        };
        Stats::inc(&self.stats.polls);
        let mut cx = Context::from_waker(&waker);
        if cfg!(target_family = "wasm") {
            let target = match kind {
                TaskKind::Call { call_id, .. } | TaskKind::Stream { call_id, .. } => {
                    self.calls.lock().get(&call_id).map(|entry| entry.target)
                }
                TaskKind::Detached => None,
            };
            set_running(target);
        }
        let polled = guard::guarded(|| future.as_mut().poll(&mut cx));
        if cfg!(target_family = "wasm") {
            set_running(None);
        }
        match polled {
            Ok(Poll::Pending) => {
                if let EndPoll::Gone(future) = self.exec.end_poll(id, Some(future)) {
                    self.drop_task_future(future);
                }
            }
            Ok(Poll::Ready(())) => {
                self.exec.end_poll(id, None);
                self.drop_task_future(Some(future));
            }
            Err(report) => {
                self.exec.end_poll(id, None);
                self.drop_task_future(Some(future));
                self.task_panicked(kind, &report);
            }
        }
    }

    fn drop_task_future(&self, future: Option<BoxFuture>) {
        if let Some(future) = future {
            self.drop_guarded_logged("a task", future);
        }
    }

    fn task_panicked(&self, kind: TaskKind, report: &PanicReport) {
        let operation = |call_id: u32| {
            let target = self.calls.lock().get(&call_id).map(|entry| entry.target);
            target.map_or_else(|| "async call".to_owned(), |t| self.operation_of(&t))
        };
        match kind {
            TaskKind::Detached => self.log_panic("a spawned task panicked", "task", report),
            TaskKind::Call { call_id, handle } => {
                self.note_panic("async call", &operation(call_id), handle, report);
                if self.calls.lock().remove(&call_id).is_some() {
                    self.send_reply(call_id, ReplyStatus::Panic, &encode_panic_body(report));
                }
            }
            TaskKind::Stream { call_id, handle } => {
                self.note_panic("stream", &operation(call_id), handle, report);
                if self.calls.lock().remove(&call_id).is_some() {
                    self.send_stream_failure(
                        call_id,
                        ReplyStatus::Panic,
                        &report.message,
                        &report.backtrace,
                    );
                }
            }
        }
    }

    // ----- ports and events --------------------------------------------------------------

    /// Binds a Rust implementation to a port id (fakes, built-ins).
    ///
    /// **Convention** (what `#[undra::port]` generates code for): `P` is the port trait
    /// (`dyn Http`) and `imp` is an `Arc<Arc<P>>` behind `dyn Any`, because an
    /// `Arc<dyn Any>` cannot be downcast to `Arc<dyn Trait>` but can be downcast to
    /// `Arc<Arc<dyn Trait>>`. [`bind_dyn_port`](Runtime::bind_dyn_port) does the wrapping for
    /// you. Typed code fetches the binding back with [`rust_port::<P>`](Runtime::rust_port); a raw
    /// [`port_call`](Runtime::port_call) reaches it through the [`PortDispatcher`] registered
    /// for the port id.
    pub fn bind_port<P: ?Sized + 'static>(&self, port_id: u32, imp: Arc<dyn Any + Send + Sync>) {
        self.ports.bind(port_id, PortBinding::Rust(imp, None));
    }

    /// Binds an implementation as the trait object `P`
    /// (`rt.bind_dyn_port::<dyn Clock>(port_id, Arc::new(FakeClock::new()))`), following the
    /// [`bind_port`](Runtime::bind_port) convention.
    pub fn bind_dyn_port<P: ?Sized + Send + Sync + 'static>(&self, port_id: u32, imp: Arc<P>) {
        self.ports
            .bind(port_id, PortBinding::Rust(Arc::new(imp), None));
    }

    /// Binds an implementation as the trait object `P`, like
    /// [`bind_dyn_port`](Runtime::bind_dyn_port), together with the dispatcher a raw
    /// [`port_call`](Runtime::port_call) on it goes through. That is how a Rust implementation of a
    /// standard port is bound so that the generated proxies reach it too
    /// (`rt.bind_dyn_port_with::<dyn Kv>(port_id, kv, &undra_ports::KV_DISPATCHER)`): the standard
    /// ports' dispatchers are linked only where they are used, not registered for every core
    /// (ADR-052). The typed accessors (`undra_ports::kv(&ctx)`) reach the binding either way.
    pub fn bind_dyn_port_with<P: ?Sized + Send + Sync + 'static>(
        &self,
        port_id: u32,
        imp: Arc<P>,
        dispatcher: &'static PortDispatcher,
    ) {
        self.ports
            .bind(port_id, PortBinding::Rust(Arc::new(imp), Some(dispatcher)));
    }

    /// Routes a port id to the platform again (through [`Host::port_call`]), replacing any
    /// Rust binding.
    pub fn bind_foreign_port(&self, port_id: u32) {
        self.ports.bind(port_id, PortBinding::Foreign);
    }

    /// Removes a Rust binding; the port is foreign again. Returns whether there was one.
    pub fn unbind_port(&self, port_id: u32) -> bool {
        self.ports.unbind(port_id)
    }

    /// The Rust binding of `port_id` as a `P` (`rt.rust_port::<dyn Http>(port_id)`), if one was
    /// bound following the [`bind_port`](Runtime::bind_port) convention (an `Arc<Arc<P>>`).
    /// `None` for a foreign port or a binding of another type.
    pub fn rust_port<P: ?Sized + Send + Sync + 'static>(&self, port_id: u32) -> Option<Arc<P>> {
        match self.ports.binding(port_id) {
            PortBinding::Rust(imp, _) => imp.downcast::<Arc<P>>().ok().map(|arc| Arc::clone(&*arc)),
            PortBinding::Foreign => None,
        }
    }

    /// Runs an encoded call on a Rust-bound port through the dispatcher it was bound with, else
    /// the one registered for the port id. `Unavailable` if there is neither.
    fn dispatch_to_rust(
        &self,
        imp: &Arc<dyn Any + Send + Sync>,
        own: Option<&'static PortDispatcher>,
        port_id: u32,
        method_id: u32,
        args: &[u8],
    ) -> PortDispatch {
        match own.or_else(|| self.port_dispatchers.get(&port_id).copied()) {
            Some(dispatcher) => (dispatcher.dispatch)(&**imp, method_id, args),
            None => PortDispatch::Sync(vec![2]),
        }
    }

    /// Calls a platform-implemented async port method (SPEC 5.7). The call is sent when this
    /// function is called; the future resolves when the host replies through
    /// [`port_reply`](Runtime::port_reply), or immediately if the host answered synchronously.
    ///
    /// After [`shutdown`](Runtime::shutdown) nothing is sent: the future resolves at once to
    /// [`PortError::Cancelled`] and a warning is logged.
    pub fn port_call(&self, port_id: u32, method_id: u32, args: Vec<u8>) -> PortFuture {
        if self.is_shut_down() {
            self.warn_shut_down("port_call");
            return PortFuture::ready(self.ports.clone(), Err(PortError::Cancelled));
        }
        Stats::inc(&self.stats.port_calls);
        if let PortBinding::Rust(imp, own) = self.ports.binding(port_id) {
            return match self.dispatch_to_rust(&imp, own, port_id, method_id, &args) {
                PortDispatch::Sync(bytes) => {
                    PortFuture::ready(self.ports.clone(), decode_dispatch_reply(&bytes))
                }
                PortDispatch::Async(future) => {
                    PortFuture::rust(Box::pin(
                        async move { decode_dispatch_reply(&future.await) },
                    ))
                }
            };
        }
        let (id, slot) = self.ports.begin(self.timers.now_ns());
        // Constructed before the host is called so that a panic in the host abandons the id.
        let future = PortFuture::new(slot, self.ports.clone(), id);
        if self.dev {
            self.log(
                DEBUG,
                "undra::devtools",
                &format!(
                    "port call {port_id:#010x}.{method_id:#010x} id={id} args={} bytes",
                    args.len()
                ),
            );
        }
        let outcome = self.host_port_call(port_id, method_id, id, &args);
        self.apply_port_outcome(id, outcome);
        future
    }

    /// Calls a platform-implemented sync port method. The host must answer synchronously; an
    /// `Async` or `Unavailable` answer is [`PortError::Unavailable`] and the call is
    /// abandoned. (An `Async` answer whose reply already arrived through
    /// [`port_reply`](Runtime::port_reply) during `Host::port_call` counts as synchronous.)
    pub fn port_call_sync(
        &self,
        port_id: u32,
        method_id: u32,
        args: &[u8],
    ) -> Result<Vec<u8>, PortError> {
        if self.is_shut_down() {
            self.warn_shut_down("port_call_sync");
            return Err(PortError::Cancelled);
        }
        Stats::inc(&self.stats.port_calls);
        if let PortBinding::Rust(imp, own) = self.ports.binding(port_id) {
            return match self.dispatch_to_rust(&imp, own, port_id, method_id, args) {
                PortDispatch::Sync(bytes) => decode_dispatch_reply(&bytes),
                // A sync call cannot wait for an implementation that answers later.
                PortDispatch::Async(future) => {
                    drop(future);
                    Err(PortError::Unavailable)
                }
            };
        }
        let (id, slot) = self.ports.begin(self.timers.now_ns());
        let mut future = PortFuture::new(slot, self.ports.clone(), id);
        let outcome = self.host_port_call(port_id, method_id, id, args);
        self.apply_port_outcome(id, outcome);
        // Dropping `future` abandons a call that has not been answered.
        future.try_take().unwrap_or(Err(PortError::Unavailable))
    }

    /// Applies the host's immediate answer to port call `id`.
    fn apply_port_outcome(&self, id: u32, outcome: PortCallOutcome) {
        match outcome {
            PortCallOutcome::Sync(reply) => {
                Stats::inc(&self.stats.port_replies);
                self.finish_port_call(id, decode_port_reply(id, &reply));
            }
            PortCallOutcome::Unavailable => {
                self.finish_port_call(id, Err(PortError::Unavailable));
            }
            // The reply will arrive through `port_reply`.
            PortCallOutcome::Async => {}
        }
    }

    /// Completes port call `id` and logs its duration in dev mode (SPEC 5.10).
    fn finish_port_call(&self, id: u32, result: Result<Vec<u8>, PortError>) -> Completion {
        let completion = self.ports.complete(id, result);
        if let Completion::Delivered { started_ns } = completion {
            if self.dev {
                let elapsed = self.timers.now_ns().saturating_sub(started_ns);
                self.log(
                    DEBUG,
                    "undra::devtools",
                    &format!("port call id={id} completed in {elapsed} ns"),
                );
            }
        }
        completion
    }

    /// The host answers a port call (`PortReply` payload, SPEC 3.6). Never takes the core
    /// lock, so it is safe to call from the host thread that runs the port, and even from
    /// inside [`Host::port_call`]. A reply for an abandoned call is discarded; a reply for an
    /// unknown id or a malformed payload is logged and ignored.
    pub fn port_reply(&self, payload: &[u8]) {
        let mut reader = Reader::new(payload);
        let reply = match PortReply::decode(&mut reader) {
            Ok(reply) => reply,
            Err(e) => {
                self.log(
                    WARN,
                    "undra::runtime",
                    &format!("port_reply: malformed payload: {e}"),
                );
                return;
            }
        };
        let result = match reply.status {
            PortStatus::Ok => Ok(reply.body.to_vec()),
            PortStatus::Error => Err(PortError::Failed(reply.body.to_vec())),
            PortStatus::Unavailable => Err(PortError::Unavailable),
        };
        Stats::inc(&self.stats.port_replies);
        let id = reply.port_call_id;
        match self.finish_port_call(id, result) {
            Completion::Delivered { .. } => {}
            Completion::Discarded => self.log(
                DEBUG,
                "undra::runtime",
                &format!("port_reply: discarded the late reply to abandoned call {id}"),
            ),
            Completion::Unknown => self.log(
                WARN,
                "undra::runtime",
                &format!("port_reply: no port call {id} is pending"),
            ),
        }
    }

    /// A host-to-core event (SPEC 5.7): fans out to the [`Events`] subscribers of
    /// `(port_id, method_id)` on the core loop, with the core lock held; each receives this
    /// runtime's [`Ctx`].
    pub fn event(&self, port_id: u32, method_id: u32, payload: &[u8]) {
        if self.is_shut_down() {
            self.warn_shut_down("event");
            return;
        }
        let Ok(_guard) = self.enter_core() else {
            self.reentrant("event");
            return;
        };
        Stats::inc(&self.stats.events);
        let callbacks = self.events.callbacks(port_id, method_id);
        if callbacks.is_empty() {
            return;
        }
        // Subscribers get the context as an argument (ADR-034): one they captured would be a
        // reference cycle through the event table.
        let ctx = self.ctx();
        for callback in callbacks {
            if let Err(report) = guard::guarded(|| callback(&ctx, payload)) {
                self.log_panic("an event subscriber panicked", "event subscriber", &report);
            }
        }
    }

    // ----- snapshot and restore ----------------------------------------------------------

    /// The fingerprint of the store type `type_id`'s plain signals (ADR-037); `0` for a type the
    /// schema does not describe as a store (a hand-written restorer), as `snapshot` writes it.
    fn store_fingerprint(&self, type_id: u32) -> u64 {
        if let Some(&(_, found)) = self
            .store_fingerprints
            .lock()
            .iter()
            .find(|(id, _)| *id == type_id)
        {
            return found;
        }
        let fingerprint = self.schema.store_fingerprint(type_id).unwrap_or(0);
        self.store_fingerprints.lock().push((type_id, fingerprint));
        fingerprint
    }

    /// The description of the store types `type_ids` (in snapshot order), reused while the set
    /// does not change.
    fn snapshot_description(&self, type_ids: &[u32]) -> Arc<str> {
        let mut key = type_ids.to_vec();
        key.sort_unstable();
        let mut cache = self.description.lock();
        if let Some((cached, text)) = cache.as_ref() {
            if *cached == key {
                return text.clone();
            }
        }
        let text: Arc<str> = Arc::from(self.schema.stores_closure(&key).canonical_json());
        *cache = Some((key, text.clone()));
        text
    }

    /// Encodes every live store (SPEC 5.9, layout 2 of ADR-037): `count u32, generation_floor
    /// u32, schema_hash u64`, the type table (each store type once, with the fingerprint of its
    /// signals), the description (the canonical JSON of the store types' closures, so a later
    /// build whose types changed can migrate the values by name), then each store's
    /// [`StoreCell::encode_snapshot`](undra_signals::StoreCell::encode_snapshot) record
    /// (`handle u64, type_id u32, signal_count u32, signals`): together exactly an
    /// `undra_wire::payload::Snapshot`. The floor is the highest handle generation issued so far
    /// (ADR-022): restoring it resumes the generation counter above everything the host may
    /// still hold. The runtime re-encodes each record with the table's own
    /// handle and the object's own type id, so a snapshot is consistent whatever the cell
    /// knows. Objects that are not stores, and stores that are [`transient`](crate::UndraObjectDyn::transient)
    /// (query handles), are not included.
    pub fn snapshot(&self) -> Vec<u8> {
        // Read-only, so it is fine even if this thread already holds the lock.
        let _guard = self.enter_core().ok();
        let mut chunks: Vec<Vec<u8>> = Vec::new();
        let mut type_ids: Vec<u32> = Vec::new();
        for (handle, object) in self.objects.stores() {
            let Some(cell) = object.as_store() else {
                continue;
            };
            if object.transient() {
                continue;
            }
            let mut w = Writer::new();
            match guard::guarded(|| cell.encode_snapshot(&mut w)) {
                Ok(()) => {}
                Err(report) => {
                    let operation = self.store_operation("snapshot", handle);
                    self.note_panic("snapshot", &operation, handle, &report);
                    continue;
                }
            }
            let mut r = Reader::new(w.as_slice());
            match StoreSnapshot::decode(&mut r) {
                Ok(record) => {
                    let type_id = object.undra_type_id();
                    let mut chunk = Writer::with_capacity(w.len());
                    StoreSnapshot {
                        handle,
                        type_id,
                        signals: record.signals,
                    }
                    .encode(&mut chunk);
                    chunks.push(chunk.into_vec());
                    if !type_ids.contains(&type_id) {
                        type_ids.push(type_id);
                    }
                }
                Err(e) => self.log(
                    ERROR,
                    "undra::runtime",
                    &format!(
                        "snapshot: `{}` wrote a malformed store record: {e}",
                        object.undra_type_name()
                    ),
                ),
            }
        }
        let description = self.snapshot_description(&type_ids);
        let mut out = Writer::with_capacity(
            32 + type_ids.len() * 12
                + description.len()
                + chunks.iter().map(Vec::len).sum::<usize>(),
        );
        out.write_len(u32::try_from(chunks.len()).unwrap_or(u32::MAX));
        // Read after the stores were listed: the counter only grows, so the floor is at least
        // every generation in the snapshot (and every one issued before it was taken).
        out.write_u32(self.objects.generation_floor());
        out.write_u64(self.schema_hash);
        // The type table (`SnapshotType`s): each store type once, with its fingerprint.
        out.write_len(u32::try_from(type_ids.len()).unwrap_or(u32::MAX));
        for &type_id in &type_ids {
            out.write_u32(type_id);
            out.write_u64(self.store_fingerprint(type_id));
        }
        out.write_str(&description);
        for chunk in &chunks {
            out.write_raw(chunk);
        }
        out.into_vec()
    }

    /// Rebuilds the object table from a snapshot (SPEC 5.9).
    ///
    /// Every store is rebuilt through its registered [`StoreRestorer`] and re-inserted at the
    /// **same handle** (index and generation), so handles the host holds stay valid. All
    /// other objects are dropped and their handles become stale (status 5). The generation
    /// counter is raised to at least the snapshot's floor (it is never lowered), so no handle
    /// issued before the snapshot, or since, can name an object created after the restore
    /// (ADR-022). Signals that
    /// were being observed before the restore (the runtime tracks this per handle) are
    /// re-observed and their current values re-emitted: **one change-set per store**, each built
    /// and delivered under that store's delivery lock, in handle order, inside one transaction so
    /// that the host converges on the core's settled values (ADR-023).
    ///
    /// Restoring into a fresh runtime (after a crash) has no memory of observations: the host
    /// re-observes what it mirrors. Detached tasks keep the objects they already hold; those
    /// stores are detached and no longer deliver change-sets. **In-flight calls and streams
    /// whose receiver the restore replaced or invalidated are cancelled** (ADR-023): a plain
    /// call is answered with status 3, exactly once, a stream ends with a failed item (flag 3,
    /// status 3; ADR-036), and their tasks are dropped, so none can report success for a write
    /// the restored store never saw. Calls without a receiver (free functions, constructors)
    /// carry on.
    ///
    /// All stores are built before anything is replaced: on error the runtime is unchanged.
    ///
    /// **Identity and migration (ADR-037).** Each store type of the snapshot carries the
    /// fingerprint of its signals. Equal to the current build's, the values decode by
    /// `signal_id`, as they were written. Different (the app was updated, or a dev reload rebuilt
    /// the core), each current signal is matched **by name** in the snapshot's description and
    /// converted structurally ([`persist`]), then by the app's `#[undra::migrate]` hooks; a signal
    /// the snapshot lacks takes its `#[undra(default)]`, a signal the store no longer has is
    /// dropped. A value that converts neither way fails the restore with
    /// [`RestoreError::Incompatible`] (logged at ERROR with the reason). One exception keeps
    /// ADR-023's all-or-nothing: a store **type** the current build no longer has is left out
    /// (its handles answer `stale_handle`) and reported, in the [`RestoreReport`] of
    /// [`Runtime::restore_with_report`] and a WARN log. The re-observe phase re-observes a handle
    /// only when the restored store there has the type it had when it was observed.
    pub fn restore(&self, payload: &[u8]) -> Result<(), RestoreError> {
        self.restore_with_report(payload).map(|_| ())
    }

    /// [`Runtime::restore`], and what it did: how many stores it rebuilt, which store types it
    /// migrated, which it left out because the current build no longer has them, and whether the
    /// snapshot was written by a core with another schema hash (`undra dev`'s reload, ADR-053,
    /// reports these).
    ///
    /// # Errors
    ///
    /// As [`Runtime::restore`]; on error the runtime is unchanged.
    pub fn restore_with_report(&self, payload: &[u8]) -> Result<RestoreReport, RestoreError> {
        if self.is_shut_down() {
            return Err(RestoreError::ShutDown);
        }
        let snapshot = {
            let mut r = Reader::new(payload);
            let snapshot = Snapshot::decode(&mut r).map_err(RestoreError::Decode)?;
            r.finish().map_err(RestoreError::Decode)?;
            snapshot
        };
        let mut seen = HashSet::new();
        for s in &snapshot.stores {
            let h = s.handle;
            if h.is_null()
                || h.generation() == 0
                || h.generation() >= GENERATION_CEILING
                || h.index() as usize > crate::object_table::MAX_RESTORE_INDEX
                || !seen.insert(h.0)
            {
                return Err(RestoreError::BadHandle { handle: h.0 });
            }
        }
        // The counter must be left real room to issue from: the generation counter is shared by
        // every runtime in the process, so obeying a floor near `u32::MAX` would let one corrupt
        // or hostile snapshot exhaust handle creation process-wide — and crash recovery restores
        // the same bytes on every launch. Anything above the ceiling is refused (re-review NF1).
        if snapshot.generation_floor >= GENERATION_CEILING {
            return Err(RestoreError::GenerationFloor {
                floor: snapshot.generation_floor,
            });
        }
        let _guard = self.enter_core().map_err(|_| RestoreError::Reentrant)?;
        let ctx = self.ctx();

        // Phase 1: build every store. Nothing is touched yet.
        let mut built: Vec<(Handle, Arc<dyn AnyObject>)> =
            Vec::with_capacity(snapshot.stores.len());
        let mut report = RestoreReport {
            schema_changed: snapshot.schema_hash != self.schema_hash,
            ..RestoreReport::default()
        };
        let mut migration = Migration::new(&snapshot);

        for s in &snapshot.stores {
            let type_id = s.type_id;
            let Some(restorer) = self.restorers.get(&type_id) else {
                // A store type this build no longer has (the app removed that screen): left out,
                // its handle answers `stale_handle`, reported (ADR-037 decision 7).
                let name = migration
                    .description(self)
                    .ok()
                    .and_then(|d| d.store(type_id).map(|store| store.name.clone()))
                    .unwrap_or_else(|| format!("{type_id:#010x}"));
                match report.dropped.iter_mut().find(|d| d.type_id == type_id) {
                    Some(dropped) => dropped.handles.push(s.handle.0),
                    None => report.dropped.push(DroppedStore {
                        type_id,
                        name,
                        handles: vec![s.handle.0],
                    }),
                }
                continue;
            };
            // A store type the schema does not describe (a hand-written restorer) has no
            // fingerprint: `0`, as `snapshot` writes it.
            let current = self.store_fingerprint(type_id);
            let bytes = if snapshot.fingerprint(type_id) == Some(current) {
                // The fast path: the values were written with today's types.
                let mut body = Writer::new();
                body.write_len(u32::try_from(s.signals.len()).unwrap_or(u32::MAX));
                for (signal_id, value) in &s.signals {
                    body.write_u32(*signal_id);
                    body.write_bytes(value);
                }
                body.into_vec()
            } else {
                let body = migration.store_body(self, s).inspect_err(|e| {
                    self.log(ERROR, "undra::persist", &format!("restore refused: {e}"));
                })?;
                let name = self.store_name(type_id);
                if !report.migrated.contains(&name) {
                    report.migrated.push(name);
                }
                body
            };
            let mut r = Reader::new(&bytes);
            let restored = guard::guarded(|| (restorer.restore)(ctx.clone(), s.handle.0, &mut r));
            let any = match restored {
                Ok(Ok(any)) => any,
                Ok(Err(source)) => return Err(RestoreError::Store { type_id, source }),
                Err(report) => {
                    self.log_panic(
                        "a store's restore panicked",
                        &format!("restore {}", self.store_name(type_id)),
                        &report,
                    );
                    return Err(RestoreError::Panicked {
                        type_id,
                        message: report.message,
                    });
                }
            };
            r.finish()
                .map_err(|source| RestoreError::Store { type_id, source })?;
            // The restorer must hand back the store it is registered for: prove it by finding
            // the cell.
            if (restorer.cell)(&*any).is_none() {
                return Err(RestoreError::Store {
                    type_id,
                    source: undra_wire::WireError::InvalidTag {
                        tag: type_id,
                        at: 0,
                        ty: "StoreRestorer.cell",
                    },
                });
            }
            let name = self
                .table
                .objects
                .get(&type_id)
                .map_or("store", |entry| entry.meta.name);
            built.push((s.handle, erased(any, type_id, name, Some(restorer.cell))));
        }
        report.restored = built.len();
        for dropped in &report.dropped {
            self.log(
                WARN,
                "undra::persist",
                &format!(
                    "restore: left out {} store(s) of `{}`, a type this build does not have",
                    dropped.handles.len(),
                    dropped.name
                ),
            );
        }

        // Phase 2: replace the table.
        // Nothing issued before the snapshot (or since) may be issued again: the counter resumes
        // above the snapshot's floor and above every generation it places (ADR-022).
        let max_generation = built.iter().map(|(h, _)| h.generation()).max().unwrap_or(0);
        self.objects
            .raise_generation_floor(snapshot.generation_floor.max(max_generation));
        // What each store handle observed before the restore, and the store type it was observed
        // as (ADR-037 decision 9: a handle is re-observed only as the same type).
        let mut observed = HashMap::new();
        // Which object each handle named before the restore (by address, for the check below).
        let mut before: HashMap<u64, usize> = HashMap::new();
        for cleared in self.objects.clear() {
            before.insert(cleared.handle.0, object_address(&cleared.object));
            if let Some(cell) = cleared.object.as_store() {
                cell.set_handle(0);
                observed.insert(
                    cleared.handle.0,
                    (cleared.object.undra_type_id(), cleared.observed),
                );
            }
            self.drop_guarded_logged("an object replaced by restore", cleared.object);
        }
        for (handle, object) in &built {
            if let Err(e) = self.objects.insert_at(*handle, object.clone()) {
                self.log(
                    ERROR,
                    "undra::runtime",
                    &format!("restore: could not place {handle:?}: {e}"),
                );
            }
        }

        // Calls and streams that were running on an object this restore replaced or invalidated
        // must not go on: they would finish on a store the handle no longer names and report
        // success for a write the restored store never saw (ADR-023, M3).
        self.cancel_calls_replaced_by_restore(&before);

        // Phase 3: re-emit what was observed, through the path `observe` uses (ADR-023, L9):
        // one change-set per re-observed store, each built and handed to the host under that
        // store's delivery lock, all inside one transaction so that writes a computed makes
        // while it is evaluated (and that do not settle within the cell's passes) commit after
        // every store's entries went out and the host converges on the core's values.
        let phase3 = guard::guarded(|| {
            undra_signals::txn(|| {
                for (handle, object) in &built {
                    let (Some((observed_type, previous)), Some(cell)) =
                        (observed.get(&handle.0), object.as_store())
                    else {
                        continue;
                    };
                    if *observed_type != object.undra_type_id() {
                        // The host mirrors this handle as another store type: re-observing would
                        // send it entries it would apply to the wrong mirror (runtime review N6).
                        self.log(
                            WARN,
                            "undra::runtime",
                            &format!(
                                "restore: {handle:?} was observed as another store type than `{}`; not re-observed",
                                object.undra_type_name()
                            ),
                        );
                        continue;
                    }
                    let signal_ids = previous.to_reobserve();
                    if !signal_ids.is_empty() {
                        if let Err(report) =
                            guard::guarded(|| self.deliver_observed(cell, &signal_ids))
                        {
                            let operation = self.store_operation("restore", *handle);
                            self.note_panic("restore", &operation, *handle, &report);
                        }
                    }
                    self.objects
                        .with_observed(*handle, |o| *o = previous.clone());
                }
            });
        });
        if let Err(panic) = phase3 {
            self.log_panic(
                "restore: committing the writes of the re-observed stores panicked",
                "restore",
                &panic,
            );
        }
        Ok(report)
    }

    /// The name of the store type `type_id` in this build's schema.
    fn store_name(&self, type_id: u32) -> String {
        self.schema
            .objects
            .iter()
            .find(|o| o.type_id == type_id)
            .map_or_else(|| format!("{type_id:#010x}"), |o| o.name.clone())
    }

    // ----- statistics --------------------------------------------------------------------

    /// A JSON document with the live handle count, tasks, calls, crossing counters, poisoned
    /// stores and panics (SPEC 6 `undra_stats_json`), and `strong_refs`: the strong references
    /// to the runtime besides the global slot (the caller's included), which makes a `Ctx` kept
    /// where a `WeakCtx` belongs visible (ADR-034). Never takes the core lock.
    pub fn stats_json(&self) -> String {
        let s = &self.stats;
        let (started, max) = self.blocking.threads();
        let (open_streams, active_calls) = {
            let calls = self.calls.lock();
            (
                calls.values().filter(|c| c.stream.is_some()).count(),
                calls.len(),
            )
        };
        // Strong references held outside the runtime's own bookkeeping (the global slot is the
        // global runtime's owner): the caller's, plus whatever app code keeps. A number that
        // grows while nothing is in flight is a `Ctx` kept where a `WeakCtx` belongs (ADR-034).
        let global = usize::from(
            GLOBAL
                .try_lock()
                .is_some_and(|g| g.as_ref().is_some_and(|g| g.id == self.id)),
        );
        let strong_refs = self.weak.strong_count().saturating_sub(global);
        let mut out = String::with_capacity(512);
        out.push_str("{\"platform\":");
        push_json_string(&mut out, &self.config.platform);
        out.push_str(",\"mode\":");
        push_json_string(&mut out, &self.config.mode);
        out.push_str(&format!(
            ",\"schema_hash\":\"{:#018x}\",\"strong_refs\":{},\"live_handles\":{},\"live_stores\":{},\"poisoned_stores\":{},\"tasks\":{},\"active_calls\":{},\"open_streams\":{},\"pending_port_calls\":{},\"abandoned_port_calls\":{},\"pending_timers\":{},\"blocking_threads\":{{\"started\":{},\"max\":{}}},\"poisoned_signals\":{},\"transactions\":{},\"panics\":{},\"off_core_writes\":{},\"turns\":{},\"polls\":{},\"crossings\":{{\"calls\":{},\"replies\":{},\"change_sets\":{},\"change_set_bytes\":{},\"port_calls\":{},\"port_replies\":{},\"stream_items\":{},\"events\":{},\"bad_requests\":{},\"cancelled\":{}}}",
            self.schema_hash,
            strong_refs,
            self.objects.live(),
            self.objects.store_count(),
            self.objects.poisoned_stores(),
            self.exec.live(),
            active_calls,
            open_streams,
            self.ports.pending_count(),
            self.ports.abandoned_count(),
            self.timers.pending(),
            started,
            max,
            self.poisoned_signals.lock().len(),
            Stats::get(&s.change_sets),
            Stats::get(&s.panics),
            Stats::get(&s.off_core_writes),
            Stats::get(&s.turns),
            Stats::get(&s.polls),
            Stats::get(&s.calls),
            Stats::get(&s.replies),
            Stats::get(&s.change_sets),
            Stats::get(&s.change_set_bytes),
            Stats::get(&s.port_calls),
            Stats::get(&s.port_replies),
            Stats::get(&s.stream_items),
            Stats::get(&s.events),
            Stats::get(&s.bad_requests),
            Stats::get(&s.cancelled),
        ));
        // ADR-046: the reports delivered to `Diagnostics`, and the background tasks: how many,
        // how much work they say is waiting (what a platform reads to decide whether to ask the OS
        // for a window), and what the runs did.
        out.push_str(&format!(
            ",\"panic_reports\":{},\"background\":{{\"tasks\":{},\"pending\":{},\"runs\":{},\"finished\":{},\"replayed\":{},\"refetched\":{}}}}}",
            Stats::get(&s.panic_reports),
            self.background.count(),
            self.background.pending(&self.ctx()),
            Stats::get(&s.background_runs),
            Stats::get(&s.background_finished),
            Stats::get(&s.background_replayed),
            Stats::get(&s.background_refetched),
        ));
        // Sections of layered crates (`undra-query`'s persistence counters), before the closing
        // brace of the document.
        let mut sections = String::new();
        let registered = self.stats_sections.lock().clone();
        for section in registered {
            if let Ok(Some(json)) = guard::guarded(|| (section.json)(self)) {
                sections.push(',');
                push_json_string(&mut sections, section.name);
                sections.push(':');
                sections.push_str(&json);
            }
        }
        if !sections.is_empty() {
            out.pop();
            out.push_str(&sections);
            out.push('}');
        }
        out
    }

    /// Advances the manual clock (test runtimes); see `TestRuntime::advance`.
    pub(crate) fn advance_clock(&self, d: Duration, after_each: impl FnMut()) -> usize {
        self.timers.advance_manual(d, after_each)
    }

    /// Number of live sleepers (tests).
    pub(crate) fn pending_timers(&self) -> usize {
        self.timers.pending()
    }

    /// Blocking closures queued or running (test runtimes settle these before they report
    /// that nothing is left to do).
    pub(crate) fn blocking_in_flight(&self) -> usize {
        self.blocking.in_flight()
    }

    /// Waits up to `timeout` for one blocking closure to finish; `false` if none is in flight
    /// or the time ran out.
    pub(crate) fn wait_blocking_progress(&self, timeout: Duration) -> bool {
        self.blocking.wait_for_progress(timeout)
    }
}

impl core::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Runtime")
            .field("id", &self.id)
            .field("platform", &self.config.platform)
            .field("mode", &self.config.mode)
            .field("shut_down", &self.is_shut_down())
            .finish_non_exhaustive()
    }
}

impl Drop for Runtime {
    /// The last strong reference went without `shutdown` (ADR-034): tears everything down the way
    /// [`shutdown`](Runtime::shutdown) does, including **answering what is in flight** (status 3
    /// for calls, the cancelled item for streams, each once): the runtime's own call and stream
    /// tasks hold it weakly, so a host may still be waiting on them. The `Host` is still owned by
    /// the runtime here. `Drop` may run on the core thread (the last upgrade released at the end
    /// of a poll) or a blocking-pool thread; it never joins the thread it runs on.
    fn drop(&mut self) {
        let was_shut_down = self.shut_down.swap(true, Ordering::AcqRel);
        self.lifeline.close(Gone::Dropped);
        self.exec.shutdown();
        if !was_shut_down {
            // Nobody else can hold the core lock (it takes a strong reference to reach it), so
            // this is the core now; the mark lets user `Drop` code that writes signals do so.
            let _held = HeldMark::enter(self.id);
            self.cancel_all_calls("the runtime was dropped");
        }
        let handle = self.core_thread.lock().take();
        if let Some(handle) = handle {
            if handle.thread().id() != std::thread::current().id() {
                let _ = handle.join();
            }
        }
        self.timers.shutdown();
        self.blocking.shutdown();
        self.ports.cancel_all();
        self.release_user_references();
        let _held = HeldMark::enter(self.id);
        self.teardown();
        unregister_runtime(self.id);
        // The global slot holds a strong reference, so a registered runtime is never dropped.
    }
}

/// Marks the calling thread as holding a runtime's core lock without taking it (`Drop`, where no
/// other thread can reach the runtime) and without making the runtime current (there is no
/// strong reference left to make current).
struct HeldMark(u64);

impl HeldMark {
    fn enter(id: u64) -> HeldMark {
        let _ = HELD.try_with(|held| held.borrow_mut().push(id));
        HeldMark(id)
    }
}

impl Drop for HeldMark {
    fn drop(&mut self) {
        let id = self.0;
        let _ = HELD.try_with(|held| {
            let mut held = held.borrow_mut();
            if let Some(pos) = held.iter().rposition(|&x| x == id) {
                held.remove(pos);
            }
        });
    }
}

/// The `undra-core` thread: wait for work, run a turn, repeat. Holds only a `Weak` to the
/// runtime, so dropping the last `Arc<Runtime>` ends it.
#[cfg(not(target_family = "wasm"))]
fn core_loop(weak: &Weak<Runtime>, shared: &Shared) {
    while let Some(batch) = shared.wait_batch(BATCH) {
        match weak.upgrade() {
            // The loop must survive anything, including a bug in the runtime itself: a dead
            // `undra-core` thread would silently stall every async call.
            Some(rt) => {
                if let Err(report) = guard::guarded(|| rt.run_batch(batch)) {
                    rt.log_panic("the executor loop panicked", "executor", &report);
                }
            }
            None => break,
        }
    }
}

/// Drives an open stream (SPEC 3.7): polls the next item, waits for credit, emits it; the end
/// and the error markers need no credit.
///
/// The stream is polled *before* credit is checked, so it runs at most one item ahead of the
/// host, and an ended stream reports its end immediately even if the host has spent all its
/// credit (as gRPC servers send trailers regardless of the flow-control window).
///
/// The driver holds the runtime weakly (ADR-034) and upgrades once per item, after the credit
/// wait and never across an `.await`: an open stream does not keep a dropped runtime alive. A
/// runtime that is gone has already ended the stream (shutdown and `Drop` do).
async fn drive_stream(
    rt: Weak<Runtime>,
    call_id: u32,
    mut stream: Pin<Box<dyn futures_core::Stream<Item = DispatchBytes> + Send>>,
    state: Arc<StreamState>,
) {
    loop {
        let next = std::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
        match next {
            Some(Ok(item)) => {
                while !state.try_take() {
                    state.notify.notified().await;
                }
                let Some(rt) = rt.upgrade() else { return };
                rt.send_stream_item(call_id, StreamFlag::Item, &item);
            }
            Some(Err(error)) => {
                let Some(rt) = rt.upgrade() else { return };
                rt.calls.lock().remove(&call_id);
                rt.send_stream_item(call_id, StreamFlag::Error, &error);
                return;
            }
            None => {
                let Some(rt) = rt.upgrade() else { return };
                rt.calls.lock().remove(&call_id);
                rt.send_stream_item(call_id, StreamFlag::End, &[]);
                return;
            }
        }
    }
}
