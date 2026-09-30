//! Test support shared by the behaviour tests: a small driver over the *real* `keel-runtime`.
//!
//! The behaviour tests call generated dispatchers, proxies and restore functions directly.
//! [`Runtime`] wraps a real `keel_runtime::Runtime` (built without threads) with the calls those
//! tests need: look a dispatcher up in the registry and call it, bind a "platform" closure to a
//! port, deliver an event, and record what the runtime sends to the host (change-sets).
#![allow(dead_code)]

use core::any::Any;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use keel::meta::{DispatchCall, DispatchFn, Registration, ids};
use keel::runtime::testing::port_reply;
use keel::runtime::{
    Ctx, DispatchResult, Host, InitError, KeelObject, MODE_INPROC, PortCallOutcome, PortError,
    RuntimeConfig, Stream,
};
use keel::wire::Reader;
use keel::wire::payload::{ChangeSet, PortStatus};

type Foreign = Arc<dyn Fn(u32, &[u8]) -> Result<Vec<u8>, PortError> + Send + Sync>;

/// The "platform": answers port calls from closures and records change-sets.
#[derive(Default)]
struct Platform {
    foreign: Mutex<HashMap<u32, Foreign>>,
    change_sets: Mutex<Vec<Vec<u8>>>,
}

impl Host for Platform {
    fn reply(&self, _call_id: u32, _payload: &[u8]) {}

    fn change_set(&self, payload: &[u8]) {
        self.change_sets.lock().unwrap().push(payload.to_vec());
    }

    fn stream_item(&self, _call_id: u32, _payload: &[u8]) {}

    fn port_call(
        &self,
        port_id: u32,
        method_id: u32,
        port_call_id: u32,
        args: &[u8],
    ) -> PortCallOutcome {
        let handler = self.foreign.lock().unwrap().get(&port_id).cloned();
        match handler.map(|handler| handler(method_id, args)) {
            Some(Ok(body)) => {
                PortCallOutcome::Sync(port_reply(port_call_id, PortStatus::Ok, &body))
            }
            Some(Err(PortError::Failed(body))) => {
                PortCallOutcome::Sync(port_reply(port_call_id, PortStatus::Error, &body))
            }
            Some(Err(_)) | None => PortCallOutcome::Unavailable,
        }
    }

    fn log(&self, _level: u8, _target: &str, _message: &str) {}
}

/// A real runtime plus the test platform behind it.
pub struct Runtime {
    rt: Arc<keel::runtime::Runtime>,
    platform: Arc<Platform>,
}

impl Runtime {
    /// A runtime with no threads and a platform that implements no port.
    pub fn new() -> Runtime {
        let platform = Arc::new(Platform::default());
        let config = RuntimeConfig {
            platform: "test".to_owned(),
            mode: MODE_INPROC.to_owned(),
            core_threads: 0,
            blocking_threads: 0,
            log_level: 0,
        };
        let rt = keel::runtime::Runtime::new(config, platform.clone())
            .unwrap_or_else(|e: InitError| panic!("the test runtime did not start: {e}"));
        // The tests call dispatchers and poll futures directly on this thread, which therefore
        // plays the core: its signal writes are allowed.
        keel::runtime::testing::drive_from_this_thread();
        Runtime { rt, platform }
    }

    /// The context handed to constructors and free functions.
    pub fn ctx(&self) -> Ctx {
        self.rt.ctx()
    }

    /// The real runtime.
    pub fn real(&self) -> &Arc<keel::runtime::Runtime> {
        &self.rt
    }

    /// Looks up an object by handle.
    pub fn object<T: KeelObject>(
        &self,
        handle: u64,
    ) -> Result<Arc<T>, keel::runtime::object_table::BadHandle> {
        self.rt.object::<T>(handle)
    }

