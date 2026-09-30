//! A naive stand-in for keel-runtime: the object table, the port table and the types the
//! generated dispatchers, proxies and restore functions name (SPEC 16.2).

use core::any::Any;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use keel_meta::{DispatchCall, PortKind, ids};
use keel_wire::{Reader, WireError};

use crate::signals::StoreCell;

pub use futures_core::Stream;
pub use keel_wire::Handle;

/// Anything the object table can hold.
pub trait KeelObject: Send + Sync + 'static {
    /// `fnv1a32(type name)`.
    const TYPE_ID: u32;
    /// The type name.
    const NAME: &'static str;
}

/// An object with signals.
pub trait StoreObject: KeelObject {
    /// The cell the signals are attached to.
    fn cell(&self) -> &Arc<StoreCell>;
    /// Rebuilds the store from the store body of a snapshot.
    fn restore(ctx: Ctx, r: &mut Reader<'_>) -> Result<Self, WireError>
    where
        Self: Sized;
}

/// A restore function registered by `#[keel::store]`.
pub struct StoreRestorer {
    /// The store's type id.
    pub type_id: u32,
    /// Rebuilds the store from a snapshot body.
    pub restore: fn(Ctx, &mut Reader<'_>) -> Result<Arc<dyn Any + Send + Sync>, WireError>,
}
keel_meta::inventory::collect!(StoreRestorer);

/// A port trait (`dyn Trait`).
pub trait Port: Send + Sync + 'static {
    /// `fnv1a32("port.<Trait>")`.
    const PORT_ID: u32;
    /// The trait name.
    const NAME: &'static str;
    /// Sync, async or event.
    const KIND: PortKind;
}

/// A Rust-side port dispatcher registered by `#[keel::port]`: it decodes an encoded call,
/// runs it on an implementation and encodes the outcome.
pub struct PortDispatcher {
    /// The port id.
    pub port_id: u32,
    /// Calls `imp` (an `Arc<dyn Trait>` behind `dyn Any`) with an encoded call.
    pub dispatch: fn(&(dyn Any + Send + Sync), u32, &[u8]) -> PortDispatch,
}
keel_meta::inventory::collect!(PortDispatcher);

/// The outcome of dispatching a port call on a Rust implementation.
///
/// Both variants carry `status u8` followed by the body: `0` ok (the return value), `1` typed
/// error (the `E` value), `2` unavailable (unknown method or undecodable arguments).
pub enum PortDispatch {
    /// The implementation answered synchronously.
    Sync(Vec<u8>),
    /// The implementation answers later.
    Async(Pin<Box<dyn Future<Output = Vec<u8>> + Send>>),
}

/// What a dispatcher answers.
pub enum DispatchResult {
    /// Finished: ok bytes or typed-error bytes.
    Sync(Result<Vec<u8>, Vec<u8>>),
    /// A future that finishes with ok or typed-error bytes.
    Async(Pin<Box<dyn Future<Output = Result<Vec<u8>, Vec<u8>>> + Send>>),
    /// A stream of item or typed-error bytes.
    Stream(Pin<Box<dyn Stream<Item = Result<Vec<u8>, Vec<u8>>> + Send>>),
    /// Unknown method, malformed arguments or a stale handle.
    Unknown,
}

impl core::fmt::Debug for DispatchResult {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DispatchResult::Sync(r) => write!(f, "Sync({r:?})"),
            DispatchResult::Async(_) => f.write_str("Async(..)"),
            DispatchResult::Stream(_) => f.write_str("Stream(..)"),
            DispatchResult::Unknown => f.write_str("Unknown"),
        }
    }
}

impl DispatchResult {
    /// The bytes of a synchronous success.
    pub fn sync_ok(self) -> Vec<u8> {
        match self {
            DispatchResult::Sync(Ok(bytes)) => bytes,
            other => panic!("expected a synchronous success, got {other:?}"),
        }
    }

    /// The bytes of a synchronous typed error.
    pub fn sync_err(self) -> Vec<u8> {
        match self {
            DispatchResult::Sync(Err(bytes)) => bytes,
            other => panic!("expected a synchronous typed error, got {other:?}"),
        }
    }

    /// Runs an asynchronous result to completion.
    pub fn run_async(self) -> Result<Vec<u8>, Vec<u8>> {
        match self {
            DispatchResult::Async(future) => crate::testing::block_on(future),
            other => panic!("expected an asynchronous result, got {other:?}"),
        }
    }

    /// Drains a stream result.
    pub fn run_stream(self) -> Vec<Result<Vec<u8>, Vec<u8>>> {
        match self {
            DispatchResult::Stream(stream) => crate::testing::collect(stream),
            other => panic!("expected a stream, got {other:?}"),
        }
    }

