//! The client and the `Kv` store: identity, migration and failures of persisted cache entries
//! (ADR-037, ADR-049).
//!
//! * **Identity.** Every entry is written with its query's fingerprint (the closure of the success
//!   type), and the closure is stored once under `undra.types.<fingerprint>` before the first entry
//!   that needs it.
//! * **Hydration** (ADR-037 decision 5, per entry): fingerprint equal, the bytes are used as they
//!   are; else the stored closure is read and the entry migrated structurally, then by the `ty`
//!   hook of its type; a migrated entry is written back in the current form at once; one that does
//!   not migrate is deleted (it can be fetched again) and reported (a WARN naming the query and the
//!   reason, `persist.dropped`). Entries of format 1 are read once: same schema hash, rewritten in
//!   format 2; otherwise dropped and reported. At most [`DEFAULT_MAX_PERSISTED_ENTRIES`] entries
//!   are kept, least recently updated evicted, and closures nothing references are deleted.
//! * **Failures are best-effort** (ADR-049 decision 1.4): a failed write keeps the entry in memory,
//!   logs one WARN per (operation, reason), counts `persist.write_failed` and is tried again at the
//!   next write; `Full` pauses new entries (each later write is one probe of the oldest failed
//!   entry) until a write succeeds. A failed read starts that entry empty.

use core::time::Duration;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use undra_meta::{ClosureRoot, Schema, TypeClosure, TypeRef};
use undra_ports::{CtxPorts, StorageError};
use undra_runtime::log::{DEBUG, WARN};
use undra_runtime::persist::{self, RegisteredHooks};
use undra_runtime::{Ctx, WeakCtx};
use undra_wire::Bytes;

use crate::erased::registered_query;
use crate::key::QueryKey;
use crate::persist::{
    CACHE_KEY_PREFIX, CACHE_KEY_PREFIX_V1, StoredEntry, TYPES_KEY_PREFIX, cache_key_of,
    decode_entry, decode_entry_v1, encode_entry, parse_cache_key, parse_cache_key_v1,
    parse_types_key, types_key,
};
use crate::shared::Shared;

/// How many persisted cache entries a client keeps by default (ADR-037 decision 8).
pub const DEFAULT_MAX_PERSISTED_ENTRIES: usize = 1_000;

/// How many times hydration asks for the `Kv` port before giving up, and how long it waits
/// between asks: a native host registers its adapters right after `undra_init`, while the core
/// thread may already be hydrating.
const KV_ATTEMPTS: u32 = 50;
const KV_RETRY_MS: u64 = 100;

/// A persisted entry read at hydration, in the current form, waiting for its query to be observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Persisted {
    pub updated_at: i64,
    pub data: Vec<u8>,
}

/// The persistence counters `stats_json` reports under `query.persist`.
#[derive(Default)]
pub(crate) struct Counters {
    pub write_failed: AtomicU64,
    pub read_failed: AtomicU64,
    pub dropped: AtomicU64,
    pub migrated: AtomicU64,
    pub dead_lettered: AtomicU64,
}

impl Counters {
    pub(crate) fn bump(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

/// What the client knows about the store (inside the client's state lock).
pub(crate) struct StorageState {
    /// The persisted cache entries: `(query_id, fnv1a64(params))` to `(updated_at, fingerprint)`.
    pub index: HashMap<(u32, u64), (i64, u64)>,
    /// The `undra.types.*` keys known to be in the store.
    pub types: HashSet<u64>,
    /// Entries whose last write failed, oldest first.
    pub failed: Vec<QueryKey>,
    /// The store said `Full` and no write has succeeded since.
    pub full: bool,
    /// `(operation, reason)` pairs already logged.
    pub warned: HashSet<(&'static str, &'static str)>,
    /// The bound of the persisted cache.
    pub max_entries: usize,
}

impl Default for StorageState {
    fn default() -> StorageState {
        StorageState {
            index: HashMap::new(),
            types: HashSet::new(),
            failed: Vec::new(),
            full: false,
            warned: HashSet::new(),
            max_entries: DEFAULT_MAX_PERSISTED_ENTRIES,
        }
    }
}

/// The variant of a storage error, for "once per reason".
pub(crate) fn reason_kind(error: &StorageError) -> &'static str {
    match error {
        StorageError::Unavailable(_) => "unavailable",
        StorageError::Full => "full",
        StorageError::Locked => "locked",
        StorageError::Corrupt(_) => "corrupt",
        StorageError::Io(_) => "io",
    }
}

/// A query's or mutation's current closure and its fingerprint.
pub(crate) type Current = Arc<(TypeClosure, u64)>;

/// The root type of a closure that describes one type.
pub(crate) fn root_type(closure: &TypeClosure) -> Option<&TypeRef> {
    match &closure.root {
        ClosureRoot::Type { ty } => Some(ty),
        _ => None,
    }
}

impl Shared {
    /// Logs `error` of `operation` at WARN, once per (operation, reason).
    pub(crate) fn warn_once(&self, ctx: &Ctx, operation: &'static str, error: &StorageError) {
        let first = self
            .state
            .lock()
            .storage
            .warned
            .insert((operation, reason_kind(error)));
        if first {
            Shared::log(
                ctx,
                WARN,
                &format!("could not {operation}: {error} (logged once per reason)"),
            );
        }
    }

