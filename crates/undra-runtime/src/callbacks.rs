//! Host callback interfaces: the core's side of `#[undra::callback]` traits (ADR-041).
//!
//! A callback interface is a port with many instances. The host implements the trait and passes
//! an instance to a method (`Arc<dyn UploadListener>`); on the wire the argument is the
//! instance's `u64` handle, which the **host** chose. The core turns it into a *proxy*: an
//! `Arc<dyn UploadListener>` whose methods are port calls (SPEC 3.6) on the trait's port id,
//! with the instance as the first argument.
//!
//! * **A client's proxies are its own.** A remote client (`undra dev`) numbers its instances from 1,
//!   as every other client does, so the core keys the proxies it interns by the client's *origin* as
//!   well (`Runtime::call_from`), and delivers a proxy's calls only while its client is the one
//!   attached ([`Runtime::set_client_origin`]): the proxies of a session that left (its objects kept
//!   for its return, or still held by something that outlived it) never reach another session's
//!   instance of the same number.
//! * **One crossing, one reference.** Every instance handle in a call's arguments is one
//!   reference the core now owns. The core **interns**: decoding an instance it already has a
//!   live proxy for returns that proxy (so `Arc::ptr_eq` holds for one host listener) and gives
//!   the duplicate reference back at once with the reserved `__release` method. A proxy sends
//!   one `__release` when it drops.
//! * **Fire-and-forget** methods are port calls with `port_call_id 0`; **async** methods are
//!   ordinary port calls. When the future awaiting one is dropped the proxy sends the reserved
//!   `__cancel` (`instance`, `port_call_id`) and the runtime abandons the id as for any port
//!   call.
//! * **Never under the core lock, never inline** is the host's rule (it enqueues and returns);
//!   the core's is that a proxy holds a [`WeakCtx`], so a listener kept by a long-lived object
//!   does not keep the runtime alive, and after shutdown a fire-and-forget call does nothing and
//!   an async one answers [`PortError::Cancelled`].
//!
//! Generated code never names this module's internals: `#[undra::callback]` implements
//! [`CallbackInterface`] for `dyn Trait` and a proxy over a [`CallbackHandle`], and the
//! dispatchers of methods that take a callback call [`Runtime::callback`].

use core::any::Any;
use core::fmt;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::HashMap;
use std::sync::{Arc, Weak};

use parking_lot::Mutex;
use undra_meta::ids::{callback_cancel_id, callback_release_id};
use undra_wire::Writer;

use crate::ctx::WeakCtx;
use crate::ports::{Port, PortError, PortFuture};
use crate::runtime::Runtime;

/// A `#[undra::callback]` trait, as the runtime sees it: a port whose implementation the host
/// passes in, one instance at a time. `#[undra::callback]` implements it for `dyn Trait`.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a callback interface",
    label = "this trait was not declared with `#[undra::callback]`",
    note = "error[undra::E0004]: `{Self}` cannot be taken as a callback\n  = note: only a trait declared with `#[undra::callback]` has a proxy the core can call and a generated protocol the host implements\n  = help: add `#[undra::callback]` to the trait, or pass a record or an id\n  = docs: https://shreypdev.github.io/undra/docs/errors.html#E0004"
)]
pub trait CallbackInterface: Port {
    /// The proxy of one host instance: an `Arc<dyn Trait>` whose methods call the host.
    fn proxy(handle: CallbackHandle) -> Arc<Self>;
}

/// One host instance of a callback interface, as the core holds it: the port, the instance
/// handle the host chose, and a weak reference to the runtime. Dropping it gives the reference
/// back to the host (`__release`).
pub struct CallbackHandle {
    ctx: WeakCtx,
    port_id: u32,
    name: &'static str,
    instance: u64,
    /// The client whose instance this is (`0`: the process's own embedder).
    origin: u64,
}

impl CallbackHandle {
    fn new(
        ctx: WeakCtx,
        port_id: u32,
        name: &'static str,
        instance: u64,
        origin: u64,
    ) -> CallbackHandle {
        CallbackHandle {
            ctx,
            port_id,
            name,
            instance,
            origin,
        }
    }

    /// The instance handle the host chose.
    pub fn instance(&self) -> u64 {
        self.instance
    }

