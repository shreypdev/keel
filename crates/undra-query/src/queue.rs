//! The offline queue (SPEC 9): idempotent mutations that failed for lack of a network wait here
//! and are replayed, first in first out, when the platform reports connectivity again.
//!
//! * A mutation is queued when it is marked `idempotent`, fails with an `HttpError::Network`
//!   (its own error type may wrap it) **and** the client believes the device is offline.
//!   Anything else fails at once: a non-idempotent mutation is never queued, and a network error
//!   while the platform says it is online is an ordinary failure.
//! * The queue is persisted under `undra.query.queue2` (see [`crate::persist`]) after every
//!   change, by a single writer task so writes cannot land out of order, and read back by
//!   `hydrate`, so queued mutations survive a restart. Each item carries the fingerprint of its
//!   mutation's parameters (ADR-037): an item an older build queued is migrated by parameter name,
//!   then by the mutation's `#[undra::migrate]` hook, and one that does not migrate is moved to
//!   the dead-letter queue (`undra.query.queue.dead`), never dropped.
//! * Storage is best-effort and the queue is never overwritten unread (ADR-049): until a read of
//!   the stored queue succeeds (an app launched before the device's first unlock reads `Locked`),
//!   the client neither replays nor writes the queue key; new offline mutations wait in memory,
//!   and the read is tried again on `Active`, on a background run and after a backoff.
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

use parking_lot::Mutex;
use undra_ports::{CtxPorts, HttpError, StorageError};
use undra_runtime::executor::Notify;
use undra_runtime::log::{DEBUG, WARN};
use undra_runtime::persist::{
    DynRecord, RegisteredHooks, decode_params, encode_params, migrate_params, mutation_hook,
    run_mutation_hook,
};
use undra_runtime::{Ctx, WeakCtx};
use undra_wire::{Bytes, Uuid};

use crate::defs::BoxFuture;
use crate::erased::{Erased, Failure, MutationVTable, Outcome, registered_mutation};
use crate::key::Invalidate;
use crate::persist::{
    DEAD_LETTER_KEY, DeadItem, QUEUE_KEY, QUEUE_KEY_V1, QueuedMutation, decode_dead, decode_queue,
    decode_queue_v1, encode_dead, encode_queue,
};
use crate::retry::{backoff_ms, backoff_sleep, with_retries};
use crate::shared::{Shared, State};
use crate::storage::{Counters, query_name};
use crate::walk::{error_type, probe_network_error};

thread_local! {
    static IDEMPOTENCY_KEY: Cell<Option<Uuid>> = const { Cell::new(None) };
}

/// The idempotency key of the mutation being run, if it is an idempotent one.
///
/// Call it from inside a `#[undra::mutation(idempotent)]` function to send the key to the server
/// (an `Idempotency-Key` header, say). The key is made when the mutation first runs and stays
/// the same across its retries and its replays from the offline queue. `None` outside an
/// idempotent mutation. It is read from the task that runs the mutation body: work the body
/// hands to `ctx.spawn` does not see it.
///
/// ```
/// assert_eq!(undra_query::idempotency_key(), None);
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

/// Whether the stored queue has been read (ADR-049 decision 1.4).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Hydration {
    /// Not read yet (start-up).
    #[default]
    Pending,
    /// The read failed with a transient error; the queue key is not written until a read
    /// succeeds, and nothing replays.
    Unreadable,
    /// Read: the queue in memory is the whole queue.
    Hydrated,
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
    /// Whether the stored queue was read.
    pub(crate) hydration: Hydration,
    /// A hydration is running (only one at a time).
    hydrating: bool,
    /// A delayed retry of an unreadable queue is scheduled.
    retry_scheduled: bool,
    /// How many reads failed in a row (the backoff of the retry).
    failed_reads: u32,
    /// The dead letters (ADR-037 decision 7), oldest first.
    pub(crate) dead: Vec<DeadItem>,
    /// The dead letters changed since they were last written.
    dead_dirty: bool,
    /// The queue of format 1 is still in the store and is deleted at the next write.
    v1_pending: bool,
}

