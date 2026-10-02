//! Handing objects to the host: the issue scope, and the origins that own what was handed out
//! (ADR-040).
//!
//! A method that returns an object (`Arc<Mailbox>`, `Option<Arc<..>>`, `Vec<Arc<..>>`) does not
//! encode the object: the generated dispatcher lowers each one to a [`Handle`] first, with
//! [`IssueScope::issue`], and encodes the handles. Every handle in a reply is **one reference the
//! host now owns** (the table counts them, [`ObjectTable`](crate::object_table::ObjectTable)),
//! so a reference must never be issued and then lost with the reply.
//!
//! The scope is that guarantee. It records what the call issued; [`commit`](IssueScope::commit)
//! says the reply carries them, and a scope that is dropped any other way gives every reference
//! back (a panic between two issues, a body that was cancelled, an error while encoding). The
//! scope lives inside the dispatcher (a synchronous method) or inside the call's future (an
//! asynchronous one, around its last poll: the future is polled, and the call answered, under the
//! core lock, so nothing can cancel a call between its issue and its reply). That is the ledger
//! of ADR-040 decision 6, held where the call is rather than in a table next to it.
//!
//! # Origins
//!
//! A client that is not in the process (an `undra dev` session) cannot give its references back
//! when it disconnects, so the runtime keeps them for it: [`Runtime::call_from`] names the
//! **origin** of a call, the runtime records the handles that call committed, and
//! [`Runtime::release_origin`] gives back what an origin still holds.
//!
//! [`Runtime::call_from`]: crate::Runtime::call_from
//! [`Runtime::release_origin`]: crate::Runtime::release_origin

use core::cell::Cell;
use core::fmt;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use parking_lot::Mutex;
use undra_wire::Handle;

use crate::object::{UndraObject, any_object};
use crate::runtime::Runtime;

/// Why an object could not be handed to the host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IssueError {
    /// A store's signals could not be attached to its cell (for example because a signal
    /// already belongs to another store).
    Attach(String),
}

impl fmt::Display for IssueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            IssueError::Attach(why) => write!(f, "the store could not attach its signals: {why}"),
        }
    }
}

impl std::error::Error for IssueError {}

/// The references one call has handed to the host and not yet committed. Dropping it without
/// [`commit`](IssueScope::commit) gives them all back.
///
/// ```
/// use std::sync::Arc;
/// use undra_runtime::testing::TestRuntime;
/// use undra_runtime::{UndraObject, Handle};
///
/// struct Mailbox;
/// impl UndraObject for Mailbox {
///     const TYPE_ID: u32 = 7;
///     const NAME: &'static str = "Mailbox";
/// }
///
/// let t = TestRuntime::new();
/// let rt = t.runtime();
/// let inbox = Arc::new(Mailbox);
///
/// // A reply that carries the handle owns one reference.
/// let mut scope = rt.issue_scope();
/// let handle = scope.issue(inbox.clone()).unwrap();
/// scope.commit();
/// assert_eq!(rt.objects().host_refs_of(handle), Some(1));
///
/// // The same object again is the same handle with one more reference ...
/// let mut scope = rt.issue_scope();
/// assert_eq!(scope.issue(inbox.clone()).unwrap(), handle);
/// scope.commit();
/// assert_eq!(rt.objects().host_refs_of(handle), Some(2));
///
/// // ... and a call that did not answer gives its reference back.
/// let mut scope = rt.issue_scope();
/// scope.issue(inbox).unwrap();
/// drop(scope);
/// assert_eq!(rt.objects().host_refs_of(handle), Some(2));
/// ```
#[must_use = "an IssueScope that is dropped gives its references back; commit it once the reply carries them"]
pub struct IssueScope<'a> {
    rt: &'a Runtime,
    issued: Vec<Handle>,
}

