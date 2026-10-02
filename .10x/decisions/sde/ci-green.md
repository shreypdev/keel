# SDE: CI green on main (wt/ci-green, 2026-10-02)

CI on `main` had been red for most of the day (the last all-green run was `76f9364`) while every piece's local matrix was
green: the hosted runners differ from the Mac the pieces were verified on. This record is what each red job turned out to be,
what changed, and what could not be reproduced. The latest completed runs read were `94b87ba` (run 36988891084), `14aae90`
and `da4fbbe`. No test was skipped or weakened to get here; where an assertion changed, the table says what it measures now.

The first thing the logs showed: **a failing step hides everything after it.** Rust (Linux) failed at `cargo fmt --check`
(already fixed on main by `906d167`), before that at `cargo test --workspace`, and the 20 steps after the tests had not run
since `abi-table` (checkpoint 16). TypeScript failed in the test step, so `build.sh --check` of the devtools page had never
run either. Two of the causes below (the ASan overflow, the stale devtools assets) were found by running those steps.

## Reproduction

* Miri: reproduced locally (macOS, `cargo +nightly miri test -p undra-ffi --lib`), same error, then clean.
* Playground web, the Kotlin runner's `LIB`, the React Native contract column, TS S25: reproduced in the clean worktree
  (no `node_modules`, no git-ignored build output), which is what a CI checkout is.
* Node: the runner's Node is **22.23.3** (setup-node 22 was already in `ci.yml`; only the React Native job had 20), not 20.
  This Mac has Node 24.21 only; no Node 22 was downloaded, so the Node 22 behaviours are read from the logs and tested with
  doubles that behave as the logs say.
* Linux-only failures (devtools, ASan): no Linux here. The devtools suite was run 30 times under a 24-process CPU load; the
  ASan overflow is a one-byte overflow read off the stack trace.
* macOS 15 (Swift): the failing tests pass on macOS 26 here; see "Not reproduced".

## Red jobs

