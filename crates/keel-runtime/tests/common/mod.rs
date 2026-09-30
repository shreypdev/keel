//! Hand-written fixtures standing in for macro output: a `Counter` store, a few free
//! functions, their metadata and dispatchers, and helpers to call them.
#![allow(dead_code)]

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;

use keel_meta::{
    DispatchCall, DispatchOutcome, FunctionMeta, MethodMeta, ObjectMeta, ParamMeta, Registration,
    SignalMeta, StoreMeta, TypeRefMeta, ids,
};
use keel_runtime::testing::{ReplyRecord, TestRuntime, call_payload, decode_reply};
use keel_runtime::{
    Ctx, DispatchBytes, DispatchResult, KeelObject, LazyList, PortError, Runtime, StoreObject,
    StoreRestorer, store,
};
use keel_signals::{Signal, StoreCell};
use keel_wire::payload::{CallTarget, ReplyStatus};
use keel_wire::{Decode, Encode, Handle, Reader, Writer};

// ----- ports used by the fixtures ---------------------------------------------------------

pub const TEST_PORT: u32 = ids::port_id("TestPort");
pub const ASK: u32 = ids::port_method_id("TestPort", "ask");

// ----- Counter: a store with two signals --------------------------------------------------

pub const COUNT_SIGNAL: u32 = 0;
pub const LABEL_SIGNAL: u32 = 1;

pub struct Counter {
    cell: Arc<StoreCell>,
    pub count: Signal<i32>,
    pub label: Signal<String>,
    ctx: Ctx,
}

impl Counter {
    fn build(ctx: Ctx, initial: i32, label: &str) -> Counter {
        let cell = StoreCell::new(<Counter as KeelObject>::TYPE_ID);
        let count = Signal::new(initial);
        let label = Signal::new(label.to_owned());
        cell.attach(&count, COUNT_SIGNAL, None);
        cell.attach(&label, LABEL_SIGNAL, None);
        Counter {
            cell,
            count,
            label,
            ctx,
        }
    }

    pub fn new(ctx: Ctx, initial: i32, label: &str) -> Arc<Counter> {
        Arc::new(Counter::build(ctx, initial, label))
    }
}

impl KeelObject for Counter {
    const TYPE_ID: u32 = ids::type_id("Counter");
    const NAME: &'static str = "Counter";
}

impl StoreObject for Counter {
    fn cell(&self) -> &Arc<StoreCell> {
        &self.cell
    }

    fn restore(ctx: Ctx, r: &mut Reader<'_>) -> Result<Self, keel_wire::WireError> {
        let signals = r.read_u32()?;
        let mut count = 0_i32;
        let mut label = String::new();
        for _ in 0..signals {
            let id = r.read_u32()?;
            let value = r.read_bytes()?;
            let mut vr = Reader::new(value);
            match id {
                COUNT_SIGNAL => count = i32::decode(&mut vr)?,
                LABEL_SIGNAL => label = String::decode(&mut vr)?,
                _ => {}
            }
        }
        Ok(Counter::build(ctx, count, &label))
    }
}

pub fn enc<T: Encode + ?Sized>(value: &T) -> Vec<u8> {
    let mut w = Writer::new();
    value.encode(&mut w);
    w.into_vec()
}

pub fn args(f: impl FnOnce(&mut Writer)) -> Vec<u8> {
    let mut w = Writer::new();
    f(&mut w);
    w.into_vec()
}

fn decode_args<T: Decode>(call: &DispatchCall<'_>) -> Result<T, DispatchResult> {
    let mut r = Reader::new(call.args);
    T::decode(&mut r)
        .and_then(|v| r.finish().map(|()| v))
        .map_err(|e| DispatchResult::BadRequest(format!("bad arguments: {e}")))
}

/// The counting stream fixture: `0..end`, optionally failing at `fail_at`.
struct Count {
    next: u32,
    end: u32,
    fail_at: Option<u32>,
    panic_at: Option<u32>,
}

impl futures_core::Stream for Count {
    type Item = DispatchBytes;

