# SDE: reload-handles - query handles across every restore (ADR-059), 2026-10-02

Branch `wt/reload-handles`, from `main` `a309e9f`. The binding text is ADR-059 (Accepted, with its "Implementation note"
of ten items: what differs from the text and why); the architect's note is `.10x/decisions/architect/reload-handles.md`.
The integrator's nine decisions are the ADR's Decision section as written.

## What landed, per step of the brief

| Step | Where | What |
|---|---|---|
| 1 wire | `undra-wire` `payload/{snapshot,mod}.rs`, `contract-tests/wire-vectors.json` (+ the Swift copy, `WireVectors.kt`) | `RECREATION_FIELD = 0xFFFF_FFFE`, `StoreSnapshot::recreation()`, the `snapshot_recreation` vector the four codecs read; no layout, envelope or ABI change |
| 2 runtime | `undra-runtime` `object.rs`, `object_table.rs`, `recreation.rs` (new), `runtime.rs`, `config.rs`, `tests/recreation.rs` | `UndraObjectDyn::recreation`, `Reviver`, `Runtime::add_reviver` (the one door; the code is linked by use through a function-pointer table), dormant entries, `clear_keeping`, `held`, `replace_object`, restore phases (check, then keep-live / place / re-issue dormant), `RestoreReport::{reissued, refused, displaced}`, build at first use (`observe(on)` and the miss path of `object` / `param` only), `stats_json` (`dormant_handles`, `revive_failed`); 24 integration tests including the property test (`restore(s); restore(s)` = `restore(s)`) |
| 3 query | `undra-query` `revive.rs` (new), `erased.rs`, `handle.rs`, `paged.rs`, `poll.rs`, `shared.rs`; tests `restore.rs` (~1,000 lines), `hand_built.rs`, `wire.rs` | the record (`format u16 = 1`, params, `poll_ms`), the reviver registered by `shared_of`, `check_params`, the fingerprint (`closure_of_params`; 0 for a hand-written `QueryDef`) |
| 4 transport | `devtools/hub.rs`, `tests/devtools.rs` | the hub neither lists nor observes recreation records; a displaced handle counts with the stores built since a step |
| 5 cli | `reload.rs`, `runner.rs`, `templates/runner/main.rs`, `tests/dev_reload.rs`, `tests/dev_devtools.rs` | `state kept (2 stores, 1 query handle, ..)`, the runner protocol's handle count, re-issued handles the session does not hold are released; the Remote-tab reload test (ticker + `Library` paged before and after) and the schema-change case |
| 6 ffi | `crates/undra-ffi/tests/abi.rs` | `undra_restore` answers 0 for a snapshot with a refused record |
| 7 TypeScript | `runtimes/ts/@undra/runtime` (`object.ts`, `core.ts`, `recovery.ts`), `undra-bindgen/src/ts.rs`, six goldens, the fixture, the generated examples | the host-side replay of ADR-049 is deleted (`recreate`, `_rebindObject`, the loop in `#reattach`); `crashRecovery()` measured 82,006 / 25,838 gz against 83,389 / 26,209 (-1,383 / -371) |
| 8 Swift, Kotlin, RN | none | no source change, as the brief predicted |
| 9 S35 | `contract-tests/scenarios.md`, TypeScript (and the React Native model column), Kotlin, Swift; `check.sh`, `run-all.sh`, README, S16's list | ten steps; the grid is 34 scenarios, 98 cells, all pass (S34 is held by `generics-fn-obj`; `build-trust.mjs` accepts the gap) |
| 10 SPEC | `docs/SPEC.md`: the reserved record id, the snapshot record, the restore rules, the dev reload and devtools paragraphs, section 9, 16.2 (`Runtime` restore API), 17.1 (TypeScript recovery), S35 | |
| 11 docs, site | `docs/DEV_LOOP.md`, `docs/ERRORS.md`, `README.md`, the playground README, `site/data/roadmap.json`, the post and its `claims.md`, regenerated site | |
| 12 older ADRs | ADR-049, 053, 054 | dated amendments pointing to ADR-059 |
| bench | `bench/budgets.toml`, `bench/common/query_rows.rs`, `workloads.rs` | `snapshot/encode_100_handles` 15.6 us (budget 78 us), `snapshot/restore_100_handles` 19.6 us (98 us), `snapshot/restore_100_handles_live` 38.3 us (200 us) |

## What the device proof found, and what it changed

