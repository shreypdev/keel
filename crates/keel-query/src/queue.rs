//! The offline queue (SPEC 9): idempotent mutations that failed for lack of a network wait here
//! and are replayed, first in first out, when the platform reports connectivity again.
//!
//! * A mutation is queued when it is marked `idempotent`, fails with an `HttpError::Network`
//!   (its own error type may wrap it) **and** the client believes the device is offline.
//!   Anything else fails at once: a non-idempotent mutation is never queued, and a network error
//!   while the platform says it is online is an ordinary failure.
//! * The queue is persisted under `keel.query.queue` (see [`crate::persist`]) after every
//!   change, by a single writer task so writes cannot land out of order, and read back by
//!   `hydrate`, so queued mutations survive a restart.
//! * The caller of a queued mutation keeps waiting: `.await` resolves when the replay does, with
//!   the mutation's own result. Dropping the future does not unqueue the mutation.
//! * A replay that fails with a network error again stays at the head of the queue. While the
//!   client is offline the replay stops and waits for the next `online` event; while it is
//!   online (the platform says so, the server disagrees) it retries with backoff. Any other
//!   error is the server's answer: the mutation leaves the queue and its caller gets the error.
//! * Every run of an idempotent mutation carries the same idempotency key (a random UUID from the
//!   `Rng` port made when it first ran), readable inside the mutation with [`idempotency_key`],
//!   so a server can drop a request it has already served.

use core::cell::Cell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use keel_ports::{CtxPorts, HttpError};
use keel_runtime::Ctx;
use keel_runtime::executor::Notify;
use keel_runtime::log::{DEBUG, WARN};
use keel_wire::{Bytes, Uuid};
use parking_lot::Mutex;

use crate::defs::BoxFuture;
use crate::erased::{Erased, Failure, MutationVTable, Outcome, registered_mutation};
use crate::key::Invalidate;
use crate::persist::{QUEUE_KEY, QueuedMutation, decode_queue, encode_queue};
use crate::retry::{backoff_sleep, with_retries};
use crate::shared::{Shared, State};
use crate::walk::{error_type, probe_network_error};

thread_local! {
    static IDEMPOTENCY_KEY: Cell<Option<Uuid>> = const { Cell::new(None) };
}

/// The idempotency key of the mutation being run, if it is an idempotent one.
///
/// Call it from inside a `#[keel::mutation(idempotent)]` function to send the key to the server
/// (an `Idempotency-Key` header, say). The key is made when the mutation first runs and stays
/// the same across its retries and its replays from the offline queue. `None` outside an
/// idempotent mutation. It is read from the task that runs the mutation body: work the body
/// hands to `ctx.spawn` does not see it.
///
/// ```
/// assert_eq!(keel_query::idempotency_key(), None);
/// ```
pub fn idempotency_key() -> Option<Uuid> {
    IDEMPOTENCY_KEY.with(Cell::get)
}

/// Runs a future with the idempotency key set for the duration of every poll.
struct WithKey<T> {
    key: Uuid,
    inner: BoxFuture<T>,
}

impl<T> Future for WithKey<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        struct Restore(Option<Uuid>);
        impl Drop for Restore {
            fn drop(&mut self) {
                IDEMPOTENCY_KEY.with(|slot| slot.set(self.0));
            }
        }
        let _restore = Restore(IDEMPOTENCY_KEY.with(|slot| slot.replace(Some(self.key))));
        self.inner.as_mut().poll(cx)
    }
}

/// `fut`, with [`idempotency_key`] answering `key` while it runs.
pub(crate) fn scoped<T: 'static>(key: Option<Uuid>, fut: BoxFuture<T>) -> BoxFuture<T> {
    match key {
        Some(key) => Box::pin(WithKey { key, inner: fut }),
        None => fut,
    }
}

/// Whether `error` (the encoded error of mutation `mutation_id`) is a network failure: an
/// `HttpError::Network`, alone or wrapped anywhere inside the mutation's own error type.
pub(crate) fn is_network_error(ctx: &Ctx, mutation_id: u32, error: &Erased) -> bool {
    if let Some(http) = error.value.downcast_ref::<HttpError>() {
        return matches!(http, HttpError::Network(_));
    }
    let schema = ctx.runtime().schema();
    schema
        .queries
        .iter()
        .find(|q| q.query_id == mutation_id)
        .and_then(|q| error_type(&q.returns))
        .is_some_and(|ty| probe_network_error(schema, ty, &error.bytes))
}

