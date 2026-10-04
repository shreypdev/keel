# `ts-size-e4` (the TypeScript runtime's size, ADR-052's amendment, and the E4 call path, ADR-056) - adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:** `wt/ts-size-e4` at
`8df31c5` (`main` `d1b35b5` merged; `main` is `8000d39`, a handoff commit, merged at the end) · **Read:** `CLAUDE.md` (R3, R6, R9, R11),
ADR-056, ADR-052's amendment, `.10x/decisions/sde/ts-size-e4.md`, SPEC 17, and `git diff main...HEAD` for the runtime's `core.ts`, `mirror.ts`,
`transport/{wasm-main,transport}.ts`, `wire/{reader,writer}.ts`, `vite.ts`, `adapters/{default-ports,standard,browser-events,events}.ts`,
`scripts/{web-size-runtime.mjs,web-budgets.mjs,wasm-size.sh}`, `bench/src/budget.rs`, `bench/budgets.toml` · **Scope:** the five surfaces of the
brief; nothing else · **Fixes:** `fix(ts-size-e4): review fixes` (listed below), then this record.

## Verdict

**Sound with fixes; merge.** No High. The direct call keeps every guarantee the brief names: the change-sets that arrived before a reply are
applied before the caller resumes, for all five reply statuses, for a reply that is there when `send` returns and one that comes later, with no
frame pacer, one that never fires and one on a timer (46 cases, and the property fails 30 of them when `queueFlush` is removed); a call from inside
a host callback is refused typed, identically to `main`, on every entry; streams are untouched. The wire changes are byte-identical to `main`'s
(12,000 random payloads, 12,000 random inputs, mutation-checked), and the shared scratch is never held across code that can run another reader or
writer. Three Medium findings, each fixed with a test that fails without the fix: a send that throws *after* the core answered rejected a call that
`main` resolved (M1); the `Timer` port of a `remote` core with an explicit timer adapter was registered after an awaited chunk load, so a core's
first `Timer.set` after the Hello could be refused (M2); and the React Native defaults test waited a fixed number of ticks for a reply that now waits
for a chunk, and failed one run in six, leaving a core open that took five tests with it (M3). Docs: the SPEC lacked the underscore convention that
`#private` -> `private _x` needs (L1), the ADR's claims about declaration files and the `es2022` browser floor were inexact (L2), and the amendment did
not say what a hello app loads over its life (L3). The cold-start row did not get slower because of this piece: main and the branch measure the same
5.1 to 5.5 ms today.

## Findings