impl QueueState {
    /// The fingerprints the stored queue and dead letters reference (for the collection of
    /// unreferenced closures).
    pub(crate) fn fingerprints(&self) -> impl Iterator<Item = u64> + '_ {
        self.items
            .iter()
            .map(|i| i.mutation.fingerprint)
            .chain(self.dead.iter().map(|d| d.mutation.fingerprint))
    }

    /// The hydration state as `stats_json` names it.
    pub(crate) fn hydration_name(&self) -> &'static str {
        match self.hydration {
            Hydration::Pending => "pending",
            Hydration::Unreadable => "unreadable",
            Hydration::Hydrated => "hydrated",
        }
    }
}

/// A queued mutation that could not be migrated to this build (ADR-037 decision 7): what
/// [`QueryClient::dead_letters`](crate::QueryClient::dead_letters) lists.
#[derive(Clone, Debug, PartialEq)]
pub struct DeadLetter {
    /// The mutation's name, or its id in hex if this build does not define it.
    pub mutation: String,
    /// The mutation's id.
    pub mutation_id: u32,
    /// The key its replays carry, which identifies the dead letter.
    pub idempotency_key: Uuid,
    /// Why it could not be migrated.
    pub reason: String,
    /// Its input by parameter name, decoded with the description it was written with; empty if
    /// that description is unknown (the bytes are in [`raw`](Self::raw)).
    pub params: DynRecord,
    /// The input as it was stored.
    pub raw: Vec<u8>,
}

/// Why [`QueryClient::retry_dead_letter`](crate::QueryClient::retry_dead_letter) left a dead letter
/// where it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RetryError {
    /// No dead letter has that idempotency key.
    NotFound,
    /// It still does not migrate (no hook accepts it); the reason is updated.
    Incompatible(String),
}

impl core::fmt::Display for RetryError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            RetryError::NotFound => f.write_str("no dead letter has that idempotency key"),
            RetryError::Incompatible(reason) => write!(f, "it still does not migrate: {reason}"),
        }
    }
}

