# objects-followups (the open items O1 to O8 of the objects-callbacks review) - adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/objects-followups` from `b898a97`, `main` `94b87ba` (types-paging) merged in (`640d83d`) · **Read:**
`.10x/decisions/sde/objects-followups.md`, the objects-callbacks review (O1 to O10), ADR-023/040/041/052, SPEC 3.7, 5.9, 7, 10.3a,
11, 16, 17, the diff of `undra-runtime`, `undra-macros`, `undra-transport`, `undra-cli`, the three runtimes and the generators ·
**Fixes:** `f2e5077`, `4397428`, `7e39e46`, `940d717`, `59dc5cb`, `906d167`, `9a16150`, `1b19d11`, `6246e15`, `318e26a` (below).

## Verdict

**Merge.** The eight items hold up under attack: each of O1 to O8 does what its record says (the tests I reverted, O3's and O5's
origin among them, fail without their fix). What the review found is at the joins, one of them a deadlock:

* **Fixed (High): a deadlock in Swift's identity map (F1).** `ObjectIdentityMap.leave` (and, before this piece, `forget`)
  loaded a weak reference under its lock; the load makes a strong temporary, and when another thread released the wrapper's
  last reference meanwhile that temporary was the last one: the wrapper deallocated inside the lock, its `deinit` closed it,
  and closing takes the lock again. The brief's eight-thread stress hung the full Swift suite (2 runs in 3 at 2,000 rounds
  against the unfixed map; 0 in 35 after the fix).
* **Fixed (Medium): the size gate moved with the checkout's path (F2).** `undra build` now remaps the project's workspace, the
  Undra checkout and Cargo's sources to fixed labels. The gate's number no longer moves by hundreds of bytes; it still moves by
  tens (below), because Cargo hashes an absolute path into every crate's identity.
* **Fixed (Medium): StrictMode handed a React component a closed object (F3)** whenever the constructor is an `Arc<Self>`
  singleton (`useUndra(Hub)`).
* **Fixed (Low-Medium):** a call holding more than four objects was left to finish after a restore (F4); a call that reused
  the id of a refused one inherited its objects (F5); a constructor that failed after taking callbacks reached the host as
  "the core panicked: ..." with a reason that did not say what happened (F6).
