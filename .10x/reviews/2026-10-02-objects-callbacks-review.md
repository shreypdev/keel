# objects-callbacks (ADR-040 objects as parameters and returns, ADR-041 host callback interfaces) — adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/objects-callbacks` at `61ca552` (`main` `b800994` an ancestor; `main` had not moved when the review ended, so the
final merge is a no-op check) · **Read:** ADR-040 and ADR-041 with their implementation notes, `.10x/decisions/sde/objects-callbacks.md`,
SPEC 1.2/2/3/4/5/10.3a/11/12/16/17, the diff of `undra-wire`, `undra-meta`, `undra-macros`, `undra-runtime`, `undra-ffi`,
`undra-transport`, `undra-bindgen` and its goldens, the three platform runtimes, React Native, S27/S28 and the playground ·
**Fixes:** `76dfedd`, `dfc1d4d`, `32c579f`, `46f2279`, `4e9df54`, `31d4a13`, `8fc5ccf` (below).

## Verdict

**Merge, with the open items below queued as one follow-up piece.** The core of the design holds up under attack: one
crossing is one reference, the `IssueScope` gives back every reference a reply did not carry (sync error, panic,
cancellation, restore and shutdown all checked), object parameters resolve before the body runs and a refused call owns
nothing, callback proxies are made last, the 24/40 handle split is honest (a process-wide counter, never reissued within
one image), and every runtime keeps one wrapper per handle. Nothing escapes as a panic on any path I drove (R6).

What the review found is at the joins between this piece and older ones (restore, shutdown, crash recovery, the identity
map's sweep, Swift's cancellation thread), and in two decisions the brief asked for:

* **D1 (the JS gate): restated at 21,800, ADR-052 note.** The lazy identity map does not work: it would take at most
  ~132 bytes out (the chunk would stay over 21,500) and the first `create()` would pay 2.6 to 3.8 ms on the loopback.
  Also, 163 of the "504 bytes of growth" were `main`'s, not this piece's (below).
* **D2 (the stress-tab smoke): not reproducible, and not the core.** 5/5 full smoke runs (30/30 tests) on the final tree;
  22 more runs of the stress test before the fixes (8 serial, 12 with four workers), and a tab-churn probe in dev
  (StrictMode), production and the embedded mode, all clean; and the tree the
  failure was recorded on (`5689900`) passes too with a freshly built core. The decision record's attribution to "the
  wider handle" is unsupported (below).