    /// The current closure of query `query_id`'s cached value.
    pub(crate) fn query_closure(&self, schema: &Schema, query_id: u32) -> Option<Current> {
        let mut cache = self.current.lock();
        if let Some(found) = cache.get(&query_id) {
            return Some(found.clone());
        }
        let closure = schema.query_closure(query_id)?;
        let fingerprint = closure.fingerprint();
        let current: Current = Arc::new((closure, fingerprint));
        cache.insert(query_id, current.clone());
        Some(current)
    }

    /// The current closure of mutation `mutation_id`'s input.
    pub(crate) fn mutation_closure(&self, schema: &Schema, mutation_id: u32) -> Option<Current> {
        let mut cache = self.current.lock();
        if let Some(found) = cache.get(&mutation_id) {
            return Some(found.clone());
        }
        let closure = schema.mutation_closure(mutation_id)?;
        let fingerprint = closure.fingerprint();
        let current: Current = Arc::new((closure, fingerprint));
        cache.insert(mutation_id, current.clone());
        Some(current)
    }

    /// A closure read before (or written by this build), without asking the store.
    pub(crate) fn cached_closure(&self, fingerprint: u64) -> Option<Arc<TypeClosure>> {
        self.closures.lock().get(&fingerprint).cloned()
    }

    /// The stored closure with `fingerprint`: from memory, else read from `undra.types.*`.
    /// `Ok(None)` when the store has no such closure (or it does not parse).
    pub(crate) async fn load_closure(
        &self,
        ctx: &Ctx,
        fingerprint: u64,
    ) -> Result<Option<Arc<TypeClosure>>, StorageError> {
        if let Some(found) = self.cached_closure(fingerprint) {
            return Ok(Some(found));
        }
        let Some(Bytes(raw)) = ctx.kv().get(types_key(fingerprint)).await? else {
            return Ok(None);
        };
        let parsed = core::str::from_utf8(&raw)
            .ok()
            .and_then(|text| TypeClosure::from_json(text).ok())
            .filter(|closure| closure.fingerprint() == fingerprint);
        Ok(parsed.map(|closure| {
            let closure = Arc::new(closure);
            self.state.lock().storage.types.insert(fingerprint);
            self.closures.lock().insert(fingerprint, closure.clone());
            closure
        }))
    }

    /// Makes sure the closure with `fingerprint` is in the store before something that needs it
    /// is written.
    pub(crate) async fn ensure_types(
        &self,
        ctx: &Ctx,
        current: &Current,
    ) -> Result<(), StorageError> {
        let fingerprint = current.1;
        if self.state.lock().storage.types.contains(&fingerprint) {
            return Ok(());
        }
        ctx.kv()
            .set(
                types_key(fingerprint),
                Bytes(current.0.canonical_json().into_bytes()),
            )
            .await?;
        self.state.lock().storage.types.insert(fingerprint);
        self.closures
            .lock()
            .entry(fingerprint)
            .or_insert_with(|| Arc::new(current.0.clone()));
        Ok(())
    }

    // ----- writing -----------------------------------------------------------------------------

