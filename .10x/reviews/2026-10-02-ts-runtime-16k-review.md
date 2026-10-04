# `ts-runtime-16k` (ADR-057: the JavaScript runtime's first chunk at 16 KB) - adversarial review

**Date:** 2026-10-02 · **Reviewer:** the integrator's adversarial reviewer (`docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/ts-runtime-16k`
at `124cb34` (39 commits on `main` `fc326d6`); `main` merged twice (`1e8f33c` first, `ae362ac`, the generics wave, before the push) · **Read:** `CLAUDE.md` (R1, R3, R5, R6, R8, R9, R11), ADR-057 with
its implementation note, the ADR-052 amendment, `.10x/decisions/sde/ts-runtime-16k.md`, SPEC 12 and 17.1, and the runtime's diff
(`core.ts`, `core-extras.ts`, `transport/{framed,transport,wasm-main,wasm-main-transport,wasm-snapshot}.ts`, `messages*.ts`, `identity.ts`,
`realtime/`, `db/`, `adapters/`), `scripts/{build,mangle,test-dist,test-dist-plugin}.mjs`, `scripts/{wasm-size.sh,web-size-runtime.mjs}`, the
generator's entry change and the CI workflow changes · **Fixes:** six commits on the branch (below), each with a test that fails without it.

## Verdict

**Merge after the fixes below (made on the branch).** The design holds and the numbers reproduce exactly; four defects were High and are
fixed, the cost is 131 bytes of the first chunk (15,680 -> **15,811**, 190 under the 16,000 budget). The rename pass is sound (deterministic,
fails closed on strings, nothing outside the package touches a renamed name, every generated package and sibling package typechecks against
the published declarations once `reclaim` is back). What was wrong was in the levers' edges: a stripped declaration generated code imports
(H1), two operations made lazy whose order with the calls around them is their meaning (H2), a background drain that needed a fetch at the
one moment a page cannot make one (H3), and text that leaves the runtime as data turned into codes against D1 (H4).

## Findings