    /// Whether this is the "unknown" answer.
    pub fn is_unknown(&self) -> bool {
        matches!(self, DispatchResult::Unknown)
    }
}

/// A handle that does not refer to a live object of the requested type.
#[derive(Debug)]
pub struct BadHandle;

/// Why a port call failed.
#[derive(Debug)]
pub enum PortError {
    /// No binding, or the binding could not answer synchronously.
    Unavailable,
    /// The caller went away.
    Cancelled,
    /// The reply could not be decoded.
    Decode(WireError),
    /// The port answered with a typed error (the encoded `E`).
    Failed(Vec<u8>),
}

/// A pending port call.
pub struct PortFuture(Pin<Box<dyn Future<Output = Result<Vec<u8>, PortError>> + Send>>);

impl Future for PortFuture {
    type Output = Result<Vec<u8>, PortError>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.0.as_mut().poll(cx)
    }
}

/// A listener registration.
pub struct Subscription {
    id: u64,
}

impl Subscription {
    /// The registration id.
    pub fn id(&self) -> u64 {
        self.id
    }
}

type Listener = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// Event fan-out.
#[derive(Default)]
pub struct Events {
    listeners: Mutex<Vec<(u64, u32, u32, Listener)>>,
}

impl Events {
    /// Registers a listener for `(port_id, method_id)`.
    pub fn subscribe(
        &self,
        port_id: u32,
        method_id: u32,
        listener: Box<dyn Fn(&[u8]) + Send + Sync>,
    ) -> Subscription {
        let mut listeners = self.listeners.lock().unwrap();
        let id = listeners.len() as u64 + 1;
        listeners.push((id, port_id, method_id, Arc::from(listener)));
        Subscription { id }
    }

    /// Delivers an event payload to every listener (what `Runtime::event` does).
    pub fn deliver(&self, port_id: u32, method_id: u32, payload: &[u8]) {
        let targets: Vec<Listener> = self
            .listeners
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, p, m, _)| *p == port_id && *m == method_id)
            .map(|(_, _, _, l)| Arc::clone(l))
            .collect();
        for listener in targets {
            listener(payload);
        }
    }
}

type Foreign = Arc<dyn Fn(u32, &[u8]) -> Result<Vec<u8>, PortError> + Send + Sync>;

enum Binding {
    Rust(Arc<dyn Any + Send + Sync>),
    Foreign(Foreign),
}

struct Slot {
    generation: u32,
    object: Option<Arc<dyn Any + Send + Sync>>,
}

struct Inner {
    slots: Mutex<Vec<Slot>>,
    ports: Mutex<HashMap<u32, Binding>>,
    events: Events,
}

/// The runtime: an object table, a port table and an event hub.
pub struct Runtime {
    inner: Arc<Inner>,
}

impl Default for Runtime {
    fn default() -> Self {
        Runtime::new()
    }
}

impl Runtime {
    /// A new, empty runtime.
    pub fn new() -> Runtime {
        Runtime {
            inner: Arc::new(Inner {
                slots: Mutex::new(Vec::new()),
                ports: Mutex::new(HashMap::new()),
                events: Events::default(),
            }),
        }
    }

    /// The context handed to constructors and free functions.
    pub fn ctx(&self) -> Ctx {
        Ctx(Arc::clone(&self.inner))
    }

    /// Looks up an object by handle.
    pub fn object<T: KeelObject>(&self, handle: u64) -> Result<Arc<T>, BadHandle> {
        let handle = Handle(handle);
        let slots = self.inner.slots.lock().unwrap();
        let slot = slots.get(handle.index() as usize).ok_or(BadHandle)?;
        if slot.generation != handle.generation() {
            return Err(BadHandle);
        }
        let object = slot.object.as_ref().ok_or(BadHandle)?;
        Arc::clone(object).downcast::<T>().map_err(|_| BadHandle)
    }

    /// Inserts an object and returns its handle.
    pub fn insert_object<T: KeelObject>(&self, object: Arc<T>) -> Handle {
        let mut slots = self.inner.slots.lock().unwrap();
        let index = u32::try_from(slots.len()).unwrap();
        slots.push(Slot {
            generation: 1,
            object: Some(object),
        });
        Handle::new(index, 1)
    }