    /// Writes the cache entry of `key` (from the entry in memory), with the failure rules of the
    /// module docs. While the store is `Full`, a new entry waits and the oldest failed entry is
    /// written instead (one probe); a success writes the other failed entries too.
    pub(crate) async fn persist_entry(&self, ctx: &Ctx, key: &QueryKey) {
        let target = {
            let mut state = self.state.lock();
            let params_hash = undra_meta::ids::fnv1a64(&key.params);
            let storage = &mut state.storage;
            if storage.full && !storage.index.contains_key(&(key.query_id, params_hash)) {
                if !storage.failed.contains(key) {
                    storage.failed.push(key.clone());
                }
                storage.failed[0].clone()
            } else {
                key.clone()
            }
        };
        if !self.write_one(ctx, &target).await {
            return;
        }
        // A write succeeded: the store has room again. Try the ones that failed, once each.
        let pending: Vec<QueryKey> = {
            let mut state = self.state.lock();
            state.storage.full = false;
            state.storage.failed.retain(|k| *k != target);
            state.storage.failed.clone()
        };
        for retry in pending {
            if !self.write_one(ctx, &retry).await {
                break;
            }
            self.state.lock().storage.failed.retain(|k| *k != retry);
        }
    }

    /// One write of `key`'s entry. `true` if it was written (or there is nothing to write any
    /// more: the entry is gone from memory or has no data).
    async fn write_one(&self, ctx: &Ctx, key: &QueryKey) -> bool {
        let schema = ctx.runtime().schema();
        let Some(current) = self.query_closure(schema, key.query_id) else {
            return true;
        };
        let snapshot = {
            let state = self.state.lock();
            state
                .entries
                .get(key)
                .and_then(|entry| Some((entry.data.as_ref()?.bytes.to_vec(), entry.updated_at?)))
        };
        let Some((data, updated_at)) = snapshot else {
            self.state.lock().storage.failed.retain(|k| k != key);
            return true;
        };
        if let Err(error) = self.ensure_types(ctx, &current).await {
            self.write_failed(ctx, key, "store a type description", &error);
            return false;
        }
        let params_hash = undra_meta::ids::fnv1a64(&key.params);
        let stored = StoredEntry {
            schema_hash: ctx.runtime().schema_hash(),
            fingerprint: current.1,
            updated_at,
            data,
        };
        match ctx
            .kv()
            .set(
                cache_key_of(key.query_id, params_hash),
                Bytes(encode_entry(&stored)),
            )
            .await
        {
            Ok(()) => {
                let fresh = self
                    .state
                    .lock()
                    .storage
                    .index
                    .insert((key.query_id, params_hash), (updated_at, current.1))
                    .is_none();
                if fresh {
                    self.evict(ctx).await;
                }
                true
            }
            Err(error) => {
                self.write_failed(ctx, key, "write a cache entry", &error);
                false
            }
        }
    }

    fn write_failed(
        &self,
        ctx: &Ctx,
        key: &QueryKey,
        operation: &'static str,
        error: &StorageError,
    ) {
        Counters::bump(&self.counters.write_failed);
        self.warn_once(ctx, operation, error);
        let mut state = self.state.lock();
        if *error == StorageError::Full {
            state.storage.full = true;
        }
        if !state.storage.failed.contains(key) {
            state.storage.failed.push(key.clone());
        }
    }

    /// Deletes the least recently updated entries beyond the bound (ADR-037 decision 8).
    pub(crate) async fn evict(&self, ctx: &Ctx) {
        let victims: Vec<(u32, u64)> = {
            let mut state = self.state.lock();
            let max = state.storage.max_entries;
            let excess = state.storage.index.len().saturating_sub(max);
            if excess == 0 {
                return;
            }
            let mut by_age: Vec<((u32, u64), i64)> = state
                .storage
                .index
                .iter()
                .map(|(key, (at, _))| (*key, *at))
                .collect();
            by_age.sort_by_key(|(key, at)| (*at, *key));
            let victims: Vec<(u32, u64)> = by_age
                .into_iter()
                .take(excess)
                .map(|(key, _)| key)
                .collect();
            for victim in &victims {
                state.storage.index.remove(victim);
                state.hydrated.remove(victim);
            }
            victims
        };
        for (query_id, params_hash) in victims {
            if let Err(error) = ctx.kv().delete(cache_key_of(query_id, params_hash)).await {
                self.warn_once(ctx, "evict a cache entry", &error);
            }
        }
    }

    // ----- hydration ---------------------------------------------------------------------------