| # | Sev | Where | Finding | Status |
|---|---|---|---|---|
| H1 | High | `tsconfig.build.json` (`stripInternal`), `identity.ts` | `reclaim` was `@internal`, so the published `dist/*.d.ts` dropped it; the bindings of every schema with an `async` method that returns objects import it. `tsc` of the playground's, two-cores' and the `object_graph` golden's bindings against the built package: `TS2305: Module '"@undra/runtime"' has no exported member 'reclaim'`. Every suite aliases the sources, so all were green. | **Fixed** `872ec61`: public again; `test/declarations.test.ts` asks the compiler (its own `isInternalDeclaration`) about every name the generator imports (goldens, examples, `ts.rs`). All 25 generated TypeScript packages (goldens and examples, ADR-058's generics included after the second merge of `main`) and the RN, testkit, devtools and contract sources typecheck against `dist` (bundler and node16 resolution, `skipLibCheck` off for the entries). |
| H2 | High | `core.ts` `snapshot`/`restore` (row 8) | Both ran after `await import("./core-extras.js")`, which yields even when the module is cached, so on **every** call, in every mode, calls made after `restore()` reached the core first: in wasm-main the restore cancelled them (or silently overwrote a finished command), and `snapshot(); close()` rejected "closed" instead of resolving with the bytes. The existing test "a core closed while the request is on its way" accepted either outcome. SPEC 5.9/17.1 order them with the calls around them. | **Fixed** `7ca5f43`: the host's exports are `WasmHost._snapshot`/`_restore`, called at the call; a transport's own `snapshot`/`restore` is called at the call; only `UndraRestoreError` for a refusal loads on demand. New test (both modes): a call made after `restore()` is still pending once it resolved (0 before the fix); the close test now requires the bytes in wasm-main. |
| H3 | High | `core.ts` `_backgroundWindow` | The page's window (ADR-046 3.4: within the page's life, at `visibilitychange`/`pagehide`/`freeze`) called `runInBackground`, which first fetched `core-extras`. Offline, or on a page being left, the import fails or never completes and the pending work is not drained; `core-extras.test.ts` called that the intended behaviour. | **Fixed** `7ca5f43`: the window calls `run_background(1000)` itself (`RUN_BACKGROUND` a pinned literal); it imports nothing. The test now fails the chunk import and requires the call to reach the core. |
| H4 | High | `realtime/`, `db/`, `adapters/{fs,http,idb,kv,secure}.ts` | The production build made data codes: `WsError.Closed(1008, "T0115; see https://…")` reached the core and, as the close frame's reason, the server; the public constants `DID_NOT_KEEP_UP` and `HEADERS_REFUSED` of `./realtime` were T-codes; every field of the `WsError`/`SseError`/`DbError`/`StorageError`/`HttpError`/`FsError` the adapters raise (also through `StorageError.from` of an IndexedDB error) reached the Rust core as a code where Swift and Kotlin send sentences. D1 keeps every field equal in both flavours. | **Fixed** `e599a9f`: the 89 sites write their sentence (both builds; on-demand and opt-in modules only, no gated byte); 78 codes retired (`null`); 4 local programming errors there stay codes. `flavours.test.ts` compares the data of both flavours (it read T0115, T0114, T0061, T0062, T0009 before). SPEC 12, ERRORS.md and the errors page say which text is data. |
| M1 | Medium | `scripts/test-dist-plugin.mjs`, `flavours.test.ts` | Gap in the proof behind H4: every suite "against the production build" swaps the development `messages.js` back in, and `flavours.test.ts` sampled 25 errors the app sees, none a port's. No behaviour test ever ran production text. | **Mitigated**: the data test above; the rest stays as designed (the suites assert sentences). |
| L1 | Low | `scripts/wasm-size.sh` | With 190 bytes of margin the next growth lands on the budget, where the message said "re-record", which then fails the same way. | **Fixed** `a509480`: over a budget it says `--record` cannot raise it and names ADR-057's levers left (host events by use, about 530) or an ADR. |
| L2 | Low | `scripts/mangle.mjs` | The string check missed a template literal (`` o[`_x`] ``). | **Fixed** `57784bd`, with a test. |
| L3 | Low | `core.ts` `typed()` | A transport is "typed" when it has `observe`; one with only some of the seven control methods gets a `TypeError` from `release`/`cancel`. The interface's doc says all or none; TypeScript cannot enforce it (all optional). | **Fixed** `960198e`: every transport but the in-process host goes through `framed.ts`'s `channel()`: all seven and `sendCall` pass as they are, none (or all seven without `sendCall`) are framed, some are refused before they start, `UndraError("options")` naming both lists (T0246). Tests: the refusal, and the framing of seven-without-`sendCall`; both failed first. |
| L4 | Low | `docs/DEV_LOOP.md`, ADR-057 section 4 | "`vite dev` resolves `development`": its SSR modules are loaded by Node and get `default` (production text). Measured with a consumer project. | **Fixed** (DEV_LOOP) in `e599a9f`. |
| L5 | Low | `test/framed.test.ts:273` | A 5 ms sleep before a negative check ("nothing arrived yet"): it cannot fail on a slow runner, only check less. | **Fixed** `1524105`: ordered by events (an entry of another signal drained, then a macrotask, then its own entry); a mutant whose waiters settle on any signal passes the old check and fails the new one. |
| L6 | Low | `mirror.ts` `whenObserved` | A public method of an exported class: on a mirror whose core did not install the waiters (`new Mirror()`, an in-process or other synchronous core's mirror) it now rejects `UndraError("state")` where `main` waited. Nothing in the repo calls it there (`UndraCore.observe` of a synchronous core never did), the method's doc says so; ADR-057 says the public members are kept, which is true of the name only. | **Fixed** `bea2d5d`, decided: the refusal stays (typed; nothing of the runtime needs waiters on an in-process core), it says `mirrorWaiters(mirror)`, and the package root now exports `mirrorWaiters` (a deep import was the only way, which the exports map forbids); SPEC 17.1 lists both. The test failed first. |
| N1 | Note | call path | Awaited call +4% on Node (below), `callSync` unchanged; within the 1,600/800 budgets. | n/a |
| N2 | Note | `.github/workflows/ci.yml` | A shared file edited from a worktree (`AGENT_WORKFLOW.md`, parallelism rules): the integrator's call at merge. | n/a |
| N4 | Note | `crates/undra-bindgen/tests/golden/generic_functions` | A cross-branch interaction at the second merge of `main`: ADR-058's new golden has a stream and was generated before ADR-057's entry change, so the golden test failed after the merge (`streams` missing from its `core.ts`). Regenerated (`UPDATE_GOLDEN=1`, that file only); `undra bindgen --check --docs` of the playground, two-cores, cookbook and fieldbook clean. | **Fixed** `e3a0075`. |
| N5 | Note | `crates/undra-transport/tests/protocol.rs` (`main`'s) | The branch's CI run on `124cb34` failed `the_cores_log_records_reach_the_client_and_the_sink` ("expected an envelope, got Silence"): the server sends its Hello before it claims the slot (`Session::on_hello`), and the test logged as soon as the client read the Hello, so the record reached only the sink. Deterministic with a 300 ms pause between the two (fails before, passes after); the test now waits until the bridge sees the client, as `handshake.rs` and `suspend.rs` do. | **Fixed** (test only). |
| N6 | Note | `runtimes/kotlin/.../wire/ChangeSetTests.kt` (`main`'s) | The push of `2e16fcf` failed the Kotlin job: "iteration allocates nothing per entry" read 2,120 bytes over 19,990 entries (bound 2,048), a one-off allocation of the JVM's. The branch fixed it (`aff1e5f`: least of five measurements; a 4 KB one-off injected fails the old check and passes the new, an allocation per entry still fails) while `main` fixed it its own way (`1a4d996`: a bound per entry); the branch takes `main`'s file. | Fixed on `main`. |
| N7 | Note | `contract-tests/swift/.../MigrationBuildB.swift:99` (`main`'s) | The push of `aff1e5f` failed "iOS 15 / 16 floor": S14 build B "timed out after 5.0 seconds waiting for build B to read build A's queue" (an absolute wait on the hosted macOS runner); it passed on `2e16fcf` and on `main`, and the piece touches no Swift. | Open, not this piece. |
| N8 | Note | `contract-tests/swift/.../S32_PagedLists.swift:220` (`main`'s) | The run on `1b593df` failed "Contract scenarios (Swift)": S32's third observer had 0 data entries for the next page right after the generated handle's rows reached 100 (it passed on `2e16fcf` and `aff1e5f`); the check reads the raw observer without waiting for its own entry. The piece touches no Swift. | Open, reported (not this piece's). |
| N3 | Note | `contract-tests/kotlin/.../S30BackgroundRun.kt:175` (`main`'s) | An absolute bound: the cache entry must be written "within 100 ms of Background, not 250 ms later". It failed once in the local contract job at load 13 and passed alone at load 6; the piece does not touch Kotlin. A reference armed beside it (the 250 ms debounce) instead of 100 ms of wall clock is the fix, for its own piece. | Open, not this piece. |

## What was reproduced

**Sizes** (`scripts/wasm-size.sh` on `124cb34` + `main`, zlib 9): `web/hello-runtime-js` **15,680**, with the helper **16,191**, all features
**40,100** (27,355 + 12,745): the implementer's numbers to the byte, the module list unchanged. After the fixes (`--record`): **15,811**,
**16,333**, **40,221** (27,455 + 12,766); the wasm line stays `main`'s (this host's rustc 1.99.0 measures 116,471 by path).
After L3 and the merges of `main` (`14a4689`, test-pacing) and `wt/reload-handles` (`7e5d238`, ADR-059, which removed the query-handle
re-creation from recovery): **15,774** (226 under the budget), **16,285**, **39,922** (27,064 + 12,858), recorded; the wasm line is the one
reload-handles recorded (118,409; this host measures 118,482).

**What a hello page loads before it shows state**: the whole template app built with the production package and opened in Chromium (the
Browser pane): `index.js`, the shared `payloads` chunk (modulepreloaded), the CSS and the wasm; "0 left" painted, a todo added, no on-demand
runtime chunk fetched. Nothing that moved out of the first chunk is on that path.

**Call path, Node** (`call-path.test.ts`'s measurement, base `main` and this tree alternating four times, load 11): awaited call 313, 314,
316, 314 ns before and 327, 332, 333, 325 after (+4%); `callSync` 162, 165, 168, 165 and 162, 166, 165, 167.

**Resolution of the two builds** (a consumer project with the built package installed): Vitest 5 resolves the development build for the test
file and for an externalized dependency alike (one copy); Vite dev SSR and plain Node resolve `default` for both (one copy); `--conditions=development`
gives the readable build. Types come from `types` first: one set of declarations, self-consistent under bundler and node16 resolution.

**The rename pass**: two builds are byte-identical; no file outside the package (generated code, goldens, examples, RN, testkit, devtools,
contracts, the CLI's assets) names a renamed property; computed accesses in the sources use public keys only; 171 names after the fixes
(`_snapshot`, `_restore` added).

## Verification

Local, on this machine (load 6 to 15 from other agents' builds):

* The runtime: `npm test` 2,030 pass + 1 skipped in 76 files, `npm run test:dist` 1,987 in 74, its three typechecks; the testkit and the devtools
  page on the sources and on `dist`; `bash runtimes/ts/devtools/build.sh --check` (rebuilt once: the bundle carries the table of messages);
  the React Native unit tests (111) and contract scenarios on both.
* `scripts/ci-local.sh` (a clone of the committed head, the workflow files' own steps): `ci/rust` (format, clippy `-D warnings`, the workspace
  tests, `undra bindgen --check` of the cookbook and the fieldbook, the docs, wasm32), `ci/ts`, `ci/wasm-ffi` (the acceptance on the sources and on
  the production build), `ci/playground-web`, `ci/react-native`, `bench/size`, `two-cores/jvm-and-node`, `site/build`: green. `ci/contracts`: the
  TypeScript column passes every scenario (sources and production build); the Kotlin column failed S30 once at load 13 and passed alone (N3).
  After the second merge of `main` the same jobs ran again (green, the generics golden fixed, N4). Pushes: `2e16fcf` (CI red: the Kotlin
  allocation flake, N6; Bench, Two cores, Site green), `aff1e5f` (CI red: the Swift S14 build-B wait on the iOS floor job, N7; Bench, Two
  cores, Site green). After the third merge of `main` (`3c279a6`): the three JavaScript rows re-measured unchanged (15,811 / 16,333 /
  40,221; the wasm is main's 117,165), the golden test green, and `ci/rust` in a clone green up to the cdylib step when the session closed.
* **The resumed session** closed L3, L5 and L6 (each test failed first), merged `main` `14a4689` and `wt/reload-handles` `7e5d238` (conflicts in
  `recovery.ts`, its test and the payload docs resolved for ADR-059 over the typed channel; codes 166 and 167, said only by the removed
  re-creation, retired), re-recorded the rows, and ran `ci-local` for `ci/ts`, `ci/wasm-ffi`, `ci/contracts`, `ci/react-native`,
  `ci/playground-web`, `two-cores/jvm-and-node`, `ci/rust`, `bench/size` and `site/build` before the final push; the hosted run of the
  pushed head is in the report.
* `node site/scripts/build-all.mjs` (current) and `node site/scripts/check-links.mjs --words` (347 of 350).
* Not verified here: the Swift column and the iOS jobs (macOS 15 runners; the piece touches no Swift), webpack and Metro resolution (not
  installed; the conditions were read, and Node's `--conditions` reproduce them), the device bench in Chromium (the Node call path was measured
  instead), and the Android emulator job (on hold).
