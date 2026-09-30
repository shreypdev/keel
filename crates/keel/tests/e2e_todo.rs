//! End to end: real macro-generated code (records, enums, errors, a store with a keyed list and
//! a computed signal, objects with sync/async/stream/panicking/failing methods, a free
//! function, a port with a Rust fake) driven through `keel_runtime::testing::TestRuntime`.
//!
//! Every scenario asserts on decoded wire payloads: the replies, change-sets, stream items and
//! port calls the recording host receives, exactly what a Swift, Kotlin or TypeScript runtime
//! would see.
#![forbid(unsafe_code)]

use std::cell::Cell;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use keel::meta::ids;
use keel::prelude::*;
use keel::runtime::testing::{ReplyRecord, TestRuntime, port_reply, port_reply_ok, sync_ok};
use keel::runtime::{Port, Stream};
use keel::signals::ALL_SIGNALS;
use keel::wire::payload::{
    CallTarget, ChangeEntry, ChangeOp, ChangeSet, PortStatus, ReplyStatus, StreamFlag,
};
use keel::wire::{Decode, Encode, KeyedPatch, PatchOp, Reader};

// ---------------------------------------------------------------------------------------------
// The application core under test (the blueprint)
// ---------------------------------------------------------------------------------------------

#[keel::api]
#[derive(Clone, Debug, PartialEq)]
pub struct Todo {
    pub id: Uuid,
    pub title: String,
    pub done: bool,
}

#[keel::api]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Filter {
    All,
    Active,
    Done,
}

impl Filter {
    fn matches(self, todo: &Todo) -> bool {
        match self {
            Filter::All => true,
            Filter::Active => !todo.done,
            Filter::Done => todo.done,
        }
    }
}

#[keel::error]
#[derive(Clone, Debug, PartialEq)]
pub enum StoreError {
    #[error("the server is unreachable")]
    Offline,
    #[error("the server rejected the todo: {0}")]
    Rejected(String),
}

#[keel::error]
#[derive(Clone, Debug, PartialEq)]
pub enum TodoError {
    #[error("title cannot be empty")]
    EmptyTitle,
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Implemented by the platform, or by a Rust fake.
#[keel::port]
pub trait Store {
    async fn save(&self, todo: Todo) -> Result<(), StoreError>;
}

#[keel::store(restore = "Self::assemble")]
pub struct Todos {
    ctx: Ctx,
    next: AtomicU64,
    #[keel(key = "id")]
    todos: Signal<Vec<Todo>>,
    filter: Signal<Filter>,
    visible: Computed<Vec<Todo>>,
}

const TODOS: u32 = 0;
const FILTER: u32 = 1;
const VISIBLE: u32 = 2;

#[keel::api(store)]
impl Todos {
    pub fn new(ctx: Ctx) -> Self {
        Self::assemble(ctx, Signal::new(vec![]), Signal::new(Filter::All))
    }

    fn assemble(ctx: Ctx, todos: Signal<Vec<Todo>>, filter: Signal<Filter>) -> Self {
        let visible = Computed::new((&todos, &filter), |(todos, filter)| {
            todos
                .iter()
                .filter(|t| filter.matches(t))
                .cloned()
                .collect()
        });
        let next = AtomicU64::new(todos.with(|list| list.len() as u64) + 1);
        Self {
            ctx,
            next,
            todos,
            filter,
            visible,
        }
    }

    pub fn set_filter(&self, filter: Filter) {
        self.filter.set(filter);
    }

    pub async fn add(&self, title: String) -> Result<Todo, TodoError> {
        let title = title.trim().to_owned();
        if title.is_empty() {
            return Err(TodoError::EmptyTitle);
        }
        let mut id = [0; 16];
        id[..8].copy_from_slice(&self.next.fetch_add(1, Ordering::Relaxed).to_be_bytes());
        let todo = Todo {
            id: Uuid(id),
            title,
            done: false,
        };
        store(&self.ctx).save(todo.clone()).await?;
        self.todos.update(|list| list.push(todo.clone()));
        Ok(todo)
    }
}

/// A store whose `count` reaches the platform even while nobody observes it.
#[keel::store]
pub struct Ticker {
    ctx: Ctx,
    #[keel(no_coalesce)]
    count: Signal<u32>,
    quiet: Signal<u32>,
}

#[keel::api(store)]
impl Ticker {
    pub fn new(ctx: Ctx) -> Self {
        Self {
            ctx,
            count: Signal::new(0),
            quiet: Signal::new(0),
        }
    }

