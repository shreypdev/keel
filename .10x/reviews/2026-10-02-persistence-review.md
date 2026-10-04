# Persisted-state migrations (ADR-037) and storage errors, worker sync ports, web recovery (ADR-049) - adversarial review

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
**Merges:** `main` `5f5c3fb` (the ABI table, ADR-044, S26, the two-core app) at `cf6e13a`; `main` `f35c038` (devtools,
ADR-054) at `586624b`. **Fixes:** `ea81c3c`, `ee9a66e`, `7fda3b7`, `a0ebff2`, `a9c6999`, `a3f1e91`, `e22b153`,
`db1f334`; tests `96895a5`; the site's merged pages `fafd2c6`; size records `30cb08f`, `a1229cf`.

## Verdict

**Sound after the fixes; merge.** The core of ADR-037 holds up under attack: a snapshot truncated at every length and
with every byte changed, on the fast and the migrating path, never panics, is always refused with code 5 or 7, and a
refused restore leaves the core's snapshot byte-identical (all or nothing, ADR-023); the streamed migration and the tree
migration agree on thousands of random (type, value, schema-evolution) triples, and what they write always decodes as
the new type; the hand-written closure JSON writer is `serde_json::to_string` on generated closures (every escape,
control characters, astral characters, the numeric limits), its reader reads both back and never accepts a spelling
that means something else; the schema hash of every schema that does not use the standard ports is unchanged by this
piece (all nine non-stdlib bindgen goldens keep their hash; only `stdlib`'s moved, by the storage signatures).

Three Highs, all fixed with a test that fails without the fix:

* **H1 (data integrity)** - a `#[undra::migrate(store, signal)]` hook whose return type is not the signal's restored a
  wrong value and answered `Ok` (an `i32` hook for an `f32` signal: the bytes of `1084227584` restored as `5.0`), the very
  misdecode ADR-037 exists to prevent. Start-up only logged E0066.
* **H2 (React Native was not migrated to ADR-049)** - the module's native `Kv` and `SecureStore` answered every failure
  with port status 2, which the core reads as "no adapter registered (E0062)": a damaged queue stayed "unreadable" for
  ever instead of being dead-lettered, a full disk never paused persistence, and `cpp/test/run.sh` **failed** on the
  branch.
* **H3 (web recovery)** - a trap the new instance reported while its restart was still under way was dropped, leaving a
  dead core that answered "restarted" for ever (and, with nothing observed, even fired `onCoreRestarted`).

One blocking merge finding: after `main`'s ABI table the hello-world web core was **120,188 bytes gzipped, 188 over the
120,000 budget** (F1). The implementer's lever is applied (the standard ports' dispatchers linked by use): **116,677** (now **116,800** with H1's check and devtools' inspector seam), 3,200 bytes of headroom.
Four Mediums fixed: the web restore floor could go down across restarts (M1); an old worker script with `worker.ports`
trapped instead of refusing at load (M2); the React Native contract column, which CI runs, failed S14 and S15 because
its harness never loaded build B (M4); and the record's open item 2 is done: `undra dev` now carries the state across a
schema change it can migrate (M3). Three merge interactions fixed: the Swift column's build-B process still called the C
ABI v1 `undra_schema_hash` (it did not compile, so the second process ran a stale binary), `main`'s reserved-entry test
caught this piece's two new TypeScript `Undra…` names, and the cross-merge's site pages are merged three-way.

## Findings

### F1 - the merged hello-world web core was over its budget (blocking; fixed, `ea81c3c`)

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
**273,685 bytes, 116,677 gzipped** (−3,511); with H1's check 116,731, re-recorded (`30cb08f`). SPEC 16.1, an ADR-052 note.
The JavaScript runtime is the tighter one: **25,984 of 26,000** after M2 (whose first wording put it at 26,032, 32 over;
the message was shortened).

### H1 - a mistyped store-and-signal hook restored wrong values with `Ok` (fixed, `ee9a66e`)

