# ADR-037: persisted state carries its type identity, migrates by name, and is never discarded silently

Status: **Accepted** (2026-10-01, implemented in `wt/persistence-v2`; see "Implementation notes" at the end
for what the code decided where this text left room, and the one deviation). Proposed 2026-10-01, from the v1.x gap audit `.10x/specs/2026-10-01-v1x-gaps.md`, gaps PS-1…PS-4
and runtime review N6; Track A, piece A5). Touches SPEC 2 (a per-type fingerprint in `undra-meta`), 3.1 (the
"evolution is a v2 feature" sentence), 5.9 (the `Snapshot` payload, **a pre-publication wire change** to an
opaque payload), 9 (persistence and the offline queue), 12 (one macro), 16.2 (`RestoreError`), `undra-meta`,
`undra-runtime` (a `persist` module: dynamic values and migration), `undra-query` (`persist.rs`, `shared.rs`,
`queue.rs`), `undra-macros` (`#[undra::migrate]`, `#[undra(default)]` on a signal) and the three platform
`Snapshot` codecs (tests only; hosts pass snapshots through as bytes). **No C ABI or wasm ABI change, no
generated platform code change.** R7 is unchanged: a host and a core with different schema hashes still refuse
to attach; this ADR is about data an older build *wrote to disk*. Constitution R11: the runtime model of
restore and the persisted formats change, so it is decided here before code.

## Context

Three artefacts outlive a process, and each is tied to the **whole** schema hash or to nothing:

| Artefact | Written | On a schema change today |
|---|---|---|
| Offline queue (`Kv` `undra.query.queue`: `schema_hash u64` + items `{mutation_id, params, idempotency_key}`, `crates/undra-query/src/persist.rs:88-122`) | on every enqueue/replay | **deleted** on load with a WARN, "dropping a persisted offline queue from another build" (`crates/undra-query/src/queue.rs:240-247`). The user's queued writes are gone. |
| Query cache entries (`Kv` `undra.query.cache.<id>.<hash>`: `schema_hash u64, updated_at i64, data`, `persist.rs:50-74`) | after each successful fetch (debounced) | each entry **deleted** with a DEBUG log (`crates/undra-query/src/shared.rs:845-857`). Adding an unrelated method changes the hash and wipes the whole cache. Entries are otherwise never evicted, and all are loaded into memory (`hydrated`, `:889`). |
| Snapshot (`undra_snapshot`/`undra_restore`, SPEC 5.9: stores × `{handle, type_id, signals × {signal_id, value}}`) | by the host, e.g. for crash recovery or a dev reload | **no identity at all.** Restore finds the store by `type_id` (a hash of its *name*) and each signal by its *position*, and decodes with the new types (`crates/undra-macros/src/impl_/store.rs:569-605`). A missing signal or an unknown store type fails the whole restore; a type change of compatible width **restores wrong values and returns `Ok`**. |