| Job | Cause | Fix |
|---|---|---|
| Rust (Linux), fmt | `crates/undra-signals/tests/lazy.rs` arrived unformatted from main | already on main (`906d167`); `cargo fmt --check` clean |
| Rust (Linux), test | `devtools.rs` `a_page_that_stops_reading_is_dropped…`: the test sent 40,000 tiny commits (about 2.4 MB) to a page that does not read, and relied on the socket filling so that the hub's 128 KiB queue overflows. macOS loopback holds a few hundred KiB; Linux's holds several MiB, so the page was never dropped | the commits are 16 KiB each (3,000 of them, 48 MiB; `Counter::set_label`/`get_label` added to the fixture), as `lifecycle.rs` already sends the app-side slow client far more than any socket holds |
| Rust (Linux), test | `devtools.rs` `a_commit_storm_costs_steps_by_time…` (flaky, `left: 19994 / right: 20002`): **a real hub bug.** `capture` read the sequence number before it snapshotted; a commit landing in between is in the snapshot but not in the number, and the capture its own wake-up causes finds the state unchanged, adds no step, and the newest step ends before a commit it holds (the page then shows those commits without a step badge) | `Ring::cover_newest`: an unchanged capture moves the newest step's `through_seq`/`txn` and re-sends it (the page replaces a step it knows, `state.ts` `#onStep`); a unit test of the ring. No deterministic hub-level test: each commit changes the snapshot, so the interleaving cannot be forced; the storm test is the integration check |
| Rust (Linux), test | `a_page_cannot_reach_the_core…` (flaky, once): the final `silent_for(100 ms)` of the app client raced the server noticing the last page's close: the last page out gives the app's observations back to the runtime, which answers each with the signal's value | wait for `!devtools_attached()` (the hub has let go, so those frames are queued), then the existing round trip takes them out of the way, then the unchanged strict silence check |
| Rust (Linux), C ABI | `crates/undra-ffi/tests/c/two_cores.c`: `uint8_t payload[32]` for a 17-byte header and 16 bytes of args (33): ASan stack-buffer-overflow (seen at `5f5c3fb`, hidden since) | `payload[64]`; passes under ASan here |
| Rust (Linux), `dev_reload` (found on the first push) | nine tests each ran `undra dev` and cargo builds into the same directories: their builds queue on cargo's locks ("Blocking waiting for file lock on build directory"), and on a 4-core runner with a cold cache (a new toolchain, a new cache key) the last tests waited for the others past the 600 s deadline of one build ("undra dev printed nothing in time"); one test also took an earlier restart of the same schema for the one under test | the tests that run `undra dev` take turns (a process-wide lock held from a test's first server to its last: the parallelism bought nothing); the silence deadline is 900 s and the failure says how long the server had run; the tests that edit the schema wait for the restart that changes it (`wait_restart_changing_schema`); the test copy declares `cfg(playground_v2)` (the `unexpected cfg` noise in every such log) |
| React Native, unit tests (found on the first push) | `a flood under a stalled reader stays bounded…` waited for the server to have written 6,000 events before reading, but the adapter gives up at the 4,097th and aborts the request, and whether Node's HTTP write path (which changed between the Node 20 and Node 24 of the jobs) has written them all by then is not the property under test | it waits for the request to be over (given up, or the whole body in), then reads; the reader is still stalled |
| TypeScript runtime | on Node 22.23.3: (1) `node:sqlite` reads text up to its first U+0000 (`"a\0b"` comes back `"a"`): `db-node` suite, 2 tests; (2) the global `WebSocket` (undici 6) fires `error` and never `close` for a refused upgrade and for an unresolvable host, so `browserWebSocket`'s connect never settled: `realtime-adapters`, 2 timeouts | CI and everything else on **Node 24** (below). Both are also fixed where they bite: `nodeSqliteDb` probes the Node's `node:sqlite` at first open (U+0000 bound and read back) and is `Unavailable`, saying why, instead of corrupting text silently; `browserWebSocket` settles a connect on `error` before `open` (the standard fires `error` then `close`; either settles). A test double of each Node 22 behaviour |
| Contract (TS + Kotlin) | TS S25 `MISSING`: `s25-db.test.ts` imports `@undra/runtime/db-worker`, whose sources import `wa-sqlite` from **the runtime's** `node_modules`, which the job never installed. Kotlin: `run.sh` line 151 used `${LIB#…}`, never set (macOS's bash 3.2 does not stop on it, bash 5 does) | `contract-tests/ts/run.sh` installs the runtime's dependencies (and its own) when missing; `LIB_DIR` |
| Playground web | `tsc` could not find `react` for `runtimes/ts/@undra/runtime/src/react.ts`; behind it, `vite build` could not resolve `wa-sqlite` for the Db worker. The app used the runtime's sources and so needed the runtime's `node_modules`, and with it a **second copy of React** | the app declares `wa-sqlite` (the optional peer it needs), `resolve.dedupe` for `react`, `react-dom`, `wa-sqlite` and a `paths` entry for react's types: it builds from its own `npm ci`, with or without the runtime's install |
| React Native | S14, S15 (build B of the playground wasm) and S27 (the two-cores wasm) read git-ignored files that only `contract-tests/ts/run.sh` makes | `contract-tests/ts/build-cores.sh` (the build half of `run.sh`) and the job calls it |
| Miri | `frames.rs`: `dladdr` and the unwinder are foreign calls Miri cannot run | a `loader` module with a `cfg(miri)` stub (no image, so no frames and an empty id: what the source already does for an image the loader does not know); the two FFI tests are `not(miri)`, a `miri` test asserts what replaces them; the four header-parser tests stay Miri-clean. `--lib` 41 pass, the `abi` subset passes |
| TypeScript runtime (not yet reached) | `bash runtimes/ts/devtools/build.sh --check`: the committed page assets were not a rebuild of their sources (the wire module changed under them; only minifier names moved) | rebuilt and committed |
| Swift runtime + C ABI (macOS) | 12 failures in 4 tests. (a) `testALoneMessageIsAnsweredWithin…`: median 10.09/10.24 ms against a hard 10 ms; here 3.1 ms. The binding answers a lone message when its 2 ms `burstGap` timer fires, and that timer fired after about 10 ms on the runner | the budget is the platform's own `burstGap` timer (median of 21) plus 5 ms: still tighter than 10 ms on a machine with accurate timers, and it measures the binding's delay, not the runner's clock |
| | (b) client-initiated close (`testEcho…`, `testSubprotocol…`, `testAConnectionTheCoreLetsGoIsClosedGoingAway`, `testShutdownDuringAPendingReceive…`): the server never saw a close code within 5 s; they pass on macOS 26, S23 (same adapter, same server) passes on the same runner | **The platform.** The failure message (the server's record: `closeCode: nil, clientClosed: false`) and a server-side timeline showed a FIN at the moment of `cancel(with:reason:)` and no frame. A matrix on the hosted macOS 15 runner (a virtual M1, 3 cores) ruled out, one by one: the URL cache of the polls (the XCTests fail alone, with the cache bypassed and `no-store`), the process (XCTest class, main-actor XCTest and a standalone program all lose the frame, on three VMs in the same run), the server's launch mode, a pending receive or none, the async `receive` and its task cancellation (completion handlers do the same), `finishTasksAndInvalidate()` at once, after 500 ms or never, a ping/pong before, a 30 ms pause before, and the context of the cancel. Some earlier VMs delivered the frame for every variant, so it is a race inside `URLSessionWebSocketTask`, not an API contract, and the adapter, which `cancel(with:)` tells nothing, cannot see or retry it. The contract is stated per OS version (SPEC 8.1, `docs/ERRORS.md`): the code is delivered from macOS 26 / iOS 26, before that the connection ends and the code arrives when the system sends it; the tests assert the code strictly from 26 and that the connection ended before (`waitForTheClientsClose`), and the server ends its side when a client half-closes so that "the client left" is observable. The adapter is unchanged. |
| iOS 15 / 16 floor (`da4fbbe` only) and Swift contract S33 | S33 (gap between the first two ticks read as 0.0 s) in iOS 15 mode on the runner, and **here, on an idle Mac, in 2 runs of 3**: a race in the scenario. `TimedRecorder` polls the store every 5 ms, `waitUntil` every 10 ms, and the first tick is already the recorder's first entry (the handle's first fetch runs while it is made), so when the wait saw the second tick before the recorder did, `lastGap` had one entry and returned 0 | the scenario waits for the recorder to have seen the second tick before it reads the gap; 6 of 6 passes. The same run's S30 ("cache entry written within 100 ms of Background") is a 100 ms window of the spec on a loaded VM; passes here, **not changed** |