`crates/undra-runtime/src/runtime.rs` `convert_signal` / `missing_signal`: the hook's bytes were spliced into the
body `StoreObject::restore` decodes, whatever type the hook returned. The macro cannot check a signal hook's return
type (it does not see the store), and `check_migrations` only logs E0066 at start-up. Reproduced by
`crates/undra/tests/migrations_mistyped_hook.rs` (`Gauge { level: f32 }`, a hook returning `i32`, an old `i64` value):
`Ok(())`, `level = 5`. Fix: `returning(hook, &signal.ty)` refuses a hook whose recorded `returns` is not the signal's
current type before running it: `Incompatible { store: "Gauge", signal: "level", reason: "the migration hook ...
returns i32 but the signal is f32 (E0066)" }`, and the core's snapshot is byte-identical afterwards. `ty` hooks were
already safe (their identity is a compile-time assertion), mutation hooks too (`encode_params` re-encodes against the
current parameters).

### H2 - React Native's native storage answered "unavailable" for every failure (fixed, `a0ebff2`)

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

### H3 - a trap during the restart was lost: a dead core passed for a restarted one (fixed, `a9c6999`)

`runtimes/ts/@undra/runtime/src/recovery.ts:556` dropped every `closed` that arrived while `#restarting`, assuming a
trap during the restart always surfaces as a rejected restart. It does not when the new instance traps after its
restore answered (a task the restore woke, polled before the restart's continuation; in `wasm-worker` the worker posts
`closed` before `restarted`): the restart resolved, `#reattach` ran against a dead instance, every later call was
"restarted" for ever, `onClose` never fired, and with nothing observed `onCoreRestarted` reported success. Fix: such a
trap is kept and taken as the next trap of the restart loop (another restart within the budget, else the core is lost
with `onClose`). Two tests (`recovery.test.ts`), both failing before: the late trap restarts the core again (two
floors, two panic reports, one `onCoreRestarted`, calls work); with the budget spent it ends the core.

### M1 - the web restore floor could go down from one restart to the next (fixed, `a9c6999`)

`recovery.ts` `#floor()` read only the handles the host still tracked; `#reattach` forgets the ones that went stale,
while the app's wrappers keep them. A second restart (from the same or an empty snapshot) then used a lower floor and
the new instance could issue a stale wrapper's handle to another object (ADR-022). Fix: the floor never goes below the
one an earlier restart used. Test: an object of generation 7 goes stale at restart 1; restart 2's floor is 7 (was 0).

### M2 - an old worker script with `worker.ports` trapped at the first synchronous port call (fixed, `a9c6999`)