The audit's probe wrote a snapshot of `Profile { age: Signal<i32> = -5, tier: Signal<Tier> = Pro, score:
Signal<i64> }` and restored it into `Profile { age: Signal<f32>, tier: Signal<Tier> (a variant added in front),
score: Signal<Timestamp> }`: `Ok(())`, `age` became `f32` NaN (the bytes `fb ff ff ff`), `tier` became `Free`,
`score` became a timestamp. Reordering two signals of one type swaps their values the same way.

An app update is exactly when this happens, and an offline-first app is exactly the app that has a queue. The
architect's v1.x note fixed the direction: version every persisted artefact with the schema it was written
under, migrate with a hook the app implements in Rust, discard nothing silently.

## Decision

1. **Type fingerprints.** `undra-meta` gains `Schema::closure(&TypeRef) -> TypeClosure` (the type plus every
   record and enum it reaches, transitively, in canonical form: field and variant names, order and types; docs
   excluded) and `TypeClosure::fingerprint() -> u64` (`fnv1a64` of its canonical JSON). A store's fingerprint is
   the closure of its non-computed signals, by name, type and `signal_id`; a query's is the closure of its return
   type; a mutation's is the closure of its parameters, by name. A change elsewhere in the schema does not move a
   fingerprint.
2. **Every persisted artefact records what wrote it.**
   * *Snapshot* (SPEC 5.9, new layout, after ADR-022's two leading words):
     ```
     count u32, generation_floor u32,
     schema_hash u64,
     type_count u32, types × { type_id u32, fingerprint u64 },      // binary: the fast path
     description_len u32, description bytes,                         // canonical JSON of the stores' closures
     stores × { handle u64, type_id u32, signal_count u32, signals × { signal_id u32, len u32, value } }
     ```
     The description names every snapshotted store type's signals (`signal_id → name, ty`) and the records and
     enums they reach; it is parsed only when a fingerprint differs.
   * *Query cache entry* (Kv `undra.query.cache2.<id>.<hash>`): `format u16 = 2, schema_hash u64, fingerprint u64,
     updated_at i64, data`. The closure is stored once per fingerprint under `undra.types.<fingerprint as 16 hex digits>`
     (content-addressed, written before the first entry that needs it, deleted when hydration finds no entry
     referencing it).
   * *Offline queue* (Kv `undra.query.queue2`): `format u16 = 2, schema_hash u64, items × { mutation_id u32,
     fingerprint u64, params, idempotency_key Uuid }`, closures under `undra.types.*` as above.
   The Kv artefacts move to new keys (`undra.query.cache2.<id>.<hash>`, `undra.query.queue2`), so a value's layout
   is never guessed from its bytes. A v1 key found at hydration is read once with the v1 decoder: when its
   `schema_hash` equals the current one its items are current and are rewritten in format 2; otherwise they have
   unknown identity (decision 5's last step: a v1 queue item becomes a dead letter, a v1 cache entry is dropped and
   reported). A snapshot in the v1 layout fails to decode with a typed error, as ADR-022 decided for the layout
   before it; nothing persisted a snapshot outside this repository.
3. **Dynamic values.** `undra-runtime` gains `persist::{DynValue, DynRecord, MigrateError}`: a value tree decoded
   from bytes by walking a `TypeRef` in a `TypeClosure` (`Bool`, `Int(i128)`, `Float(f64)`, `String`, `Bytes`,
   `Duration`, `Timestamp`, `Uuid`, `None`/`Some`, `List`, `Map`, `Record { name, fields by name }`,
   `Enum { name, variant, fields by name }`), and encoded back against a (possibly different) `TypeRef`. The
   decoder is bounded like `Reader` (`MAX_DEPTH`, counts checked against the remaining bytes).
4. **Structural migration, by name, lossless only.** Converting an old value to a new type succeeds without app
   code when every step is one of:
   * records: fields matched **by name**; a field the old value lacks takes its default when it is
     `#[undra(default)]` or an `Option` (`None`); a field the new type lacks is dropped; order is irrelevant;
   * enums: variants matched **by name**, their fields as records (index changes are irrelevant);
   * integers widened without loss (`i8→i16→i32→i64`, `u8→u16→u32→u64`, `uN→i2N`), `f32→f64`, `T→Option<T>`,
     `Bytes↔Vec<u8>`; `Vec`, `Option` and `Map` recurse;
   * store signals matched **by name**; a signal the snapshot lacks takes `Default` when the field carries
     `#[undra(default)]` (now accepted on `Signal<T>` fields); a signal the new store lacks is dropped.
   Anything else - a narrowing, a type change, a renamed field, variant or signal, an enum variant the new type
   does not have - is not structural and goes to decision 5.
5. **Load algorithm, per item** (a store record, a cache entry, a queued mutation):
   1. fingerprint equal → decode as today (no `DynValue`, no JSON; the common case costs one comparison);
   2. else structural migration (4) from the recorded closure to the current one;
   3. else the app's hook (6) for that item, else for its root type;
   4. else the **refusal of that kind** (7). An item without a recorded closure (unknown identity) starts here.
   A migrated cache entry or queue is re-persisted in the current format at once, so migration runs once.
