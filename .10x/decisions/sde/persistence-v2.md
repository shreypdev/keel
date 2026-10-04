# SDE - persistence-v2: persisted state migrates by name, storage has an error channel, a trapped web core restarts (wt/persistence-v2, 2026-10-01)

Track A, pieces A5 (ADR-037), A6 and A7 (ADR-049), from the gap audit `.10x/specs/2026-10-01-v1x-gaps.md` (PS-1…PS-4,
N6; PO-3, PO-4, PO-11, PC-2, PA-5). One worktree, no push; the Rust side by the piece's implementer, the three runtimes
(and their contract columns) by three delegated agents working in the same worktree on disjoint paths, each committing
only its own paths. `main` merged twice (3e8a304, the web size gates of ADR-052; then 6db6749, dev-reload, rn-adapters
and derived-lists) before the final verification.

## The wire and ABI decision

**No version moves.** ADR-036 §6 kept the envelope `version` at 1 ("nothing is published and every peer is in this
repository") and Amendment C bundles ADR-036 and ADR-037 into one pre-publication wire revision; ADR-037 and ADR-049
both say "no C ABI or wasm ABI change". So: envelope `VERSION` stays **1**, `undra_abi_version` stays 1, no import or
export changes signature (ADR-049's "the `random` import throws" is implemented without one, below). What moves:

* the **Snapshot payload** is layout 2 (`count, generation_floor, schema_hash u64, types × {type_id, fingerprint},
  description, stores`); a layout-1 snapshot is refused with a typed error (restore code 5) because the decoder rejects
  a store whose type is not in the type table and a type listed twice: tested in the Rust codec and in the Swift,
  Kotlin and TypeScript codecs (vector `snapshot_v2`), and in contract scenario S15 step 14 on all three columns;
* `undra_restore` gains result code **7** (`INCOMPATIBLE`);
* **every core's schema hash moves once** (the standard surface: `StorageError`, the storage ports' signatures, two
  `FsError` variants; the standard surface alone is `0xbbf6_f70d_0c56_7f47`), which R7's load-time check enforces as
  ever (S16 on all three columns). The playground's hash is **0xfa536b9ac6f06149** after the last merge (new surface
  of this piece: the `updates` module, see S14/S15/S21 below; main's `platform` module now answers `StorageError`).

## ADR-037, per brief item

1. **`undra-meta`** (`closure.rs`): `TypeClosure { root: Type | Params | Signals, records, enums }` in a canonical JSON
   of structure only (names, order, types, indices, tuple-ness, `default` flags; no docs, messages or type ids),
   `fingerprint() = fnv1a64`; `Schema::closure / closure_of_params / store_closure / store_fingerprint / query_closure /
   mutation_closure / stores_closure` (`StoresClosure`, the snapshot description, gives back each store's closure with
   the writer's fingerprint); `narrowed`. `SignalDef.default` (serialized only when `true`; a schema without such a
   signal hashes as before). Unit tests: docs, messages, computed signals and unrelated items do not move a fingerprint;
   14 kinds of structural change each do.
2. **`undra-runtime::persist`** (new, public): `DynValue` (with `Float32`, bit-exact), `DynRecord` (get/set/remove/
   rename), `MigrateError` (path + message), `decode_dyn`/`encode_dyn`, `decode_params`/`encode_params`, `migrate`
   (streamed: old bytes to new bytes walking both types, a type described identically copied as bytes; `persist/
   stream.rs`), `migrate_value` (the tree form), `migrate_params`; `Migration { name, target, from, returns, hook }`
   through `inventory`, `find_hook` (an exact `from` beats none), `check_migrations` (the start-up E0066 ERRORs).
   Bounded like `Reader` (`MAX_DEPTH`, counts against the remaining bytes). Proptests: `encode_dyn(decode_dyn(x)) == x`
   and the identity migration for every primitive (every f32/f64 bit pattern), containers, records and enums, and for
   random nested types with random valid encodings; T→Option<T>; every lossless widening; the streamed and the tree
   conversion agree. Unit tests for each structural rule and each refusal, hooks (nested `ty`, `from`, a panicking
   hook names itself).
3. **Snapshot layout 2 + migrating restore** (`undra-wire` `payload/snapshot.rs`, `undra-runtime` `runtime.rs`):
   `snapshot()` writes the schema hash, each store type once with its fingerprint (computed lazily per type), and the
   description (cached per set of types). `restore()` takes the fast path when the fingerprint matches; otherwise
   works out each store type's old and new closure once (checking the description against its fingerprint), converts
   each current signal by name (structurally, then the store-and-signal hook, then the root type's hook), fills a
   missing `#[undra(default)]` signal (the generated restore uses `T::default()`), drops removed signals, and refuses
   with `RestoreError::Incompatible { type_id, store, signal, reason }` (ERROR log; `undra-ffi` restore code 7). A store
   type the build no longer has is left out and reported (`restore_with_report` → `RestoreReport { restored, migrated,
   dropped, schema_changed }`, a WARN). Re-observe only as the same store type (N6). `undra-ffi`: the empty snapshot
   with no runtime is layout 2.
4. **`undra-macros`**: `#[undra(default)]` on a `Signal<T>` (E0008 on a `Computed<T>`), recorded in `SignalMeta` and
   used by the generated restore; `#[undra::migrate(ty | store+signal | mutation, from?, crate?)]` (`impl_/migrate.rs`)
   with E0066 (code constant, `diag.rs` row, SPEC §12 row, two UI goldens: the arguments and shapes, and the `ty`
   identity const assertion; the catalogue audit passes; the errors page regenerated: 46 codes). Facade: `undra::migrate`,
   `undra::persist`, and `DynValue`/`DynRecord`/`MigrateError` in the prelude. End to end: `crates/undra/tests/
   migrations.rs` (an older build's snapshot restores by name with a signal hook, a `ty` hook inside a list, a default, a
   widening and a variant added in front; a refusing hook refuses the whole restore and changes nothing; the audit's
   probe `i32 -5 → f32` is `Incompatible`, no longer a NaN).
5. **`undra-query`** (`persist.rs`, new `storage.rs`, `queue.rs`, `shared.rs`, `client.rs`): format 2 under
   `undra.query.cache2.*` / `undra.query.queue2` / `undra.query.queue.dead`, closures under `undra.types.<fp>` written
   before the first value that needs them; hydration per item (fast path, migrate, hook, refusal); a migrated item is
   rewritten at once; cache entries that do not migrate are deleted and reported (`persist.dropped`), queued mutations
   become dead letters (`persist.dead_lettered`), never deleted; format-1 keys read once; `max_persisted_entries`
   (default 1,000) with least-recently-updated eviction; unreferenced closures deleted at hydration (only when
   everything was read). `QueryClient::dead_letters / retry_dead_letter / discard_dead_letter /
   set_max_persisted_entries / persist_stats` (`PersistStats`); `stats_json` gains a `query` section through a new
   runtime extension point (`StatsSection`, `Runtime::try_extension`). Tests: `tests/storage.rs` (14: migration of an
   entry and a queued mutation from an "older build" schema, drop and dead-letter paths, a missing closure, eviction,
   Full and retry, a failed read, an unreadable queue never overwritten and replayed on `Active`, a backoff retry, a
   mutation hook, retrying a dead letter), `tests/persistence.rs` and `tests/offline.rs` moved to format 2.
6. **Platform `Snapshot` codecs**: Swift `Wire/Payloads.swift`, Kotlin `wire/Payloads.kt`, TS `wire/payloads.ts`, each
   with the layout bytes, round trips, truncation at every length, the refusals and the layout-1 refusal; vector
   `snapshot_v2` in `contract-tests/wire-vectors.json`, synced to Swift, regenerated for Kotlin, read by TS.
7. **Contract scenarios**: S14 steps 6–9 (format-2 queue; build A queues `save_note`/`tag_note`, build B migrates the
   first by parameter name and dead-letters the second, replays with the same `Idempotency-Key`) and S15 steps 11–14
   (build B restores a snapshot by name with a reordered and an added `#[undra(default)]` signal; an incompatible signal
   refuses the restore with code 7; a layout-1 snapshot is refused with code 5). "Two playground core builds": build B is
   `UNDRA_PLAYGROUND_V2=1 undra build ...` (the core's `build.rs` turns it, or the `migration-v2` feature, into
   `cfg(playground_v2)`; `examples/playground/core/src/updates.rs`). TS loads both wasm modules in one process; Swift and
   Kotlin run build B in a second process fed by a handover file and print only FAIL lines, so `check.sh` (last line of
   an id wins) fails the scenario only if build B fails.
8. **Bench**: `snapshot/restore_100kb_migrated` (new; every store migrated) and the ratio `migrated_vs_fast_restore`
   (max 10, the ADR's bound) in `bench/budgets.toml`; numbers below.
9. **Docs**: SPEC §2.2, new §2.5 (closures), §3.1, §4.3, new §4.5a (`#[undra::migrate]`), §5.9, §6 (restore codes), §9,
   §12 (E0066), §16.2; the site page **Shipping an update** (`site/docs/updates.html`, in the Guides group) and the
   queries page; ADR-037 Accepted with its implementation notes; ADR-023 note.

## ADR-049, per brief item

1. **`undra-ports`**: `StorageError { Unavailable(String)=0, Full, Locked, Corrupt(String), Io(String) }` with
   `From<PortError>` and `is_transient()`; `Kv`/`SecureStore` return `Result<_, StorageError>`; `FsError::{Full=3,
   Unavailable(String)=4}` (an unbound `Fs` is now `Unavailable`); fakes: `MemKv`/`MemSecureStore` with
   `fail(FailOn, StorageError)`, `fail_times`, `heal`, `failed_ops`, and `FailingKv`. Wire tests for every variant on every
   method; the E0062 golden now shows `Rng.fill` (storage no longer panics).
2. **`undra-bindgen`**: the `stdlib` table (nine types; the storage signatures), its test and the `stdlib` golden; the
   pre-ADR-031 hash test lists `stdlib` among the cases written after (its surface moved on purpose).
3. **`undra-query`**: every `Kv` call handles `Result` (above; ADR-049 decision 1.4): a failed write keeps the entry,
   one WARN per (operation, reason), `persist.write_failed`, retried at the next write, `Full` pauses new entries (one
   probe per trigger); a failed read starts the entry empty; a failed queue read leaves the queue unread (no replay, the
   key never written) and retries on `Active`, `Background` and a backoff; a queue that does not decode (or that the
   store says is `Corrupt`) is dead-lettered with its bytes. The raw `list` loop is a typed call that retries typed
   `Unavailable` for five seconds at start-up.
4. **Adapters**: Swift (`KeyValueAdapters.swift` with typed throws and a Keychain seam, `FsAdapter.swift`, the bridge's
   ERROR log naming the port, method and adapter; `StorageFailureTests`, 15), Kotlin (`StorageAdapters.kt`:
   `KeyValueBackend`, `StoragePort`; `FileAdapters.kt`; `PortRegistry`; `android-adapters`: Kv, Keystore SecureStore
   (`SecureSeal.failure` mapping), Fs; `StorageFailureTests` on the JVM and shared with the device; a `FaultyFileSystem`
   test file system), TypeScript (`adapters/*`: IndexedDB/WebCrypto/OPFS/Node, the storage ports registered anyway
   when no backend, `storage-failures.test.ts`).
5. **wasm randomness**: `builtin.rs` asks the host for 16 canary bytes more; an untouched or zeroed canary means no
   CSPRNG, so `Rng.fill` is unavailable after an ERROR record naming the cause (the proxy's E0062 then traps the core
   loudly). TS: the `random` import writes nothing without a CSPRNG; `UndraCore.load` rejects
   `UndraTransportError("unsupported", "WebCrypto is required")` first.
6. **TS worker**, protocol 3, `worker.ports`, the load-time error for a main-thread sync port in worker mode: see the
   TS agent's section below.
7. **TS recovery**: see below.
8. **Scenarios**: ADR-049's provisional S29/S30/S31 are **S20, S21, S22** (S19 is ADR-039's derived keyed list, which
   landed on main meanwhile). S20 (storage failures typed) runs on all three columns; its step 4 (an unreadable queue)
   has a native variant (one core per process: the harness fails the first read at load and S20 checks what that did)
   and the full TS variant (a fresh core). S21 (worker sync ports) and S22 (web recovery) are TS-only; `check.sh`
   expects them only from `ts`; the grid reads n/a for an id a platform's check does not expect. **62 cells**: 20
   Swift, 20 Kotlin, 22 TS.
9. **Bench**: `ts/snapshot_take_100kb` and `ts/recovery_restart_100kb`: see below.
10. **Docs**: SPEC §7, §8, §9 (Rust side), §11/§17.1 (TS side, below); ADR-024 and ADR-025 amendment notes; `docs/ERRORS.md`
    (typed storage failures); the web page (below).

## The TypeScript side (ADR-049 items 4 to 7 and 9; delegated, then reviewed and merged)

* **Storage errors** (`adapters/types.ts`, `codecs.ts`, `ports.ts`, `port-dispatch.ts`): `StorageError` (abstract
  class + namespace of five variants, Rust's messages), `StorageErrorCodec`, `StorageError.from`, `FsError.Full` /
  `Unavailable`, `fsErrorFrom`. The `Kv`, `SecureStore` and `Fs` ports answer status 1 for a `StorageError` / `FsError`
  instance; the built-in adapters map raw platform errors themselves (`QuotaExceededError`, `ENOSPC` → `Full`; an
  `OperationError` on decrypt or a bad stored format → `Corrupt`; no OPFS → `FsError.Unavailable`); an untyped throw is
  still status 2 with an ERROR naming the port (`PortImpl.name`, set by generated adapters) and `onError`.
  `browserAdapters()` registers `Kv`, `SecureStore` and `Fs` always, answering `Unavailable` with the reason.
  `test/storage-failures.test.ts` (66).
* **Snapshot layout 2** codec (`wire/payloads.ts`) and the `snapshot_v2` vector; `UndraRestoreError` constants 2/5/6/7.
* **WebCrypto**: `cryptoRng` throws without `crypto.getRandomValues`; the guarded `random` import writes nothing, so the
  core's canary trips; `UndraCore.load` rejects `UndraTransportError("unsupported", ...)` in both wasm modes first.
* **Worker protocol 3** (`worker-protocol.ts`, `worker.ts`, `wasm-worker.ts`): `init { asyncPorts, portsModule,
  recovery }`, `ports`, `restart`/`restarted`; ports from the `worker.ports` module are answered in the worker, async
  host ports cross, everything else gets 2 so the built-ins serve Clock/Rng/Log; a main-thread sync port is refused at
  load (`UndraError("options")` naming it); `WorkerPortsModule` with an `adapters` export for clock/rng/timer.
* **Recovery** (`recovery.ts`): `recovery: crashRecovery(options?)`, a layer over the core's transport (the restart
  sequence of ADR-049 3.4 in order: `onPanic`, in-flight calls fail `restarted`, the same compiled module on a `twin()`
  transport, restore with the generation floor raised to the highest the host holds, re-observe, re-create query
  handles from their recorded constructor call (`StoreOptions.recreate`, generated), `onCoreRestarted` + `onError`);
  the snapshot keeper where the core runs; the restart budget. `test/recovery.test.ts`, plus real-core tests in both
  wasm modes in `crates/undra-ffi/tests/wasm/ts-runtime.test.mjs`.
* **Size**: ADR-052's gate on the hello-world JavaScript runtime (26,000 bytes gzipped) came after ADR-049 was written;
  with recovery and protocol 3 built into `UndraCore` the hello runtime measured 29,788. Recovery became an object the
  app imports, the ports ship only the codec halves they use, and the hello runtime is 25,906 (the record re-taken).
* **Playground web**: recovery on, a Debug panel (restarts, "Crash the core", "down for good"), `configureRemote`
  re-applied in `onCoreRestarted`. **Contract column**: S14 steps 6 to 9 and S15 steps 11 to 14 with build B loaded in
  the same process, S20 (fresh cores for steps 3 and 4), S21, S22.

## Numbers

Final run on the tree with `main` (6db6749) merged, Apple M5 Pro, macOS 26.5 (a shared host: load average about 13).

| Check | Result |
|---|---|
| `cargo fmt --check`; `cargo clippy --workspace --all-targets -D warnings`; `clippy -p undra-ffi --target wasm32-unknown-unknown -D warnings`; `cargo doc --no-deps -D warnings` | clean |
| `cargo test --workspace` | 2,787 passed, 13 ignored; 4 of main's `undra-cli` `dev_reload` tests failed under the workspace's parallel load and pass alone (8/8, three runs, serial and parallel): main's timing, not this piece |
| `bash crates/undra-ffi/tests/wasm/run.sh` | raw 22/22, ts-runtime 32/32 |
| Swift `swift test` | 538, 0 failures |
| Kotlin `test-local.sh` (kotlinc 2.4.20 and 2.0.21) | 639 cases each, 0 failed, 2 skipped (no native library) |
| `./gradlew :android-adapters:test` | 284 (both variants), 0 failed, 2 skipped |
| `connectedAndroidTest` on the `undra` AVD (`emulator-5554`, Android 15) | 124: 123 pass, 1 skipped (the gated network toggle), 0 failed (one earlier run lost `LifecycleOnDeviceTest.recreating_the_activity...` to the shared emulator; the class and the suite then passed) |
| TS `npm test` + typecheck | 1,259 in 36 files; clean |
| `bash contract-tests/run-all.sh` | **62/62**: S01–S20 on Swift, Kotlin and TS, S21 and S22 on TS |
| interop `run.sh ts`, `run.sh kotlin` | OK, OK |
| `undra bindgen -C examples/playground --check --docs` | up to date, **0xfa536b9ac6f06149** (build B: its own hash, read from the module by the runners) |
| `cargo test -p undra-cli --test schema_docs -- --ignored` | 1 passed |
| `cargo test --release -p undra-bench --test budgets` | 6 passed (one earlier run on the loaded host missed a row and passed on the rerun); ratio `migrated_vs_fast_restore` 2.7 (max 10) |
| `sync_alloc`, `commit_alloc` (release) | 2 + 5 passed |
| `scripts/wasm-size.sh --record` | hello wasm **119,565** bytes gzipped (budget 120,000; was 102,722), hello JS runtime **25,906** (budget 26,000; was 24,841) |
| playground web `npm test` / `tsc` / `npm run build` | 117; clean; built |
| live demo (`vite preview` of the built playground, the browser pane) | Counter at 3, Debug, "Crash the core": "the call failed: restarted", "1 restart: crashed on purpose ... stores restored from a snapshot 5741 ms old", the counter still 3, then 4 |
| `node site/scripts/build-all.mjs`; `check-links.mjs --words` | up to date; links OK, landing 342 words |

Bench (`bench/RESULTS.md`): `snapshot/restore_100kb` 28.0 µs and `snapshot/restore_100kb_migrated` 76.5 µs p50 in the
budgets test (2.7x; criterion medians on a quieter host earlier: 30.3 µs and 88.8 µs); `ts/snapshot_take_100kb` p50
0.063–0.066 ms (budget 2 ms) and `ts/recovery_restart_100kb` p50 3.12–3.22 ms, p99 at most 9.6 ms (budget 50 ms).

## Deviations, with why

* **`Rng.fill` unavailable without a signature change.** ADR-049 says the `random` import "throws"; a JS exception
  thrown through wasm frames abandons them without unwinding (locks held), and the shim could not tell otherwise. A
  16-byte canary the host must overwrite detects a host that wrote nothing or zeros, with no ABI change (which both
  ADRs require).
* **`DynValue::Float32`** (ADR-037 lists `Float(f64)` only): keeps an `f32` bit-exact across a decode and an encode.
* **Zero values** for a missing `#[undra(default)]` record field (the generated constructors' defaults); a named type has
  none (needs a hook). A store signal's `#[undra(default)]` is `T::default()`.
* **E0066 split** between the compiler (arguments, shape, `ty` identity) and start-up (store, signal, mutation
  existence and a signal hook's return type), because the macro cannot see those items.
* **`restore_with_report`** is an additive API for ADR-037's "reported" (and for `dev-reload`).
* **Dropped cache entries of undefined or no-longer-persisted queries** (format 1 kept them): bounded storage.
* **A queue the store reports `Corrupt`** cannot be moved with its bytes (they cannot be read): a dead letter with
  empty bytes, and the queue counts as read since `Corrupt` is not transient.
* **Build B by an environment variable** (`UNDRA_PLAYGROUND_V2`) as well as a Cargo feature: `undra build` passes no
  features to a core (and `tooling` owns the CLI).
* **ADR-052's size gates** landed on `main` while this piece was open. Every core carries snapshot layout 2 and the
  migrating restore (`undra_restore` is an export), which put the hello-world web core at 135.3 KB gzipped. Brought back
  under the 120 KB budget (119,565, re-recorded) by: a closure JSON written and read by hand (`closure_json.rs`; the
  reader reads the canonical form only, which is all a core writes; `serde` and `serde_json`'s deserializer were about
  100 KB of wasm before `wasm-opt`); hooks reached through the `HookSupport` each `#[undra::migrate]` carries (a core
  without hooks links no value decoder, hook runner or E0066 check); insertion sorts and lists instead of maps;
  widened integers written unchecked; the streamer's skip checks lengths only (the restorer validates the copy); one-line
  doc comments on the storage types (a core embeds its schema's docs). The JS runtime: recovery is `crashRecovery()`
  (below). **The hello core has 435 bytes of headroom left** (open items).
* **`LoadOptions.recovery: crashRecovery(options?)`**, not `true | { .. }`: the gate on the hello JS runtime folds
  lazy imports into what it measures, so only code the app imports can stay out (recovery built into `UndraCore` was
  29,788 bytes against 26,000).
* **The query client persists only queries the schema describes** (a persisted entry carries its type's closure): a
  `QueryDef` written by hand with no `Registration::Query` is not persisted (main's `hand_built` test now registers
  its query's meta).
* **Swift, merged with main's sealed `Kv` entries**: a damaged entry header is `Corrupt` on `get` (ADR-049), not "no
  value" as rn-adapters had it; `list` still skips it, `delete` and `set` repair it; a `Locked` read in `list`
  propagates instead of reporting an empty store.
* Kotlin: the bridge also accepts a standard error type thrown raw (additive); no `Unavailable` stand-in storage ports
  on Android (an install-after-load would otherwise end hydration's wait). Swift: typed throws in `KeyValueBackend`;
  `EPERM` before first unlock is `Locked`, `EACCES` is `Io`; extra Keychain statuses mapped (`errSecAuthFailed` →
  Locked, `errSecDecode` → Corrupt, `errSecNotAvailable`/`errSecMissingEntitlement` → Unavailable, `errSecDiskFull` →
  Full).

## For `dev-reload` (ADR-053 section 4; it landed on `main` while this piece was open)

ADR-053 treats a changed schema hash as "fresh state" because, before ADR-037, a restore into changed types could
misdecode. With this piece the restore is safe across a rebuild, so the one place ADR-053 named changes:

* When the new runner's schema hash differs from the old one, **attempt the restore** instead of answering `reset schema
  changed`: call `Runtime::restore_with_report(&snapshot)` (the public shape of `Runtime::restore` is unchanged; the
  report is additive). Store types whose fingerprint did not change take the fast path; changed ones migrate by name
  (and through the app's `#[undra::migrate]` hooks, which the rebuilt core links).
* `Ok(report)`: print `restored <report.restored> ...`; count `report.dropped` (store types the rebuild removed: their
  handles are stale) among the lost objects and remove their handles from the retained session, as for non-store
  handles; `report.migrated` names the store types that were converted, `report.schema_changed` is true.
* `Err(RestoreError::Incompatible { store, signal, reason, .. })`: the runtime is unchanged; answer `reset the core refused
  the snapshot: <error>` as for any refusal today (the message names the store and signal).
* What still resets: a client whose generated bindings carry the old hash is refused at its `Hello` (R7, unchanged), so
  a schema change still means `undra bindgen` for the app; the state the core holds is kept.

## Open items

* **The hello-world web core is at 119,565 of 120,000 bytes gzipped.** The next change to what every core links
  must pay for itself. Candidates measured while cutting: the standard ports' Rust dispatchers (`Kv`, `SecureStore`,
  `Fs`, `Http`: about 6 KB of wasm before `wasm-opt`, used only when a Rust implementation is bound) could be linked by
  use as ADR-052 did for the query runtime; the snapshot description could be binary (a wire change).
* **The hello JS runtime is at 25,906 of 26,000.** The TS agent's question for ADR-052: the gate folds the lazily
  imported `WasmWorkerTransport` (about 2 KB gzipped) into the measured chunk, which a default Vite build splits out;
  measuring only what an app loads up front (and importing `RemoteTransport` lazily) would free about 3.7 KB.
* **dev-reload** still resets on a schema change; with this piece it can restore across one (section above).
* **ADR-046's panic report**: `onPanic` receives a minimal `UndraPanicReport`; `thread`, `namespace`,
  `core_version`, `image_id` and address frames wait for that piece.
* `remote` keeps `UndraModeError` for `snapshot()`/`restore()` (no host-to-core snapshot request on the wire).
* Main's `undra-cli` `dev_reload` tests flake under heavy parallel load (Close frame lost); they pass alone.

HEAD: the commit that adds this record, on top of 05603ea (`main` 6db6749 is an ancestor).