There is no protocol version check (the brief's "typed load error" is not what the branch does): a worker answers
`ready { features }` and the host degrades by feature. A worker older than protocol 3 ignores `portsModule`, so the
app's synchronous ports crossed to the main thread and trapped the core. Fix: a host given `worker.ports` refuses a
worker without the `"ports"` feature at load with `UndraTransportError("unsupported", "the worker script is older than
this runtime ...")`; without `worker.ports` an older worker still loads as before (existing tests). SPEC 11.1. Test in
`snapshot.test.ts`, failing before. The `bytes` → `data` rename of the snapshot payloads within the same `"snapshot"`
feature is harmless only because both sides ship in one package and nothing is published (open item).

### M3 - `undra dev` reset the state on every schema change although the restore can migrate (open item 2, applied, `7fda3b7`)

`reload.rs` no longer skips the snapshot when the hash changed; the runner restores with
`Runtime::restore_with_report`, which migrates by name or refuses as a whole and changes nothing. A store type the
rebuild removed counts among the objects not carried over. The notice says `Reloaded, state kept (the schema changed[:
<stores> migrated])`; a refusal is `state reset: the core refused the snapshot: store `Counter` ... signal `edits`:
...`. Old bindings are still refused at `Hello` (R7). `dev_reload.rs`: `an_additive_schema_change_keeps_the_state`
(a method added: the counter at 5 survives, a client on the new bindings resuming its session finds it, `increment`
gives 6) and `a_schema_change_the_state_cannot_follow_resets_it_and_says_why` (a signal renamed); the `reload.rs`
unit test; SPEC 5.10 and `docs/DEV_LOOP.md`.

### M4 - the React Native contract column failed S14 and S15 (fixed, `db1f334`)

CI runs `npm run test:contract` in `runtimes/rn/@undra/react-native`: `contract-tests/ts`'s scenario files with the
harness swapped for the module's. That harness ignored `boot({ build: "B" })` and loaded build A, so S14 step 8 timed out
("waiting for build B to read build A's queue") and S15 asserted "build B is another schema than the bindings'". It now
has `playgroundModule(build)`, `PLAYGROUND_WASM_B` (what `contract-tests/ts/run.sh` builds) and loads build B with the hash
it reports: 18 passed, 1 skipped (was 2 failed). The column still excludes S20 and S26 (open item).

### Merge interactions (fixed)

* **The Swift column's build B** (`e22b153`): `MigrationBuildB.swift` called `undra_schema_hash()`, gone with ABI v2, so
  the test target did not compile and `run.sh`'s second process ran a stale binary ("the core in .build/core is build
  A"). It reads the hash from the `playground_core` table and loads through `.inproc(api:)`; the S20 method is
  `testS20_`. Swift column 21/21 with "MIGRATION S14/S15 build B ok".
* **Reserved entries** (`a3f1e91`): `main`'s ADR-044 test, every `Undra…` name a runtime declares is a reserved
  namespace entry, failed on the merged tree for `UndraCoreRestarted` and `UndraPanicReport`.
* **The site** (`fafd2c6`): the cross-merge took `main`'s side of the conflicted pages for regeneration, which dropped
  this piece's prose; they are merged three-way (`docs.json` lists both new pages) and regenerated.
* **The runners**: build B beside S26's two cores in all three `run.sh`, the libraries renamed `lib<namespace>`, the
  Kotlin build-B process loading through the bindings' `UndraCoreNative` (`cf6e13a`).

### Lows and coverage added

* L1 - the `Corrupt`-queue path (`queue.rs:398`: a queue the store reports `Corrupt` becomes a dead letter with no
  bytes and counts as read) had no test anywhere; `crates/undra-query/tests/storage.rs` now has one (`a3f1e91`).
* Coverage (`96895a5`): the damaged-snapshot fuzz, the streamed-vs-tree differential on random schema evolutions plus
  a hostile-bytes proptest, and the closure-JSON differential against `serde_json` (all described under the attacks).

## The attack, surface by surface

### 1. The migrating restore (ADR-023 all-or-nothing, one stated exception)

* **Every structural rule, proptest-level.** `persist/tests.rs` `the_two_conversions_agree_on_random_schema_evolutions`
  (1,024 cases per run; 20,000 run once): a random type up to depth 3 over records, enums, `Option`, `Vec`, `Map` and
  the primitives, a random valid value, read by a build with up to three random edits - fields reordered, dropped,
  added with and without `#[undra(default)]` (`Option`, `u32`, `Timestamp`, `Uuid`, a named type), a field renamed, a
  field retyped (`Option`↔non-`Option`, `Vec<T>` → `Vec<Option<T>>`, `Bytes`↔`Vec<u8>`, a narrowing), enum variants
  reindexed, removed, added in front, a variant's fields narrowed - and possibly a changed root type (wrapped,
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
  where there is one), one O(state) snapshot per `snapshotEveryMs`. Measured on the built playground (`vite preview`,
  the browser pane, recovery on, the Stress screen at 10k/s): 9,996 generated and 10,078 change-sets received a second,
  83 drains a second, drain p50 under the 0.1 ms clock step and p99 300 µs, 734 ns per change-set, 0 dropped frames in
  5 s, JS heap 18.7 MB. "Crash the core" while it ran: restarted from a snapshot 894 ms old, 0 calls failed, 0 objects
  stale; the firehose, a core task, is not in a snapshot and reads "stopped" (as designed).
* **The live demo** (Counter): 3, "Crash the core" → "the call failed: restarted", "1 restart: crashed on purpose from
  the debug panel, stores restored from a snapshot 967 ms old", the counter still 3, then 4. (Read through the page's
  text; the pane was not displayed, so a screenshot could not be taken.)
* **The web bench rows** (`npm run bench:recovery`, headless Chromium, 3 runs): `ts/snapshot_take_100kb` p50
  0.039–0.040 ms, p99 ≤ 0.053 ms (budget 2 ms); `ts/recovery_restart_100kb` p50 1.70–1.73 ms, p99 ≤ 5.93 ms (budget
  50 ms), 101,446-byte snapshot.