/// A caller waiting for a queued mutation to run.
pub(crate) struct Waiter {
    done: Mutex<Option<Outcome>>,
    notify: Notify,
}

impl Waiter {
    fn new() -> Arc<Waiter> {
        Arc::new(Waiter {
            done: Mutex::new(None),
            notify: Notify::new(),
        })
    }

    fn complete(&self, outcome: Outcome) {
        *self.done.lock() = Some(outcome);
        self.notify.notify_one();
    }

    /// Resolves when the replay has finished.
    pub(crate) async fn wait(&self) -> Outcome {
        loop {
            let done = self.done.lock().take();
            if let Some(outcome) = done {
                return outcome;
            }
            self.notify.notified().await;
        }
    }
}

/// A mutation in the queue, with what only this process knows about it.
pub(crate) struct QueueItem {
    pub(crate) mutation: QueuedMutation,
    /// Whoever is awaiting the mutation, if it was queued in this process.
    waiter: Option<Arc<Waiter>>,
    /// The invalidations its caller asked for, on top of the mutation's own key.
    invalidations: Vec<Invalidate>,
}

/// The queue and the bookkeeping of its writer and its replay.
#[derive(Default)]
pub(crate) struct QueueState {
    pub(crate) items: VecDeque<QueueItem>,
    writer_running: bool,
    dirty: bool,
    replaying: bool,
    /// The mutations this process has run, so a replay does not depend on the registry.
    known: HashMap<u32, &'static MutationVTable>,
}

impl Shared {
    /// Appends a mutation to the queue and persists it. The returned waiter resolves when the
    /// mutation has been replayed.
    pub(crate) fn enqueue(
        self: &Arc<Self>,
        ctx: &Ctx,
        vt: &'static MutationVTable,
        params: Vec<u8>,
        idempotency_key: Uuid,
        invalidations: Vec<Invalidate>,
    ) -> Arc<Waiter> {
        let waiter = Waiter::new();
        {
            let mut state = self.state.lock();
            state.queue.known.insert(vt.id, vt);
            state.queue.items.push_back(QueueItem {
                mutation: QueuedMutation {
                    mutation_id: vt.id,
                    params,
                    idempotency_key,
                },
                waiter: Some(waiter.clone()),
                invalidations,
            });
            self.mark_queue_dirty(ctx, &mut state);
        }
        Shared::log(
            ctx,
            DEBUG,
            &format!("queued mutation `{}` for replay", vt.key),
        );
        waiter
    }

    /// How many mutations are waiting for the network.
    pub(crate) fn queue_len(&self) -> usize {
        self.state.lock().queue.items.len()
    }

    /// Notes that the queue changed and makes sure a writer will persist it.
    fn mark_queue_dirty(self: &Arc<Self>, ctx: &Ctx, state: &mut State) {
        state.queue.dirty = true;
        if !state.queue.writer_running {
            state.queue.writer_running = true;
            ctx.spawn(run_queue_writer(self.clone(), ctx.clone()));
        }
    }

    /// Reads the persisted queue (at hydration) and replays it if the client is online.
    pub(crate) async fn hydrate_queue(self: &Arc<Self>, ctx: &Ctx) {
        let kv = ctx.kv();
        let Some(Bytes(raw)) = kv.get(QUEUE_KEY.to_owned()).await else {
            return;
        };
        let schema_hash = ctx.runtime().schema_hash();
        match decode_queue(&raw) {
            Ok((hash, items)) if hash == schema_hash => {
                {
                    let mut state = self.state.lock();
                    let mut at = 0;
                    for mutation in items {
                        // A mutation queued in this process before hydration finished is
                        // already in memory (and in the store): keep it once.
                        let known = state
                            .queue
                            .items
                            .iter()
                            .any(|i| i.mutation.idempotency_key == mutation.idempotency_key);
                        if !known {
                            state.queue.items.insert(
                                at,
                                QueueItem {
                                    mutation,
                                    waiter: None,
                                    invalidations: Vec::new(),
                                },
                            );
                            at += 1;
                        }
                    }
                }
                self.replay_queue(ctx);
            }
            _ => {
                Shared::log(
                    ctx,
                    WARN,
                    "dropping a persisted offline queue from another build",
                );
                kv.delete(QUEUE_KEY.to_owned()).await;
            }
        }
    }