    fn poll_next(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<DispatchBytes>> {
        if Some(self.next) == self.panic_at {
            panic!("stream kaboom");
        }
        if Some(self.next) == self.fail_at {
            self.next = self.end;
            return Poll::Ready(Some(Err(enc("stream failed"))));
        }
        if self.next >= self.end {
            return Poll::Ready(None);
        }
        let item = enc(&(self.next as i32));
        self.next += 1;
        Poll::Ready(Some(Ok(item)))
    }
}

macro_rules! method_ids {
    ($($name:ident = $s:literal;)*) => {
        $(pub const $name: u32 = ids::method_id("Counter", $s);)*
    };
}

method_ids! {
    NEW = "new";
    GET = "get";
    ADD = "add";
    ADD_TWICE = "add_twice";
    LABEL = "label";
    FAIL = "fail";
    BOOM = "boom";
    SLOW_ADD = "slow_add";
    FOREVER = "forever";
    ASK_PORT = "ask_port";
    TICKS = "ticks";
    TICKS_FAIL = "ticks_fail";
    TICKS_PANIC = "ticks_panic";
    PANIC_ASYNC = "panic_async";
    BLOCKING_SQUARE = "blocking_square";
    BLOCKING_BOOM = "blocking_boom";
    CURRENT_RUNTIME_ID = "current_runtime_id";
    EVEN_MORE = "not_in_metadata";
    SPAWN_LATER = "spawn_later";
    THREADS = "threads";
}

fn counter_dispatch(rt: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
    let Some(rt) = rt.downcast_ref::<Runtime>() else {
        return DispatchOutcome::new(DispatchResult::Unknown);
    };
    DispatchOutcome::new(counter_call(rt, call))
}

fn counter_call(rt: &Runtime, call: DispatchCall<'_>) -> DispatchResult {
    if call.method_id == NEW {
        let (initial, label): (i32, String) = match decode_args(&call) {
            Ok(v) => v,
            Err(bad) => return bad,
        };
        let counter = Counter::new(rt.ctx(), initial, &label);
        let handle = rt.insert_store(counter);
        return DispatchResult::Sync(Ok(enc(&handle.0)));
    }
    let counter = match rt.object::<Counter>(call.handle) {
        Ok(c) => c,
        Err(e) => return DispatchResult::BadRequest(e.to_string()),
    };
    match call.method_id {
        GET => DispatchResult::Sync(Ok(enc(&counter.count.get()))),
        LABEL => DispatchResult::Sync(Ok(enc(&counter.label.get()))),
        ADD => {
            let n: i32 = match decode_args(&call) {
                Ok(v) => v,
                Err(bad) => return bad,
            };
            counter.count.update(|c| *c += n);
            DispatchResult::Sync(Ok(enc(&counter.count.get())))
        }
        ADD_TWICE => {
            let n: i32 = match decode_args(&call) {
                Ok(v) => v,
                Err(bad) => return bad,
            };
            counter.ctx.txn(|| {
                counter.count.update(|c| *c += n);
                counter.label.set(format!("n={n}"));
            });
            DispatchResult::Sync(Ok(Vec::new()))
        }
        FAIL => {
            let code: u16 = match decode_args(&call) {
                Ok(v) => v,
                Err(bad) => return bad,
            };
            DispatchResult::Sync(Err(enc(&code)))
        }
        BOOM => panic!("kaboom"),
        CURRENT_RUNTIME_ID => DispatchResult::Sync(Ok(enc(&Ctx::current().runtime().id()))),
        SLOW_ADD => {
            let n: i32 = match decode_args(&call) {
                Ok(v) => v,
                Err(bad) => return bad,
            };
            let ctx = rt.ctx();
            DispatchResult::Async(Box::pin(async move {
                ctx.sleep(Duration::from_millis(10)).await;
                counter.count.update(|c| *c += n);
                Ok(enc(&counter.count.get()))
            }))
        }
        FOREVER => DispatchResult::Async(Box::pin(async move {
            std::future::pending::<()>().await;
            Ok(Vec::new())
        })),
        ASK_PORT => {
            let x: i32 = match decode_args(&call) {
                Ok(v) => v,
                Err(bad) => return bad,
            };
            let ctx = rt.ctx();
            DispatchResult::Async(Box::pin(async move {
                let _keep = counter;
                match ctx.port_call(TEST_PORT, ASK, enc(&x)).await {
                    Ok(body) => Ok(body),
                    Err(PortError::Failed(e)) => Err(e),
                    Err(other) => Err(enc(&other.to_string())),
                }
            }))
        }
        TICKS | TICKS_FAIL | TICKS_PANIC => {
            let n: u32 = match decode_args(&call) {
                Ok(v) => v,
                Err(bad) => return bad,
            };
            DispatchResult::Stream(Box::pin(Count {
                next: 0,
                end: n,
                fail_at: (call.method_id == TICKS_FAIL).then_some(n / 2),
                panic_at: (call.method_id == TICKS_PANIC).then_some(n / 2),
            }))
        }
        PANIC_ASYNC => DispatchResult::Async(Box::pin(async move {
            keel_runtime::executor::yield_now().await;
            let _keep = counter;
            panic!("async kaboom");
        })),
        BLOCKING_SQUARE => {
            let n: i32 = match decode_args(&call) {
                Ok(v) => v,
                Err(bad) => return bad,
            };
            let ctx = rt.ctx();
            DispatchResult::Async(Box::pin(async move {
                let _keep = counter;
                let squared = ctx.spawn_blocking(move || n * n).await;
                Ok(enc(&squared))
            }))
        }
        BLOCKING_BOOM => {
            let ctx = rt.ctx();
            DispatchResult::Async(Box::pin(async move {
                let _keep = counter;
                let n: i32 = ctx
                    .spawn_blocking(|| -> i32 { panic!("blocking kaboom") })
                    .await;
                Ok(enc(&n))
            }))
        }
        THREADS => {
            // Reports the thread that runs the async body, the blocking closure and the
            // continuation after it, as "<body thread>|<blocking thread>|<after thread>".
            let ctx = rt.ctx();
            let name = || std::thread::current().name().unwrap_or("?").to_owned();
            DispatchResult::Async(Box::pin(async move {
                let _keep = counter;
                let body = name();
                let blocking = ctx.spawn_blocking(move || {
                    std::thread::current().name().unwrap_or("?").to_owned()
                });
                let blocking = blocking.await;
                Ok(enc(&format!("{body}|{blocking}|{}", name())))
            }))
        }
        SPAWN_LATER => {
            // Fire-and-forget: a detached task adds 100 after a yield.
            let ctx = rt.ctx();
            ctx.spawn(async move {
                keel_runtime::executor::yield_now().await;
                counter.count.update(|c| *c += 100);
            });
            DispatchResult::Sync(Ok(Vec::new()))
        }
        _ => DispatchResult::Unknown,
    }
}

const fn param(name: &'static str, ty: TypeRefMeta) -> ParamMeta {
    ParamMeta { name, ty }
}

const fn method(
    name: &'static str,
    method_id: u32,
    params: &'static [ParamMeta],
    returns: TypeRefMeta,
    is_async: bool,
) -> MethodMeta {
    MethodMeta {
        name,
        method_id,
        params,
        returns,
        is_async,
        takes_ctx: false,
        docs: "",
    }
}

static COUNTER_META: ObjectMeta = ObjectMeta {
    name: "Counter",
    type_id: ids::type_id("Counter"),
    constructors: &[method(
        "new",
        NEW,
        &[
            param("initial", TypeRefMeta::I32),
            param("label", TypeRefMeta::String),
        ],
        TypeRefMeta::Named("Counter"),
        false,
    )],
    methods: &[
        method("get", GET, &[], TypeRefMeta::I32, false),
        method("label", LABEL, &[], TypeRefMeta::String, false),
        method(
            "add",
            ADD,
            &[param("n", TypeRefMeta::I32)],
            TypeRefMeta::I32,
            false,
        ),
        method(
            "add_twice",
            ADD_TWICE,
            &[param("n", TypeRefMeta::I32)],
            TypeRefMeta::Unit,
            false,
        ),
        method(
            "fail",
            FAIL,
            &[param("code", TypeRefMeta::U16)],
            TypeRefMeta::Result(&TypeRefMeta::Unit, &TypeRefMeta::U16),
            false,
        ),
        method("boom", BOOM, &[], TypeRefMeta::Unit, false),
        method(
            "slow_add",
            SLOW_ADD,
            &[param("n", TypeRefMeta::I32)],
            TypeRefMeta::I32,
            true,
        ),
        method("forever", FOREVER, &[], TypeRefMeta::Unit, true),
        method(
            "ask_port",
            ASK_PORT,
            &[param("x", TypeRefMeta::I32)],
            TypeRefMeta::I32,
            true,
        ),
        method(
            "ticks",
            TICKS,
            &[param("n", TypeRefMeta::U32)],
            TypeRefMeta::Stream(&TypeRefMeta::I32),
            false,
        ),
        method(
            "ticks_fail",
            TICKS_FAIL,
            &[param("n", TypeRefMeta::U32)],
            TypeRefMeta::Stream(&TypeRefMeta::I32),
            false,
        ),
        method(
            "ticks_panic",
            TICKS_PANIC,
            &[param("n", TypeRefMeta::U32)],
            TypeRefMeta::Stream(&TypeRefMeta::I32),
            false,
        ),
        method("panic_async", PANIC_ASYNC, &[], TypeRefMeta::Unit, true),
        method(
            "blocking_square",
            BLOCKING_SQUARE,
            &[param("n", TypeRefMeta::I32)],
            TypeRefMeta::I32,
            true,
        ),
        method("blocking_boom", BLOCKING_BOOM, &[], TypeRefMeta::I32, true),
        method(
            "current_runtime_id",
            CURRENT_RUNTIME_ID,
            &[],
            TypeRefMeta::U64,
            false,
        ),
        method("spawn_later", SPAWN_LATER, &[], TypeRefMeta::Unit, false),
        method("threads", THREADS, &[], TypeRefMeta::String, true),
    ],
    store: Some(StoreMeta {
        signals: &[
            SignalMeta {
                name: "count",
                signal_id: COUNT_SIGNAL,
                ty: TypeRefMeta::I32,
                computed: false,
                key: None,
            },
            SignalMeta {
                name: "label",
                signal_id: LABEL_SIGNAL,
                ty: TypeRefMeta::String,
                computed: false,
                key: None,
            },
        ],
    }),
    docs: "",
    dispatch: counter_dispatch,
};

keel_meta::inventory::submit! { Registration::Object(&COUNTER_META) }

keel_meta::inventory::submit! {
    StoreRestorer {
        type_id: ids::type_id("Counter"),
        restore: |ctx, r| Ok(store(Arc::new(<Counter as StoreObject>::restore(ctx, r)?))),
    }
}

// ----- free functions ---------------------------------------------------------------------

pub const ECHO: u32 = ids::function_id("echo");
pub const SUM: u32 = ids::function_id("sum");
pub const MAKE_LAZY: u32 = ids::function_id("make_lazy");
pub const SLOW_FN: u32 = ids::function_id("slow_fn");
pub const CURRENT_LEVEL_LOG: u32 = ids::function_id("log_something");

fn functions_dispatch(rt: &dyn Any, call: DispatchCall<'_>) -> DispatchOutcome {
    let Some(rt) = rt.downcast_ref::<Runtime>() else {
        return DispatchOutcome::new(DispatchResult::Unknown);
    };
    DispatchOutcome::new(match call.method_id {
        ECHO => {
            let bytes: keel_wire::Bytes = match decode_args(&call) {
                Ok(v) => v,
                Err(bad) => return DispatchOutcome::new(bad),
            };
            DispatchResult::Sync(Ok(enc(&bytes)))
        }
        SUM => {
            let (a, b): (i32, i32) = match decode_args(&call) {
                Ok(v) => v,
                Err(bad) => return DispatchOutcome::new(bad),
            };
            DispatchResult::Sync(Ok(enc(&(a + b))))
        }
        MAKE_LAZY => {
            let n: u32 = match decode_args(&call) {
                Ok(v) => v,
                Err(bad) => return DispatchOutcome::new(bad),
            };
            let list = LazyList::new();
            for i in 0..n {
                list.push_encoded(&(i as i32 * 10));
            }
            DispatchResult::Sync(Ok(enc(&rt.insert_lazy_list(&list).0)))
        }
        SLOW_FN => DispatchResult::Async(Box::pin(async {
            keel_runtime::executor::yield_now().await;
            Ok(enc(&7_i32))
        })),
        CURRENT_LEVEL_LOG => {
            keel_runtime::keel_warn!("from a free function");
            DispatchResult::Sync(Ok(Vec::new()))
        }
        _ => DispatchResult::Unknown,
    })
}

const fn function(
    name: &'static str,
    method_id: u32,
    params: &'static [ParamMeta],
    returns: TypeRefMeta,
    is_async: bool,
) -> FunctionMeta {
    FunctionMeta {
        name,
        method_id,
        params,
        returns,
        is_async,
        takes_ctx: false,
        docs: "",
        dispatch: functions_dispatch,
    }
}

static ECHO_META: FunctionMeta = function(
    "echo",
    ECHO,
    &[param("data", TypeRefMeta::Bytes)],
    TypeRefMeta::Bytes,
    false,
);
static SUM_META: FunctionMeta = function(
    "sum",
    SUM,
    &[param("a", TypeRefMeta::I32), param("b", TypeRefMeta::I32)],
    TypeRefMeta::I32,
    false,
);
static MAKE_LAZY_META: FunctionMeta = function(
    "make_lazy",
    MAKE_LAZY,
    &[param("n", TypeRefMeta::U32)],
    TypeRefMeta::Lazy(&TypeRefMeta::I32),
    false,
);
static SLOW_FN_META: FunctionMeta = function("slow_fn", SLOW_FN, &[], TypeRefMeta::I32, true);
static LOG_META: FunctionMeta = function(
    "log_something",
    CURRENT_LEVEL_LOG,
    &[],
    TypeRefMeta::Unit,
    false,
);

keel_meta::inventory::submit! { Registration::Function(&ECHO_META) }
keel_meta::inventory::submit! { Registration::Function(&SUM_META) }
keel_meta::inventory::submit! { Registration::Function(&MAKE_LAZY_META) }
keel_meta::inventory::submit! { Registration::Function(&SLOW_FN_META) }
keel_meta::inventory::submit! { Registration::Function(&LOG_META) }

// ----- helpers ----------------------------------------------------------------------------

pub fn counter_target(handle: Handle, method_id: u32) -> CallTarget {
    CallTarget::Method { handle, method_id }
}

pub fn function_target(method_id: u32) -> CallTarget {
    CallTarget::Function { method_id }
}

pub fn expect_ok(reply: &ReplyRecord) -> &[u8] {
    assert_eq!(reply.status, ReplyStatus::Ok, "reply: {reply:?}");
    &reply.body
}

pub fn decode_body<T: Decode>(reply: &ReplyRecord) -> T {
    let mut r = Reader::new(expect_ok(reply));
    let value = T::decode(&mut r).expect("reply body decodes");
    r.finish().expect("reply body fully consumed");
    value
}

pub fn reason_of(reply: &ReplyRecord) -> String {
    let mut r = Reader::new(&reply.body);
    r.read_str().expect("status 5 body is a String").to_owned()
}

/// Constructs a `Counter` through `call_sync` and returns its handle.
pub fn new_counter(t: &TestRuntime, initial: i32, label: &str) -> Handle {
    let reply = t.call_sync(
        CallTarget::Constructor {
            type_id: ids::type_id("Counter"),
            method_id: NEW,
        },
        1,
        &args(|w| {
            initial.encode(w);
            label.encode(w);
        }),
    );
    Handle(decode_body::<u64>(&reply))
}

/// Sync call on a counter, decoded reply.
pub fn call_counter(
    t: &TestRuntime,
    handle: Handle,
    method_id: u32,
    call_id: u32,
    a: &[u8],
) -> ReplyRecord {
    t.call_sync(counter_target(handle, method_id), call_id, a)
}

/// Builds a raw call payload for a counter method.
pub fn counter_call_payload(handle: Handle, method_id: u32, call_id: u32, a: &[u8]) -> Vec<u8> {
    call_payload(counter_target(handle, method_id), call_id, a)
}

pub fn reply_of(rt: &Runtime, payload: &[u8]) -> ReplyRecord {
    decode_reply(&rt.call_sync(payload))
}

/// Decodes a `Vec<(signal_id, value bytes)>` out of the entries of a decoded change-set.
pub fn entries_of(cs: &keel_wire::payload::ChangeSet) -> Vec<(u64, u32, Vec<u8>)> {
    cs.entries
        .iter()
        .map(|e| (e.handle.0, e.signal_id, e.value.clone()))
        .collect()
}

/// A future that never completes and counts its own drops (for cancellation tests).
pub struct DropCounter(pub Arc<AtomicU32>);

impl Drop for DropCounter {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

pub fn never_with_guard(guard: DropCounter) -> Pin<Box<dyn Future<Output = DispatchBytes> + Send>> {
    Box::pin(async move {
        let _guard = guard;
        std::future::pending::<()>().await;
        Ok(Vec::new())
    })
}

// ----- stores whose restore misbehaves (restore error paths) -------------------------------

pub const PANICKY: u32 = ids::type_id("Panicky");
pub const REJECTING: u32 = ids::type_id("Rejecting");

macro_rules! misbehaving_store {
    ($name:ident, $type_id:expr, $label:literal, |$ctx:ident, $r:ident| $restore:expr) => {
        pub struct $name {
            cell: Arc<StoreCell>,
        }

        impl KeelObject for $name {
            const TYPE_ID: u32 = $type_id;
            const NAME: &'static str = $label;
        }

        impl StoreObject for $name {
            fn cell(&self) -> &Arc<StoreCell> {
                &self.cell
            }

            fn restore($ctx: Ctx, $r: &mut Reader<'_>) -> Result<Self, keel_wire::WireError> {
                $restore
            }
        }

        keel_meta::inventory::submit! {
            StoreRestorer {
                type_id: $type_id,
                restore: |ctx, r| Ok(store(Arc::new(<$name as StoreObject>::restore(ctx, r)?))),
            }
        }
    };
}

misbehaving_store!(Panicky, PANICKY, "Panicky", |_ctx, _r| panic!(
    "restore kaboom"
));
misbehaving_store!(Rejecting, REJECTING, "Rejecting", |_ctx, _r| Err(
    keel_wire::WireError::InvalidTag {
        tag: 9,
        at: 0,
        ty: "Rejecting"
    }
));

// ----- helpers for tests that use a real (threaded) `Runtime` -------------------------------

/// `Runtime::call_sync` with a payload built from its parts, decoded.
pub fn call_sync_rt(rt: &Runtime, target: CallTarget, call_id: u32, a: &[u8]) -> ReplyRecord {
    decode_reply(&rt.call_sync(&call_payload(target, call_id, a)))
}

/// Constructs a `Counter` on a real runtime.
pub fn new_counter_rt(rt: &Runtime, initial: i32, label: &str) -> Handle {
    let reply = call_sync_rt(
        rt,
        CallTarget::Constructor {
            type_id: ids::type_id("Counter"),
            method_id: NEW,
        },
        1,
        &args(|w| {
            initial.encode(w);
            label.encode(w);
        }),
    );
    Handle(decode_body::<u64>(&reply))
}

/// Runs `f` on a helper thread and fails the test if it takes longer than `limit`: a deadlock
/// must fail loudly instead of hanging the test run.
pub fn with_timeout<T: Send + 'static>(
    what: &str,
    limit: std::time::Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(limit) {
        Ok(value) => value,
        Err(_) => panic!("{what}: no result within {limit:?} (deadlock?)"),
    }
}

/// Polls `cond` until it holds or `limit` passes.
pub fn wait_until(limit: std::time::Duration, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    cond()
}