## Node

Every workflow (`ci.yml`, `release.yml`, `site.yml`, `bench.yml`, `rn-devices.yml`, `two-cores.yml`) and the project
workflow `undra init` writes (`crates/undra-cli/templates/ci/web.yml`, with its test) pin **Node 24**, and so do the jobs that
used the image's Node and spawn the realtime server (Kotlin, macOS, iOS floor, Swift contracts). Why 24 and not 22: 22.23.3 is
what failed; 24.21 is what the contract notes measured and what this Mac runs. The packages' `engines` stay `>=20` (an app
does not need `node:sqlite` or the global `WebSocket`); `docs/ONBOARDING.md` says which suites need the newer one. No test
skips on Node: the one adapter that cannot serve a Node says so as a typed error.

## Rust 1.99.0

The header of `ci.yml` said how: every `rust-toolchain` pin (ci, bench, site, two-cores, rn-devices, release) to 1.99.0, the
goldens regenerated, the budgets and the size re-recorded. `cas_update` had already replaced `fetch_update`.

* clippy (`--workspace --all-targets -D warnings`, and `-p undra-ffi --target wasm32-unknown-unknown`): clean, no new lint.
* trybuild goldens (6 files of 76): the only change is that rustc lists `(A,)` before `(A, B)` in "the following other types
  implement trait"; nothing of ours moved.
* `cargo test --workspace` (with `UNDRA_REQUIRE_TOOLCHAINS=1`): 3,536 pass, one fails for the machine: `symbols`
  `android_frames…` wants the `undra` emulator online.