impl<'a> IssueScope<'a> {
    pub(crate) fn new(rt: &'a Runtime) -> IssueScope<'a> {
        IssueScope {
            rt,
            issued: Vec::new(),
        }
    }

    /// Hands `object` to the host: the handle it already has with one more reference, or a new,
    /// transient entry (a snapshot leaves it out and a restore makes it stale). A store's signals
    /// are attached first.
    ///
    /// # Errors
    ///
    /// [`IssueError::Attach`] if a store cannot attach its signals; nothing was issued.
    ///
    /// # Panics
    ///
    /// Once the process has issued every handle generation (logged at FATAL first, ADR-022);
    /// the scope gives back what it had issued while the panic unwinds.
    pub fn issue<T: UndraObject>(&mut self, object: Arc<T>) -> Result<Handle, IssueError> {
        self.issue_as(object, true)
    }

    /// Like [`issue`](IssueScope::issue) for a constructor that returns `Arc<Self>` (the
    /// singleton pattern): a new entry is not transient, the host constructed it.
    ///
    /// # Errors
    ///
    /// As [`issue`](IssueScope::issue).
    pub fn issue_constructed<T: UndraObject>(
        &mut self,
        object: Arc<T>,
    ) -> Result<Handle, IssueError> {
        self.issue_as(object, false)
    }

    fn issue_as<T: UndraObject>(
        &mut self,
        object: Arc<T>,
        transient: bool,
    ) -> Result<Handle, IssueError> {
        object
            .__undra_attach()
            .map_err(|e| IssueError::Attach(e.to_string()))?;
        let address = Arc::as_ptr(&object).cast::<()>() as usize;
        let (handle, _new) =
            self.rt
                .objects()
                .issue_with(address, || any_object(Arc::clone(&object)), transient);
        self.issued.push(handle);
        Ok(handle)
    }

    /// How many references this scope holds.
    pub fn len(&self) -> usize {
        self.issued.len()
    }

    /// Whether nothing was issued.
    pub fn is_empty(&self) -> bool {
        self.issued.is_empty()
    }

    /// The reply carries what was issued: the references are the host's now, and are recorded
    /// against the call's origin when it has one.
    pub fn commit(mut self) {
        let issued = core::mem::take(&mut self.issued);
        let origin = current_origin();
        if origin != 0 && !issued.is_empty() {
            self.rt.origins().record(origin, &issued);
        }
    }
}

impl IssueScope<'_> {
    /// [`commit`](IssueScope::commit) for a constructor's reply (an `Arc<Self>` singleton): the
    /// references are the caller's own, which the one that made the constructor call counts (the
    /// transport's session, per reference), so they are not recorded against the call's origin as
    /// well: a reference lives in exactly one ledger.
    pub fn commit_constructed(mut self) {
        self.issued.clear();
    }
}

impl Drop for IssueScope<'_> {
    fn drop(&mut self) {
        // Uncommitted: nobody will ever release these, so give them back. The caller holds the
        // core lock (a dispatcher, or a task poll), which is what releasing needs.
        for handle in self.issued.drain(..).rev() {
            self.rt.release_issued(handle);
        }
    }
}

// ----- origins ---------------------------------------------------------------------------

thread_local! {
    /// The origin of the call being served on this thread (`0`: the process's own embedder).
    static ORIGIN: Cell<u64> = const { Cell::new(0) };
}

/// The origin of the call running on this thread.
pub(crate) fn current_origin() -> u64 {
    ORIGIN.try_with(Cell::get).unwrap_or(0)
}

/// Sets the thread's origin for as long as it lives.
pub(crate) struct OriginScope {
    previous: u64,
}

impl OriginScope {
    pub(crate) fn enter(origin: u64) -> OriginScope {
        let previous = ORIGIN.try_with(|o| o.replace(origin)).unwrap_or(0);
        OriginScope { previous }
    }
}

impl Drop for OriginScope {
    fn drop(&mut self) {
        let previous = self.previous;
        let _ = ORIGIN.try_with(|o| o.set(previous));
    }
}

/// A call's future, polled with its origin set: an asynchronous method issues its handles in its
/// last poll, which the executor runs on whichever thread it is on.
pub(crate) struct WithOrigin<F> {
    origin: u64,
    inner: Pin<Box<F>>,
}

impl<F> WithOrigin<F> {
    pub(crate) fn new(origin: u64, inner: F) -> WithOrigin<F> {
        WithOrigin {
            origin,
            inner: Box::pin(inner),
        }
    }
}

impl<F: Future> Future for WithOrigin<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let _origin = OriginScope::enter(self.origin);
        self.inner.as_mut().poll(cx)
    }
}

/// What each origin holds: its handles, with how many references of each.
#[derive(Default)]
pub(crate) struct Origins {
    held: Mutex<HashMap<u64, BTreeMap<u64, u32>>>,
}

impl Origins {
    /// `origin` committed `handles` (one reference each, repeats counted).
    pub(crate) fn record(&self, origin: u64, handles: &[Handle]) {
        let mut held = self.held.lock();
        let of = held.entry(origin).or_default();
        for handle in handles {
            let count = of.entry(handle.0).or_insert(0);
            *count = count.saturating_add(1);
        }
    }

    /// `origin` gave one reference to `handle` back.
    pub(crate) fn forget_one(&self, origin: u64, handle: Handle) {
        let mut held = self.held.lock();
        if let Some(of) = held.get_mut(&origin) {
            if let Some(count) = of.get_mut(&handle.0) {
                *count -= 1;
                if *count == 0 {
                    of.remove(&handle.0);
                }
            }
            if of.is_empty() {
                held.remove(&origin);
            }
        }
    }

    /// Takes everything `origin` holds, handle order.
    pub(crate) fn take(&self, origin: u64) -> Vec<(Handle, u32)> {
        self.held
            .lock()
            .remove(&origin)
            .map(|of| of.into_iter().map(|(h, n)| (Handle(h), n)).collect())
            .unwrap_or_default()
    }

    /// How many references every origin holds together (`stats_json`'s `origin_refs`).
    pub(crate) fn total(&self) -> u64 {
        self.held
            .lock()
            .values()
            .map(|of| of.values().map(|&n| u64::from(n)).sum::<u64>())
            .sum()
    }
}