    /// Starts replaying the queue if the client is online and nothing is replaying already.
    pub(crate) fn replay_queue(self: &Arc<Self>, ctx: &Ctx) {
        if !self.is_online() {
            return;
        }
        let mut state = self.state.lock();
        if state.queue.replaying || state.queue.items.is_empty() {
            return;
        }
        state.queue.replaying = true;
        ctx.spawn(run_replay(self.clone(), ctx.clone()));
    }

    /// Removes the head of the queue (it was answered) and persists the change.
    fn pop_queue(self: &Arc<Self>, ctx: &Ctx) -> Option<QueueItem> {
        let mut state = self.state.lock();
        let item = state.queue.items.pop_front();
        self.mark_queue_dirty(ctx, &mut state);
        item
    }
}

/// Persists the queue whenever it is dirty, one write at a time.
async fn run_queue_writer(shared: Arc<Shared>, ctx: Ctx) {
    struct Guard(Arc<Shared>, bool);
    impl Drop for Guard {
        fn drop(&mut self) {
            if self.1 {
                self.0.state.lock().queue.writer_running = false;
            }
        }
    }
    let mut guard = Guard(shared.clone(), true);
    let schema_hash = ctx.runtime().schema_hash();
    loop {
        let bytes = {
            let mut state = shared.state.lock();
            if !state.queue.dirty {
                state.queue.writer_running = false;
                guard.1 = false;
                return;
            }
            state.queue.dirty = false;
            if state.queue.items.is_empty() {
                None
            } else {
                let items: Vec<QueuedMutation> = state
                    .queue
                    .items
                    .iter()
                    .map(|i| i.mutation.clone())
                    .collect();
                Some(encode_queue(schema_hash, &items))
            }
        };
        match bytes {
            Some(bytes) => ctx.kv().set(QUEUE_KEY.to_owned(), Bytes(bytes)).await,
            None => ctx.kv().delete(QUEUE_KEY.to_owned()).await,
        }
    }
}