The first proof (iPhone 17 Pro simulator and the `undra` AVD, the playground in dev-remote mode, the Remote tab) showed the
handle machinery working (`state kept (6 stores, 3 query handles, 411 KiB, restored in 3.5 ms)`, no `restore:` WARN, the
tab's list back from the persisted entry, `refetch` accepted, the ticker polling on the rebuilt code) and one thing that was
not about handles: the Remote tab ended in "the remote server is not configured". The server's address is set by
`configureRemote(..)` into a runtime extension, which is not a store, so no snapshot carries it; only the app's start-up
call sets it. That is ADR-049's rule for a web core restarted after a crash, and a rebuilt core is the same case.

Fix (ADR-059 implementation note 10): the three playground apps tell the new core again once the runtime goes from
`reconnecting` back to `connected` (`configureRemote(..)` on the main thread, then `refetch()` on the Remote tab's handle):
iOS `UndraBootstrap.connectionChanged` + `PlaygroundApp`, Android `UndraApp.onConnection` + `RemoteViewModel`, web
`undra.ts` (`onConnectionChange`; typechecked and built, not driven in a browser). `configure_remote`'s Rust doc says it,
the bindings were regenerated (playground, two-cores a/b), `docs/DEV_LOOP.md` lists "state the core holds outside its
stores" next to what carries over, and the post, its claim D10 and the roadmap say "no code for the handle" instead of "no
app code". Nothing in the core changed.

The second proof (the same two devices, `undra dev` on the real `examples/playground`, three reloads each, no relaunch):

* Remote tab settles to Status Success with its items after every reload; pull-to-refresh (iOS) and Refresh (both) make a
  request; adding an item after a reload works (Android `POST -> 201`, iOS `POST` then `GET`); the Ticker keeps counting
  (counter 2 right after the reload, then 8, 16, 35, 50 by itself); a reload with the Ticker tab open also refetches the
  Remote handle.
* Android (`UndraApp`, logcat): `Reconnecting(attempt=1, cause=..the core is reloading)`, `Connected`, `dev server: Reloaded,
  state kept`, then `GET /lists/inbox/todos -> 200` about 1.5 s later; the dev server's lines `Restarted: ws://127.0.0.1:7443
  (schema hash 0x437ce0973ce54a88); state kept (1 query handle, 1 KiB, restored in 155 us)` (2 handles with the Ticker open).
* iOS: `remote transport: the connection ended (.. 1001: the core is reloading)`, `client reconnected: platform=ios mode=dev
  .. (9 object(s) kept)`, `Reloaded, state kept`; exactly one `Http` call about 1 s after each reload; `state kept (6
  stores, 3 query handles, 411 KiB, restored in 3.8 ms)` (4 with the Ticker open).
* The stale-handle WARNs at every iOS reload (`observe: stale handle Handle(index=6, ..)`, `index=7`) are the Workshop's two
  shelves, objects a method returned (stale by design, ADR-053), not query handles.
* Artifacts: `scratchpad/devproof/` (first proof), `scratchpad/devproof2/` (second); removed with the scratchpad at the end.

## Sizes and counts

* `scripts/wasm-size.sh` (this machine, rustc 1.99.0, wasm-opt 133): hello-world wasm 117,749 bytes gzipped against a record
  of 116,690 (+1,059, inside the +1.3 KB the decision allows and the 120,000 gate); up-front JS 22,067 gzipped (record
  22,100). Both recorded with `--record` at the end (see below).
* Rust workspace: 3,598 pass, 21 ignored, 2 fail: `undra-macros --test compile_fail` (`diagnostics_render_as_documented`,
  `leaf_types_without_their_feature_name_it`), which is rustc 1.99.0's order of the "other types implement `Encode`" list
  against goldens recorded on 1.98.1 (CI pins 1.98.1; the crate has no diff from main; the CI piece owns it).
* TypeScript runtime 1,862 (66 files), React Native 110, Swift runtime 870 XCTest, Kotlin runtime 784 + 32 (testkit),
  contract grid 98 / 98 (S21 and S22 are TypeScript only), C ABI under ASan, wasm ABI, `schema_retention` and `schema_docs`,
  the release write-context tests, interop, the budgets test (6 pass), `bindgen --check --docs` on playground, cookbook,
  fieldbook and two-cores a/b, `ios15-sample --check`, clippy (host and wasm32), `cargo doc -D warnings`, the wasm32 builds,
  playground web `typecheck`, `test`, `build`, site `build-all` and `check-links --words` (347 words).
* The Kotlin contract column passes under both compilers (brew's 2.4.20 and CI's 2.0.21).

## Decisions of mine, and the findings

* **Test discipline (the founder's rule for CI).** No test of this piece asserts a time or depends on the machine's speed:
  the dev notice is waited for rather than read after a quiet window, the polls and the ticker advance wait up to 20 to 60 s
  (they return at once when it happens), the deadlock detectors (a snapshot or a restore waiting in `recreation()` while the
  table takes a write) have 60 s, and every `restore` assertion is about values. Absence checks ("a restore fetches
  nothing") are quiet windows and pass vacuously, never fail, on a slow machine.
* **A real bug the reload test found** (`e5baca2`): a restore registered a store's page servers while it was still placing
  handles, so the `Library`'s servers took the slot of the ticker's recreation record, which was then skipped as "slot
  reused". Fixed by deferring lazy registration to after the whole placement (`insert_at_deferring_lazy`, `enter_lazy`);
  the same hazard existed between two stores.
* **A first-use build of a wrongly typed handle** builds before it can say the type is wrong (implementation note 9).
* **The hydration gap** (not mine, left): an observe that reaches a fresh runtime before the client's `Kv` has answered
  starts a fetch though a persisted entry would have been fresh; `adopt_persisted` then shows the stored data while the
  fetch runs (ADR-046). The Remote tab after a dev reload shows its persisted rows at once and refetches; harmless.
* **Not done on purpose:** user-object `recreate` (decision 5, deferred), a cache hand-over (decision 4: a non-persisted
  query shows loading once, documented in DEV_LOOP), persisting runtime extensions (needs a schema description, R1).

## Helpers and cleanup

* `proto/reload-handles` is the architect's prototype branch; nothing refers to it now, it can be deleted.
* No other branch, worktree or clone was made for this piece. Every process started (dev servers, the emulator the first
  proof booted, log captures) is stopped; the scratch files under the session scratchpad that this piece made are removed.