    /// Reads the persisted cache entries (both formats). `true` if every key could be listed and
    /// read, which is when unreferenced closures may be collected.
    pub(crate) async fn hydrate_cache(&self, weak: &WeakCtx) -> bool {
        let Some(keys) = list_when_available(self, weak, CACHE_KEY_PREFIX).await else {
            return false;
        };
        let mut complete = true;
        for key in keys {
            let Some((query_id, params_hash)) = parse_cache_key(&key) else {
                continue;
            };
            let Ok(ctx) = weak.upgrade() else {
                return false;
            };
            let raw = match ctx.kv().get(key.clone()).await {
                Ok(Some(Bytes(raw))) => raw,
                Ok(None) => continue,
                Err(error) => {
                    // The entry starts empty: it can be fetched again (ADR-049 decision 1.4).
                    Counters::bump(&self.counters.read_failed);
                    self.warn_once(&ctx, "read a cache entry", &error);
                    complete = false;
                    continue;
                }
            };
            match decode_entry(&raw) {
                Ok(stored) => {
                    if !self
                        .take_stored(&ctx, &key, query_id, params_hash, stored)
                        .await
                    {
                        complete = false;
                    }
                }
                Err(e) => {
                    self.drop_entry(
                        &ctx,
                        &key,
                        query_id,
                        &format!("the stored entry does not decode: {e}"),
                    )
                    .await;
                }
            }
        }
        if !self.hydrate_cache_v1(weak).await {
            complete = false;
        }
        if let Ok(ctx) = weak.upgrade() {
            self.evict(&ctx).await;
        }
        complete
    }

    /// Entries of format 1: the same schema hash means current bytes, rewritten in format 2;
    /// otherwise their identity is unknown, so they are dropped and reported.
    async fn hydrate_cache_v1(&self, weak: &WeakCtx) -> bool {
        let Ok(ctx) = weak.upgrade() else {
            return false;
        };
        let keys = match ctx.kv().list(CACHE_KEY_PREFIX_V1.to_owned()).await {
            Ok(keys) => keys,
            Err(error) => {
                Counters::bump(&self.counters.read_failed);
                self.warn_once(&ctx, "list the cache entries of format 1", &error);
                return false;
            }
        };
        let mut complete = true;
        for key in keys {
            let Some((query_id, params_hash)) = parse_cache_key_v1(&key) else {
                continue;
            };
            let raw = match ctx.kv().get(key.clone()).await {
                Ok(Some(Bytes(raw))) => raw,
                Ok(None) => continue,
                Err(error) => {
                    Counters::bump(&self.counters.read_failed);
                    self.warn_once(&ctx, "read a cache entry", &error);
                    complete = false;
                    continue;
                }
            };
            let current = self.query_closure(ctx.runtime().schema(), query_id);
            match (decode_entry_v1(&raw), current) {
                (Ok(old), Some(current)) if old.schema_hash == ctx.runtime().schema_hash() => {
                    let stored = StoredEntry {
                        schema_hash: old.schema_hash,
                        fingerprint: current.1,
                        updated_at: old.updated_at,
                        data: old.data,
                    };
                    let new_key = cache_key_of(query_id, params_hash);
                    if self
                        .rewrite(&ctx, &new_key, query_id, params_hash, &current, &stored)
                        .await
                    {
                        self.adopt_persisted(
                            &ctx,
                            query_id,
                            params_hash,
                            Persisted {
                                updated_at: stored.updated_at,
                                data: stored.data,
                            },
                        );
                        let _ = ctx.kv().delete(key).await;
                    } else {
                        complete = false;
                    }
                }
                _ => {
                    self.drop_entry(
                        &ctx,
                        &key,
                        query_id,
                        "it was written by another build before persisted entries carried their type (format 1)",
                    )
                    .await;
                }
            }
        }
        complete
    }