/// Runs the queued mutations in order until the queue is empty or the network is gone.
async fn run_replay(shared: Arc<Shared>, ctx: Ctx) {
    struct Guard(Arc<Shared>, bool);
    impl Drop for Guard {
        fn drop(&mut self) {
            if self.1 {
                self.0.state.lock().queue.replaying = false;
            }
        }
    }
    let mut guard = Guard(shared.clone(), true);
    let mut attempt = 0_u32;
    loop {
        // The head of the queue, and how to run it.
        let head = {
            let mut state = shared.state.lock();
            let online = shared.is_online();
            match state.queue.items.front() {
                Some(item) if online => {
                    let vt = state
                        .queue
                        .known
                        .get(&item.mutation.mutation_id)
                        .copied()
                        .or_else(|| registered_mutation(item.mutation.mutation_id));
                    Some((item.mutation.clone(), vt))
                }
                _ => {
                    state.queue.replaying = false;
                    guard.1 = false;
                    None
                }
            }
        };
        let Some((mutation, vt)) = head else {
            return;
        };
        let Some(vt) = vt else {
            Shared::log(
                &ctx,
                WARN,
                &format!(
                    "dropping a queued mutation ({:#010x}) this build does not define",
                    mutation.mutation_id
                ),
            );
            if let Some(item) = shared.pop_queue(&ctx) {
                if let Some(waiter) = item.waiter {
                    waiter.complete(Err(Failure::Broken(
                        "the mutation is not defined".to_owned(),
                    )));
                }
            }
            continue;
        };

        let key = mutation.idempotency_key;
        let params = mutation.params.clone();
        let outcome = with_retries(&ctx, vt.retry, Failure::retryable, || {
            scoped(Some(key), (vt.execute)(ctx.clone(), &params))
        })
        .await;

        if let Err(Failure::Error(error)) = &outcome {
            if is_network_error(&ctx, vt.id, error) {
                if shared.is_online() {
                    // The platform says online, the request says otherwise: try again later.
                    backoff_sleep(&ctx, attempt).await;
                    attempt = attempt.saturating_add(1);
                } else {
                    let mut state = shared.state.lock();
                    state.queue.replaying = false;
                    guard.1 = false;
                    return;
                }
                continue;
            }
        }
        attempt = 0;

        let item = shared.pop_queue(&ctx);
        let (waiter, invalidations) = match item {
            Some(item) => (item.waiter, item.invalidations),
            None => (None, Vec::new()),
        };
        if outcome.is_ok() {
            let mut targets = invalidations;
            let prefix = shared.render_key(&ctx, vt.id, vt.key, &params);
            if !prefix.is_empty() {
                targets.push(Invalidate::Prefix(prefix));
            }
            shared.invalidate(&ctx, &targets);
        }
        if let Some(waiter) = waiter {
            waiter.complete(outcome);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_runtime::executor::yield_now;
    use keel_runtime::testing::TestRuntime;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::task::Waker;

    fn poll<T>(fut: &mut BoxFuture<T>) -> Poll<T> {
        fut.as_mut().poll(&mut Context::from_waker(Waker::noop()))
    }

    #[test]
    fn the_key_is_set_only_while_the_body_is_polled() {
        let key = Uuid([9; 16]);
        let mut fut: BoxFuture<(Option<Uuid>, Option<Uuid>)> = scoped(
            Some(key),
            Box::pin(async {
                let before = idempotency_key();
                yield_now().await;
                (before, idempotency_key())
            }),
        );
        assert_eq!(poll(&mut fut), Poll::Pending);
        assert_eq!(idempotency_key(), None, "not left set between polls");
        assert_eq!(poll(&mut fut), Poll::Ready((Some(key), Some(key))));
        assert_eq!(idempotency_key(), None);
    }

    #[test]
    fn without_a_key_the_future_is_untouched() {
        let mut fut: BoxFuture<Option<Uuid>> = scoped(None, Box::pin(async { idempotency_key() }));
        assert_eq!(poll(&mut fut), Poll::Ready(None));
    }

    #[test]
    fn scopes_nest_and_restore_the_outer_key() {
        let (outer, inner) = (Uuid([1; 16]), Uuid([2; 16]));
        let mut fut: BoxFuture<(Option<Uuid>, Option<Uuid>)> = scoped(
            Some(outer),
            Box::pin(async move {
                let mut nested: BoxFuture<Option<Uuid>> =
                    scoped(Some(inner), Box::pin(async { idempotency_key() }));
                let inside = match nested
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                {
                    Poll::Ready(key) => key,
                    Poll::Pending => None,
                };
                (inside, idempotency_key())
            }),
        );
        assert_eq!(poll(&mut fut), Poll::Ready((Some(inner), Some(outer))));
    }

    #[test]
    fn a_panicking_body_does_not_leave_the_key_set() {
        let mut fut: BoxFuture<()> = scoped(
            Some(Uuid([3; 16])),
            Box::pin(async {
                panic!("boom");
            }),
        );
        let caught = catch_unwind(AssertUnwindSafe(|| poll(&mut fut)));
        assert!(caught.is_err());
        assert_eq!(idempotency_key(), None);
    }

    #[test]
    fn a_waiter_gets_the_outcome_whether_it_was_completed_before_or_after_it_waited() {
        let t = TestRuntime::new();
        let early = Waiter::new();
        early.complete(Ok(Erased::new(1_u8)));
        let value = t.run_until(async { early.wait().await });
        assert_eq!(value.ok().and_then(|e| e.typed::<u8>()), Some(1));

        let late = Waiter::new();
        let completer = late.clone();
        t.ctx()
            .spawn(async move { completer.complete(Err(Failure::Broken("x".into()))) });
        let outcome = t.run_until(async { late.wait().await });
        assert!(matches!(outcome, Err(Failure::Broken(m)) if m == "x"));
    }

    #[test]
    fn network_errors_are_recognised_by_type_when_the_error_is_http_error() {
        let t = TestRuntime::new();
        let ctx = t.ctx();
        let net = Erased::new(HttpError::Network("dns".into()));
        let timeout = Erased::new(HttpError::Timeout);
        assert!(is_network_error(&ctx, 1, &net));
        assert!(!is_network_error(&ctx, 1, &timeout));
        // Some other error type of a mutation the schema does not know: not a network error.
        assert!(!is_network_error(
            &ctx,
            1,
            &Erased::new("offline".to_owned())
        ));
    }
}