6. **Migration hooks in Rust.** `#[undra::migrate]` on a free function registers a `Migration` through
   `inventory`:
   ```rust
   #[undra::migrate(ty = "Todo")]                      // any persisted Todo whose structure changed
   fn todo_v1(old: &DynValue) -> Result<Todo, MigrateError> { .. }

   #[undra::migrate(store = "Profile", signal = "age")] // one signal of one store (also: a signal added without a default)
   fn age(old: Option<&DynValue>) -> Result<f32, MigrateError> { .. }

   #[undra::migrate(mutation = "add_todo")]            // queued inputs of one mutation, by parameter name
   fn add_todo(old: &DynRecord) -> Result<DynRecord, MigrateError> { .. }
   ```
   Typed hooks are encoded with the type's `Encode`; the mutation hook's `DynRecord` is encoded against the
   current parameter types with decision 4's rules. Hooks run on the core inside hydration or restore, under the
   panic guard (a panicking hook is a failed migration, logged with the hook's name). An optional
   `from = "0x…"` restricts a hook to one old fingerprint. Wrong targets (an unknown type, store, signal or
   mutation) are compile errors (`E0066`, what/why/fix) checked against the registrations the macro can see, and
   a runtime ERROR at start-up for the rest.
7. **Refusals are loud and kind-specific: nothing is discarded silently.**
   * *Snapshot*: the restore fails as a whole, as ADR-023 requires, with `RestoreError::Incompatible { type_id,
     store, signal, reason }` (a new `restore_code`, and the reason in the ERROR log the host's Log port receives).
     One exception: a store **type** the current schema no longer has is left out (its handle answers
     `stale_handle`) and reported, instead of failing everything - the app removed that screen.
   * *Query cache entry*: deleted, because it can be fetched again, and reported (WARN log naming the query and the
     reason, a `persist.dropped` counter in `stats_json`).
   * *Offline queue item*: **never deleted.** It moves to a dead-letter queue (`Kv` `undra.query.queue.dead`, same
     format plus a `reason String` per item) and the rest of the queue replays. `QueryClient::dead_letters() ->
     Vec<DeadLetter { mutation, idempotency_key, reason, params: DynRecord }>`, `retry_dead_letter(key)` (runs the
     hooks again, e.g. after an update that adds one) and `discard_dead_letter(key)` let the app show, export or
     drop them; a `persist.dead_lettered` counter and a WARN log report each one.
8. **The persisted cache is bounded.** At most `max_persisted_entries` (default 1,000, configurable on the
   `QueryClient`) entries are kept, least recently updated evicted (Kv key deleted) when a new one is written and
   at hydration; `undra.types.*` keys without a referencing entry are deleted at hydration.
9. **Restore by name everywhere, and re-observe by type.** With 2 and 4, restore matches signals by name when
   fingerprints differ and by `signal_id` when they are equal (they then agree). Restore's re-observe phase
   (ADR-023 §1) records the store type with each observation and re-observes a handle only when the restored store
   there has that type; otherwise the observation is dropped and logged, so a mirror typed for one store never
   receives another's entries (runtime review N6).

## Alternatives considered

* **Keep the whole-schema hash and add an app hook for "the hash changed".** The hook would receive opaque bytes
  of types that no longer exist in the binary; without the old structure it cannot decode them. That is why the
  closure travels with the data.
* **Embed the full schema in every artefact.** Simpler, but a snapshot or a queue would carry every type of the
  app; per-item closures carry only what the data needs, and the Kv side stores each closure once.
* **Serde/JSON for persisted values.** Self-describing and migratable by name, but a second encoding of every
  type (R1: one description) and slower and larger than the wire bytes the cache already holds. The binary
  stays; only the description is JSON, and only read when a fingerprint differs.
* **Partial restore by default** (restore what migrates, start the rest fresh). Kinder for crash recovery, but
  ADR-023's all-or-nothing exists because a half-restored app has handles that point at nothing; the one
  exception (a store type that no longer exists) keeps that property. A `RestoreOptions { partial: true }` can be
  added later if apps ask.
* **Version numbers the developer bumps** (`#[undra::store(version = 3)]`). Developers forget; a fingerprint
  cannot be forgotten, and decision 4 handles the additive changes that make up most updates without any code.

## Consequences

* An app update that adds a field (with `#[undra(default)]` or as an `Option`), adds a signal with a default,
  reorders fields or signals, adds enum variants or widens an integer keeps every persisted cache entry, every
  queued mutation and every snapshot, with no app code. Anything harder needs a hook, and without one the
  outcome is a typed refusal or a dead letter that the app can show.
* Restore of an unchanged store costs one fingerprint comparison per store type more than today; the
  `snapshot/cold_start_restore_100kb` budget (3 ms; 71 µs measured) has the room. A snapshot grows by its
  description (a few KB for a typical app).
* The `Snapshot` payload changes (SPEC 5.9): the three platform codecs and their tests follow; hosts are
  unaffected (opaque bytes). It ships in the same pre-publication wire revision as ADR-036.
* SPEC 3.1's "evolution across hash mismatch is a v2 feature" is narrowed: *persisted* state evolves in v1.x;
  *live* peers with different hashes still refuse each other (R7).
* `#[undra(default)]` gains a meaning on signals; `#[undra::migrate]` is the seventh attribute macro (re-exported
  from the facade and the prelude). E0066 joins SPEC §12.
* B3 (state-preserving dev reload) restores across a rebuild that changed a store without misdecoding, and A6
  (web crash recovery) can keep a snapshot across an app update.

## Implementation brief

1. `crates/undra-meta`: `TypeClosure` (canonical JSON of a `TypeRef` plus the reachable `RecordDef`/`EnumDef`s,
   sorted by name), `Schema::closure`, `fingerprint`; store/query/mutation fingerprint helpers; unit tests that a
   doc change or an unrelated type does not move a fingerprint and that each structural change does.
2. `crates/undra-runtime/src/persist.rs` (new, public): `DynValue`, `DynRecord`, `MigrateError`, `decode_dyn(bytes,
   &TypeRef, &TypeClosure)`, `encode_dyn(&DynValue, &TypeRef, &TypeClosure)` implementing decision 4; the
   `Migration` registration type and lookup (`inventory`); proptest: `encode_dyn(decode_dyn(x)) == x` for every
   wire type, and each structural rule.
3. `crates/undra-wire/src/payload/snapshot.rs` + `crates/undra-runtime/src/runtime.rs` (`snapshot` `:2003`,
   `restore` `:2078`): the new layout; restore builds, per store record, either the fast path or a migrated body
   (signals re-encoded by name into the body `StoreObject::restore` expects), then proceeds exactly as today;
   `RestoreError::Incompatible`, the removed-store-type exception, the new restore code in `undra-ffi`.
4. `crates/undra-macros`: `#[undra(default)]` on `Signal<T>` fields (the restore path fills it when absent);
   `#[undra::migrate(ty | store+signal | mutation, from?)]` with E0066; facade re-export.
5. `crates/undra-query/src/persist.rs`: format 2 for entries and the queue, `undra.types.*` closure keys;
   `shared.rs` hydration (`:832-859`) and `queue.rs` (`:206-249`) run decision 5; dead-letter queue and the
   `QueryClient` API; `max_persisted_entries` and eviction; counters in `stats_json`.
6. Platform `Snapshot` codecs (Swift `Wire/Payloads.swift`, Kotlin `wire/Payloads.kt`, TS `wire/payloads.ts`) and
   their tests; `contract-tests/wire-vectors.json` gains `snapshot_v2` (synced to Swift, regenerated for Kotlin).
7. Contract scenarios: S14 gains "a queue written by build A replays in build B after an additive change, and an
   incompatible item is dead-lettered, not lost" (two playground core builds: the contract runner builds the second
   with a feature flag that adds a field); S15 gains "a snapshot from build A restores into build B with a
   reordered and an added signal" and "an incompatible signal refuses the restore with the typed error".
8. Bench: `snapshot/cold_start_restore_100kb` unchanged within budget on the fast path; a new
   `snapshot/restore_100kb_migrated` row (every store migrated structurally) with a budget of 10x the fast path.
9. SPEC 2 (fingerprints), 3.1, 5.9, 9, 12, 16.2; the cookbook's "shipping an update" page (H1) lists what migrates
   for free and how to write a hook.

## Dependencies

Extends ADR-022 (the snapshot's leading words stay) and ADR-023 (all-or-nothing restore, with one stated
exception). Lands in `wt/persistence-v2` after ADR-034/035/036; its snapshot layout ships in the same wire revision
as ADR-036. B3 and A6 depend on it.

## Implementation notes (2026-10-01, `wt/persistence-v2`)

Landed as specified, items 1 to 9, with these decisions and one deviation:

* **Wire revision.** As ADR-036 decided for itself and Amendment C bundles ("one wire revision for 036+037"), nothing
  is published and every peer is in this repository: the envelope `version` stays **1** and `undra_abi_version` is
  unchanged. Snapshot layout 2 is the revision; a layout-1 snapshot fails to decode (restore code 5) because the
  decoder rejects a store whose type is not in the type table and a type listed twice (tested on the three platform
  codecs and in contract scenario S15 step 14). Live peers with different schema hashes still refuse each other (S16).
* **Closures** (`undra-meta`): the canonical form covers structure only (names, order, types, indices, tuple-ness,
  `default` flags); error messages, type ids and docs are not in it. `SignalDef.default` (serialized only when
  `true`) records `#[undra(default)]` on a `Signal<T>`.
* **Zero values.** A record field the old value lacks takes its *zero value* when it is `#[undra(default)]` (`false`,
  `0`, `""`, empty, `None`, the nil UUID, the epoch: what the generated constructors default to); a named type has no
  zero value the schema knows, so such a field is not structural and needs a hook. A store signal with
  `#[undra(default)]` takes `T::default()` (the generated restore fills it).
* **`DynValue`** has a `Float32(f32)` variant besides `Float(f64)`, so an `f32` (a NaN's payload included) survives
  a decode and an encode bit for bit; the proptests round-trip every bit pattern.
* **Hooks.** Structural migration first, then the item's hook (store and signal; mutation), then the hook of the root
  type; `ty` hooks also apply inside (to a record or enum in a list, say). Among hooks for one target, one whose
  `from` equals the old fingerprint wins over one without `from`. E0066 at compile time covers the arguments, the
  function's shape and, for `ty`, the identity of the returned type (a const assertion like E0061); a store, signal
  or mutation is not named by the signature, so its existence (and a signal hook's return type) is checked when a
  runtime starts, which logs an E0066 ERROR.
* **Restore.** `Runtime::restore_with_report` (additive) returns `RestoreReport { restored, migrated, dropped,
  schema_changed }`; `restore` is unchanged in shape. A store type the build no longer has is in `dropped` and a WARN.
  `RestoreError::UnknownStoreType` is no longer produced (kept for code that matches it).
* **Performance.** The structural conversion is streamed (old bytes to new bytes, walking both types; a type
  described identically in both builds is copied as bytes); a `DynValue` is decoded only for a hook. A runtime
  computes a store type's fingerprint lazily, for the types a snapshot or restore holds. Measured: the fast path
  `snapshot/restore_100kb` 27 µs, `snapshot/restore_100kb_migrated` 85 µs (3.1x; the budget is 10x, held by a
  budget and a ratio gate), `snapshot/cold_start_restore_100kb` within its 3 ms budget.
* **`undra-query`.** A cache entry of a query this build does not define, or no longer persists, is dropped and
  reported (format 1 kept it). A queue the store reports `Corrupt` cannot be moved with its bytes (they cannot be
  read): it becomes a dead letter with empty bytes and the reason, and since `Corrupt` is not transient the queue
  counts as read (ADR-049 1.4's rule that the key is never written unread applies to `Locked`, `Io`, `Unavailable`).
* **Contract scenarios.** S14 steps 7 to 9 and S15 steps 11 to 14; "two playground core builds" is build B selected
  by `UNDRA_PLAYGROUND_V2=1` (or the `migration-v2` feature; `build.rs` turns either into `cfg(playground_v2)`),
  because the CLI passes no Cargo features to a core. Swift and Kotlin run build B in a second process.

**Deviation:** none in behaviour. The ADR's `DynValue` list gains `Float32` (above), which only widens what a hook
can receive.