    /// One stored entry of format 2: used as it is, migrated, or dropped (decision 5). `false`
    /// when a read it needed failed (the entry is kept for the next hydration).
    async fn take_stored(
        &self,
        ctx: &Ctx,
        key: &str,
        query_id: u32,
        params_hash: u64,
        stored: StoredEntry,
    ) -> bool {
        let schema = ctx.runtime().schema();
        let Some(vt) = registered_query(query_id) else {
            self.drop_entry(ctx, key, query_id, "this build has no such query")
                .await;
            return true;
        };
        if !vt.persist {
            self.drop_entry(ctx, key, query_id, "the query is no longer persisted")
                .await;
            return true;
        }
        let Some(current) = self.query_closure(schema, query_id) else {
            self.drop_entry(ctx, key, query_id, "this build has no such query")
                .await;
            return true;
        };
        if stored.fingerprint == current.1 {
            self.state.lock().storage.index.insert(
                (query_id, params_hash),
                (stored.updated_at, stored.fingerprint),
            );
            self.adopt_persisted(
                ctx,
                query_id,
                params_hash,
                Persisted {
                    updated_at: stored.updated_at,
                    data: stored.data,
                },
            );
            return true;
        }
        let old = match self.load_closure(ctx, stored.fingerprint).await {
            Ok(Some(old)) => old,
            Ok(None) => {
                self.drop_entry(
                    ctx,
                    key,
                    query_id,
                    "the description of the type it was written with is missing",
                )
                .await;
                return true;
            }
            Err(error) => {
                Counters::bump(&self.counters.read_failed);
                self.warn_once(ctx, "read a type description", &error);
                return false;
            }
        };
        let migrated = migrate_value(&stored.data, &old, &current.0);
        let data = match migrated.and_then(|data| {
            (vt.decode_data)(&data)
                .map(|_| data)
                .map_err(|e| format!("the migrated value does not decode: {e}"))
        }) {
            Ok(data) => data,
            Err(reason) => {
                self.drop_entry(ctx, key, query_id, &reason).await;
                return true;
            }
        };
        Counters::bump(&self.counters.migrated);
        Shared::log(
            ctx,
            DEBUG,
            &format!(
                "migrated the cache entry `{key}` of query `{}`",
                query_name(schema, query_id)
            ),
        );
        let rewritten = StoredEntry {
            schema_hash: ctx.runtime().schema_hash(),
            fingerprint: current.1,
            updated_at: stored.updated_at,
            data,
        };
        // Written back in the current form at once, so the migration runs once.
        self.rewrite(ctx, key, query_id, params_hash, &current, &rewritten)
            .await;
        self.adopt_persisted(
            ctx,
            query_id,
            params_hash,
            Persisted {
                updated_at: rewritten.updated_at,
                data: rewritten.data,
            },
        );
        true
    }

    /// Writes `stored` under `key` (and its closure first). `true` if written.
    async fn rewrite(
        &self,
        ctx: &Ctx,
        key: &str,
        query_id: u32,
        params_hash: u64,
        current: &Current,
        stored: &StoredEntry,
    ) -> bool {
        if let Err(error) = self.ensure_types(ctx, current).await {
            Counters::bump(&self.counters.write_failed);
            self.warn_once(ctx, "store a type description", &error);
            return false;
        }
        match ctx
            .kv()
            .set(key.to_owned(), Bytes(encode_entry(stored)))
            .await
        {
            Ok(()) => {
                self.state.lock().storage.index.insert(
                    (query_id, params_hash),
                    (stored.updated_at, stored.fingerprint),
                );
                true
            }
            Err(error) => {
                Counters::bump(&self.counters.write_failed);
                self.warn_once(ctx, "write a cache entry", &error);
                false
            }
        }
    }

    /// Deletes a cache entry that cannot be used and reports it (ADR-037 decision 7: it can be
    /// fetched again).
    async fn drop_entry(&self, ctx: &Ctx, key: &str, query_id: u32, reason: &str) {
        Counters::bump(&self.counters.dropped);
        if let Some((query_id, params_hash)) =
            parse_cache_key(key).or_else(|| parse_cache_key_v1(key))
        {
            self.state
                .lock()
                .storage
                .index
                .remove(&(query_id, params_hash));
        }
        Shared::log(
            ctx,
            WARN,
            &format!(
                "dropped the persisted cache entry `{key}` of query `{}`: {reason}",
                query_name(ctx.runtime().schema(), query_id)
            ),
        );
        if let Err(error) = ctx.kv().delete(key.to_owned()).await {
            self.warn_once(ctx, "delete a cache entry", &error);
        }
    }

    /// Deletes the `undra.types.*` keys no persisted entry, queued mutation or dead letter
    /// references (ADR-037 decision 8). Runs only after every one of them was read.
    pub(crate) async fn collect_types(&self, weak: &WeakCtx) {
        let Ok(ctx) = weak.upgrade() else {
            return;
        };
        let keys = match ctx.kv().list(TYPES_KEY_PREFIX.to_owned()).await {
            Ok(keys) => keys,
            Err(error) => {
                self.warn_once(&ctx, "list the type descriptions", &error);
                return;
            }
        };
        let referenced: HashSet<u64> = {
            let state = self.state.lock();
            state
                .storage
                .index
                .values()
                .map(|(_, fingerprint)| *fingerprint)
                .chain(state.queue.fingerprints())
                .collect()
        };
        for key in keys {
            let Some(fingerprint) = parse_types_key(&key) else {
                continue;
            };
            if referenced.contains(&fingerprint) {
                self.state.lock().storage.types.insert(fingerprint);
                continue;
            }
            match ctx.kv().delete(key).await {
                Ok(()) => {
                    self.state.lock().storage.types.remove(&fingerprint);
                }
                Err(error) => self.warn_once(&ctx, "delete a type description", &error),
            }
        }
    }