* **The Rng canary:** `UndraCore.load` rejects `UndraTransportError("unsupported", "WebCrypto is required ...")` before
  instantiating in both wasm modes when `crypto.getRandomValues` is missing; when it throws later the guarded `random`
  import writes nothing and no exception crosses wasm frames, the canary trips and `Rng.fill` answers unavailable. As
  ADR-049 2.5 decides, `Rng` has no error channel, so the proxy's E0062 then panics: a loud trap (which recovery then
  restarts, and a deterministic loop ends dead after the budget) - "nothing traps" holds for the throw itself, not for
  the core's use of `Rng` afterwards.
* **Worker protocol 3:** M2.

### 5. R7 and R12

Layout 2 with the envelope `VERSION` still 1 is Amendment C's "one wire revision for 036+037". A layout-1 snapshot is
refused with code 5 on all three platforms (S15 step 14 in each column, the `snapshot_v2` vector); live peers with
different hashes still refuse each other (S16). Nothing in the persistence code reads a clock, a random source or
starts a thread (`persist*`, `closure*`, `undra-query` storage, `snapshot.rs`): the cache's `updated_at` comes from the
`Clock` port. The TypeScript recovery's rate limits use `Date.now` (host side, not the core; open item, Low).

## Suites (after the merge and the fixes)

Host: Apple M5 Pro, macOS 26.5, shared with other agents' builds. Unless said otherwise, on the tree with both merges
(`586624b` and later); `UNDRA_REQUIRE_TOOLCHAINS=1`.

| Suite | Result |
|---|---|
| `cargo fmt --check`; `clippy --workspace --all-targets -D warnings`; `clippy -p undra-ffi --target wasm32-unknown-unknown -D warnings`; `cargo doc --no-deps -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` | **2,891 passed, 0 failed, 15 ignored** (`dev_reload` 9/9 and `dev_devtools` 3/3 in the parallel run). The first run, before the devtools merge, failed 19 `undra-bindgen` TypeScript-toolchain cases only because `tsc` was not on its `PATH` (green with it) and the reserved-entry test (fixed, `a3f1e91`) |
| `crates/undra-ffi/tests/wasm/run.sh` | raw 22/22, ts-runtime 32/32 |
| Swift `swift test` (runtime) | 553, 0 failures |
| Kotlin `test-local.sh`, kotlinc 2.4.20 and 2.0.21 | 652 cases each, 0 failed, 2 skipped (no fixture library) |
| `./gradlew :android-adapters:test` | 142 + 142 (debug, release), 1 skipped each, 0 failed |
| `:android-adapters:connectedAndroidTest` on the `undra` AVD (`emulator-5554`) | 125 tests: 124 passed, 1 skipped (the network toggle), 0 failed |
| TypeScript runtime `npm test` + typecheck | 1,264 in 36 files; clean |
| React Native `npm test` + typecheck; `test:contract`; `cpp/test/run.sh` | 65; clean; 18 passed, 1 skipped; 74 ok (`UndraPlatformApple.mm` compiles against the iOS SDK) |
| `bash contract-tests/run-all.sh` | **65/65**: S01–S20 and S26 on TypeScript, Kotlin and Swift, S21 and S22 on TypeScript; build B "MIGRATION S14/S15 ok" on both native columns |
| interop `run.sh ts`, `run.sh kotlin` | OK, OK |
| `undra bindgen -C examples/playground --check --docs` | up to date, **0xfa536b9ac6f06149** (unchanged by both merges) |
| `schema_docs --ignored`; `schema_retention --include-ignored` | 1 passed; 1 passed |
| `cargo test --release -p undra-bench --test budgets` | 6 passed, 1 ignored; `snapshot/restore_100kb` 27.1 µs, `restore_100kb_migrated` 75.2 µs p50, ratio **2.77** (max 10), cold start with restore 84.6 µs |
| `sync_alloc`, `commit_alloc` (release) | 2 + 5 passed |
| `npm run bench:recovery` (playground web) | `ts/snapshot_take_100kb` p50 0.039–0.040 ms; `ts/recovery_restart_100kb` p50 1.70–1.73 ms, p99 ≤ 5.93 ms |
| `scripts/wasm-size.sh --record` | hello wasm **116,800** of 120,000; hello JS runtime **25,984** of 26,000; both gates ok |
| playground web `npm test` / `tsc` / `npm run build`; the live demo | 117; clean; built; Counter 3 → crash → restored 3 → 4 (above) |
| `node site/scripts/build-all.mjs`; `check-links.mjs --words` | up to date after regeneration; links OK, landing 342 words |