impl std::error::Error for RetryError {}

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
        let fingerprint = self
            .mutation_closure(ctx.runtime().schema(), vt.id)
            .map_or(0, |current| current.1);
        {
            let mut state = self.state.lock();
            state.queue.known.insert(vt.id, vt);
            state.queue.items.push_back(QueueItem {
                mutation: QueuedMutation {
                    mutation_id: vt.id,
                    fingerprint,
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

    /// Notes that the queue changed and makes sure a writer will persist it, once the stored
    /// queue has been read (it is never overwritten unread).
    fn mark_queue_dirty(self: &Arc<Self>, ctx: &Ctx, state: &mut State) {
        state.queue.dirty = true;
        self.start_queue_writer(ctx, state);
    }

    fn start_queue_writer(self: &Arc<Self>, ctx: &Ctx, state: &mut State) {
        if state.queue.hydration != Hydration::Hydrated {
            return;
        }
        if !(state.queue.dirty || state.queue.dead_dirty || state.queue.v1_pending) {
            return;
        }
        if !state.queue.writer_running {
            state.queue.writer_running = true;
            ctx.spawn(run_queue_writer(self.clone(), ctx.downgrade()));
        }
    }

    /// Reads the stored queue and dead letters (at hydration, and again while they could not be
    /// read), migrates what an older build queued, and replays if the client is online. `true` if
    /// they were read.
    pub(crate) async fn hydrate_queue(self: &Arc<Self>, weak: &WeakCtx) -> bool {
        {
            let mut state = self.state.lock();
            if state.queue.hydrating {
                return false;
            }
            state.queue.hydrating = true;
        }
        let read = self.read_queue(weak).await;
        self.state.lock().queue.hydrating = false;
        let Ok(ctx) = weak.upgrade() else {
            return false;
        };
        match read {
            Ok(read) => {
                self.adopt_queue(&ctx, read);
                true
            }
            Err(error) => {
                Counters::bump(&self.counters.read_failed);
                self.warn_once(&ctx, "read the offline queue", &error);
                {
                    let mut state = self.state.lock();
                    if state.queue.hydration != Hydration::Hydrated {
                        state.queue.hydration = Hydration::Unreadable;
                    }
                    state.queue.failed_reads = state.queue.failed_reads.saturating_add(1);
                }
                self.schedule_queue_retry(&ctx);
                false
            }
        }
    }

    /// Reads the stored queue, both formats, and the dead letters, and resolves every item
    /// against this build. A transient read failure is an error (nothing is changed); a stored
    /// value that does not decode, or that the store says is `Corrupt`, is dead-lettered (bytes
    /// intact where they could be read) and reported.
    async fn read_queue(self: &Arc<Self>, weak: &WeakCtx) -> Result<QueueRead, StorageError> {
        let ctx = weak
            .upgrade()
            .map_err(|gone| StorageError::Unavailable(gone.to_string()))?;
        let kv = ctx.kv();
        let schema_hash = ctx.runtime().schema_hash();
        let mut read = QueueRead::default();

        // The dead letters first: they are never overwritten unread either.
        match kv.get(DEAD_LETTER_KEY.to_owned()).await {
            Ok(Some(Bytes(raw))) => match decode_dead(&raw) {
                Ok(items) => read.dead = items,
                Err(e) => read.dead.push(raw_dead_letter(
                    raw,
                    format!("the stored dead-letter queue does not decode: {e}"),
                )),
            },
            Ok(None) => {}
            Err(StorageError::Corrupt(why)) => read.dead.push(raw_dead_letter(
                Vec::new(),
                format!("the stored dead-letter queue cannot be read back: {why}"),
            )),
            Err(error) => return Err(error),
        }

        let mut stored: Vec<QueuedMutation> = Vec::new();
        match kv.get(QUEUE_KEY.to_owned()).await {
            Ok(Some(Bytes(raw))) => match decode_queue(&raw) {
                Ok((_, items)) => stored = items,
                Err(e) => read.new_dead.push(raw_dead_letter(
                    raw,
                    format!("the stored offline queue does not decode: {e}"),
                )),
            },
            Ok(None) => {}
            Err(StorageError::Corrupt(why)) => read.new_dead.push(raw_dead_letter(
                Vec::new(),
                format!("the stored offline queue cannot be read back: {why}"),
            )),
            Err(error) => return Err(error),
        }

        match kv.get(QUEUE_KEY_V1.to_owned()).await {
            Ok(Some(Bytes(raw))) => {
                read.v1_found = true;
                match decode_queue_v1(&raw) {
                    Ok((hash, items)) if hash == schema_hash => {
                        // Written by this very build: the items are current.
                        for item in items {
                            let fingerprint = self
                                .mutation_closure(ctx.runtime().schema(), item.mutation_id)
                                .map_or(0, |current| current.1);
                            stored.push(QueuedMutation {
                                mutation_id: item.mutation_id,
                                fingerprint,
                                params: item.params,
                                idempotency_key: item.idempotency_key,
                            });
                        }
                    }
                    Ok((_, items)) => {
                        for item in items {
                            read.new_dead.push(DeadItem {
                                mutation: QueuedMutation {
                                    mutation_id: item.mutation_id,
                                    fingerprint: 0,
                                    params: item.params,
                                    idempotency_key: item.idempotency_key,
                                },
                                reason: "queued by another build before queued mutations carried the type of their input (format 1)".to_owned(),
                            });
                        }
                    }
                    Err(e) => read.new_dead.push(raw_dead_letter(
                        raw,
                        format!("the stored offline queue of format 1 does not decode: {e}"),
                    )),
                }
            }
            Ok(None) => {}
            Err(StorageError::Corrupt(_)) => read.v1_found = true,
            Err(error) => return Err(error),
        }

        // The descriptions the stored items and dead letters were written with.
        let fingerprints: Vec<u64> = stored
            .iter()
            .map(|i| i.fingerprint)
            .chain(read.dead.iter().map(|d| d.mutation.fingerprint))
            .filter(|f| *f != 0)
            .collect();
        for fingerprint in fingerprints {
            let current = stored
                .iter()
                .find(|i| i.fingerprint == fingerprint)
                .and_then(|i| self.mutation_closure(ctx.runtime().schema(), i.mutation_id))
                .is_some_and(|c| c.1 == fingerprint);
            if !current {
                self.load_closure(&ctx, fingerprint).await?;
            }
        }
        for item in stored {
            match self.resolve(&ctx, &item) {
                Ok(resolved) => {
                    if resolved.fingerprint != item.fingerprint || resolved.params != item.params {
                        read.migrated += 1;
                    }
                    read.items.push(resolved);
                }
                Err(reason) => read.new_dead.push(DeadItem {
                    mutation: item,
                    reason,
                }),
            }
        }
        Ok(read)
    }

    /// One stored item against this build (decision 5): current, migrated by parameter name,
    /// migrated by the mutation's hook, or the reason it is a dead letter. Uses closures already
    /// in memory.
    pub(crate) fn resolve(
        &self,
        ctx: &Ctx,
        item: &QueuedMutation,
    ) -> Result<QueuedMutation, String> {
        let schema = ctx.runtime().schema();
        let Some(def) = schema
            .queries
            .iter()
            .find(|q| q.query_id == item.mutation_id && q.kind == undra_meta::QueryKind::Mutation)
        else {
            return Err("this build does not define the mutation".to_owned());
        };
        let Some(current) = self.mutation_closure(schema, item.mutation_id) else {
            return Err("this build does not define the mutation".to_owned());
        };
        if item.fingerprint == current.1 {
            return Ok(item.clone());
        }
        if item.fingerprint == 0 {
            return Err("the type of its input is unknown (queued before format 2)".to_owned());
        }
        let Some(old) = self.cached_closure(item.fingerprint) else {
            return Err("the description of the type of its input is missing".to_owned());
        };
        let input = decode_params(&item.params, &def.name, &old).map_err(|e| e.to_string())?;
        let params = match migrate_params(&input, &old, &current.0, &RegisteredHooks) {
            Ok(params) => params,
            Err(error) => match mutation_hook(&def.name, item.fingerprint) {
                Some(hook) => {
                    let migrated = run_mutation_hook(hook, &input).map_err(|e| e.to_string())?;
                    encode_params(&migrated, &current.0).map_err(|e| {
                        format!(
                            "the migration hook `{}` returned an input that does not fit: {e}",
                            hook.name
                        )
                    })?
                }
                None => {
                    return Err(format!("its input does not migrate to this build: {error}"));
                }
            },
        };
        Ok(QueuedMutation {
            mutation_id: item.mutation_id,
            fingerprint: current.1,
            params,
            idempotency_key: item.idempotency_key,
        })
    }

    /// Takes in what was read: stored items go in front of the ones queued in memory meanwhile
    /// (each key once), dead letters are added and reported, and the queue is written back at
    /// once if anything was migrated, dead-lettered or merged.
    fn adopt_queue(self: &Arc<Self>, ctx: &Ctx, read: QueueRead) {
        for dead in &read.new_dead {
            Counters::bump(&self.counters.dead_lettered);
            Shared::log(
                ctx,
                WARN,
                &format!(
                    "moved a queued `{}` to the dead-letter queue: {}",
                    query_name(ctx.runtime().schema(), dead.mutation.mutation_id),
                    dead.reason
                ),
            );
        }
        for _ in 0..read.migrated {
            Counters::bump(&self.counters.migrated);
        }
        {
            let mut state = self.state.lock();
            let had_memory = !state.queue.items.is_empty();
            let mut at = 0;
            for mutation in read.items {
                // A mutation queued in this process before the read finished is already in
                // memory: keep it once.
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
            let mut dead = read.dead;
            for item in &state.queue.dead {
                if !dead
                    .iter()
                    .any(|d| d.mutation.idempotency_key == item.mutation.idempotency_key)
                {
                    dead.push(item.clone());
                }
            }
            let added_dead = !read.new_dead.is_empty();
            dead.extend(read.new_dead);
            state.queue.dead = dead;
            state.queue.hydration = Hydration::Hydrated;
            state.queue.failed_reads = 0;
            state.queue.v1_pending |= read.v1_found;
            state.queue.dead_dirty |= added_dead;
            if read.migrated > 0 || added_dead || had_memory || read.v1_found {
                state.queue.dirty = true;
            }
            self.start_queue_writer(ctx, &mut state);
        }
        self.replay_queue(ctx);
    }

    /// Reads an unreadable queue again (on `Active`, on a background run).
    pub(crate) fn retry_unreadable_queue(self: &Arc<Self>, ctx: &Ctx) {
        if self.state.lock().queue.hydration != Hydration::Unreadable {
            return;
        }
        let shared = self.clone();
        let weak = ctx.downgrade();
        ctx.spawn(async move {
            shared.hydrate_queue(&weak).await;
        });
    }

    /// Schedules another read of an unreadable queue after a backoff (1 s doubling to 30 s).
    fn schedule_queue_retry(self: &Arc<Self>, ctx: &Ctx) {
        let attempt = {
            let mut state = self.state.lock();
            if state.queue.retry_scheduled || state.queue.hydration == Hydration::Hydrated {
                return;
            }
            state.queue.retry_scheduled = true;
            state.queue.failed_reads.saturating_sub(1)
        };
        let shared = self.clone();
        let weak = ctx.downgrade();
        let delay = core::time::Duration::from_millis(backoff_ms(attempt, 0));
        ctx.spawn(async move {
            let slept = weak.sleep(delay).await;
            shared.state.lock().queue.retry_scheduled = false;
            if slept.is_ok() {
                shared.hydrate_queue(&weak).await;
            }
        });
    }

    /// The dead letters, decoded with the descriptions their inputs were written with.
    pub(crate) fn dead_letters(&self, ctx: &Ctx) -> Vec<DeadLetter> {
        let schema = ctx.runtime().schema();
        let dead = self.state.lock().queue.dead.clone();
        dead.into_iter()
            .map(|d| {
                let name = query_name(schema, d.mutation.mutation_id);
                let params = self
                    .cached_closure(d.mutation.fingerprint)
                    .or_else(|| {
                        self.mutation_closure(schema, d.mutation.mutation_id)
                            .filter(|c| c.1 == d.mutation.fingerprint)
                            .map(|c| std::sync::Arc::new(c.0.clone()))
                    })
                    .and_then(|closure| decode_params(&d.mutation.params, &name, &closure).ok())
                    .unwrap_or_else(|| DynRecord::new(name.clone()));
                DeadLetter {
                    mutation: name,
                    mutation_id: d.mutation.mutation_id,
                    idempotency_key: d.mutation.idempotency_key,
                    reason: d.reason,
                    params,
                    raw: d.mutation.params,
                }
            })
            .collect()
    }

    /// Runs the migration of a dead letter again (after an update that added a hook, say): on
    /// success it goes back to the end of the queue and replays when online.
    pub(crate) fn retry_dead_letter(
        self: &Arc<Self>,
        ctx: &Ctx,
        key: Uuid,
    ) -> Result<(), RetryError> {
        let item = {
            let state = self.state.lock();
            state
                .queue
                .dead
                .iter()
                .find(|d| d.mutation.idempotency_key == key)
                .cloned()
                .ok_or(RetryError::NotFound)?
        };
        match self.resolve(ctx, &item.mutation) {
            Ok(resolved) => {
                let mut state = self.state.lock();
                state
                    .queue
                    .dead
                    .retain(|d| d.mutation.idempotency_key != key);
                state.queue.dead_dirty = true;
                state.queue.items.push_back(QueueItem {
                    mutation: resolved,
                    waiter: None,
                    invalidations: Vec::new(),
                });
                self.mark_queue_dirty(ctx, &mut state);
                drop(state);
                self.replay_queue(ctx);
                Ok(())
            }
            Err(reason) => {
                let mut state = self.state.lock();
                if let Some(d) = state
                    .queue
                    .dead
                    .iter_mut()
                    .find(|d| d.mutation.idempotency_key == key)
                {
                    if d.reason != reason {
                        d.reason.clone_from(&reason);
                        state.queue.dead_dirty = true;
                        self.start_queue_writer(ctx, &mut state);
                    }
                }
                Err(RetryError::Incompatible(reason))
            }
        }
    }

    /// Drops a dead letter for good. `false` if there was none with that key.
    pub(crate) fn discard_dead_letter(self: &Arc<Self>, ctx: &Ctx, key: Uuid) -> bool {
        let mut state = self.state.lock();
        let before = state.queue.dead.len();
        state
            .queue
            .dead
            .retain(|d| d.mutation.idempotency_key != key);
        if state.queue.dead.len() == before {
            return false;
        }
        state.queue.dead_dirty = true;
        self.start_queue_writer(ctx, &mut state);
        true
    }

    /// Moves a queued mutation this build cannot run to the dead letters (the replay found it).
    fn dead_letter_in_memory(
        self: &Arc<Self>,
        ctx: &Ctx,
        mutation: QueuedMutation,
        reason: String,
    ) {
        Counters::bump(&self.counters.dead_lettered);
        Shared::log(
            ctx,
            WARN,
            &format!(
                "moved a queued `{}` to the dead-letter queue: {reason}",
                query_name(ctx.runtime().schema(), mutation.mutation_id)
            ),
        );
        let mut state = self.state.lock();
        state.queue.dead.push(DeadItem { mutation, reason });
        state.queue.dead_dirty = true;
        self.start_queue_writer(ctx, &mut state);
    }

    /// Starts replaying the queue if the client is online and nothing is replaying already.
    pub(crate) fn replay_queue(self: &Arc<Self>, ctx: &Ctx) {
        if !self.is_online() {
            return;
        }
        let mut state = self.state.lock();
        // A queue that could not be read yet does not replay: it may hold older mutations that
        // must go first (ADR-049 decision 1.4).
        if state.queue.replaying
            || state.queue.items.is_empty()
            || state.queue.hydration != Hydration::Hydrated
        {
            return;
        }
        state.queue.replaying = true;
        ctx.spawn(run_replay(self.clone(), ctx.downgrade()));
    }

    /// Removes the head of the queue (it was answered) and persists the change.
    fn pop_queue(self: &Arc<Self>, ctx: &Ctx) -> Option<QueueItem> {
        let mut state = self.state.lock();
        let item = state.queue.items.pop_front();
        self.mark_queue_dirty(ctx, &mut state);
        item
    }
}

/// Persists the queue and the dead letters whenever they are dirty, one write at a time, and only
/// once the stored queue was read. A failed write is logged once per reason, counted and tried
/// again after a backoff.
async fn run_queue_writer(shared: Arc<Shared>, weak: WeakCtx) {
    struct Guard(Arc<Shared>, bool);
    impl Drop for Guard {
        fn drop(&mut self) {
            if self.1 {
                self.0.state.lock().queue.writer_running = false;
            }
        }
    }
    let mut guard = Guard(shared.clone(), true);
    let mut failures = 0_u32;
    loop {
        // One write per step, with the runtime upgraded for that step only (ADR-034).
        let Ok(ctx) = weak.upgrade() else {
            return;
        };
        let schema_hash = ctx.runtime().schema_hash();
        let step = {
            let mut state = shared.state.lock();
            let queue = &mut state.queue;
            if queue.hydration != Hydration::Hydrated {
                queue.writer_running = false;
                guard.1 = false;
                return;
            }
            if queue.dirty {
                queue.dirty = false;
                let items: Vec<QueuedMutation> =
                    queue.items.iter().map(|i| i.mutation.clone()).collect();
                Some(WriteStep::Queue(items))
            } else if queue.dead_dirty {
                queue.dead_dirty = false;
                Some(WriteStep::Dead(queue.dead.clone()))
            } else if queue.v1_pending {
                queue.v1_pending = false;
                Some(WriteStep::DeleteV1)
            } else {
                queue.writer_running = false;
                guard.1 = false;
                return;
            }
        };
        let Some(step) = step else { return };
        let written = write_step(&shared, &ctx, schema_hash, &step).await;
        if let Err((operation, error)) = written {
            Counters::bump(&shared.counters.write_failed);
            shared.warn_once(&ctx, operation, &error);
            {
                let mut state = shared.state.lock();
                match step {
                    WriteStep::Queue(_) => state.queue.dirty = true,
                    WriteStep::Dead(_) => state.queue.dead_dirty = true,
                    WriteStep::DeleteV1 => state.queue.v1_pending = true,
                }
            }
            drop(ctx);
            let delay = core::time::Duration::from_millis(backoff_ms(failures, 0));
            failures = failures.saturating_add(1);
            if weak.sleep(delay).await.is_err() {
                return;
            }
        } else {
            failures = 0;
        }
    }
}

/// One write of the queue writer.
enum WriteStep {
    Queue(Vec<QueuedMutation>),
    Dead(Vec<DeadItem>),
    DeleteV1,
}

async fn write_step(
    shared: &Shared,
    ctx: &Ctx,
    schema_hash: u64,
    step: &WriteStep,
) -> Result<(), (&'static str, StorageError)> {
    let kv = ctx.kv();
    match step {
        WriteStep::Queue(items) if items.is_empty() => kv
            .delete(QUEUE_KEY.to_owned())
            .await
            .map_err(|e| ("write the offline queue", e)),
        WriteStep::Queue(items) => {
            // Every closure an item needs is in the store before the queue that needs it.
            let mut ids: Vec<u32> = items.iter().map(|i| i.mutation_id).collect();
            ids.sort_unstable();
            ids.dedup();
            for id in ids {
                if let Some(current) = shared.mutation_closure(ctx.runtime().schema(), id) {
                    shared
                        .ensure_types(ctx, &current)
                        .await
                        .map_err(|e| ("store a type description", e))?;
                }
            }
            kv.set(
                QUEUE_KEY.to_owned(),
                Bytes(encode_queue(schema_hash, items)),
            )
            .await
            .map_err(|e| ("write the offline queue", e))
        }
        WriteStep::Dead(items) if items.is_empty() => kv
            .delete(DEAD_LETTER_KEY.to_owned())
            .await
            .map_err(|e| ("write the dead-letter queue", e)),
        WriteStep::Dead(items) => kv
            .set(
                DEAD_LETTER_KEY.to_owned(),
                Bytes(encode_dead(schema_hash, items)),
            )
            .await
            .map_err(|e| ("write the dead-letter queue", e)),
        WriteStep::DeleteV1 => kv
            .delete(QUEUE_KEY_V1.to_owned())
            .await
            .map_err(|e| ("delete the offline queue of format 1", e)),
    }
}

/// What one read of the stored queue found.
#[derive(Default)]
struct QueueRead {
    /// The stored items, resolved against this build.
    items: Vec<QueuedMutation>,
    /// How many of them were migrated.
    migrated: usize,
    /// The stored dead letters.
    dead: Vec<DeadItem>,
    /// The items that became dead letters now.
    new_dead: Vec<DeadItem>,
    /// A queue of format 1 was found (and is deleted at the next write).
    v1_found: bool,
}

/// A dead letter for stored bytes nobody can read as a queue.
fn raw_dead_letter(raw: Vec<u8>, reason: String) -> DeadItem {
    DeadItem {
        mutation: QueuedMutation {
            mutation_id: 0,
            fingerprint: 0,
            params: raw,
            idempotency_key: Uuid([0; 16]),
        },
        reason,
    }
}

/// Runs the queued mutations in order until the queue is empty or the network is gone.
async fn run_replay(shared: Arc<Shared>, weak: WeakCtx) {
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
        // One queued mutation per step, with the runtime upgraded for that step only (ADR-034).
        let Ok(ctx) = weak.upgrade() else {
            return;
        };
        let Some(vt) = vt else {
            // Never dropped (ADR-037 decision 7): a dead letter the app can show or retry.
            if let Some(item) = shared.pop_queue(&ctx) {
                if let Some(waiter) = item.waiter {
                    waiter.complete(Err(Failure::Broken(
                        "the mutation is not defined".to_owned(),
                    )));
                }
                shared.dead_letter_in_memory(
                    &ctx,
                    item.mutation,
                    "this build does not define the mutation".to_owned(),
                );
            }
            continue;
        };

        let key = mutation.idempotency_key;
        let params = mutation.params.clone();
        let outcome = with_retries(&weak, vt.retry, Failure::retryable, || {
            scoped(Some(key), (vt.execute)(ctx.clone(), &params))
        })
        .await;

        if let Err(Failure::Error(error)) = &outcome {
            if is_network_error(&ctx, vt.id, error) {
                if shared.is_online() {
                    // The platform says online, the request says otherwise: try again later.
                    drop(ctx);
                    if backoff_sleep(&weak, attempt).await.is_err() {
                        return;
                    }
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
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::task::Waker;
    use undra_runtime::executor::yield_now;
    use undra_runtime::testing::TestRuntime;

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