* budgets (`undra-bench --test budgets --release`): pass; `sync_alloc`, `commit_alloc`, `derived_alloc`, `lazy_alloc` pass.
  Against the machine baseline (`UNDRA_BENCH_BASELINE=apple-m5-pro`, p50 within 1.5x) two rows are out:
  `snapshot/cold_start_restore_100kb` 1.82x and `…_core_thread` 1.66x. **That is not the compiler**: the baseline's own
  commit (`8004a21`) built under 1.99.0 measures 72.96 us (1.07x of its 68.04 us). It is something in the snapshot restore
  path since 2026-09-30; the baseline is not re-recorded over it (it is not gated in CI, which records its own base).
* `scripts/wasm-size.sh`: hello wasm 116,181 gzipped (116,690 at 1.98.1; gate 120,000), runtime JavaScript up front 22,100
  (gate 22,100); both recorded inside the gates. The Android record (978,552 / 1,045,200) re-measured under the pin:
  byte-identical.

## Landing rule (founder, 2026-10-02)

No piece lands on `main` unless CI is green on its branch's exact head. `ci.yml`, `bench.yml`, `two-cores.yml` and `site.yml`
also run on pushes to `wt/**` (a newer push cancels the run it supersedes on `wt/**` only; main's runs have a group per
commit and are never cancelled, as `bench.yml` already did; `rn-devices.yml` keeps its path filter; Site's deploy job and
its Pages steps are `main`-only). Bench compares a branch with the merge base with main, as a pull request is compared with
its base. `scripts/wt.sh merge <slug>` fast-forwards only when main is an ancestor of the branch and
`gh run list --branch wt/<slug> --commit <head>` shows CI, Bench, Two cores (and Site when the branch touches its path
filter) completed with success for that sha, naming what is missing or red and how to push; `--no-ci` is the loud override
for state-only commits. The parsing is `scripts/wt-ci-check.sh`, tested on fixtures by `scripts/wt-ci-check.test.sh` (a CI
step); the merge itself was exercised against a scratch repository with a stub `gh` (red, green, stale branch, `--no-ci`).

## Clean-clone verification (macOS, Rust 1.99.0, Node 24, `UNDRA_REQUIRE_TOOLCHAINS` as each step needs it)

A fresh `git clone` of the branch (no `node_modules`, no build output), every step of the workflows that gate code, in order,
all exit 0: Rust job (npm ci of the runtime, fmt, vectors, device bench report, clippy, `cargo test --workspace`
3,537 passed 0 failed, bindgen checks, realtime recipes, cookbook TS, Fieldbook web test and build, build integrations,
write rule in release, wasm32 clippy, C ABI under ASan, schema retention and docs, rustdoc, wasm32 builds), TypeScript (runtime
1,858, testkit 37, devtools 71 and `build.sh --check`), wasm ABI, Playground web, leaf features, the Site workflow's own
steps (core, playground build, rustdoc, script tests, `build-all.mjs` leaves nothing stale, both link checks, staging),
React Native (cores, derived vectors, C++ host and JSI, Android Java, 110 unit tests, the contract column), contracts
(`run-all.sh` ts kotlin and swift: 95 of 95), Kotlin runtime (881), Swift (870), `typecheck_swift`, Swift over the C ABI,
iOS cross-checks, Xcode, cookbook Swift and Kotlin, iOS floor (runtime, golden, apps, contract). Miri: `--lib` and the `abi` subset.

## What the first push taught about process

Temporary instrumentation of a hosted runner has a cost: a `stderr` write per received message killed the whole Swift job (`NSFileHandleOperationException`, a non-blocking stderr) in the run that was meant to explain three tests, and every push cancels the previous run of the same branch. The diagnostics were removed in one commit; the platform findings above came from standalone programs and short matrices.

## Not reproduced, and what to watch

* S30 in the floor job (S33 was a race and is fixed). The macOS 15 client close is a platform race, documented per OS.
* Linux: after the devtools tests the Rust job runs about 20 more steps that have not run on Linux for a day. Each was run
  here on macOS from a clean clone (see the report); a Linux-only difference in one of them would show on the next run.
* `a_commit_storm…` and the page tests were hardened under CPU load, not on a 4-core Linux runner.