    /// The counters, for [`QueryClient::persist_stats`](crate::QueryClient::persist_stats).
    pub(crate) fn persist_stats(&self) -> crate::client::PersistStats {
        let c = &self.counters;
        crate::client::PersistStats {
            write_failed: c.write_failed.load(Ordering::Relaxed),
            read_failed: c.read_failed.load(Ordering::Relaxed),
            dropped: c.dropped.load(Ordering::Relaxed),
            migrated: c.migrated.load(Ordering::Relaxed),
            dead_lettered: c.dead_lettered.load(Ordering::Relaxed),
            queue_readable: self.state.lock().queue.hydration_name() == "hydrated",
        }
    }

    /// The `query` section of `stats_json`.
    pub(crate) fn stats_json(&self) -> String {
        let c = &self.counters;
        let state = self.state.lock();
        format!(
            "{{\"cached_entries\":{},\"persisted_entries\":{},\"pending_mutations\":{},\"dead_letters\":{},\"queue\":\"{}\",\"persist\":{{\"write_failed\":{},\"read_failed\":{},\"dropped\":{},\"migrated\":{},\"dead_lettered\":{}}}}}",
            state.entries.len(),
            state.storage.index.len(),
            state.queue.items.len(),
            state.queue.dead.len(),
            state.queue.hydration_name(),
            c.write_failed.load(Ordering::Relaxed),
            c.read_failed.load(Ordering::Relaxed),
            c.dropped.load(Ordering::Relaxed),
            c.migrated.load(Ordering::Relaxed),
            c.dead_lettered.load(Ordering::Relaxed),
        )
    }
}

/// A cached value written with `old`, as bytes of the type `new` describes: structurally (with
/// `ty` hooks inside), else the `ty` hook of the root type.
fn migrate_value(data: &[u8], old: &TypeClosure, new: &TypeClosure) -> Result<Vec<u8>, String> {
    let (Some(old_ty), Some(new_ty)) = (root_type(old), root_type(new)) else {
        return Err("the stored description is not a query's".to_owned());
    };
    // Streamed first; the value is decoded only for the root type's hook.
    let error = match persist::migrate(data, old_ty, old, new_ty, new, &RegisteredHooks) {
        Ok(bytes) => return Ok(bytes),
        Err(error) => error,
    };
    match persist::root_type_hook(new_ty, old.fingerprint()) {
        Some(hook) => {
            let value = persist::decode_dyn(data, old_ty, old).map_err(|e| e.to_string())?;
            persist::run_value_hook(hook, Some(&value)).map_err(|e| e.to_string())
        }
        None => Err(format!("{old_ty} cannot become {new_ty}: {error}")),
    }
}

/// The name of query or mutation `id`, or its id in hex.
pub(crate) fn query_name(schema: &Schema, id: u32) -> String {
    schema
        .queries
        .iter()
        .find(|q| q.query_id == id)
        .map_or_else(|| format!("{id:#010x}"), |q| q.name.clone())
}

/// The keys under `prefix`. Hydration runs at start-up, possibly before the platform has
/// registered its adapters, so `Unavailable` is asked again for a few seconds; `None` if the
/// store never answers (the cache then starts empty) or fails otherwise.
pub(crate) async fn list_when_available(
    shared: &Shared,
    weak: &WeakCtx,
    prefix: &str,
) -> Option<Vec<String>> {
    for _ in 0..KV_ATTEMPTS {
        let listed = {
            let ctx = weak.upgrade().ok()?;
            let kv = ctx.kv();
            drop(ctx);
            kv.list(prefix.to_owned()).await
        };
        match listed {
            Ok(keys) => return Some(keys),
            Err(StorageError::Unavailable(_)) => {
                weak.sleep(Duration::from_millis(KV_RETRY_MS)).await.ok()?;
            }
            Err(error) => {
                let ctx = weak.upgrade().ok()?;
                Counters::bump(&shared.counters.read_failed);
                shared.warn_once(&ctx, "list the persisted cache", &error);
                return None;
            }
        }
    }
    let ctx = weak.upgrade().ok()?;
    Shared::log(
        &ctx,
        WARN,
        "the Kv port never became available; the query cache starts empty",
    );
    None
}
