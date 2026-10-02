# Persisted-state migrations (ADR-037) and storage errors, worker sync ports, web recovery (ADR-049) — adversarial review

**Date:** 2026-10-02 · **Reviewer:** adversarial reviewer (`docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/persistence-v2` at `336637b` (`main` `6db6749` merged by the implementer) · **Read:** `CLAUDE.md` (R1, R4, R5, R6,
R7, R9, R11, R12), ADR-037 and ADR-049 with their implementation notes, ADR-022, ADR-023, ADR-052, ADR-053,
`.10x/decisions/sde/persistence-v2.md`, and the diff: `undra-meta` (`closure.rs`, `closure_json.rs`, `sort.rs`),
`undra-wire` (`payload/snapshot.rs`), `undra-runtime` (`persist.rs`, `persist/stream.rs`, the restore in `runtime.rs`),
`undra-query` (`storage.rs`, `queue.rs`), `undra-ffi` (`builtin.rs`, `api.rs`), `undra-ports`, `undra-macros`
(`port.rs`, `migrate.rs`), the TypeScript runtime (`recovery.ts`, `core.ts`, `transport/wasm-main.ts`,
`transport/wasm-worker.ts`, `worker.ts`, `worker-protocol.ts`, the adapters), the React Native module's C++ defaults,
the Swift, Kotlin and `android-adapters` storage adapters, the contract columns (S14, S15, S20–S22) and the dev reload
(`undra-cli` `reload.rs`, the runner template). Two read-only sub-audits (the storage adapters of every platform; the
web recovery sequence) fed findings H2 and H3, each then reproduced here with a failing test before it was fixed.
**Merge:** `main` `5f5c3fb` (the ABI table, ADR-044, S26, the two-core app) merged at `cf6e13a`. **Fixes:**
`ea81c3c`, `ee9a66e`, `7fda3b7`, `a0ebff2`, `a9c6999`, `TBD-q`; tests `96895a5`; size record `TBD-size`.

## Verdict

**Sound after the fixes; merge.** The core of ADR-037 holds up under attack: a snapshot truncated at every length and
with every byte changed, on the fast and the migrating path, never panics, is always refused with code 5 or 7, and a
refused restore leaves the core's snapshot byte-identical (all or nothing, ADR-023); the streamed migration and the tree
migration agree on thousands of random (type, value, schema-evolution) triples, and what they write always decodes as
the new type; the hand-written closure JSON writer is `serde_json::to_string` on generated closures (every escape,
control characters, astral characters, the numeric limits), its reader reads both back and never accepts a spelling
that means something else; the schema hash of every schema that does not use the standard ports is unchanged by this
piece (all ten non-stdlib bindgen goldens keep their hash; only `stdlib`'s moved, by the storage signatures).

Three Highs, all fixed with a test that fails without the fix:

* **H1 (data integrity)** — a `#[undra::migrate(store, signal)]` hook whose return type is not the signal's restored a
  wrong value and answered `Ok` (an `i32` hook for an `f32` signal: the bytes of `1084227584` restored as `5.0`), the very
  misdecode ADR-037 exists to prevent. Start-up only logged E0066.
* **H2 (React Native was not migrated to ADR-049)** — the module's native `Kv` and `SecureStore` answered every failure
  with port status 2, which the core reads as "no adapter registered (E0062)": a damaged queue stayed "unreadable" for
  ever instead of being dead-lettered, a full disk never paused persistence, and `cpp/test/run.sh` **failed** on the
  branch.
* **H3 (web recovery)** — a trap the new instance reported while its restart was still under way was dropped, leaving a
  dead core that answered "restarted" for ever (and, with nothing observed, even fired `onCoreRestarted`).

One blocking merge finding: after `main`'s ABI table the hello-world web core was **120,188 bytes gzipped, 188 over the
120,000 budget** (F1). The implementer's lever is applied (the standard ports' dispatchers linked by use): **116,677**,
3,323 bytes of headroom. Two Mediums fixed (the web restore floor could go down across restarts; an old worker script
with `worker.ports` trapped instead of refusing at load), and the record's open item 2 is done: `undra dev` now carries
the state across a schema change it can migrate (M3).

## Findings

### F1 — the merged hello-world web core was over its budget (blocking; fixed, `ea81c3c`)

Measured first thing after the merge: `web/hello-wasm` 287,450 bytes, **120,188 gzipped**, gate 120,000 (budget):
OVER by 188. The branch had left 435 bytes of headroom and `main`'s table added the rest. The lever the record named,
with ADR-052's "linked by use" pattern: the eight standard request/reply ports' Rust-side dispatchers were
`inventory`-submitted by `undra-ports`, and fat LTO keeps every submission, so every core linked them although only a
raw port call on a Rust-bound standard port (fakes, the dev runner's native ports) ever runs one. Now
`#[undra::port(dispatcher_by_use)]` (a hidden flag only `undra-ports` uses) emits `pub static <PORT>_DISPATCHER`
instead, and `Runtime::bind_dyn_port_with` / `Ctx::bind_dyn_port_with` bind a Rust implementation together with the
dispatcher raw calls run through (`fakes::install` and the dev runner's `Clock`/`Rng`/`Log` do). The accessors
(`undra_ports::kv(&ctx)`) never needed one; app ports are unchanged. Tests: the standard ports register no dispatcher;
a binding without one answers raw calls unavailable, with one it runs them; the macro's by-use expansion. Measured
**273,685 bytes, 116,677 gzipped** (−3,511); re-recorded (`TBD-size`). SPEC 16.1, an ADR-052 note.

### H1 — a mistyped store-and-signal hook restored wrong values with `Ok` (fixed, `ee9a66e`)

`crates/undra-runtime/src/runtime.rs` `convert_signal` / `missing_signal`: the hook's bytes were spliced into the
body `StoreObject::restore` decodes, whatever type the hook returned. The macro cannot check a signal hook's return
type (it does not see the store), and `check_migrations` only logs E0066 at start-up. Reproduced by
`crates/undra/tests/migrations_mistyped_hook.rs` (`Gauge { level: f32 }`, a hook returning `i32`, an old `i64` value):
`Ok(())`, `level = 5`. Fix: `returning(hook, &signal.ty)` refuses a hook whose recorded `returns` is not the signal's
current type before running it: `Incompatible { store: "Gauge", signal: "level", reason: "the migration hook ...
returns i32 but the signal is f32 (E0066)" }`, and the core's snapshot is byte-identical afterwards. `ty` hooks were
already safe (their identity is a compile-time assertion), mutation hooks too (`encode_params` re-encodes against the
current parameters).

### H2 — React Native's native storage answered "unavailable" for every failure (fixed, `a0ebff2`)

`runtimes/rn/@undra/react-native/cpp/UndraDefaults.cpp:225-319`: the branch never touched `runtimes/rn`. Success
encodings matched the new signatures (so nothing undecodable), but every `Kv`/`SecureStore` failure was status 2, so:
a damaged queue entry read as transient `Unavailable` and `queue.rs` waited for it for ever instead of dead-lettering
it (new offline mutations then live only in memory); ENOSPC never engaged "Full pauses new entries"; every WARN told
the developer to register an adapter. `cpp/test/host_test.cpp:951` still asserted the pre-ADR-049 call status 2, so
**`cpp/test/run.sh` failed** on the branch (`not ok - a failing store is unavailable ...`). Fix: `KvStore` reports a
`StorageErrorKind` (a damaged entry `Corrupt`; ENOSPC/EDQUOT `Full`; EPERM `Locked`, Swift's mapping; else `Io`) and
the defaults answer status 1 with the encoded `StorageError`; `SecureStore` failures are `Io` with the Keychain's or
Keystore's text (the platform layer gives no variant; open item); `FsErrorKind` gains `Full` (ENOSPC/EDQUOT) and
`Unavailable`. `stores_test.cpp` and `host_test.cpp` check `Corrupt` through the core, `Io` from a failing keychain and
`Unavailable` on out-of-memory. `cpp/test/run.sh`: 74 ok, `UndraPlatformApple.mm` compiles against the iOS SDK.

### H3 — a trap during the restart was lost: a dead core passed for a restarted one (fixed, `a9c6999`)

`runtimes/ts/@undra/runtime/src/recovery.ts:556` dropped every `closed` that arrived while `#restarting`, assuming a
trap during the restart always surfaces as a rejected restart. It does not when the new instance traps after its
restore answered (a task the restore woke, polled before the restart's continuation; in `wasm-worker` the worker posts
`closed` before `restarted`): the restart resolved, `#reattach` ran against a dead instance, every later call was
"restarted" for ever, `onClose` never fired, and with nothing observed `onCoreRestarted` reported success. Fix: such a
trap is kept and taken as the next trap of the restart loop (another restart within the budget, else the core is lost
with `onClose`). Two tests (`recovery.test.ts`), both failing before: the late trap restarts the core again (two
floors, two panic reports, one `onCoreRestarted`, calls work); with the budget spent it ends the core.

### M1 — the web restore floor could go down from one restart to the next (fixed, `a9c6999`)

`recovery.ts` `#floor()` read only the handles the host still tracked; `#reattach` forgets the ones that went stale,
while the app's wrappers keep them. A second restart (from the same or an empty snapshot) then used a lower floor and
the new instance could issue a stale wrapper's handle to another object (ADR-022). Fix: the floor never goes below the
one an earlier restart used. Test: an object of generation 7 goes stale at restart 1; restart 2's floor is 7 (was 0).

### M2 — an old worker script with `worker.ports` trapped at the first synchronous port call (fixed, `a9c6999`)

There is no protocol version check (the brief's "typed load error" is not what the branch does): a worker answers
`ready { features }` and the host degrades by feature. A worker older than protocol 3 ignores `portsModule`, so the
app's synchronous ports crossed to the main thread and trapped the core. Fix: a host given `worker.ports` refuses a
worker without the `"ports"` feature at load with `UndraTransportError("unsupported", "the worker script is older than
this runtime ...")`; without `worker.ports` an older worker still loads as before (existing tests). SPEC 11.1. Test in
`snapshot.test.ts`, failing before. The `bytes` → `data` rename of the snapshot payloads within the same `"snapshot"`
feature is harmless only because both sides ship in one package and nothing is published (open item).

### M3 — `undra dev` reset the state on every schema change although the restore can migrate (open item 2, applied, `7fda3b7`)

`reload.rs` no longer skips the snapshot when the hash changed; the runner restores with
`Runtime::restore_with_report`, which migrates by name or refuses as a whole and changes nothing. A store type the
rebuild removed counts among the objects not carried over. The notice says `Reloaded, state kept (the schema changed[:
<stores> migrated])`; a refusal is `state reset: the core refused the snapshot: store `Counter` ... signal `edits`:
...`. Old bindings are still refused at `Hello` (R7). `dev_reload.rs`: `an_additive_schema_change_keeps_the_state`
(a method added: the counter at 5 survives, a client on the new bindings resuming its session finds it, `increment`
gives 6) and `a_schema_change_the_state_cannot_follow_resets_it_and_says_why` (a signal renamed); the `reload.rs`
unit test; SPEC 5.10 and `docs/DEV_LOOP.md`.

### Lows and coverage added

* L1 — the `Corrupt`-queue path (`queue.rs:398`: a queue the store reports `Corrupt` becomes a dead letter with no
  bytes and counts as read) had no test anywhere; `crates/undra-query/tests/storage.rs` now has one (`TBD-q`).
* Coverage (`96895a5`): the damaged-snapshot fuzz, the streamed-vs-tree differential on random schema evolutions plus
  a hostile-bytes proptest, and the closure-JSON differential against `serde_json` (all described under the attacks).

## The attack, surface by surface

### 1. The migrating restore (ADR-023 all-or-nothing, one stated exception)

* **Every structural rule, proptest-level.** `persist/tests.rs` `the_two_conversions_agree_on_random_schema_evolutions`
  (1,024 cases per run; 20,000 run once): a random type up to depth 3 over records, enums, `Option`, `Vec`, `Map` and
  the primitives, a random valid value, read by a build with up to three random edits — fields reordered, dropped,
  added with and without `#[undra(default)]` (`Option`, `u32`, `Timestamp`, `Uuid`, a named type), a field renamed, a
  field retyped (`Option`↔non-`Option`, `Vec<T>` → `Vec<Option<T>>`, `Bytes`↔`Vec<u8>`, a narrowing), enum variants
  reindexed, removed, added in front, a variant's fields narrowed — and possibly a changed root type (wrapped,
  unwrapped, widened, narrowed). Both conversions convert to the same bytes or both refuse, and the output always
  decodes as the new type and re-encodes to itself. Clean. Store-level rules (signals by name, reordered, added with a
  default, removed, a renamed store is incompatible) are covered end to end in `crates/undra/tests/migrations.rs`; a
  keyed list whose key field changed is a `Vec<Record>` whose record changed: covered by the record rules (the key is
  not part of the persisted bytes).
* **A panicking hook** is a typed `Incompatible` naming the hook (existing `run_hook`; `persist/tests.rs`); on wasm a
  panic aborts, which is the trap recovery handles. **A hook of the wrong shape:** H1.
* **A damaged layout-2 payload at every byte offset:** `migrations.rs`
  `a_damaged_snapshot_is_refused_typed_at_every_byte_and_changes_nothing` truncates at every length and changes every
  byte four ways, on a fast-path snapshot and on an older build's (migrating) one: never a panic, every refusal is
  `Decode`/`Store`/`BadHandle`/`GenerationFloor` (code 5) or `Incompatible` (code 7), never `Panicked` (2), and a
  refused restore leaves `snapshot()` byte-identical. Clean. Cosmetic: a damaged *description* is reported as 7
  (incompatible), not 5.
* **`generation_floor` (ADR-022)** unchanged: a raised floor that still decodes restores and the counter never goes
  down (the fuzz re-takes its baseline after such a restore for exactly that reason); `u32::MAX` is refused.
* **The dev reload:** M3.

### 2. The size-cut code

* **The canonical closure JSON** (`closure_json.rs`): differential proptests (2,048 cases) against `serde_json` on
  generated `TypeClosure`s and `StoresClosure`s with names drawn from any `String` and from `"`, `\`, `/`, every control
  character, DEL, U+2028, `é`, `🦀`, U+FFFD, and `u32`/`u16` ids at 0, the maximum and random: the writer is
  `serde_json::to_string`, the reader reads ours and serde's back, serde reads ours. Damaged canonical text (a byte
  replaced, removed or inserted anywhere) never panics the reader, and what it accepts serde reads as the same closure.
  There are no floats in a closure, so `-0.0` and NaN do not arise. Slicing is only ever at ASCII boundaries.
* **The schema hash** moved only through the standard surface: every bindgen golden that does not use the standard
  ports keeps its hash between `main` and this branch (`enums`, `errors`, `full`, `objects`, `ports`, `queries`,
  `records`, `recursive`, `stores`); `stdlib` moved (`0x6b4638c4a5e34313` → `0x939c8bf0009876ba`). The merge of `main`
  did not move the playground's: **0xfa536b9ac6f06149** before and after (`main`'s own playground is
  `0xc5f05c376fde398c`, without this piece).
* **"Unchecked widened integers":** every `as` cast the branch added is bounded: `write_widened` (`stream.rs:265`) runs
  only when `widens(from, to)` holds and its input is read as `from`, so it is lossless; the `u64` arm is reached only
  for an unsigned source; `builtin.rs` `len as usize` after `len <= 1 << 24`; `closure_json.rs` on a `char < 0x20`;
  `persist.rs:654` `f64 as f32` is checked to be exact right after. Indexes driven by a snapshot (`convert_fields`'
  spans, `Reader::at`, `consumed_since`) are positions the reader produced, and counts are bounded by `read_count`
  against the remaining bytes. The fuzz above is the evidence.
* **Insertion sorts** (`sort.rs` `insertion_by_key` / `insertion_by_name`): stable and equal to `sort_by_key` on any
  input (existing proptests); they order a closure's signals by id, variants by index and a snapshot's store types by
  name, which feed the fingerprint, so stability matters and holds. Both they and the streamer's `SortedEntries`
  (binary search + `Vec::insert`) are O(n²) on adversarial input; only a crafted snapshot description or map can reach
  that (open item, Low).
* **Layout 1** is refused structurally, not by a tag: the first store's handle reads as the hash and its type id (a
  32-bit hash) as the type count, which `read_count` refuses unless it is below the remaining bytes / 12, and even then
  the type table and every store must line up. Negligible, and fail-safe (a misparse would still meet the fingerprints).

### 3. Storage errors and the queue

* `undra-query` with a `Kv` that fails `Full` on write, `Locked`/`Io` on read, `Corrupt` on the queue: the Rust tests
  cover the first two (`tests/storage.rs`), L1 adds the third.
* **The contract columns** (from the storage sub-audit, checked): Kotlin and Swift inject real failures through their
  harness `MemoryKv` and the runtime's port tables (status 1 with the encoded error), not only codecs; but their S20 step
  4 is thin (the `Locked` read injected at load, a later good read, no write between) and step 3 is skipped (one core
  per process); only TypeScript exercises "never overwritten while unreadable, replayed once readable" end to end.
  Open item.
* **Android `SecureStore`:** `SecureSeal.kt:96-105` maps `KeyPermanentlyInvalidatedException` to `Corrupt`,
  `UserNotAuthenticatedException` to `Locked`, a failed authentication tag to `Corrupt`; tested through a fake key source
  (`StorageFailureTests`), not a really invalidated Keystore key, and the invalidated key stays cached (open item, Low).
* **React Native:** H2.

### 4. Web recovery

* The restart sequence (from the recovery sub-audit, checked against the tests): `onPanic` first, every call and stream
  in flight rejected `restarted` exactly once, the same compiled module again, the last snapshot restored with the floor
  raised, stores re-observed, query handles re-created from their recorded call, `onCoreRestarted` then `onError` once
  per round; a trap during the restart counts against the budget; the fourth trap within `perMs` (`maxRestarts` 3)
  leaves the core closed with `UndraTransportError("trap")` and `onClose` once. Found: H3, M1.
* **The snapshot keeper at 10k commits/s:** O(1) per change-set (a flag; one timer per period; `requestIdleCallback`
  where there is one), one O(state) snapshot per `snapshotEveryMs`. TBD-stress.
* **The Rng canary:** `UndraCore.load` rejects `UndraTransportError("unsupported", "WebCrypto is required ...")` before
  instantiating in both wasm modes when `crypto.getRandomValues` is missing; when it throws later the guarded `random`
  import writes nothing and no exception crosses wasm frames, the canary trips and `Rng.fill` answers unavailable. As
  ADR-049 2.5 decides, `Rng` has no error channel, so the proxy's E0062 then panics: a loud trap (which recovery then
  restarts, and a deterministic loop ends dead after the budget) — "nothing traps" holds for the throw itself, not for
  the core's use of `Rng` afterwards.
* **Worker protocol 3:** M2.

### 5. R7 and R12

Layout 2 with the envelope `VERSION` still 1 is Amendment C's "one wire revision for 036+037". A layout-1 snapshot is
refused with code 5 on all three platforms (S15 step 14 in each column, the `snapshot_v2` vector); live peers with
different hashes still refuse each other (S16). Nothing in the persistence code reads a clock, a random source or
starts a thread (`persist*`, `closure*`, `undra-query` storage, `snapshot.rs`): the cache's `updated_at` comes from the
`Clock` port. The TypeScript recovery's rate limits use `Date.now` (host side, not the core; open item, Low).

## Suites (after the merge and the fixes)

TBD-suites

## Open items

TBD-open

HEAD: TBD-head.