Not run: `runtimes/ts/devtools` (`main`'s new package, untouched here), the React Native module's `android/test/run.sh`
and a React Native app on a simulator or device (the RN change is in portable C++ that `cpp/test/run.sh` covers under
ASan/UBSan, and in the Apple file it compiles).

## Open items

* **Size headroom.** The hello JavaScript runtime is 16 bytes under its 26,000 budget: the next change to what the hello
  app imports pays for itself, or `ts-runtime-size` (ADR-052) lands first. The record's measured lever (the gate folds the
  lazily imported worker transport into the measured chunk) still stands.
* **React Native `SecureStore`** answers every Keychain/Keystore failure as `StorageError::Io` with the platform's text:
  the platform layer (`UndraPlatformApple.mm`, `UndraPlatformAndroid.cpp`, `SecureSeal.java`) passes a string, not a
  variant, so `Locked` (`errSecInteractionNotAllowed`, `UserNotAuthenticatedException`) and `Corrupt` (an invalidated key,
  a failed tag) are not told apart there yet (`Kv`, which `undra-query` uses, is classified).
* **The React Native contract column** excludes S20 and S26, never reaches the C++ defaults (it runs over a stand-in of
  the native module), and CI does not run `check.sh rn`; the C++ defaults are covered by `cpp/test/run.sh` only.
* **Native S20** (Kotlin, Swift): step 4 only checks the `Locked` read injected at load, a later good read and no write
  between; nothing is queued while unreadable and no replay is checked, and step 3 is skipped (one core per process).
  TypeScript covers the whole of ADR-049 1.4.
* **Web recovery, Lows from the sub-audit, not fixed:** the port-reply epoch is bumped once per recovery, not per restart
  attempt (a reply of an instance that trapped during its restart can reach the next one); `#mayRestart` ignores the
  worker transport's `canRestart` (a worker that does not announce `recovery` spends a budget slot before the core is
  lost); the rate limits use `Date.now` (a clock jump stalls snapshots or empties the window early); `rejectedCalls`
  omits, in `wasm-main`, the call whose send trapped; a re-created query handle's mirror registration drops its
  `noCoalesce` options; `#restarting` ends before `#reattach` has moved the query wrappers (a call in that window gets a
  stale `BadRequest`); a `crashRecovery()` is not released by a `load` that failed; the import guard's `onError` reaches
  the app's Log adapter unguarded; the worker mode's WebCrypto check is not waived when `worker.ports` supplies `Rng`.
* **Worker protocol**: no version handshake beyond features; protocol 3 renamed the snapshot payloads `bytes` → `data`
  under the same `"snapshot"` feature, harmless only because both sides ship in one package and nothing is published.
* **`Rng` after WebCrypto dies** (ADR-049 2.5 by design): `Rng.fill` answers unavailable and the proxy's E0062 traps the
  core; with recovery on, a page whose WebCrypto stays broken restarts until the budget ends it.
* **Quadratic paths on crafted input**: the insertion sorts over a snapshot description's stores, signals and variants,
  and the streamer's `SortedEntries` (binary search + `Vec::insert`), are O(n²) on adversarial order; only a crafted
  snapshot reaches them. A damaged description is refused as 7 (incompatible) rather than 5.
* **Dev reload across a schema change** keeps the state in the core and the session for a client that resumes it on the
  new bindings; a web page that reloads onto new bindings starts a new session, so the kept stores are reachable only if
  the app re-finds them (a reloaded page re-creates its stores). The dev bar says "state kept (the schema changed)".
* **Android**: the invalidated-Keystore-key path is tested through a fake key source only, and the invalidated key stays
  cached (`set` cannot recover until `delete`). **Swift**: `EPERM` maps to `Locked` broadly; `list` skips entries whose
  read fails with `Io`; `delete` can report success through a `fileExists` that cannot search the directory.
* From the record, unchanged: ADR-046's panic-report fields; `remote` has no snapshot/restore; `dev_reload` under heavy
  parallel load (this run: 9/9 in the parallel workspace run, and the two new tests alone).

HEAD: the commit that adds this review, on top of `a1229cf` (`main` `f35c038` is an ancestor).
