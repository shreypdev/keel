# `ts-size-e4`: the JavaScript runtime's size (ADR-052's amendment) and the web call path (ADR-056, E4)

Worktree `wt/ts-size-e4`, from `main` `1801951`, merged with `main` at the end. Author: the implementer of the piece.
Reviews: none yet (the integrator's adversarial review is next). Two ADRs: ADR-056 (new) and the amendment at the end of
ADR-052. No generated file changed (no golden, no `ts.rs` edit), no wire or ABI change, no schema hash moved.

## What landed

**Part 1, `ts-runtime-size`.** The JavaScript gate counts what the page loads up front (Rolldown's `$initial` group:
the runtime modules the entry reaches by static imports), and the on-demand chunks are `lazy_gzipped` in the record, not
in the gate: 25,996 (the old gate, the `wasm-worker` transport folded in) -> 24,335 for the same tree measured that way.
Then, each its own commit, with its gzipped numbers in the message: the remote transport is a dynamic import (-1,540),
the four default ports (Http, Kv, SecureStore, Fs) load on their first call (-2,314; `adapters/default-ports.ts`,
`standard.ts`, `browser-events.ts`, `events.ts`), the default ports' types stay out of the first chunk (-591), the framed
transports' wire code (`Kind`, `wire/session.ts`) leaves it (-241): **24,335 -> 19,649**. The E4 levers brought it to
**21,159** (+1,510: no `#private` on the call path +798, the rest +712), record `bench/results/web-size.jsonl`,
`[size."web/hello-runtime-js"]` budget **21,500** (was 26,000; the ADR's 16 KB is not reached: the amendment says why).
README/site numbers regenerated (`build-all`; the wasm line of the record was refreshed by the same run, 116,575 ->
116,966, `main`'s own drift, 117 KB).

**Part 2, E4.** Measured first (a Node microbench of the generated call through the playground's `Bench`, V8 CPU
profiles, the device bench in Chromium): allocation was the cause, and two V8 facts decided the levers (a typed array of
at most 64 bytes lives on the heap; asking a small one for its `ArrayBuffer` costs 230 ns). Eight commits (ADR-056 has
the table): byte-wise writer and reader with no `DataView` per buffer, the call payload in one allocation with a handle
cache, the direct call (no promise built before the send on an in-process core), the small-reply copy, **no `#private`
on the call path's classes**, the plugin's `es2022` default (and the playground's), a shared scratch `DataView`,
`sendCall` and `callSyncParts` (optional on `Transport`). The web budgets are tests: a `[web."id"]` table kind in
`bench/budgets.toml` and `bench/src/budget.rs`, read by the device bench (`bench.spec.ts`) and by `call-path.test.ts`
(`scripts/web-budgets.mjs`).

## Numbers

* Node 24 microbench, alternating runs on a loaded host, `await bench.benchAdd`: 1,010 -> 462 ns (quiet pairs), 1,693 ->
  659 in the first, loaded A/B; `callSync` 561 -> 291 ns; the 1 KB echo 2,348 -> 1,114 ns; the stub-core guard 760 to 780
  and 340 to 360 ns on the old code, 312 and 159 now.
* Chromium 153 (`bench/results/device/2026-10-02-web-chromium-headless*.json`, 3 runs, `es2022` build, load 24 to 26;
  load 10 on the quietest run of the earlier set): handle call 0.47 to 0.68 us (0.44 to 0.48), `callSync` 0.32 to 0.44
  (0.29 to 0.32), 1 KB 1.6 to 2.0 us (1.21 to 1.50), keyed insert 16.7 to 22.4 us (13.4 to 14.3), 100-signal change-set
  19.7 to 27.4 us (16.7 to 18.5), merged frame 1.46 to 2.08 ms (1.26 to 1.54), against the committed 3.16 to 3.48, 3.55 to
  3.89, 4.57 to 5.30, 20.6 to 22.6, 87.7 to 95.9 us and 4.03 to 4.28 ms (load 2 to 4). At Vite 6's default target the same
  tree measured 1.08, 0.70, 2.94, 23.0, 47.9 us and 2.25 ms (load 28 to 31).
* The playground's stress screen at 100,000 updates a second in the browser pane, before (main's runtime, Vite 6
  default build) and after: per change-set 108 -> 61 ns, drain p99 900 -> 200 us, JS heap 21.3 -> 7.8 MB (one reading
  each, the host loaded), 0 dropped frames both ways.
* React Native on Hermes, iOS simulator, Release, the app built twice and run alternately (two runs each, load 28 to 40):
  `callSync` 9.65 to 9.85 -> 1.69 to 1.72 us, `encodeCall` 6.0 to 6.7 -> 1.5 us, an awaited generated call 24.6 to 25.3 ->
  7.7 us, the 1,667-patch frame (parse and drain) 21.5 to 22.0 -> 5.0 to 5.4 ms of 16.7. `docs/REACT_NATIVE.md`'s limit is
  restated (it did not fit a frame; it does).
* Counts: runtime `npm test` 1,463 pass (was 1,432: the new files are `call-path`, `direct-call`, `wire-small-paths` and
  `default-ports`), the RN package 87 pass, the playground web 121 pass and its Playwright smoke 5/5, `[web]` budget tests
  in `undra-bench`. COUNTS_PLACEHOLDER

## Deviations

1. **16 KB was not reached** (21,159; gate 21,500). The first three levers are what a hello page does not run; what is
   left is `UndraCore`, the mirror, the in-process transport, the wire and the error classes, each required behaviour
   (ADR-052's amendment, "Why it stops at 21 KB"). The call path's speed cost 1,510 bytes, 798 of them the choice of
   `private _x` over `#x` (property names are not mangled); it is deliberate (3x to 5x on a lowered build, on Hermes,
   against 0.8 KB) and ADR-056 records the alternative.
2. The brief allowed edits to `ts.rs` for the call-path shape; none were needed. A generated call builder (one allocation
   less for a small call, one copy for a large one) would have changed every golden and the minimum runtime for new
   bindings, for what `sendCall` already takes (ADR-056, "Alternatives").
3. The 5x budget rule of `bench/budgets.toml` is used for the web rows, so they catch an order of magnitude, not a 2x
   drift (the Node guard's old-code numbers pass it); the device bench files are the fine ratchet. A baseline gate for
   the JavaScript rows is the next step if wanted.
4. The "mirror apply avoiding allocation per patch op" lever was not taken: the burst profile puts 54% of the time in
   wasm and 25% in the JavaScript enqueue, and the fused patch decode needs a generated or wire-level API (ADR-056).
5. React Native was measured on the iOS simulator only (a dedicated simulator, created and deleted by the run); the
   Android emulator was not measured again.
6. Drive-by: `examples/playground/web/src/stories/Todos.stories.tsx` imported `undra_core.wasm`, which ADR-044 renamed
   to `playground_core.wasm`; `npm run build` of the playground failed on `main`. One path fixed.
7. `crates/undra-transport/interop/run.sh` failed once in its Kotlin reconnect step (`ConnectedCore.failAll`, a
   `NoSuchElementException` from a concurrent map during a reconnect; every TypeScript step passed, the reconnect, the
   resumed session and the time travel included); it is the JVM client and not this piece's code. INTEROP_PLACEHOLDER
8. `check-links` reports the chrome of `docs/db.html` and `docs/realtime.html` out of sync (and one over-long
   description on `main` itself); both are on `main`.
9. The commit trailer is the brief's (`Claude Fable 5.1`), not the harness's default.

## How to reproduce

```bash
node scripts/web-size-runtime.mjs <project> runtimes/ts/@undra/runtime <out>   # the gate's measurement; UNDRA_SIZE_MODULES=exports, UNDRA_SIZE_TARGET=es2020
bash scripts/wasm-size.sh                      # both gates;  --record writes them
scripts/bench-device.sh --device web --runs 3  # the Chromium rows (they now fail a run over its [web."id"] budget)
cd runtimes/ts/@undra/runtime && npm test      # includes call-path.test.ts
```

## Open

* A baseline gate (`bench/baselines`-style) for the JavaScript call-path rows, so that a 2x drift is seen; the Android
  emulator and a phone for the React Native rows.
* Whether to lower the first chunk further by changing behaviour (a mirror without compaction, no snapshot API in the
  first chunk): ADR-052's amendment lists what it would take, and it is a decision, not a size piece.
* `main`'s check-links problems (above).