| # | Sev | Where | Finding | Status |
|---|---|---|---|---|
| M1 | Medium | `core.ts:1176-1186` (`_callDirect`) | `main`'s `_send` ignores a throw from `send` once the call's entry has been answered (`if (this._pending.get(callId) === entry)`); the direct call returned `Promise.reject(error)` whatever the entry held. A core that replies and then traps in the same export (the reply is delivered, `undra_call` does not return) resolved the call before and rejected it now, dropping the reply the core gave. `direct-call-ordering.test.ts` ("a send that fails after the core already answered") failed with the `UndraTransportError`. | **Fixed**: a throw before the answer fails the call, one after leaves the answer standing; the same test, also through the call-with-a-signal path (`_send`), passes. |
| M2 | Medium | `core.ts:932-950` (`_start`) | An explicit `Timer` adapter on a `remote` core is served by `timerPort` from `adapters/ports.js`, a dynamic import since this piece; it was awaited *after* `transport.start`, so the port was registered a chunk fetch after the Hello instead of in the continuation of `start`. A native core that arms a timer the moment it connects (the next macrotask) got status 2 "unavailable" for it. `timer-port-start.test.ts` (chunk delayed 40 ms, a core that calls `Timer.set` on the macrotask after its Hello) failed with `2`. | **Fixed**: the module is imported before the transport starts, the port is registered where it was. |
| M3 | Medium | `rn/@undra/react-native/test/defaults.test.ts:146,179` | Two tests waited `tick(5)` and `tick(20)` (fixed `setTimeout`s) for a port reply. A default port (Kv, Http) now runs after `import("./standard.js")`, whose time is the module loader's: one run in six (observed at the 6th of 12 loop runs, and in the matrix) the reply was not there, the test threw before `core.close()`, and the open shared core failed the next four tests. | **Fixed**: the tests wait until the reply is there (`until`, 5 s cap); 15 of 15 runs pass. |
| L1 | Low | `docs/SPEC.md` 17.1 | `#private` -> `private _x` makes the fields reachable from JavaScript (the brief's R3 point); SPEC 17 said nothing about the convention. | **Fixed**: a paragraph at the head of 17.1 (underscore = not API; the compiler enforces `private`; a declaration file lists them; generated code does not use them, and cannot collide: `heck`'s camel case drops a leading underscore). |
| L2 | Low | ADR-056 Consequences | "the declaration files hide them" is wrong: `tsc` emits `private _pending;` (name, no type; checked in a `tsc` emit of `core`, `mirror`, `stream`, `signal`, `object`). And "Chrome 94, Firefox 93 and Safari 16.4 are the floor of that syntax" is the floor of *all* ES2022 syntax (class static blocks), not of the runtime's output. | **Fixed** (measured, below). Lever 1's "DataView on first use" annotated as removed by lever 7 (no per-buffer `DataView` exists any more, so there is no lazily created `DataView` path to test at 64/65 bytes; the 64/65 boundary that remains is `finish()`'s copy, tested). |
| L3 | Low | ADR-052 amendment | The amendment argues the gate counts the up-front chunk, but not what a page that calls a lazy port loads in all. | **Fixed**: "What a page loads over its life": 21,159 + `standard` 2,471 + `ports` 1,688 = **25,318** for an app that calls any of Http, Kv, SecureStore or Fs, against 25,996 before (a saving of 678 bytes and a round trip, not 4,686); the failed-chunk behaviour; per-port ordering. |
| L4 | Low | `docs/DEV_LOOP.md:318` | Said what the plugin sets, not what `es2022` excludes. | **Fixed** (the floor measured below; a target below `es2022`, `safari15` included, lowers the class fields again). |
| N1 | Note | `default-ports.ts:34`, `:50` | Calls made before a port's chunk arrives run in the order they were made, **per port**; across two ports the port whose code arrived first runs all of its queued calls first (real ES loader, chunk delayed 400 ms: Kv, SecureStore, Kv, SecureStore ran as Kv, Kv, SecureStore, SecureStore). Port calls are answered by id and are independent, so nothing depends on it; before, the adapters ran in call order. | Documented in the amendment. |
| N2 | Note | `bench/budgets.toml`, `bench/results/web-size.jsonl` | After the fixes the gate measures **21,173** (record 21,159; +14 bytes for M1 and M2). Within the 21,500 budget and the 5%; the record is the integrator's `--record` (README and site read 21.2 KB either way). 327 bytes of headroom for `ns-storage`, which waits on this. | Open. |
| N3 | Note | `main` | `undra bindgen -C examples/ios15-sample --check --docs` fails on `main` too (four generated Swift files differ; the generator and the sample are untouched by this piece), and `check-links` reports `docs/cookbook/ios-15-16.html` (description 171 chars) and the chrome of `docs/db.html`, `docs/realtime.html`. | Open, not this piece. |

## What was attacked, and what happened

**1. The direct call and ordering (ADR-031, 023, 036).** `direct-call-ordering.test.ts`: in-process and asynchronous fakes x three pacers (never fires, 20 ms
later, the default) x ok, error (1), panic (2), cancelled (3), bad request (5): a store written by a change-set before the reply has its value in the
continuation, for an `await` and for a `.then` on the settled promise; and for a reply that arrives after `send` returned (held responder, the change-set
then the reply, as `undra_poll` does). 46 pass; replacing `this.mirror.queueFlush()` in `_onReply` fails 30. Statuses map as before (the same `UndraReplyError`
with the same status and body: the reply path is unchanged). A call made from a signal subscriber inside a drain is not interleaved (listener, call,
call returned, listener of the call's change-set in the drain's next round, then the caller resumes). A call from inside a host callback of the running
core (the real playground core; the Log adapter is called on the thread that holds the lock while the panic hook logs): `add` rejects
`UndraCallError.Refused`, `core.call` rejects `UndraReplyError` status 5 ("the core refused the call (undra_call returned 5)"), `callSync` throws status 5
with `E_REENTRANT`: **identical on `main`** (the same test run against `main`'s runtime source); kept as `contract-tests/ts/test/direct-call-reentry.test.ts`.
The change-set listener itself never re-enters: the runtime's `changeSet` handler only enqueues, and subscribers run in the drain, outside the core. The
reused 17-byte header is copied by the transport before the core runs, so a nested `callSync` from a port rewriting `_head` cannot corrupt the outer
call. A transport with neither `sendCall` nor `callSyncParts` (the fakes, the remote, worker, native and recovery transports) takes `send`/`callSync`; the
whole contract TS column, `remote.test.ts`, the recovery scenario (S22) and the RN transport's contract all pass. M1 came out of this surface.

**2. Wire reader and writer.** A differential fuzz against `main`'s `wire/` (copied to a scratch directory, since removed): random sequences of every
write operation, initial capacities 0 to 256, strings of 0 to 300 units with non-ASCII at random offsets and lengths around 23/24/25, 47/48/49 and 62 to
66 bytes, lone surrogates, a leading U+FEFF, u64/i64 at the limits, f32/f64 with -0, NaN, infinities, subnormals, `u64n`/`i64n` at 2^53, uuid, bytes, raw
ranges, `finish()` then reuse; 12,000 payloads: identical bytes, identical `RangeError` and `WireError` messages and positions, identical positions. Readers:
12,000 inputs (valid, corrupt, truncated, UTF-8-corrupted), at every `byteOffset` 0 to 7: identical values and errors. Nested codecs
(`vec<option<result<string, vec<u64>>>>`) 4,000 values. Mutation checks: shrinking the short-string reserve and moving the reader's ASCII threshold are each
caught. No difference. Kept, without `main`'s code, as `wire-reference-fuzz.test.ts` (a `DataView`/`TextEncoder` reference, 12,000 cases). The shared
scratch `DataView` is used by writer and reader only between the write (or copy) and the read (or copy) inside one method, with no user code between: the one
place user code runs is a value's own coercion, which happens before the scratch is touched (a `valueOf` that runs a writer is tested). `finish()` at 63, 64,
65 bytes: exact, never overwritten by the next write.

**3. Lazy default ports.** A real ES module loader with `standard.js` delayed 400 ms (`node:module` hooks over a `tsc` build): four calls on two ports made
at once were all held, none dropped, each port's in order (N1). The same in `default-ports.test.ts` for one port (a Vitest module mock answers a second
`import()` of a mocked module with the real one, so the cross-port case could not be tested there). A chunk that cannot load (a CSP that allows the entry
but not its chunks, a deploy that replaced the file, offline): the core gets "unavailable" (port status 2), `onError` gets an `UndraUnhandledError` whose
operation is `Kv port 0x... method 0x...` and whose cause is the failed import, and the next call loads again; a policy by origin, or a nonce with
`strict-dynamic`, allows a chunk next to the entry, so this is not reachable without breaking the entry too. A synchronous port cannot be a lazy chunk:
`lazyPort` returns `sync: false`, every default method is wrapped in an `async` function, so `wasm-worker`'s refusal of a main-thread synchronous port
(ADR-049) still happens at `load` (S21, `worker-ports.test.ts` pass). **Vite plugin:** `config()` returns `es2022` only when `build.target` is undefined;
an app's `es2020` and `["chrome100", "safari16"]` are kept (tested). **es2022 and Safari 15:** the hello app built at `es2022` and at `safari15`
(`UNDRA_SIZE_TARGET`) differ in the runtime chunk only by public class fields turned into helper calls (+ one `typeof` helper): 21,147 against 21,594
gzipped. The `es2022` output has no class static block, no `#x in obj`, no top-level `await`, no `.at()`/`Object.hasOwn` in the up-front chunk, and no
private field in it at all (the lazy `remote` and worker chunks keep `#private`, Safari 14.1). So iOS 15 Safari runs it; the plugin need not stand aside for
a Safari 15 floor. The cost is on the other side: the pinned Vite lowers class fields for a `safari15` target, so an app that sets that target keeps its
target and the slow helper path. ADR-056 and DEV_LOOP say so now (L2, L4). ADR-045's floor is the native iOS deployment target and does not concern the web.

**4. `#private` -> `private _x` (R3).** `tsc` declaration emit of `main` and the branch compared after removing comments and `private` lines: the public
surface did not change; the differences are moves and re-exports (`Kind` to `wire/kind.ts`, `PortCallPayload`/`Hello`/`Log` to `wire/session.ts`,
`NetKind`/`AppState`/`emitConnectivity` to `adapters/events.ts`, `browserConnectivity` to `browser-events.ts`, all re-exported from the old modules) and the
two optional `Transport` methods plus the two on `WasmMainTransport`. Private members **do** appear in a `.d.ts`, as `private _pending;` (name only); the
ADR said they did not (L2). `undra bindgen --check --docs` is clean on the playground, `two-cores/{a,b}`, the cookbook and the fieldbook, and the
bindgen goldens did not move. Generated code calls only `core.call`, `core.mirror`, `Signal._set` and the store base class's protected members, and a generated
name cannot start with an underscore, so it cannot collide with `_undraClosed`. The remaining non-mechanical differences in `mirror.ts`, `signal.ts`,
`stream.ts`, `object.ts` after normalising the renames: none.

**5. Gates and numbers (R9, ADR-052).** `scripts/wasm-size.sh`: hello-wasm 116,966 (gate 120,000, ceiling 120,000), hello-runtime-js 21,159 at the branch's
head, 21,173 after the fixes (gate 21,500). The measurement change is argued honestly in the amendment for the gate (the up-front chunk, 25,996 -> 24,335
by measurement alone, both numbers given), and it now says the rest (L3). The `web."id"` rows are read by both harnesses from one file (`web-budgets.mjs`
parses the tables; `budget.rs` parses them strictly and checks each `measured_ns` against its `budget_ns`, and the 5x-and-two-significant-figures rule is
the file's own, which nothing but a reader enforces: I checked all eight rows by hand, they satisfy it; `cargo test -p undra-bench` 69 pass; a doc comment there
named `call-path.test.mjs`, corrected to `.ts`). **A slowed call fails
both:** a 12 us spin in `callSync` and `_callDirect` fails `call-path.test.ts` (an awaited call 12,428 ns against 1,600; `callSync` 12,230 against 800) and
`bench.spec.ts` in Chromium (`sync_call` p50 15,040 ns against 2,200); restored, the bench passes: handle call 435 ns, `callSync` 297, 1 KB 1,210, keyed
insert 13.2 us, 100-signal change-set 16.9 us, merged frame 1.13 ms (load 4). **Cold start, three alternating rounds of ten fresh contexts each, two
sessions** (Chromium 153, the bench page built from `main` and from the branch, same wasm; host load 9 then 4): main 5.15 / 5.42 / 5.52 ms and 5.08 / 5.23 / 5.31
(round p50s, sorted), branch 5.18 / 5.20 / 5.34 and 5.28 / 5.28 / 5.35: **no difference attributable to the branch** (-0.2 and +0.05 ms of medians, inside the spread).
The 4.5 -> 5.3 ms of the committed rows is the host and the browser of the day, not the load of the page: `main` itself reads 5.2 ms now. (The compile
step is 0.84 to 0.91 ms for both.) The playground's stress screen in the browser pane at 100,000 updates a second: generated 100,015 per second, received
100,103, applied 176, **42 ns per change-set**, drain p50 under the clock step (0.1 ms), p99 200 us, 0 dropped frames in 5 s (longest 9.4 ms), JS heap 15.0 MB
(one reading, the garbage collector's timing; the record's 7.8 MB is another single reading).

## Counts (the tree merged with `main`, after the fixes)

`cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean. `cargo test --workspace --no-fail-fast`: **3,079 pass, 0 fail, 16
ignored** (19 of them are the bindgen tests that compile generated TypeScript and need `tsc` on `PATH`; rerun with it). TypeScript runtime: `npm test`
**1,517 pass in 51 files** (1,463 + 54 added in review), the three typechecks clean. `crates/undra-ffi/tests/wasm/run.sh` pass. `contract-tests/run-all.sh`
**74/74**. Interop pass. `@undra/react-native`: 87 pass, typecheck clean, `test:contract` 18 pass and 1 skipped. Testkit 32. Devtools page 71 and
`build.sh --check`. Playground web 121, typecheck, build, Playwright smoke 5/5. Fieldbook web 13 and its build (with `UNDRA_BIN`). `undra bindgen --check
--docs`: playground, `two-cores/{a,b}`, cookbook, fieldbook clean (`ios15-sample`: N3). `cargo test -p undra-bench` 69. Site `build-all` current;
`check-links --words` 342 of 350 words (the two problems of N3 are on `main`).

## Open items

* N2: re-record the web size (`scripts/wasm-size.sh --record`) at the integration; 21,173 against 21,159. `ns-storage` has 327 bytes of room under the 21,500 gate.
* A real-browser check of a chunk blocked by a policy (the unit test mocks the loader; the ES-loader run delayed it but did not fail it).
* N3 (`main`): the `ios15-sample` generated Swift, the cookbook page's description, the chrome of two docs pages.
* A baseline gate for the JavaScript rows (ADR-056's own note): the 5x budgets catch an order of magnitude, not a 2x drift; the alternating cold-start runs above are the
  evidence that the committed row's 4.5 ms is not comparable with a later host.