    /// Binds a Rust implementation of a port (`imp` is an `Arc<Arc<dyn Trait>>` behind `Any`).
    pub fn bind_port<P: ?Sized + 'static>(&self, port_id: u32, imp: Arc<dyn Any + Send + Sync>) {
        self.rt.bind_port::<P>(port_id, imp);
    }

    /// Makes the "platform" implement a port: `handler` gets the method id and the encoded
    /// arguments and answers like a platform binding would.
    pub fn bind_foreign(
        &self,
        port_id: u32,
        handler: impl Fn(u32, &[u8]) -> Result<Vec<u8>, PortError> + Send + Sync + 'static,
    ) {
        self.platform
            .foreign
            .lock()
            .unwrap()
            .insert(port_id, Arc::new(handler));
        self.rt.bind_foreign_port(port_id);
    }

    /// Fans an event payload out to subscribers.
    pub fn event(&self, port_id: u32, method_id: u32, payload: &[u8]) {
        self.rt.event(port_id, method_id, payload);
    }

    /// The change-sets the runtime delivered to the platform since the last call, decoded.
    pub fn change_sets(&self) -> Vec<ChangeSet> {
        std::mem::take(&mut *self.platform.change_sets.lock().unwrap())
            .iter()
            .map(|bytes| ChangeSet::decode(&mut Reader::new(bytes)).expect("a change-set"))
            .collect()
    }

    fn outcome(&self, dispatch: DispatchFn, call: DispatchCall<'_>) -> Dispatched {
        // Signal writes made by the dispatched code find this runtime through `Ctx::current`.
        let _scope = self.ctx().enter();
        let rt: &keel::runtime::Runtime = &self.rt;
        match dispatch(rt, call).downcast::<DispatchResult>() {
            Ok(result) => Dispatched(result),
            Err(_) => panic!("the dispatcher returned something other than a DispatchResult"),
        }
    }

    /// Calls a method or constructor of the registered object `type_name`.
    pub fn call_object(
        &self,
        type_name: &str,
        method: &str,
        handle: u64,
        args: &[u8],
    ) -> Dispatched {
        self.call_object_raw(type_name, ids::method_id(type_name, method), handle, args)
    }

    /// Calls the object dispatcher with an arbitrary method id.
    pub fn call_object_raw(
        &self,
        type_name: &str,
        method_id: u32,
        handle: u64,
        args: &[u8],
    ) -> Dispatched {
        let meta = keel::meta::registrations()
            .find_map(|r| match r {
                Registration::Object(o) if o.name == type_name => Some(*o),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no registered object `{type_name}`"));
        let call = DispatchCall {
            method_id,
            call_id: 1,
            handle,
            args,
        };
        self.outcome(meta.dispatch, call)
    }

    /// Calls the registered free function `name`.
    pub fn call_function(&self, name: &str, args: &[u8]) -> Dispatched {
        let meta = keel::meta::registrations()
            .find_map(|r| match r {
                Registration::Function(f) if f.name == name => Some(*f),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no registered function `{name}`"));
        let call = DispatchCall {
            method_id: ids::function_id(name),
            call_id: 1,
            handle: 0,
            args,
        };
        self.outcome(meta.dispatch, call)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // Stores and in-flight calls hold a `Ctx`, which is a cycle with the runtime.
        self.rt.shutdown();
    }
}

/// What a dispatcher answered, with the accessors the tests use.
pub struct Dispatched(pub DispatchResult);

impl Dispatched {
    /// The bytes of a synchronous success.
    pub fn sync_ok(self) -> Vec<u8> {
        match self.0 {
            DispatchResult::Sync(Ok(bytes)) => bytes,
            other => panic!("expected a synchronous success, got {other:?}"),
        }
    }

    /// The bytes of a synchronous typed error.
    pub fn sync_err(self) -> Vec<u8> {
        match self.0 {
            DispatchResult::Sync(Err(bytes)) => bytes,
            other => panic!("expected a synchronous typed error, got {other:?}"),
        }
    }

    /// Runs an asynchronous result to completion.
    pub fn run_async(self) -> Result<Vec<u8>, Vec<u8>> {
        match self.0 {
            DispatchResult::Async(future) => testing::block_on(future),
            other => panic!("expected an asynchronous result, got {other:?}"),
        }
    }

    /// Drains a stream result.
    pub fn run_stream(self) -> Vec<Result<Vec<u8>, Vec<u8>>> {
        match self.0 {
            DispatchResult::Stream(stream) => testing::collect(stream),
            other => panic!("expected a stream, got {other:?}"),
        }
    }

    /// Whether this is the "unknown method" answer.
    pub fn is_unknown(&self) -> bool {
        matches!(self.0, DispatchResult::Unknown)
    }

    /// The reason of a bad request (undecodable arguments, stale handle, failed attach).
    pub fn bad_request(self) -> String {
        match self.0 {
            DispatchResult::BadRequest(reason) => reason,
            other => panic!("expected a bad request, got {other:?}"),
        }
    }
}

/// Helpers for driving generated futures and streams.
pub mod testing {
    use super::*;

    /// Polls `future` to completion on the current thread. Panics if it stays pending (nothing
    /// in these tests waits for another thread).
    pub fn block_on<F: Future>(future: F) -> F::Output {
        let mut cx = Context::from_waker(Waker::noop());
        let mut future = Box::pin(future);
        for _ in 0..10_000 {
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
        }
        panic!("future did not complete");
    }

    /// Drains a stream to the end on the current thread.
    pub fn collect<S: Stream + ?Sized>(stream: Pin<Box<S>>) -> Vec<S::Item> {
        let mut cx = Context::from_waker(Waker::noop());
        let mut stream = stream;
        let mut items = Vec::new();
        for _ in 0..10_000 {
            match stream.as_mut().poll_next(&mut cx) {
                Poll::Ready(Some(item)) => items.push(item),
                Poll::Ready(None) => return items,
                Poll::Pending => {}
            }
        }
        panic!("stream did not finish");
    }

    /// A stream that yields the items of an iterator.
    pub struct Iter<I>(pub I);

    impl<I: Iterator + Unpin> Stream for Iter<I> {
        type Item = I::Item;

        fn poll_next(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Poll::Ready(self.0.next())
        }
    }

    /// A stream over `items`.
    pub fn stream_of<T>(items: Vec<T>) -> Iter<std::vec::IntoIter<T>> {
        Iter(items.into_iter())
    }
}