* **Decided (O8, Swift's weak wrapper): `fatalError` stays, with the evidence (F7).** The brief's premise, that a typed error is
  always possible through `UndraCallError.unavailable`, does not hold under `throws(E)`.

* **Fixed (merge arithmetic): the JavaScript up front was 22,105 of 22,100 after merging `types-paging`** (22,068 plus this
  piece's +39); trimmed to **22,100** (F10). No headroom is left.

Nothing escapes as a panic on any path I drove (R6). **Sizes on the merged tree:** wasm **116,864** gzipped of 120,000, JavaScript
up front **22,100** of 22,100.

## Findings

| # | Sev | Finding | Where | Fix |
|---|---|---|---|---|
| F1 | High | Swift identity map deadlocks when a wrapper is released on one thread while another adopts or closes the same handle: a weak load under the map's lock holds a strong temporary, and releasing it deallocates the wrapper inside the lock; `deinit` closes it, which takes the lock again. Present before the piece in `forget`; the piece's `leave` made the window larger (it runs the cleanup inside the lock). | `runtimes/swift/UndraRuntime/Sources/UndraRuntime/Core/ObjectIdentity.swift:24` (`Slot`), `:82` (`leave`) | A slot names its wrapper by `ObjectIdentifier`; no weak load under the lock. `testEightThreadsAdoptingAndClosingOneHandleLoseNoReference` (8 threads x 2,000 rounds, deadlock is a failure) hangs on the old map, passes on the new. |
| F2 | Med | The hello wasm measured 120,031 at `.work/objects-followups` and 119,856 at a sibling path (ADR-052's remap covered `$HOME` only; the rest of the checkout's path stayed in every panic location). | `crates/undra-cli/src/cargo.rs:662` (`path_remap`), `:786` (`RemapRoots`), `session.rs:80` | See "The size gate". |
| F3 | Med | `useUndra` of an interned object (an `Arc<Self>` constructor) under StrictMode's effect, cleanup, effect: both creations resolve to one wrapper in one turn; the first effect's cleanup closes it under the second. The same for two components that ask for the same singleton. | `runtimes/ts/@undra/runtime/src/lifetime.ts:29,41` (`openUndra`) | Lives are counted per object; the last one closes it; an object that arrives after its life ended is closed one turn later unless another life took it. `identity.test.ts` and `lifetime.test.ts` (both fail before). |
| F4 | Low-Med | A call that holds a fifth object (or a `Vec<Arc<T>>` of more than four) was not checked by a restore: it finished on a replaced store. | `crates/undra-runtime/src/runtime.rs:393-420` (`Held`, `note_param`), `:2189` | The fifth makes the last slot read `MANY_PARAMS`; any restore cancels such a call (cancelled is a status the host handles). `e2e_todo.rs`: the fifth-object test fails before; a call with exactly four is still precise. |
| F5 | Low | `Runtime::param` noted the first object of `count_pair(a, stale)` for call 7, then the call was refused; a later call 7 (no objects) took them from the slot and a restore cancelled it. Reachable by `undra dev` clients, which number calls from 1 and reach `call_from` unmapped. | `runtime.rs:1592` | The slot is cleared at the start of `call_from`. `e2e_todo.rs` fails before. |
| F6 | Low | Status 2 for a constructor that failed after taking callbacks is right by ADR-032 (it owns nothing the host may give back twice) but the host words it "the core panicked: <reason>" and the reason said `store X could not attach its signals`. | `crates/undra-macros/src/impl_/object.rs:1041` | The reason now says "the constructor failed after it took its callbacks (not a panic; the core released them and the host must not)". |
| F7 | Info | O8, Swift's weak wrapper. `throws(AuthError)` cannot throw anything else: `thrown expression type 'CallError' cannot be converted to error type 'AuthError'` (swiftc 6). So "throw `UndraCallError.unavailable`" is possible only with `swift_typed_throws = false`, where it already is. | `runtimes/swift/.../Core/Callbacks.swift:624` | Kept, with ADR-041's note: it is app code calling its own wrapper after the target died (the core resolves the target first and answers unavailable), not an R6 boundary entry. |
| F8 | Info | The slot of `Runtime::param` is per thread, not per runtime: a stream *function* (user code runs at dispatch) that calls another core on the same thread replaces it, and the outer stream goes unchecked by a restore. | `runtime.rs:405-412` | Accepted and written down in the code and ADR-040: a missed cancel, never a wrong one; same-runtime nesting is impossible (`Reentrant`), which a test pins. |
| F9 | Info | `crates/undra-signals/tests/lazy.rs` arrived from `main` unformatted (`cargo fmt --check` fails on `main`). | `906d167` | Formatted. |
| F10 | Med (gate) | After the merge the JavaScript up front measured 22,105, 5 over the 22,100 gate (`types-paging` 22,068 + this piece's +39: the `orphan` parameter through `call`/`_request`/`_send`, the restart counter, the leak record's epoch). | `runtimes/ts/@undra/runtime/src/core.ts:1303-1322`, `object.ts` | The abort path settles the promise before it sends the cancel, which makes the no-op `reject` on the abandoned entry unnecessary (a settled promise ignores a second rejection; same order for the caller, `core.test.ts` still pins the wasm-main case); `_restarts` is `_era`, the leak field `era`. 22,100: the gate, with nothing to spare. |

## Attacks and results

**1. Call table, restore, ledger.**
* *The per-thread slot under nesting.* Between `Runtime::param` and `take_params` runs only the dispatcher (and, for a stream
  *function*, its body). `call_from` holds the core lock across `dispatch` and `spawn_call`/`open_stream`, and `enter_core`
  refuses a nested call into the same runtime, so a nested dispatch cannot overwrite the outer's parameters:
  `a_call_made_while_another_is_being_dispatched_on_the_same_runtime_is_refused` drives it from inside a stream function
  (status 5, the outer stream still cancelled by the restore). The cross-runtime case is F8, the stale slot F5.
* *More than four objects:* F4. I preferred the conservative cancel to a refusal: a refusal would make a legitimate
  `Vec<Arc<Store>>` call impossible, and the macro cannot count a `Vec`.
* *Per-origin proxies under reconnects.* `set_client_origin` is set after the slot is claimed and cleared (compare-and-swap) by
  the leaving session before the slot is vacated, so a late teardown cannot clear a newer client's origin and there is no
  window in which the old origin addresses the new socket. Two clients cannot come back out of order: a client that is not
  resuming the kept session ends it (`a_resumed_session_keeps_its_returned_objects_...`). New test
  `a_resumed_session_hears_its_own_listener_again_after_the_slot_stood_empty` (the proxy is silent while the slot is empty, speaks
  once to the resumed session, one release back); removing `set_client_origin` fails it and the O5 test.
* *Status 2 for `Failed`:* F6.
* *Abort racing a reply (O7), the transports.* The entry is abandoned before the Cancel is sent. Covered: a transport that
  answers the cancel inside the send (`wasm-main`; `core.test.ts`, with and without an orphan, and the contract grid S06/S27/S28/S30
  on the real module), a transport that answers later (`core.test.ts` fake, and now the real `RemoteTransport` against the fake
  socket server, `remote.test.ts`: Cancel sent, the late success gives its object back, `pendingCalls` back to 0), and React
  Native over the stand-in native module (`objects-callbacks.test.ts`). `WasmWorkerTransport` shares `UndraCore`'s path and the
  deferred-answer fake; there is no test of its own for an abort with an orphan.

**2. Swift and TypeScript wrappers.** F1 for Swift (the existing deterministic hook test plus the new stress, 35/35 after the
fix); TypeScript's identity map is single-threaded, so the question there is StrictMode: F3. `_restarts`: the epoch is taken
at a wrapper's birth and compared in the finalizer; `_restarts++` runs after the replay of held-back releases and before
re-observation, so a finalizer between the restore and the increment could only see a newer wrapper if one could be adopted
while the core refuses calls, which it cannot. A rebound query handle registers its leak with the old epoch and so leaks
instead of giving back after a restart (the lesser harm, as in O4).

**3. O8.** F7.

## The size gate

The remap, widest first (rustc applies the last prefix that matches): `$HOME` to `~`, a `CARGO_HOME` outside it to `/cargo`,
Cargo's registry sources and git checkouts to `/undra/deps`, the Undra checkout the core depends on to `/undra/src`, the
project's Cargo workspace (the project itself when the core is a workspace of its own) to `/undra/app`; for every release
profile (wasm, iOS, Android, host). The *workspace*, not the project directory, because Cargo fingerprints rustflags: a flag
per project would rebuild every dependency whenever two cores of one workspace are built into one target directory.
`scripts/wasm-size.sh` fails when the module names `$HOME`, the checkout or the hello project. Unit tests in `cargo.rs`; the
integration test `the_web_module_does_not_depend_on_where_the_project_lives` builds two copies of the template at paths of
different length and compares them (fails without the project and Undra roots: it names the project's path).

**The test cannot ask for byte identity, and the gate is not exactly path-independent.** The two modules differ in 4 bytes of
278,794. A `TypeId` is a hash of its crate's identity, and Cargo derives that from the absolute path of a path dependency
outside the shim's workspace (the core, the Undra checkout): the ten or so `TypeId` constants differ in value and, as LEB128, in
width. With the Undra checkout at another path every crate's identity changes, and the optimiser's choices with it. The test
therefore compares every string of 16 printable bytes or more (equal) and bounds the length difference (16 bytes). Measured on
`f2e5077`'s tree, same source, four checkout paths:

| checkout path | wasm gzipped |
|---|---|
| `.work/objects-followups` | 119,927 |
| `.work/s1` (short) | 119,931 |
| `.work/a-rather-long-...` (60 characters) | 119,818 |
| `main` + the fix at `.work/m1` | 119,842 |
| this piece at `.work/m1` (same path as the row above) | 119,931 |

Before the fix the same source moved 120,031 / 119,856 between two paths (monotonic in path length); now it moves by up to 113 in
no order. The piece's own cost is **+89 bytes** at one path (119,842 to 119,931). Reproducible exactly only at one path, so the
record is taken where the integrator merges, as before. After the merge with `main` (116,628 recorded on `main`): wasm **116,864** gzipped at `.work/objects-followups` (gate 120,000; `main` recorded 116,628 at its own path without the remap, so the two are not comparable to better than the 113 bytes above). JavaScript 22,100 of 22,100 (F10).

## Counts

Run on the final tree (`UNDRA_REQUIRE_TOOLCHAINS=1`; an iOS simulator and the `undra` AVD booted for the symbol tests):

| Suite | Result |
|---|---|
| `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo clippy -p undra-ffi --target wasm32-unknown-unknown -- -D warnings`, `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps` | clean (the first `fmt` run failed on `main`'s `lazy.rs`, F9) |
| `cargo test --workspace --no-fail-fast` | **3,536 pass**, 0 fail, 21 ignored; `symbols` 5/5 and `debugging` 3/3 again with `--test-threads=1`; `schema_docs -- --ignored` 1/1 |
| bench (release): `budgets` 6 (1 ignored), `stress` 12, unit 69 | pass |
| wasm harness; C; Swift over the C ABI; JNI; transport interop | 22 + 36; `c smoke`, `c lifetime`, `c two cores` ok; 6; 16; OK |
| Swift runtime (`swift test`) | **870 pass** (the first full run hung on F1) |
| Kotlin runtime, Kotlin 2.4.20 and CI's 2.0.21 | 881 cases in 48 suites, 0 failed, 2 skipped, on both; the testkit 32 in 5 suites on both |
| TypeScript runtime | **1,856 pass** (66 files), `typecheck` clean |
| React Native | **110 pass** (10 files), `typecheck` clean, `cpp/test/run.sh` 30 checks |
| Contract grid (`rm -rf contract-tests/swift/.build`, `run-all.sh`) | **95/95** (TypeScript 33, Kotlin 31, Swift 31). The first run had Swift S33 "polling: the gap between the first two ticks was 0.0 s" (a recorder that started after the first fetch had completed: `TimedRecorder.lastGap` reads 0 with fewer than two recorded changes), which passed on the rerun; see open items |
| `undra bindgen --check --docs`: playground, two-cores a and b, cookbook, fieldbook; `--check`: ios15-sample | up to date (the playground's hash is `0xcaec1b9d8ea1f199`, `docs/TESTING.md` follows) |
| `scripts/wasm-size.sh` | wasm 116,864 of 120,000; JS 22,100 of 22,100 |
| site `build-all.mjs`, `check-links.mjs --words` | clean, up to date; landing prose 342 of 350 words |

Tests added by the review: `e2e_todo.rs` x3 (fifth object, reused call id, nested call refused), `cargo.rs` x3 (the remap, replacing
one), `build_web.rs` x1 (two copies at two paths), `objects.rs` (transport) x1, `remote.test.ts` x1, `identity.test.ts` x1,
`lifetime.test.ts` x2, Swift x2 (the eight-thread stress, the store variant). Each of those that tests a fix fails without it
(the nested-call test and the store stress are invariants and pass either way).

## Open items

1. **The size gate is exact only per checkout path** (tens of bytes, from crate identities). With `main` at 116,6xx the wasm has
   3,100 bytes of headroom, so it no longer matters; if the budget is ever tight again, a `RUSTC_WRAPPER` could pin `-C metadata`.
2. **The JavaScript up front has no headroom** (22,100 of 22,100): the next change to the chunk makes room or restates the budget
   in ADR-052.
3. `.lldbinit` does not map `/undra/app` and `/undra/src` back to the source tree (it did not map `~` either): a debugger shows
   the labels. Cheap: `settings set target.source-map`.
4. The worker transport has no real-transport test for an abort with an orphan (attack 1); it shares `UndraCore`'s path.
5. F8, the cross-core stream-function nesting, is a documented missed cancel; Kotlin and Swift keep their documented limit for an
   abort racing an object-carrying reply.
6. Swift S33 (`contract-tests/swift/Tests/ContractTests/S33_Polling.swift:74`) is timing-sensitive: `TimedRecorder` must start
   sampling before the first fetch completes, which it cannot guarantee; it failed once in two runs here. Handing the recorder
   the first value would make it deterministic.

The stray branch `backup/of-foreign-458fafa` is deleted: its tip, `458fafa`, was a merge commit (parents `5ae4aa8`, `4169bde`)
titled "fix(meta): an infinite query may return Result<Vec<T>, E>" that carried types-paging's whole in-flight work (350
files: bindgen, lazy lists, queries, the Swift, Kotlin and TypeScript runtimes) onto this branch by mistake; that fix is on
`main` as `d930064` and the work as `686a983`. Under it were the branch's own earlier O3 and O6 commits (`55e6daf`, `a1fdb93`),
which this branch carries under other shas.