    /// Binds a Rust implementation of a port (`imp` is an `Arc<Arc<dyn Trait>>` behind `Any`).
    pub fn bind_port<P: ?Sized + 'static>(&self, port_id: u32, imp: Arc<dyn Any + Send + Sync>) {
        self.inner
            .ports
            .lock()
            .unwrap()
            .insert(port_id, Binding::Rust(imp));
    }

    /// Binds a "foreign" implementation: a closure standing in for the host.
    pub fn bind_foreign(
        &self,
        port_id: u32,
        handler: impl Fn(u32, &[u8]) -> Result<Vec<u8>, PortError> + Send + Sync + 'static,
    ) {
        self.inner
            .ports
            .lock()
            .unwrap()
            .insert(port_id, Binding::Foreign(Arc::new(handler)));
    }

    /// Fans an event payload out to subscribers (`Runtime::event` in the real runtime).
    pub fn event(&self, port_id: u32, method_id: u32, payload: &[u8]) {
        self.inner.events.deliver(port_id, method_id, payload);
    }

    // --- test drivers -----------------------------------------------------------------

    fn outcome(
        dispatch: keel_meta::DispatchFn,
        rt: &Runtime,
        call: DispatchCall<'_>,
    ) -> DispatchResult {
        dispatch(rt, call)
            .downcast::<DispatchResult>()
            .unwrap_or_else(|_| panic!("dispatcher returned something other than a DispatchResult"))
    }

    /// Calls a method or constructor of the registered object `type_name`.
    pub fn call_object(
        &self,
        type_name: &str,
        method: &str,
        handle: u64,
        args: &[u8],
    ) -> DispatchResult {
        let meta = keel_meta::registrations()
            .find_map(|r| match r {
                keel_meta::Registration::Object(o) if o.name == type_name => Some(*o),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no registered object `{type_name}`"));
        let call = DispatchCall {
            method_id: ids::method_id(type_name, method),
            call_id: 1,
            handle,
            args,
        };
        Runtime::outcome(meta.dispatch, self, call)
    }

    /// Calls the registered free function `name`.
    pub fn call_function(&self, name: &str, args: &[u8]) -> DispatchResult {
        let meta = keel_meta::registrations()
            .find_map(|r| match r {
                keel_meta::Registration::Function(f) if f.name == name => Some(*f),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no registered function `{name}`"));
        let call = DispatchCall {
            method_id: ids::function_id(name),
            call_id: 1,
            handle: 0,
            args,
        };
        Runtime::outcome(meta.dispatch, self, call)
    }

    /// Calls the object dispatcher with an arbitrary method id.
    pub fn call_object_raw(
        &self,
        type_name: &str,
        method_id: u32,
        handle: u64,
        args: &[u8],
    ) -> DispatchResult {
        let meta = keel_meta::registrations()
            .find_map(|r| match r {
                keel_meta::Registration::Object(o) if o.name == type_name => Some(*o),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no registered object `{type_name}`"));
        let call = DispatchCall {
            method_id,
            call_id: 1,
            handle,
            args,
        };
        Runtime::outcome(meta.dispatch, self, call)
    }
}

/// A cheap handle to the runtime, passed to constructors and free functions.
#[derive(Clone)]
pub struct Ctx(Arc<Inner>);

impl Ctx {
    /// The event hub.
    pub fn events(&self) -> &Events {
        &self.0.events
    }

    /// Calls a port method asynchronously.
    pub fn port_call(&self, port_id: u32, method_id: u32, args: Vec<u8>) -> PortFuture {
        let inner = Arc::clone(&self.0);
        PortFuture(Box::pin(async move {
            let foreign = match inner.ports.lock().unwrap().get(&port_id) {
                Some(Binding::Foreign(f)) => Arc::clone(f),
                _ => return Err(PortError::Unavailable),
            };
            foreign(method_id, &args)
        }))
    }

    /// Calls a port method synchronously.
    pub fn port_call_sync(
        &self,
        port_id: u32,
        method_id: u32,
        args: &[u8],
    ) -> Result<Vec<u8>, PortError> {
        let foreign = match self.0.ports.lock().unwrap().get(&port_id) {
            Some(Binding::Foreign(f)) => Arc::clone(f),
            _ => return Err(PortError::Unavailable),
        };
        foreign(method_id, args)
    }

    /// The Rust binding of a port, if one is bound.
    pub fn rust_port<P: Port + ?Sized>(&self, port_id: u32) -> Option<Arc<P>> {
        match self.0.ports.lock().unwrap().get(&port_id) {
            Some(Binding::Rust(any)) => Arc::clone(any)
                .downcast::<Arc<P>>()
                .ok()
                .map(|boxed| Arc::clone(&*boxed)),
            _ => None,
        }
    }
}

impl core::fmt::Debug for Ctx {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Ctx")
    }
}