    /// Calls a fire-and-forget method: `port_call_id 0`, nothing is read back. A runtime that is
    /// gone makes it a no-op.
    pub fn notify(&self, method_id: u32, args: impl FnOnce(&mut Writer)) {
        let Ok(ctx) = self.ctx.upgrade() else {
            return;
        };
        if !ctx.runtime().callback_deliverable(self.origin) {
            return;
        }
        let mut w = Writer::new();
        w.write_u64(self.instance);
        args(&mut w);
        ctx.runtime()
            .port_notify(self.port_id, method_id, w.into_vec());
    }

    /// Calls an `async` method: an ordinary port call, answered through `port_reply`. Dropping
    /// the returned future before it completes sends `__cancel`. A runtime that is gone answers
    /// [`PortError::Cancelled`].
    pub fn call(&self, method_id: u32, args: impl FnOnce(&mut Writer)) -> CallbackCall {
        let Ok(ctx) = self.ctx.upgrade() else {
            return CallbackCall {
                future: None,
                gone: true,
                cancel: None,
            };
        };
        if !ctx.runtime().callback_deliverable(self.origin) {
            // The client these calls belong to is not the attached one: nobody to ask.
            return CallbackCall {
                future: None,
                gone: true,
                cancel: None,
            };
        }
        let mut w = Writer::new();
        w.write_u64(self.instance);
        args(&mut w);
        let future = ctx
            .runtime()
            .port_call(self.port_id, method_id, w.into_vec());
        let id = future.port_call_id();
        CallbackCall {
            future: Some(future),
            gone: false,
            // An id of 0 is a call that is already complete (shut down, or answered by a Rust
            // binding): there is nothing to cancel.
            cancel: (id != 0).then(|| Cancel {
                ctx: self.ctx.clone(),
                port_id: self.port_id,
                method_id: callback_cancel_id(self.name),
                instance: self.instance,
                port_call_id: id,
                origin: self.origin,
            }),
        }
    }
}

impl fmt::Debug for CallbackHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallbackHandle")
            .field("interface", &self.name)
            .field("instance", &self.instance)
            .finish()
    }
}

impl Drop for CallbackHandle {
    fn drop(&mut self) {
        let Ok(ctx) = self.ctx.upgrade() else {
            return;
        };
        let rt = ctx.runtime();
        rt.callbacks()
            .prune(self.origin, self.port_id, self.instance);
        release(rt, self.origin, self.port_id, self.name, self.instance);
    }
}

/// Gives one reference to `instance` of the client `origin` back to the host: nothing when that
/// client is not the one attached (its numbers mean nothing to another client).
fn release(rt: &Runtime, origin: u64, port_id: u32, name: &str, instance: u64) {
    if !rt.callback_deliverable(origin) {
        return;
    }
    rt.port_notify(
        port_id,
        callback_release_id(name),
        instance.to_le_bytes().to_vec(),
    );
}

struct Cancel {
    ctx: WeakCtx,
    port_id: u32,
    method_id: u32,
    instance: u64,
    port_call_id: u32,
    origin: u64,
}

/// The future of an `async` callback method: resolves to the encoded reply, like a
/// [`PortFuture`]. Dropping it before it completes cancels the call on the host.
#[must_use = "a CallbackCall does nothing unless awaited; dropping it cancels the call"]
pub struct CallbackCall {
    future: Option<PortFuture>,
    gone: bool,
    cancel: Option<Cancel>,
}