    pub fn bump(&self) {
        let _ = &self.ctx;
        self.ctx.txn(|| {
            self.count.update(|n| *n += 1);
            self.quiet.update(|n| *n += 1);
        });
    }
}

/// A store whose constructor hands out a signal that already belongs to another store.
#[keel::store]
pub struct Shared {
    count: Signal<u32>,
}

static SHARED_SIGNAL: OnceLock<Signal<u32>> = OnceLock::new();

#[keel::api(store)]
#[allow(clippy::new_without_default)]
impl Shared {
    pub fn new() -> Self {
        Self {
            count: SHARED_SIGNAL.get_or_init(|| Signal::new(0)).clone(),
        }
    }
}

#[keel::error]
#[derive(Clone, Debug, PartialEq)]
pub enum CalcError {
    #[error("the calculation failed on purpose")]
    Failed,
}

pub struct Calculator {
    ctx: Ctx,
    base: i64,
}

#[keel::api]
impl Calculator {
    pub fn new(ctx: Ctx, base: i64) -> Self {
        Calculator { ctx, base }
    }

    pub fn add(&self, a: i64, b: i64) -> i64 {
        self.base + a + b
    }

    pub async fn slow_add(&self, a: i64, b: i64) -> i64 {
        self.ctx.sleep(Duration::from_secs(1)).await;
        self.base + a + b
    }

    pub fn ticks(&self, n: u32) -> impl Stream<Item = u32> {
        Ticks { next: 0, n }
    }

    pub fn boom(&self) -> i64 {
        panic!("kaboom")
    }

    pub fn fail(&self) -> Result<i64, CalcError> {
        Err(CalcError::Failed)
    }
}

struct Ticks {
    next: u32,
    n: u32,
}

impl Stream for Ticks {
    type Item = u32;

    fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<u32>> {
        if self.next < self.n {
            self.next += 1;
            Poll::Ready(Some(self.next - 1))
        } else {
            Poll::Ready(None)
        }
    }
}

#[keel::api]
pub fn version() -> String {
    "keel-e2e 1".to_owned()
}

/// A Rust implementation of the `Store` port; `fail` scripts an error.
#[derive(Default)]
struct FakeStore {
    saved: Mutex<Vec<Todo>>,
    fail: Mutex<Option<StoreError>>,
}

#[keel::port]
impl Store for FakeStore {
    async fn save(&self, todo: Todo) -> Result<(), StoreError> {
        if let Some(error) = self.fail.lock().unwrap().clone() {
            return Err(error);
        }
        self.saved.lock().unwrap().push(todo);
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Test plumbing: what a platform runtime does
// ---------------------------------------------------------------------------------------------

/// A test runtime, the call-id counter of a host, and the fake behind the `Store` port.
struct Core {
    t: TestRuntime,
    fake: Arc<FakeStore>,
    next_call: Cell<u32>,
}

impl Core {
    /// A runtime with the fake bound to the `Store` port.
    fn new() -> Core {
        let core = Core::without_fake();
        let fake = Arc::new(FakeStore::default());
        core.t.ctx().bind_dyn_port::<dyn Store>(
            <dyn Store as Port>::PORT_ID,
            fake.clone() as Arc<dyn Store>,
        );
        Core { fake, ..core }
    }

    /// A runtime whose `Store` port is implemented by the (recording) host.
    fn without_fake() -> Core {
        Core {
            t: TestRuntime::new(),
            fake: Arc::new(FakeStore::default()),
            next_call: Cell::new(1),
        }
    }

    fn call_id(&self) -> u32 {
        let id = self.next_call.get();
        self.next_call.set(id + 1);
        id
    }

    /// A synchronous call.
    fn sync(&self, target: CallTarget, args: &[u8]) -> ReplyRecord {
        self.t.call_sync(target, self.call_id(), args)
    }

    /// Starts a call and returns its id (asserts the runtime accepted it).
    fn start(&self, target: CallTarget, args: &[u8]) -> u32 {
        let id = self.call_id();
        assert_eq!(self.t.call(target, id, args), 0, "the call was refused");
        id
    }

    /// Starts a call, runs the executor, and returns the single reply.
    fn run(&self, target: CallTarget, args: &[u8]) -> ReplyRecord {
        let id = self.start(target, args);
        self.t.run_pending();
        let mut replies = self.t.take_replies();
        assert_eq!(replies.len(), 1, "expected one reply, got {replies:?}");
        let reply = replies.remove(0);
        assert_eq!(reply.call_id, id);
        reply
    }

    /// Calls the constructor `new` of `type_name` and returns the new object's handle.
    fn construct(&self, type_name: &str, args: &[u8]) -> Handle {
        let reply = self.sync(
            CallTarget::Constructor {
                type_id: ids::type_id(type_name),
                method_id: ids::method_id(type_name, "new"),
            },
            args,
        );
        assert_eq!(reply.status, ReplyStatus::Ok, "{type_name}::new: {reply:?}");
        decode(&reply.body)
    }

    fn todos(&self) -> Handle {
        self.construct("Todos", &[])
    }

    fn calculator(&self, base: i64) -> Handle {
        self.construct("Calculator", &enc(&base))
    }

    /// Starts observing every signal of `store`; returns the initial change-set.
    fn observe(&self, store: Handle) -> ChangeSet {
        self.t.take_change_sets();
        self.t.runtime().observe(store.0, ALL_SIGNALS, true);
        one(self.change_sets())
    }

    fn change_sets(&self) -> Vec<ChangeSet> {
        self.t.host().take_decoded_change_sets()
    }
}

fn method(handle: Handle, type_name: &str, name: &str) -> CallTarget {
    CallTarget::Method {
        handle,
        method_id: ids::method_id(type_name, name),
    }
}

fn enc<T: Encode>(value: &T) -> Vec<u8> {
    value.encode_to_vec()
}

fn decode<T: Decode>(bytes: &[u8]) -> T {
    match T::decode_exact(bytes) {
        Ok(value) => value,
        Err(e) => panic!("{} bytes do not decode: {e}", bytes.len()),
    }
}

fn decode_patch(bytes: &[u8]) -> KeyedPatch<Todo> {
    let mut r = Reader::new(bytes);
    let patch = KeyedPatch::<Todo>::decode(&mut r).expect("a keyed patch");
    r.finish().expect("no bytes after the patch");
    patch
}

fn one<T: std::fmt::Debug>(mut items: Vec<T>) -> T {
    assert_eq!(items.len(), 1, "expected exactly one, got {items:?}");
    items.remove(0)
}

fn entry(cs: &ChangeSet, signal_id: u32) -> &ChangeEntry {
    cs.entries
        .iter()
        .find(|e| e.signal_id == signal_id)
        .unwrap_or_else(|| panic!("no entry for signal {signal_id} in {cs:?}"))
}

fn signal_ids(cs: &ChangeSet) -> Vec<u32> {
    cs.entries.iter().map(|e| e.signal_id).collect()
}

/// The `n`th todo the store hands out (ids count up from 1).
fn todo(n: u64, title: &str) -> Todo {
    let mut id = [0; 16];
    id[..8].copy_from_slice(&n.to_be_bytes());
    Todo {
        id: Uuid(id),
        title: title.to_owned(),
        done: false,
    }
}

fn milk(n: u64) -> Todo {
    todo(n, "Milk")
}

// ---------------------------------------------------------------------------------------------
// Scenarios: the todo store
// ---------------------------------------------------------------------------------------------

#[test]
fn constructor_call_answers_a_live_handle() {
    let core = Core::new();
    let reply = core.sync(
        CallTarget::Constructor {
            type_id: ids::type_id("Todos"),
            method_id: ids::method_id("Todos", "new"),
        },
        &[],
    );
    assert_eq!(reply.status, ReplyStatus::Ok);
    let handle: Handle = decode(&reply.body);
    assert!(!handle.is_null());
    // The handle works: a method call on it succeeds.
    let reply = core.sync(method(handle, "Todos", "set_filter"), &enc(&Filter::Active));
    assert_eq!(reply.status, ReplyStatus::Ok);
    assert!(reply.body.is_empty());
    // Handles are per object: a second store gets another one.
    assert_ne!(core.todos(), handle);
}

#[test]
fn observe_all_sends_the_initial_values_of_all_three_signals() {
    let core = Core::new();
    let store = core.todos();
    let initial = core.observe(store);
    assert_eq!(signal_ids(&initial), [TODOS, FILTER, VISIBLE]);
    for e in &initial.entries {
        assert_eq!(e.handle, store);
        assert_eq!(e.op, ChangeOp::Full);
    }
    assert_eq!(decode::<Vec<Todo>>(&entry(&initial, TODOS).value), []);
    assert_eq!(
        decode::<Filter>(&entry(&initial, FILTER).value),
        Filter::All
    );
    assert_eq!(decode::<Vec<Todo>>(&entry(&initial, VISIBLE).value), []);
    // Nothing else crossed the boundary, and the runtime did not need the platform.
    assert!(core.t.host().port_calls().is_empty());
}

#[test]
fn sync_command_emits_one_change_set_with_the_signal_and_the_computed() {
    let core = Core::new();
    let store = core.todos();
    core.observe(store);
    let added = core.run(method(store, "Todos", "add"), &enc(&"Milk".to_owned()));
    assert_eq!(added.status, ReplyStatus::Ok);
    core.change_sets();

    let reply = core.sync(method(store, "Todos", "set_filter"), &enc(&Filter::Done));
    assert_eq!(reply.status, ReplyStatus::Ok);
    let cs = one(core.change_sets());
    // `todos` did not change; `filter` did, and `visible` was recomputed from it.
    assert_eq!(signal_ids(&cs), [FILTER, VISIBLE]);
    assert_eq!(decode::<Filter>(&entry(&cs, FILTER).value), Filter::Done);
    assert_eq!(entry(&cs, VISIBLE).op, ChangeOp::Full);
    assert_eq!(decode::<Vec<Todo>>(&entry(&cs, VISIBLE).value), []);

    // Switching back: again exactly one change-set, and `visible` is current.
    core.sync(method(store, "Todos", "set_filter"), &enc(&Filter::All));
    let cs = one(core.change_sets());
    assert_eq!(decode::<Vec<Todo>>(&entry(&cs, VISIBLE).value), [milk(1)]);
}

#[test]
fn async_add_replies_and_ships_a_keyed_insert_patch() {
    let core = Core::new();
    let store = core.todos();
    core.observe(store);

    // The list goes from empty to non-empty: no key is shared, so the full list is sent
    // (SPEC 3.8 sends a patch only when there is something to patch).
    core.run(method(store, "Todos", "add"), &enc(&"Bread".to_owned()));
    let first = one(core.change_sets());
    assert_eq!(entry(&first, TODOS).op, ChangeOp::Full);
    assert_eq!(
        decode::<Vec<Todo>>(&entry(&first, TODOS).value),
        [todo(1, "Bread")]
    );

    let id = core.start(method(store, "Todos", "add"), &enc(&"Milk".to_owned()));
    // Asynchronous: nothing has happened until the executor runs.
    assert!(core.t.take_replies().is_empty());
    assert!(core.change_sets().is_empty());
    core.t.host().take_timeline();
    core.t.run_pending();

    let reply = one(core.t.take_replies());
    assert_eq!((reply.call_id, reply.status), (id, ReplyStatus::Ok));
    assert_eq!(decode::<Todo>(&reply.body), todo(2, "Milk"));
    assert_eq!(
        *core.fake.saved.lock().unwrap(),
        [todo(1, "Bread"), todo(2, "Milk")]
    );

    // The change-set reached the platform before the reply of the call that caused it.
    use keel::runtime::testing::HostEvent::{ChangeSet as Cs, Reply};
    assert_eq!(core.t.host().take_timeline(), [Cs, Reply(id)]);

    let cs = one(core.change_sets());
    assert_eq!(signal_ids(&cs), [TODOS, VISIBLE]);
    let patch_entry = entry(&cs, TODOS);
    assert_eq!(patch_entry.op, ChangeOp::KeyedPatch);
    assert_eq!(
        decode_patch(&patch_entry.value).ops,
        [PatchOp::Insert {
            index: 1,
            item: todo(2, "Milk")
        }]
    );
    // The computed is not keyed: it ships in full.
    assert_eq!(entry(&cs, VISIBLE).op, ChangeOp::Full);
    assert_eq!(
        decode::<Vec<Todo>>(&entry(&cs, VISIBLE).value),
        [todo(1, "Bread"), todo(2, "Milk")]
    );
}

#[test]
fn empty_title_is_a_typed_error_and_changes_nothing() {
    let core = Core::new();
    let store = core.todos();
    core.observe(store);
    let reply = core.run(method(store, "Todos", "add"), &enc(&"   ".to_owned()));
    assert_eq!(reply.status, ReplyStatus::Error);
    assert_eq!(decode::<TodoError>(&reply.body), TodoError::EmptyTitle);
    assert!(core.change_sets().is_empty());
    assert!(core.fake.saved.lock().unwrap().is_empty());
}

#[test]
fn a_failing_port_propagates_its_typed_error_through_the_command() {
    let core = Core::new();
    let store = core.todos();
    core.observe(store);

    *core.fake.fail.lock().unwrap() = Some(StoreError::Offline);
    let reply = core.run(method(store, "Todos", "add"), &enc(&"Milk".to_owned()));
    assert_eq!(reply.status, ReplyStatus::Error);
    assert_eq!(
        decode::<TodoError>(&reply.body),
        TodoError::Store(StoreError::Offline)
    );

    // An error with a payload survives the trip too.
    *core.fake.fail.lock().unwrap() = Some(StoreError::Rejected("quota".to_owned()));
    let reply = core.run(method(store, "Todos", "add"), &enc(&"Milk".to_owned()));
    assert_eq!(
        decode::<TodoError>(&reply.body),
        TodoError::Store(StoreError::Rejected("quota".to_owned()))
    );

    // The failed adds left the store untouched, and it recovers.
    assert!(core.change_sets().is_empty());
    *core.fake.fail.lock().unwrap() = None;
    let reply = core.run(method(store, "Todos", "add"), &enc(&"Milk".to_owned()));
    assert_eq!(reply.status, ReplyStatus::Ok);
    assert_eq!(one(core.change_sets()).entries.len(), 2);
}

#[test]
fn a_foreign_port_is_answered_by_the_host() {
    let core = Core::without_fake();
    let port_id = ids::port_id("Store");
    let save = ids::port_method_id("Store", "save");
    core.t.host().script_port(port_id, save, |call| {
        // The platform saves the todo and answers `Ok(())`.
        sync_ok(call, &[])
    });
    let store = core.todos();
    core.observe(store);

    let reply = core.run(method(store, "Todos", "add"), &enc(&"Milk".to_owned()));
    assert_eq!(reply.status, ReplyStatus::Ok);
    let calls = core.t.host().take_port_calls();
    let call = one(calls);
    assert_eq!((call.port_id, call.method_id), (port_id, save));
    // The arguments are the encoded parameters of `save`.
    assert_eq!(decode::<Todo>(&call.args), milk(1));
    assert_eq!(one(core.change_sets()).entries.len(), 2);
}

#[test]
fn a_foreign_port_error_and_a_late_reply_reach_the_command() {
    let core = Core::without_fake();
    let port_id = ids::port_id("Store");
    let save = ids::port_method_id("Store", "save");
    let store = core.todos();
    core.observe(store);

    // The platform answers a typed error synchronously.
    core.t.host().script_port(port_id, save, |call| {
        let body = enc(&StoreError::Rejected("nope".to_owned()));
        keel::runtime::PortCallOutcome::Sync(port_reply(
            call.port_call_id,
            PortStatus::Error,
            &body,
        ))
    });
    let reply = core.run(method(store, "Todos", "add"), &enc(&"Milk".to_owned()));
    assert_eq!(reply.status, ReplyStatus::Error);
    assert_eq!(
        decode::<TodoError>(&reply.body),
        TodoError::Store(StoreError::Rejected("nope".to_owned()))
    );

    // The platform answers later: the command waits, then completes.
    core.t.host().script_port_async(port_id, save);
    core.t.host().take_port_calls();
    let id = core.start(method(store, "Todos", "add"), &enc(&"Milk".to_owned()));
    core.t.run_pending();
    assert!(
        core.t.take_replies().is_empty(),
        "the port has not answered yet"
    );
    let call = one(core.t.host().take_port_calls());
    core.t
        .runtime()
        .port_reply(&port_reply_ok(call.port_call_id, &[]));
    core.t.run_pending();
    let reply = one(core.t.take_replies());
    assert_eq!((reply.call_id, reply.status), (id, ReplyStatus::Ok));
}

#[test]
fn an_unavailable_port_is_contained_as_a_panic_reply() {
    // No fake and no script: the platform does not implement `Store`.
    let core = Core::without_fake();
    let store = core.todos();
    let reply = core.run(method(store, "Todos", "add"), &enc(&"Milk".to_owned()));
    assert_eq!(reply.status, ReplyStatus::Panic);
    let (message, _backtrace): (String, String) = decode(&reply.body);
    assert!(message.contains("Store.save"), "{message}");
    // The runtime still answers.
    let reply = core.sync(method(store, "Todos", "set_filter"), &enc(&Filter::All));
    assert_eq!(reply.status, ReplyStatus::Ok);
}

#[test]
fn snapshot_and_restore_rebuild_the_store_under_the_same_handle() {
    let core = Core::new();
    let store = core.todos();
    let calc = core.calculator(10);
    core.run(method(store, "Todos", "add"), &enc(&"Milk".to_owned()));
    core.run(method(store, "Todos", "add"), &enc(&"Eggs".to_owned()));
    core.sync(method(store, "Todos", "set_filter"), &enc(&Filter::Active));
    let snapshot = core.t.runtime().snapshot();

    // A new process: fresh runtime, restore, and the host's old handle still works.
    let revived = Core::new();
    revived
        .t
        .runtime()
        .restore(&snapshot)
        .expect("the snapshot restores");
    let initial = revived.observe(store);
    assert_eq!(signal_ids(&initial), [TODOS, FILTER, VISIBLE]);
    let mut eggs = milk(2);
    eggs.title = "Eggs".to_owned();
    let all = vec![milk(1), eggs];
    assert_eq!(decode::<Vec<Todo>>(&entry(&initial, TODOS).value), all);
    assert_eq!(
        decode::<Filter>(&entry(&initial, FILTER).value),
        Filter::Active
    );
    // The computed was rebuilt by the restore hook and is current.
    assert_eq!(decode::<Vec<Todo>>(&entry(&initial, VISIBLE).value), all);

    // The store is live: a command works and ids continue where they stopped.
    let reply = revived.run(method(store, "Todos", "add"), &enc(&"Bread".to_owned()));
    assert_eq!(reply.status, ReplyStatus::Ok);
    assert_eq!(decode::<Todo>(&reply.body).id, milk(3).id);
    let cs = one(revived.change_sets());
    assert_eq!(entry(&cs, TODOS).op, ChangeOp::KeyedPatch);

    // Plain objects are not snapshotted: the calculator's handle is stale in the new runtime.
    let reply = revived.sync(
        method(calc, "Calculator", "add"),
        &[enc(&1_i64), enc(&2_i64)].concat(),
    );
    assert_eq!(reply.status, ReplyStatus::BadRequest);
}

#[test]
fn a_no_coalesce_signal_reaches_the_platform_while_unobserved() {
    let core = Core::new();
    let ticker = core.construct("Ticker", &[]);
    // Nobody observes anything, yet `count` is delivered and `quiet` is not.
    let reply = core.sync(method(ticker, "Ticker", "bump"), &[]);
    assert_eq!(reply.status, ReplyStatus::Ok);
    let cs = one(core.change_sets());
    assert_eq!(signal_ids(&cs), [0]);
    assert_eq!(decode::<u32>(&entry(&cs, 0).value), 1);
    // Observing `quiet` sends its current value: the write it missed is not lost.
    core.t.runtime().observe(ticker.0, 1, true);
    let cs = one(core.change_sets());
    assert_eq!(signal_ids(&cs), [1]);
    assert_eq!(decode::<u32>(&entry(&cs, 1).value), 1);
}

#[test]
fn a_store_that_cannot_attach_its_signals_is_a_bad_request_not_a_panic() {
    let core = Core::new();
    let first = core.sync(
        CallTarget::Constructor {
            type_id: ids::type_id("Shared"),
            method_id: ids::method_id("Shared", "new"),
        },
        &[],
    );
    assert_eq!(first.status, ReplyStatus::Ok);
    // The second `Shared` reuses the first one's signal, which belongs to a store already.
    let second = core.sync(
        CallTarget::Constructor {
            type_id: ids::type_id("Shared"),
            method_id: ids::method_id("Shared", "new"),
        },
        &[],
    );
    assert_eq!(second.status, ReplyStatus::BadRequest);
    let reason: String = decode(&second.body);
    assert!(
        reason.contains("store `Shared` could not attach its signals")
            && reason.contains("already attached"),
        "{reason}"
    );
    // Nothing was published and the runtime is fine.
    assert_eq!(
        core.sync(
            CallTarget::Function {
                method_id: ids::function_id("version")
            },
            &[]
        )
        .status,
        ReplyStatus::Ok
    );
}

// ---------------------------------------------------------------------------------------------
// Scenarios: objects and functions
// ---------------------------------------------------------------------------------------------

#[test]
fn sync_method_and_free_function() {
    let core = Core::new();
    let calc = core.calculator(100);
    let reply = core.sync(
        method(calc, "Calculator", "add"),
        &[enc(&1_i64), enc(&2_i64)].concat(),
    );
    assert_eq!(reply.status, ReplyStatus::Ok);
    assert_eq!(decode::<i64>(&reply.body), 103);

    let reply = core.sync(
        CallTarget::Function {
            method_id: ids::function_id("version"),
        },
        &[],
    );
    assert_eq!(reply.status, ReplyStatus::Ok);
    assert_eq!(decode::<String>(&reply.body), "keel-e2e 1");

    // Malformed arguments are a bad request, not a crash.
    let reply = core.sync(method(calc, "Calculator", "add"), &[1, 2, 3]);
    assert_eq!(reply.status, ReplyStatus::BadRequest);
    // An async method cannot be called synchronously.
    let reply = core.sync(
        method(calc, "Calculator", "slow_add"),
        &[enc(&1_i64), enc(&2_i64)].concat(),
    );
    assert_eq!(reply.status, ReplyStatus::BadRequest);
}

#[test]
fn slow_add_completes_after_its_sleep() {
    let core = Core::new();
    let calc = core.calculator(1);
    let id = core.start(
        method(calc, "Calculator", "slow_add"),
        &[enc(&2_i64), enc(&3_i64)].concat(),
    );
    core.t.run_pending();
    assert!(core.t.take_replies().is_empty());
    core.t.advance(Duration::from_millis(999));
    assert!(core.t.take_replies().is_empty());
    core.t.advance(Duration::from_millis(1));
    let reply = one(core.t.take_replies());
    assert_eq!((reply.call_id, reply.status), (id, ReplyStatus::Ok));
    assert_eq!(decode::<i64>(&reply.body), 6);
}

#[test]
fn cancelling_an_in_flight_call_answers_cancelled_exactly_once() {
    let core = Core::new();
    let calc = core.calculator(1);
    let id = core.start(
        method(calc, "Calculator", "slow_add"),
        &[enc(&2_i64), enc(&3_i64)].concat(),
    );
    core.t.run_pending();
    assert!(core.t.take_replies().is_empty());

    core.t.runtime().cancel(id);
    let reply = one(core.t.take_replies());
    assert_eq!((reply.call_id, reply.status), (id, ReplyStatus::Cancelled));
    assert!(reply.body.is_empty());

    // The sleep would have finished by now; the cancelled call stays silent.
    core.t.advance(Duration::from_secs(5));
    assert!(core.t.take_replies().is_empty());
    // Cancelling again is harmless.
    core.t.runtime().cancel(id);
    assert!(core.t.take_replies().is_empty());
}

#[test]
fn a_panicking_method_is_status_two_and_the_runtime_keeps_answering() {
    let core = Core::new();
    let calc = core.calculator(5);
    let reply = core.sync(method(calc, "Calculator", "boom"), &[]);
    assert_eq!(reply.status, ReplyStatus::Panic);
    let (message, backtrace): (String, String) = decode(&reply.body);
    assert!(message.contains("kaboom"), "{message}");
    assert!(!backtrace.is_empty());
    // A fatal-level log record went to the host.
    assert!(
        core.t
            .host()
            .take_logs()
            .iter()
            .any(|l| l.level == 5 && l.message.contains("kaboom"))
    );

    // The very same object and the runtime still work.
    let reply = core.sync(
        method(calc, "Calculator", "add"),
        &[enc(&1_i64), enc(&1_i64)].concat(),
    );
    assert_eq!(reply.status, ReplyStatus::Ok);
    assert_eq!(decode::<i64>(&reply.body), 7);
    // Asynchronously too.
    let reply = core.run(method(calc, "Calculator", "boom"), &[]);
    assert_eq!(reply.status, ReplyStatus::Panic);
}

#[test]
fn an_err_result_is_status_one_with_the_typed_error() {
    let core = Core::new();
    let calc = core.calculator(5);
    let reply = core.sync(method(calc, "Calculator", "fail"), &[]);
    assert_eq!(reply.status, ReplyStatus::Error);
    assert_eq!(decode::<CalcError>(&reply.body), CalcError::Failed);
}

#[test]
fn a_stream_delivers_only_as_much_as_the_host_has_credited() {
    let core = Core::new();
    let calc = core.calculator(0);
    let id = core.start(method(calc, "Calculator", "ticks"), &enc(&5_u32));
    // The stream was opened; no items flow before credit.
    let opened = one(core.t.take_replies());
    assert_eq!(
        (opened.call_id, opened.status),
        (id, ReplyStatus::StreamOpened)
    );
    core.t.run_pending();
    assert!(core.t.host().take_stream_items().is_empty());

    core.t.runtime().stream_credit(id, 2);
    core.t.run_pending();
    let items = core.t.host().take_stream_items();
    assert_eq!(items.len(), 2, "{items:?}");
    assert!(
        items
            .iter()
            .all(|i| i.call_id == id && i.flag == StreamFlag::Item)
    );
    assert_eq!(decode::<u32>(&items[0].body), 0);
    assert_eq!(decode::<u32>(&items[1].body), 1);

    // Still only two until more credit arrives.
    core.t.run_pending();
    assert!(core.t.host().take_stream_items().is_empty());

    core.t.runtime().stream_credit(id, 10);
    core.t.run_pending();
    let items = core.t.host().take_stream_items();
    let flags: Vec<StreamFlag> = items.iter().map(|i| i.flag).collect();
    assert_eq!(
        flags,
        [
            StreamFlag::Item,
            StreamFlag::Item,
            StreamFlag::Item,
            StreamFlag::End
        ]
    );
    let values: Vec<u32> = items[..3].iter().map(|i| decode(&i.body)).collect();
    assert_eq!(values, [2, 3, 4]);
    assert!(items[3].body.is_empty());
    // The stream is over: no more replies, and further credit is ignored.
    core.t.runtime().stream_credit(id, 5);
    core.t.run_pending();
    assert!(core.t.host().take_stream_items().is_empty());
}

// ---------------------------------------------------------------------------------------------
// The schema and the generators
// ---------------------------------------------------------------------------------------------

#[test]
fn the_registered_schema_validates_and_generates_all_three_languages() {
    let schema = keel::meta::collect_schema("e2e-todo");
    schema.validate().expect("the collected schema is valid");
    assert!(schema.records.iter().any(|r| r.name == "Todo"));
    let todos = schema
        .objects
        .iter()
        .find(|o| o.name == "Todos")
        .expect("Todos is registered");
    let store_def = todos.store.as_ref().expect("Todos is a store");
    let signals: Vec<(&str, bool, Option<&str>)> = store_def
        .signals
        .iter()
        .map(|s| (s.name.as_str(), s.computed, s.key.as_deref()))
        .collect();
    assert_eq!(
        signals,
        [
            ("todos", false, Some("id")),
            ("filter", false, None),
            ("visible", true, None)
        ]
    );
    assert!(schema.ports.iter().any(|p| p.name == "Store"));
    assert!(schema.functions.iter().any(|f| f.name == "version"));
    // The runtime's own schema is this one (the crate name is a label, not part of the hash).
    let core = Core::new();
    assert_eq!(core.t.runtime().schema_hash(), schema.hash());

    let generator = keel_bindgen::Generator::for_crate("e2e-todo");
    let swift = generator.swift(&schema).expect("swift generates");
    let kotlin = generator.kotlin(&schema).expect("kotlin generates");
    let typescript = generator.typescript(&schema).expect("typescript generates");
    for (language, files) in [
        ("swift", &swift),
        ("kotlin", &kotlin),
        ("typescript", &typescript),
    ] {
        assert!(!files.is_empty(), "{language} produced no files");
        let all: String = files.iter().map(|f| f.contents.as_str()).collect();
        for needle in [
            "Todos",
            "Todo",
            "Filter",
            "TodoError",
            "Calculator",
            "version",
        ] {
            assert!(
                all.contains(needle),
                "{language} output does not mention {needle}"
            );
        }
    }
}

#[test]
fn nothing_is_left_undecoded_in_a_change_set() {
    // A guard for the helper above: a payload with trailing garbage does not decode.
    let core = Core::new();
    let store = core.todos();
    core.t.runtime().observe(store.0, ALL_SIGNALS, true);
    let mut payload = one(core.t.take_change_sets());
    assert!(ChangeSet::decode(&mut Reader::new(&payload)).is_ok());
    payload.truncate(payload.len() - 1);
    assert!(ChangeSet::decode(&mut Reader::new(&payload)).is_err());
}