* **Fixed (High):** the TypeScript identity sweep made a store un-adoptable (F1); **(Medium)** a restore reset every store's
  host references to one (F2), a call finishing as shutdown began answered status 0 with an empty body (F3), the TypeScript
  callback registry survived a crash restart (F4, the implementer's known limit), Swift ran the app's cancellation handler
  inside the core's port callback (F5), 1,760 SwiftPM build files were committed (F6); **(Low)** `run.sh` could `npm ci`
  through a symlink (F7), stale `u32` floor docs and a wrong saturation log (F8), the TypeScript helpers were undocumented
  (F9, the R3 condition of the brief).

## Findings

| # | Sev | What | Where | State |
|---|---|---|---|---|
| F1 | High | TS: `adopt` unregistered a collected wrapper's mirror registration only while the identity map still had its entry; the map's sweep (at 64, 128, ...) drops entries of collected wrappers whose finalizer has not run, so a reply carrying that handle again threw `handle N is already registered with the mirror` (the method call rejected; the half-made wrapper was left to the finalizer) | `runtimes/ts/@undra/runtime/src/identity.ts:57-59` | fixed `32c579f`; test `identity.test.ts` "re-adopts a store whose collected wrapper's entry was swept" (fails before: the exact error) |
| F2 | Medium | Restore put every snapshot store back with `host_refs = 1` whatever the host held: a store with two references (a second `Arc<Self>` construction, which is Swift's second wrapper; a reply whose extra reference is still on its way back over a worker or socket transport when the restore lands) went stale at the first wrapper's release while the other still used it | `crates/undra-runtime/src/object_table.rs:573` (`insert_at`), `runtime.rs` restore phase 2 | fixed `dfc1d4d`: the count is carried across (`insert_at_with_refs`); test `object_values.rs` `a_restore_keeps_every_reference_the_host_holds_to_a_store` (fails before: `Some(1)` vs `Some(2)`) |
| F3 | Medium | An async method returning objects whose last poll ran as shutdown began: its lowering's `WeakCtx` no longer upgrades and returns `Ok(vec![])`, `finish_call` still found the call (shutdown waits for the core lock) and answered **status 0 with an empty body**, which no host decodes as a handle (typed `malformed`, not a crash, but wrong) | `crates/undra-macros/src/impl_/object.rs:979-987`, `crates/undra-runtime/src/runtime.rs` `finish_call` | fixed `dfc1d4d`: a call finishing after shutdown began is answered cancelled; deterministic test `an_async_object_return_that_finishes_as_the_runtime_shuts_down_is_answered_cancelled` (fails before: `Ok` vs `Cancelled`) |
| F4 | Medium | TS: after a crash-recovery restart the callback registry kept every entry (strongly holding the app's listeners) and invocations the trapped instance had queued were still delivered | `runtimes/ts/@undra/runtime/src/callbacks.ts` (`#bridge`, `#invoke`) | fixed `32c579f` (well under the hour the brief allowed): the bridge's `PortImpl.dispose`, which `#recover` already calls, drops the registry and aborts what runs; a queued invocation whose instance is gone is not delivered; the over-release report after a drop covers only the dropped instances. Test in `recovery.test.ts` (fails before: `liveCount` 1) |
| F5 | Medium | Swift: `__cancel` called `Task.cancel()` inside the core's port callback; `Task.cancel()` runs `withTaskCancellationHandler`'s `onCancel` (app code) on the cancelling thread, which may hold the core lock (SPEC 6 host contract 2 and 4). Kotlin already cancels from another thread | `runtimes/swift/UndraRuntime/Sources/UndraRuntime/Core/Callbacks.swift` `cancel(_:)` | fixed `46f2279`: cancelled from a global queue; test `testACancelNeverRunsTheImplementationsCancellationHandlerInsideTheCoresCallback` (fails before) |
| F6 | Medium | `229a200` committed 1,760 files of `examples/playground/generated/swift/.build` (SwiftPM output: module caches, dependency files with one machine's absolute paths) | `.gitignore` | fixed `76dfedd`: untracked, `.build/` ignored everywhere |
| F7 | Low | `contract-tests/ts/run.sh` ran `npm ci` through a linked `node_modules` (the implementer's hazard: it emptied the main checkout's) | `contract-tests/ts/run.sh:80` | fixed `76dfedd`: refuses with what to do; header says S01..S28 |
| F8 | Low | Docs still said `generation_floor u32` (`undra-ffi/src/api.rs`, `runtime-internals.md`, a test comment); the `u32::MAX` saturation log said "the host leaks" (the opposite: references past it are not counted, so the object can go early) | as named | fixed `dfc1d4d` |
| F9 | Low (R3) | The TypeScript free helpers (`adopt`, `adoptObject`, `adoptOptional`, `adoptList`, `requireOwn`, `lend`, `lending`, `giveBack`, `callbacks`) were chosen for size but documented nowhere a reader looks | `site/docs/api-typescript.html` | fixed `32c579f`: a paragraph says what they are, why they are free functions, and that apps do not call them |

### Open items (not fixed here; one follow-up piece)

| # | Sev | What | Where |
|---|---|---|---|
| O1 | Medium | **Streams that take objects or callbacks.** Swift generates no `requireOwn` and no give-back for a stream method (an object of another core is sent; a refused stream's lent listener stays in the registry, held strongly); TypeScript's streams lend outside `lending` (same leak on refusal); Kotlin's `flow {}` wraps `emitAll` in the `catch` that gives references back, so an `UndraException` thrown by the *collector* (or a decode failure downstream) gives back a reference the core still holds, and `coreTookArguments` treats a `WireException` from decoding a successful reply as "not taken". **No golden, typecheck or run test covers a stream with object or callback parameters on any platform.** | `crates/undra-bindgen/src/swift.rs:1444`, `ts.rs` stream branch, `kotlin.rs:1768-1790`, Kotlin `UndraCallbacks.kt:131-141` |
| O2 | Medium | **Swift store wrappers.** (a) An `Arc<Self>` `new` constructor of a *store* gives a second wrapper (documented) that also replaces the first one's mirror registration (Swift's mirror is last-wins), so the first stops receiving changes; closing either unregisters the handle for both. (b) A `close()`/`deinit` racing an `adopt` of the same handle on another thread unregisters the new wrapper's routing and clears its observed set (Kotlin guards this with `identity.unlessLive`; Swift's `forget` is identity-guarded but the unregister and `release` after it are not). The core's count stays right in both; the wrapper freezes. | `runtimes/swift/.../Core/UndraObject.swift:52-55`, `Mirror.swift:181` |
| O3 | Medium | **Restore and object parameters.** `cancel_calls_replaced_by_restore` looks at a call's receiver only: an async free function or constructor that took a `&Store` keeps running on the pre-restore store and answers status 0 (ADR-023's M3 hazard, newly reachable through ADR-040's parameter positions). | `crates/undra-runtime/src/runtime.rs:1871` |
| O4 | Medium | **TS crash recovery and give-backs.** A `Release` sent while the core restarts is replayed as `core.release(handle)` (`recovery.ts:756`): when it was a give-back (a superseded wrapper's finalizer), the replay unregisters the live newer wrapper and releases the restored entry's only reference; the same finalizer running after the restart does it through `_giveBack`. Needs a GC-timing and trap coincidence; fix with a restart epoch in the `Leak` record and a replay that skips handles a live wrapper holds. | `runtimes/ts/@undra/runtime/src/recovery.ts:592,756`, `identity.ts` `collected` |
| O5 | Low-Medium (dev only) | **`undra dev` sessions.** The transport records a constructed handle in both `constructed` and the session's origin, so an `Arc<Self>` singleton constructed by two sessions loses both references when one disconnects; one `Release` wipes all of a handle's tracking, so a store constructed and also returned leaks a reference per session; callback proxies are interned by `(port, instance)` without the origin while every client numbers instances from 1; the TypeScript registry drops on `reconnecting` although a resumed session keeps its proxies; Swift keeps its registry on a lost connection while Kotlin and TypeScript clear it. | `crates/undra-transport/src/session.rs:376,532,539`, `tracker.rs:123`, `resume.rs:275`, `crates/undra-runtime/src/callbacks.rs:292-312` |
| O6 | Low | A constructor that took callbacks and then fails *after* its body (`issue_constructed` refused: table or generations exhausted; a store's `__undra_attach_all` failed) answers status 5 although the proxies were made and are released with the value: the host gives its references back too (a double release; an interned instance the core still holds elsewhere is freed on the host). Should not be status 5 once proxies exist. | `crates/undra-macros/src/impl_/object.rs:1031-1060` |
| O7 | Low | An abort that races a successful reply carrying an object drops the reference until the core closes, on every transport but `wasm-main` (Kotlin's documented limit applies to TypeScript and React Native too). | `runtimes/ts/@undra/runtime/src/core.ts` (`_send` onAbort) |
| O8 | Low (R3) | Kotlin's companion `operator fun invoke` is generated only for a `new` with no parameters that is synchronous and cannot fail, so `Account()` works and `Uploader(listener)` does not (`Uploader.create(..)`); generate it for every `new` or for none. Swift's weak wrapper's `targetGone()` never resumes, so a direct call on a dead weak wrapper (not the core's path) hangs. | `crates/undra-bindgen/src/kotlin.rs:1502-1507` |
| O9 | Info | **The wasm budget.** `web/hello-wasm` is **119,055 of 120,000** (record re-taken in `4e9df54`; this piece added about 2,350 bytes, the review's fixes 126 of them). The landing page shows 99% of budget. The next core feature will not fit without a cut or a restatement. | `bench/results/web-size.jsonl` |
| O10 | Info | The playground's tab bar overflows horizontally at phone width now that it has eight tabs (304 px pane: the page scrolls to 491 px). | `examples/playground/web/src/App.tsx` |

## The attack, surface by surface

**1. Handle layout and the generation floor.** `Handle` masks to 24 + 40 bits; the counter is one process-wide static
per linked image, so two cores of one image never issue the same pair, and a restore only re-places a snapshot's own
pairs (`raise_to`). At `2^40 - 1` the counter logs FATAL and panics before any lock is taken; the guard contains it and the
scope unwinds (read, not driven). Two ADR-044 images each have their own static and *do* collide (surface 4). **A snapshot
main wrote** (layout 2 with a `u32` floor and the same tag): the branch's reader shifts by four bytes and fails with
`RestoreError::Decode`; if a corrupt one decoded, the floor would read at or past the ceiling (`GenerationFloor`). Typed,
and nothing is released, but refused by accident of the layout rather than by a tag: pinned by
`a_snapshot_with_the_old_u32_floor_is_refused_typed_and_releases_nothing` (`crates/undra-runtime/tests/issue.rs`). The
dev-reload ring and a dev machine's persisted snapshots therefore start fresh after the upgrade (no misread). The RN copy
of the header is byte-identical to Swift's (`abi.rs` `the_swift_and_react_native_copies_of_undra_h_are_identical`, in the
workspace run). The issue/release/restore property test (`host_refs_always_equal_what_the_model_host_owns`) holds; it models
plain objects only, which is why F2 (stores with several references) was not caught: the new test covers it.

**2. Reference counting across the boundary.** Finalizers: TS `collected` releases when no wrapper holds the handle and
gives back when a newer one does (once each; the existing gc tests and F1's); Kotlin's cleaner runs once and clears the
weak reference first (fine); Swift's `closedFlag` makes `deinit` release once (fine), but see O2(b). `adopt` twice returns
the same wrapper on all three (the runtimes' identity tests). Un-adopted replies: the sync path cannot drop one (the scope
commits only when the reply is built); an awaiting caller that aborts is O7. A `Vec<Arc<T>>` with one stale, released or
foreign handle: refused `BadRequest` naming the parameter before the body runs, the earlier `Arc`s are locals and drop,
callback proxies are made after every argument resolved (macro tests `a_stale_or_wrongly_typed_object_parameter_is_a_bad_request_naming_it`);
the host refuses a foreign one first (`requireOwn`, every element of an `Option` or list, in methods, constructors, free
functions; commands report it to `onError` with the operation name: Swift and Kotlin read, TS tested), except streams (O1).
Restore with references outstanding: F2 (fixed); plain wrappers go stale and their later release fails the generation check
(no double decrement: verified by the property test and the new test).

**3. Callbacks.** Typed `E` is status 1, any other throw is `onError` plus status 2, a fire-and-forget throw is only
reported: on Swift, Kotlin and TypeScript (runtime tests) and in RN, where the JS drain is wrapped (`UndraJsi.cpp:330`) and
callback ports are never answered synchronously (`native.ts:197`), so nothing unwinds into the core. Re-entrancy: a
`main`-delivered callback runs at the drain, outside the port callback, and may call the core (S28 step 1 does: `note`
calls `announce`); a `background` one runs on its own executor and may too (not tested on any platform: recorded, low).
Cancellation reaches a running Swift `Task` / Kotlin `Job` / TS `AbortSignal` at once, and drops a queued one (runtime
tests); Swift's ran app code on the core's thread (F5, fixed). `coalesce` keeps the newest per (instance, method) per drain
round on all three. A callback the host released while the core holds its proxy answers unavailable (typed, the bridge's
"no such instance" path). Shutdown with a background callback mid-run: proxies hold a `WeakCtx`, an async call is
`Cancelled`, a notify is a no-op, nothing panics. The crash-restart limit is fixed (F4).

**4. Cross-core foreign handles.** Every runtime refuses an object of another core before sending (`requireOwn`: TS
`identity.test.ts`, Kotlin `ObjectIdentityTests`, Swift identity tests, RN over its fake native; the two-core apps run
S26). **The core does not refuse a handle whose generation it never issued**: `check` compares only the slot's generation,
and two images (ADR-044) or a restore of another core's snapshot can produce a valid-looking pair. The host check is the
only line for a well-formed foreign handle; the core cannot do better without a core id in the handle (a wire change), so
this is accepted and stated here rather than fixed. Swift streams skip even the host check (O1).

**5. R3 and R8.** By eye, the `object_graph` and `callbacks` goldens read native on all three: Swift `UndraObject`
`Hashable` by identity and `@MainActor` protocols for main delivery (UIKit-delegate style), `Sendable` refinement needed for
`background`; Kotlin interfaces with a `weak` companion and an `internal` wrapper constructor (O8 for `invoke`); TypeScript
interfaces with an `AbortSignal` last parameter and `weakX(target)` functions. The free TS helpers are acceptable now that
they are documented (F9). E0064, E0004 and E0071 each have code, what, why, fix and a docs link (`tests/ui/*.stderr`), and
the errors page carries them.

## D1: the JavaScript gate

Measured with the gate's own build (`scripts/web-size-runtime.mjs`, zlib 9): `main` at `b800994` is **21,336** (ns-storage's 163
bytes over the record 21,173 were never re-recorded; its review says so), the piece is **21,677** (+341), and after the
review's TS fixes **21,672**. The identity map's logic is about **132** of the 341 (the chunk with `adopt` reduced to
`new type(core, handle)` and `collected` to `release` measures 21,533). Lazy-loading it therefore cannot bring the chunk under
21,500. Its cost, measured in headless Chromium against `vite preview` on the loopback: the first dynamic import of a 5.5 to
6.8 KB chunk the page had not loaded took **2.6, 2.9, 3.7 and 3.8 ms**, an already loaded one 0.1 ms, a repeat 0.000 ms. Over
the 1 ms threshold, so the budget is restated: `budget_gzip_bytes = 21800`, with ADR-052's note of 2026-10-02 saying exactly
what the bytes are and why the alternative was rejected; README, SPEC and the CI comment say 21.8 KB; `--record` re-took both
rows (wasm 119,055, JS 21,672). `scripts/wasm-size.sh` passes both gates.

## D2: the stress tab

* **Reproduction attempts.** On the final tree: the full smoke 5/5 (30/30); the stress test 8 times serially and 12 times with
  four workers; a tab-churn probe (start, leave to Counter and Workshop, return, restart, six cycles) in the production build
  and under `vite` dev with React StrictMode, and the embedded `?embed=1` mode: no console error, no "stale handle".
* **Bisect.** The tree where the failure was written down (`5689900`, before `main`'s ts-size-e4 changed the call path) built
  with a fresh core in a separate target directory: the stress test passes 3/3. So it is neither the handle layout nor the
  `IssueScope`.
* **Reading.** The only `Stress.stop` reports come from the Stop button and the panel's unmount cleanup, which is guarded by
  `stress.closed`; on unmount React runs a deleted subtree's passive cleanups parent first, so `useUndra` closes the store
  before the panel's cleanup reads `closed`. Calls are sent synchronously (`core.call` -> `_callDirect`/`_request`), so even
  child-first order would send `stop` before the release.
* **Most likely cause:** a stale `examples/playground/build/web/playground_core.wasm` or `dist/` at the time (neither is
  rebuilt by `npx playwright test`; `npm run smoke` rebuilds only `dist`). No code change; the decision record's attribution
  ("comes from the core's changes (the wider handle)") is not supported and should not be carried into the status file.

## Suite counts (final tree, macOS, Xcode 26, JDK 17)

| Suite | Result |
|---|---|
| `cargo fmt --check`; `cargo clippy --workspace --all-targets -D warnings`; `cargo clippy -p undra-ffi --target wasm32-unknown-unknown -D warnings`; `cargo doc --workspace --no-deps` with `-D warnings` | clean (the first clippy run caught two lints in the review's own tests, fixed in `8fc5ccf` and re-run clean) |
| `cargo test --workspace` | **3,133 pass**, 0 failed, 16 ignored (3,130 + the review's three); `schema_docs`, `schema_retention` and `the_swift_and_react_native_copies_of_undra_h_are_identical` among them |
| Miri | `undra-ffi --lib` 36 pass; `--test abi` (host-allocated replies) 2 pass; `undra-runtime --lib object_table` 17 pass (with `-Zmiri-disable-isolation`: the generation-ceiling test unwinds a panic, which reads the working directory) |
| Budgets (`cargo test -p undra-bench --test budgets --release`), `sync_alloc` / `commit_alloc` / `derived_alloc` (release) | pass (6 + 1 ignored; 5 + 2 + 1) |
| C harness; Swift over the C table; JNI end to end; wasm ABI | `c smoke`, `c lifetime`, `c two cores` ok; 6 pass; 16 pass; 22 + 32 pass |
| Swift runtime (`swift test`) | **705 pass** (704 + F5's test) |
| Kotlin runtime (`test-local.sh`), Kotlin 2.4.20 and CI's 2.0.21 | **782 cases in 45 suites, 0 failed, 2 skipped** and the testing kit's **30**, under both compilers |
| TypeScript runtime: `tsc --noEmit`, `npm test` | clean; **1,613 pass** in 56 files (1,611 + F1's and F4's tests) |
| React Native: `npm test`, typecheck, `test:contract`, `cpp/test/run.sh`, `scripts/rn-device-checks.sh` | **92 pass**; clean; 20 pass + S17 skipped (S27, S28 pass); 15 + 33 + 33 + 30 host checks; iPhone 17 Pro simulator **21/21 and 24/24**, emulator-5554 **21/21 and 25/25** |
| Contract grid, `bash contract-tests/run-all.sh` (Swift column from a clean `.build`) | **80/80** (S01 to S28; S21 and S22 TypeScript only) |
| Interop (`crates/undra-transport/interop/run.sh`) | ok |
| `undra bindgen --check --docs` | up to date: playground, two-cores a and b, cookbook, fieldbook; `--check` ios15-sample |
| `scripts/wasm-size.sh` (after `--record`) | `web/hello-wasm` **119,055** of 120,000 ok; `web/hello-runtime-js` **21,672** of **21,800** ok |
| Playground web: `npm test`, `npm run build`, Playwright smoke | 124 pass; built; **smoke 5/5 (6 of 6 tests each)** |
| Workshop tab in the browser pane | stocked the left shelf twice, merged it onto the right (left 0, right 2), ran a job: three ordered `note`s, the `confirm` question answered Yes, "done: 3 steps"; no console error |
| Playground Android `./gradlew assembleDebug`; iOS `undra build --platform ios` then `xcodebuild` (iPhone 17 Pro simulator, Debug) | BUILD SUCCESSFUL; **BUILD SUCCEEDED** (the decision record's "not run" is now run) |
| Site: `node site/scripts/build-all.mjs`, `check-links.mjs --words` | clean (landing prose 342 of 350 words); the regenerated files committed in `31d4a13` |

## Not verified

* **Kotlin and Swift streams with object or callback parameters** (O1): nothing compiles or runs that path, so the
  findings there are from reading the emitters and runtimes; no test was written for them in this review.
* **O2(b)** (Swift close racing adopt) is a thread interleaving I read but did not reproduce deterministically.
* **O5** (`undra dev` sessions): read, not driven end to end (there is still no WebSocket end-to-end test of callbacks).
* **2^40 generations** and `u32::MAX` host references: read and covered by the existing unit tests, not exhausted for real.
* The **Android APK's native core** was the one `build/android` already held when `assembleDebug` ran (Gradle was up to
  date in a second); the Kotlin code was compiled against this tree, the native fixes ran on the JVM, JNI, contract and
  device paths instead.
* D2's original failure: not reproduced, so its true cause is inferred (a stale artefact), not proven.