impl Future for CallbackCall {
    type Output = Result<Vec<u8>, PortError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        if self.gone {
            return Poll::Ready(Err(PortError::Cancelled));
        }
        let Some(future) = self.future.as_mut() else {
            return Poll::Ready(Err(PortError::Cancelled));
        };
        match Pin::new(future).poll(cx) {
            Poll::Ready(result) => {
                // Answered: nothing is left for the host to cancel.
                self.cancel = None;
                Poll::Ready(result)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for CallbackCall {
    fn drop(&mut self) {
        let Some(cancel) = self.cancel.take() else {
            return;
        };
        let Ok(ctx) = cancel.ctx.upgrade() else {
            return;
        };
        if !ctx.runtime().callback_deliverable(cancel.origin) {
            return;
        }
        let mut w = Writer::new();
        w.write_u64(cancel.instance);
        w.write_u32(cancel.port_call_id);
        // The abandonment of the id itself is the `PortFuture`'s drop, which follows.
        ctx.runtime()
            .port_notify(cancel.port_id, cancel.method_id, w.into_vec());
    }
}

// ----- the proxy intern map --------------------------------------------------------------

/// A weak reference to a proxy of any interface.
trait ErasedWeak: Send + Sync {
    fn alive(&self) -> bool;
    fn as_any(&self) -> &dyn Any;
}

impl<P: ?Sized + Send + Sync + 'static> ErasedWeak for Weak<P> {
    fn alive(&self) -> bool {
        self.strong_count() > 0
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// The live proxies of one runtime, by `(origin, port id, instance)`: how the core interns the
/// host's instances (ADR-041 decision 5). The origin is the client that lent the instance, which
/// numbers its instances from 1 like every other client.
#[derive(Default)]
pub(crate) struct CallbackRegistry {
    live: Mutex<HashMap<(u64, u32, u64), Box<dyn ErasedWeak>>>,
}

impl CallbackRegistry {
    /// Forgets `(origin, port_id, instance)` if its proxy is gone (a newer proxy of the same
    /// instance may have taken the entry meanwhile, and stays).
    pub(crate) fn prune(&self, origin: u64, port_id: u32, instance: u64) {
        let mut live = self.live.lock();
        if live
            .get(&(origin, port_id, instance))
            .is_some_and(|weak| !weak.alive())
        {
            live.remove(&(origin, port_id, instance));
        }
    }

    /// How many instances have a live proxy.
    pub(crate) fn live(&self) -> usize {
        self.live
            .lock()
            .values()
            .filter(|weak| weak.alive())
            .count()
    }
}

impl Runtime {
    /// The proxy of the host instance `instance` of the callback interface `P`: what a
    /// generated dispatcher calls for a parameter `Arc<dyn P>`, **after** every argument has
    /// decoded (a refused call transfers nothing, ADR-041 decision 5).
    ///
    /// The instance is one reference the core now owns. If a live proxy for it exists that
    /// proxy is returned and this crossing's reference is given back at once; otherwise a new
    /// proxy is made, which gives the reference back when it drops.
    ///
    /// ```
    /// use std::sync::Arc;
    /// use undra_runtime::testing::TestRuntime;
    /// use undra_runtime::{CallbackHandle, CallbackInterface, Port};
    ///
    /// trait Listener: Send + Sync { fn ping(&self); }
    /// struct ListenerProxy(CallbackHandle);
    /// impl Listener for ListenerProxy { fn ping(&self) {} }
    /// impl Port for dyn Listener {
    ///     const PORT_ID: u32 = 9;
    ///     const NAME: &'static str = "Listener";
    ///     const KIND: undra_runtime::undra_meta::PortKind = undra_runtime::undra_meta::PortKind::Callback;
    /// }
    /// impl CallbackInterface for dyn Listener {
    ///     fn proxy(handle: CallbackHandle) -> Arc<dyn Listener> { Arc::new(ListenerProxy(handle)) }
    /// }
    ///
    /// let t = TestRuntime::new();
    /// let first = t.runtime().callback::<dyn Listener>(41);
    /// let second = t.runtime().callback::<dyn Listener>(41);
    /// assert!(Arc::ptr_eq(&first, &second));
    /// // The duplicate reference went back to the host at once.
    /// assert_eq!(t.host().port_calls().len(), 1);
    /// ```
    pub fn callback<P: ?Sized + CallbackInterface>(&self, instance: u64) -> Arc<P> {
        // The client this call is served for: its instance 1 is not another client's.
        let origin = crate::issue::current_origin();
        let key = (origin, P::PORT_ID, instance);
        {
            let live = self.callbacks().live.lock();
            if let Some(existing) = live
                .get(&key)
                .and_then(|weak| weak.as_any().downcast_ref::<Weak<P>>())
                .and_then(Weak::upgrade)
            {
                drop(live);
                release(self, origin, P::PORT_ID, P::NAME, instance);
                return existing;
            }
        }
        let handle = CallbackHandle::new(
            self.ctx().downgrade(),
            P::PORT_ID,
            P::NAME,
            instance,
            origin,
        );
        let proxy = P::proxy(handle);
        self.callbacks()
            .live
            .lock()
            .insert(key, Box::new(Arc::downgrade(&proxy)));
        proxy
    }
}
